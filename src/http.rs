use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::breaker::Breakers;
use crate::config::Config;
use crate::error::AppError;
use crate::fetcher;
use crate::redis_store::{self, RedisManager};
use crate::stats::Stats;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub redis: RedisManager,
    pub http: reqwest::Client,
    pub breakers: Arc<Breakers>,
    pub stats: Arc<Stats>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/cover", get(cover))
        .route("/set", get(set))
        .route("/stats", get(stats))
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

/// Count a rejected `/cover` request and build its 400 response.
fn reject(state: &AppState, error: AppError) -> Response {
    state.stats.rejected();
    error.into_response()
}

async fn cover(State(state): State<AppState>, Query(q): Query<CoverQuery>) -> Response {
    state.stats.cover_request();
    let ids_raw = match q.id {
        Some(v) if v.len() >= 8 => v,
        _ => return reject(&state, AppError::MissingId),
    };
    let ids: Vec<String> = ids_raw.split(',').map(str::to_string).collect();
    if ids.is_empty() {
        return reject(&state, AppError::BadId);
    }
    if ids.len() > state.config.max_ids {
        return reject(&state, AppError::TooManyIds(state.config.max_ids));
    }

    let providers: Vec<String> = match q.provider {
        Some(p) => p.split(',').map(str::to_string).collect(),
        None => state.config.providers.clone(),
    };

    for p in &providers {
        if !state.config.providers.contains(p) {
            return reject(&state, AppError::UnavailableProvider(p.clone()));
        }
    }

    state.stats.ids_requested(ids.len());
    let url_map = fetcher::fetch(&ids, &providers, &state).await;

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
        Some(cb) if !is_valid_callback(&cb) => reject(&state, AppError::BadCallback),
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

/// Force the cover URL of an ID for a provider (e.g. to fix a wrong cover),
/// for 10 years. Requires the configured `setToken`.
async fn set(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<SetQuery>) -> Response {
    // An empty token would match an empty `Bearer ` header: treat it as unset.
    let Some(expected) = state.config.set_token.as_deref().filter(|t| !t.is_empty()) else {
        return AppError::SetDisabled.into_response();
    };
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !given.is_some_and(|t| constant_time_eq(t.as_bytes(), expected.as_bytes())) {
        tracing::warn!(provider = %q.provider, id = %q.id, "/set refused: missing or bad token");
        return AppError::Unauthorized.into_response();
    }
    if !state.config.providers.contains(&q.provider) {
        return AppError::UnavailableProvider(q.provider).into_response();
    }
    if !(q.url.starts_with("https://") || q.url.starts_with("http://")) {
        return AppError::BadUrl.into_response();
    }

    let mut con = state.redis.clone();
    let key = format!("{}.{}", q.provider, q.id);
    let timeout_ms = state.config.redis.timeout;
    match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        redis_store::set_ex(&mut con, &key, 315_360_000, &q.url),
    )
    .await
    {
        Ok(Ok(())) => {
            tracing::info!(%key, url = %q.url, "cover URL set");
            Json(serde_json::json!({ "success": true })).into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(%key, error = %e, "redis write failed");
            AppError::CacheUnavailable.into_response()
        }
        Err(_) => {
            tracing::warn!(%key, timeout_ms, "redis write timed out");
            AppError::CacheUnavailable.into_response()
        }
    }
}

/// A JSONP callback is echoed into the JavaScript response, so only accept
/// a (possibly dotted) function name, e.g. `populateImg` or `Coce.cb_1`.
fn is_valid_callback(cb: &str) -> bool {
    !cb.is_empty()
        && cb.len() <= 128
        && cb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'.'))
}

/// Compare secrets without leaking, through timing, how many leading bytes
/// match.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}


/// Activity counters, provider and breaker state, Redis figures. Counters
/// are per process and reset on restart. Redis figures only use
/// constant-time commands (no key scan), so this stays cheap on a large
/// cache.
async fn stats(State(state): State<AppState>) -> Response {
    let cfg = &state.config;

    let providers: serde_json::Map<String, serde_json::Value> = cfg
        .providers
        .iter()
        .map(|p| {
            let mut entry = serde_json::to_value(state.breakers.snapshot(p)).unwrap();
            if let (Some(obj), Ok(serde_json::Value::Object(counters))) =
                (entry.as_object_mut(), serde_json::to_value(state.stats.provider(p)))
            {
                obj.extend(counters);
            }
            (p.clone(), entry)
        })
        .collect();

    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": state.stats.started_at(),
        "uptime_s": state.stats.uptime_s(),
        "config": {
            "providers": cfg.providers,
            "timeout_ms": cfg.timeout,
            "provider_timeout_ms": cfg.provider_timeout,
            "provider_retry_s": cfg.provider_retry,
            "max_ids": cfg.max_ids,
            "redis": format!("{}:{}", cfg.redis.host, cfg.redis.port),
            "local_cache": cfg.cache.is_some(),
        },
        "requests": state.stats.requests(),
        "providers": providers,
        "redis": redis_stats(&state).await,
    }))
    .into_response()
}

async fn redis_stats(state: &AppState) -> serde_json::Value {
    let mut con = state.redis.clone();
    let timeout = Duration::from_millis(state.config.redis.timeout);
    let started = std::time::Instant::now();
    let figures = async {
        redis_store::ping(&mut con).await?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let keys = redis_store::dbsize(&mut con).await?;
        let used_memory = redis_store::used_memory(&mut con).await?;
        Ok::<_, redis::RedisError>((latency_ms, keys, used_memory))
    };
    match tokio::time::timeout(timeout, figures).await {
        Ok(Ok((latency_ms, keys, used_memory))) => serde_json::json!({
            "reachable": true,
            "latency_ms": latency_ms,
            "keys": keys,
            "used_memory": used_memory,
        }),
        _ => serde_json::json!({ "reachable": false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_validation() {
        assert!(is_valid_callback("populateImg"));
        assert!(is_valid_callback("Coce.cb_1"));
        assert!(is_valid_callback("jQuery$123"));
        assert!(!is_valid_callback(""));
        assert!(!is_valid_callback("alert(1)//"));
        assert!(!is_valid_callback("a;b"));
        assert!(!is_valid_callback("<script>"));
        assert!(!is_valid_callback(&"a".repeat(129)));
    }

    #[test]
    fn token_comparison() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret2"));
        assert!(!constant_time_eq(b"", b"secret"));
    }
}
