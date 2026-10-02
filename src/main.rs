mod breaker;
mod cache_check;
mod config;
mod error;
mod fetcher;
mod http;
mod isbn;
mod providers;
mod redis_store;
mod stats;

use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
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

/// Book cover URL cache: fetches cover image URLs from providers (Amazon,
/// Google Books, Open Library, ORB), caches them in Redis and serves them
/// as a REST web service.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Configuration file; `COCE_*` environment variables override its
    /// values, and built-in defaults apply when it doesn't exist
    #[arg(short, long, env = "COCE_CONFIG", default_value = "config.json", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default when no command is given)
    Serve,
    /// Compare the local image copies with the URLs stored in Redis
    ///
    /// Checks the providers with `cache: true`: keys pointing to missing or
    /// broken files, keys not pointing to an existing local file, files
    /// without keys. Reports only, unless --fix is given.
    CacheCheck(cache_check::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_logging();
    let cfg = Arc::new(config::Config::load(&cli.config)?);

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(cfg, &cli.config).await,
        Command::CacheCheck(args) => {
            let mut redis = redis_store::connect(&cfg.redis.host, cfg.redis.port).await?;
            let code = cache_check::run(&cfg, &mut redis, &args).await?;
            std::process::exit(code);
        }
    }
}

async fn serve(cfg: Arc<config::Config>, config_path: &Path) -> anyhow::Result<()> {
    tracing::info!(
        config = %config_path.display(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_commands() {
        let cli = Cli::try_parse_from(["coce"]).unwrap();
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["coce", "-c", "/etc/coce.json", "serve"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("/etc/coce.json"));
        assert!(matches!(cli.command, Some(Command::Serve)));

        let cli = Cli::try_parse_from(["coce", "cache-check", "--provider=orb", "--fix", "-v"]).unwrap();
        let Some(Command::CacheCheck(args)) = cli.command else {
            panic!("expected cache-check");
        };
        assert_eq!(args.provider.as_deref(), Some("orb"));
        assert!(args.fix && args.verbose && !args.restore);

        // --config is global: accepted after the subcommand too.
        let cli = Cli::try_parse_from(["coce", "cache-check", "--config", "x.json"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("x.json"));

        assert!(Cli::try_parse_from(["coce", "cache-check", "--restore"]).is_err());
        assert!(Cli::try_parse_from(["coce", "cache-check", "--provider"]).is_err());
        assert!(Cli::try_parse_from(["coce", "bogus"]).is_err());
    }
}
