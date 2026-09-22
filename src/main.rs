mod config;
mod error;
mod fetcher;
mod http;
mod providers;
mod redis_store;

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("COCE_CONFIG").unwrap_or_else(|_| "config.json".to_string());
    let cfg = Arc::new(config::Config::load(&config_path)?);

    let redis = redis_store::connect(&cfg.redis.host, cfg.redis.port).await?;
    let http_client = reqwest::Client::builder().build()?;

    let port = cfg.port;
    let state = http::AppState {
        config: cfg,
        redis,
        http: http_client,
    };

    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("coce listening on port {port}");
    axum::serve(listener, app).await?;

    Ok(())
}
