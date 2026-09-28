use axum::body::Body;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::error::AppError;
use crate::fetcher;
use crate::redis_store::{self, RedisManager};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub redis: RedisManager,
    pub http: reqwest::Client,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/cover", get(cover))
        .route("/set", get(set))
        .with_state(state)
        // Coce is meant to be called from browser JS running on whatever
        // site embeds the cover images (a different origin than Coce
        // itself), and the API has no auth model to scope by origin anyway.
        .layer(CorsLayer::permissive())
        // Access log: one event per request/response, at `debug` level under
        // the `tower_http` target, so it stays silent unless explicitly
        // enabled (e.g. `RUST_LOG=coce=info,tower_http=debug`).
        .layer(TraceLayer::new_for_http())
}

async fn index() -> &'static str {
    "Welcome to coce"
}

#[derive(Deserialize)]
struct CoverQuery {
    id: Option<String>,
    provider: Option<String>,
    all: Option<String>,
    callback: Option<String>,
}

async fn cover(State(state): State<AppState>, Query(q): Query<CoverQuery>) -> Response {
    let ids_raw = match q.id {
        Some(v) if v.len() >= 8 => v,
        _ => return AppError::MissingId.into_response(),
    };
    let ids: Vec<String> = ids_raw.split(',').map(str::to_string).collect();
    if ids.is_empty() {
        return AppError::BadId.into_response();
    }

    let providers: Vec<String> = match q.provider {
        Some(p) => p.split(',').map(str::to_string).collect(),
        None => state.config.providers.clone(),
    };

    for p in &providers {
        if !state.config.providers.contains(p) {
            return AppError::UnavailableProvider(p.clone()).into_response();
        }
    }

    let url_map = fetcher::fetch(&ids, &providers, &state.config, &state.redis, &state.http).await;

    let body: HashMap<String, serde_json::Value> = if q.all.is_some() {
        url_map
            .into_iter()
            .map(|(id, per_provider)| (id, serde_json::to_value(per_provider).unwrap()))
            .collect()
    } else {
        // No &all param: pick the first URL available, following provider
        // priority order (the order the caller requested them in).
        let mut ret = HashMap::new();
        for (id, per_provider) in url_map {
            if let Some(first) = providers.iter().find(|p| per_provider.contains_key(*p)) {
                ret.insert(id, serde_json::Value::String(per_provider[first].clone()));
            }
        }
        ret
    };

    match q.callback {
        Some(cb) => {
            let js = format!("{cb}({})", serde_json::to_string(&body).unwrap());
            Response::builder()
                .header("content-type", "application/javascript")
                .body(Body::from(js))
                .unwrap()
        }
        None => Json(body).into_response(),
    }
}

#[derive(Deserialize)]
struct SetQuery {
    provider: String,
    id: String,
    url: String,
}

async fn set(State(state): State<AppState>, Query(q): Query<SetQuery>) -> Response {
    let mut con = state.redis.clone();
    let key = format!("{}.{}", q.provider, q.id);
    let timeout_ms = state.config.redis.timeout;
    match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        redis_store::set_ex(&mut con, &key, 315_360_000, &q.url),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(%key, error = %e, "redis write failed"),
        Err(_) => tracing::warn!(%key, timeout_ms, "redis write timed out"),
    }
    Json(serde_json::json!({ "success": true })).into_response()
}
