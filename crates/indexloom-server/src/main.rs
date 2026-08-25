use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use indexloom_server::{AppState, router};
use indexloom_storage::Store;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("indexloom=info,tower_http=info")),
        )
        .init();

    let bind = env_or("INDEXLOOM_BIND", "0.0.0.0:8080")
        .parse::<SocketAddr>()
        .context("INDEXLOOM_BIND must be a socket address")?;
    let database_url = env_or(
        "INDEXLOOM_DATABASE_URL",
        "sqlite:///data/indexloom.db?mode=rwc",
    );
    let web_root = PathBuf::from(env_or("INDEXLOOM_WEB_ROOT", "/app/web"));

    let store = Store::connect(&database_url)
        .await
        .context("failed to initialize IndexLoom storage")?;
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;

    tracing::info!(%bind, "IndexLoom is ready");
    axum::serve(listener, router(AppState::new(store), web_root))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("IndexLoom HTTP server failed")?;

    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
