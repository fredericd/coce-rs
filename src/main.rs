mod breaker;
mod config;
mod error;
mod fetcher;
mod http;
mod providers;
mod redis_store;
mod stats;

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::task::TaskTracker;
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
        set_enabled = cfg.set_token.as_deref().is_some_and(|t| !t.is_empty()),
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

    let stats = Arc::new(stats::Stats::new(&cfg.providers));
    let tasks = TaskTracker::new();

    let port = cfg.port;
    let shutdown_grace = Duration::from_millis(cfg.timeout);
    let state = http::AppState {
        config: cfg,
        redis,
        http: http_client,
        breakers,
        stats,
        tasks: tasks.clone(),
    };

    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("coce listening on port {port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Requests are done; let background work (cache writes, providers
    // finishing after the global timeout) complete, within bounds.
    tasks.close();
    if !tasks.is_empty() {
        tracing::info!(pending = tasks.len(), "waiting for background tasks");
    }
    if tokio::time::timeout(shutdown_grace, tasks.wait()).await.is_err() {
        tracing::warn!(pending = tasks.len(), "background tasks still running, exiting anyway");
    }
    tracing::info!("coce stopped");
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM (sent by systemd and Docker to stop the
/// service): the server then stops accepting connections and finishes the
/// requests in progress.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown requested, finishing requests in progress");
}
