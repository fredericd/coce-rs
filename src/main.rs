mod breaker;
mod config;
mod error;
mod fetcher;
mod http;
mod providers;
mod redis_store;

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

/// Used when `RUST_LOG` is unset: Coce's own events at `info` and above,
/// everything else (dependencies) only at `warn` and above.
const DEFAULT_LOG_FILTER: &str = "warn,coce=info";

fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);

    match std::env::var("COCE_LOG_FORMAT").as_deref() {
        Ok("json") => builder.json().init(),
        // Color codes are noise in journald or `docker logs`, so only emit
        // them when writing to an actual terminal.
        _ => builder.with_ansi(std::io::stdout().is_terminal()).init(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config_path = std::env::var("COCE_CONFIG").unwrap_or_else(|_| "config.json".to_string());
    let cfg = Arc::new(config::Config::load(&config_path)?);
    tracing::info!(
        config = %config_path,
        providers = ?cfg.providers,
        timeout_ms = cfg.timeout,
        provider_timeout_ms = cfg.provider_timeout,
        provider_retry_s = cfg.provider_retry,
        max_ids = cfg.max_ids,
        redis = %format!("{}:{}", cfg.redis.host, cfg.redis.port),
        local_cache = cfg.cache.is_some(),
        "configuration loaded"
    );
    if cfg.provider_timeout >= cfg.timeout {
        tracing::warn!(
            provider_timeout_ms = cfg.provider_timeout,
            timeout_ms = cfg.timeout,
            "providerTimeout should be lower than timeout, otherwise stuck \
             providers are cut by the global timeout and never disabled"
        );
    }

    let redis = redis_store::connect(&cfg.redis.host, cfg.redis.port).await?;
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(cfg.provider_timeout))
        .build()?;
    let breakers = Arc::new(breaker::Breakers::new(
        &cfg.providers,
        Duration::from_secs(cfg.provider_retry),
    ));

    let port = cfg.port;
    let state = http::AppState {
        config: cfg,
        redis,
        http: http_client,
        breakers,
    };

    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("coce listening on port {port}");
    axum::serve(listener, app).await?;

    Ok(())
}
