use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use indexloom_server::{
    AdminToken, AppState,
    catalog::{CatalogSource, synchronize},
    router,
};
use indexloom_storage::{SecretVault, Store};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroize;

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
    let mut admin_token_value =
        env::var("INDEXLOOM_ADMIN_TOKEN").context("INDEXLOOM_ADMIN_TOKEN is required")?;
    let admin_token = AdminToken::new(&admin_token_value);
    admin_token_value.zeroize();
    let admin_token = admin_token.map_err(anyhow::Error::msg)?;
    let mut master_key =
        env::var("INDEXLOOM_MASTER_KEY").context("INDEXLOOM_MASTER_KEY is required")?;
    let vault = SecretVault::from_base64(&master_key);
    master_key.zeroize();
    let vault = vault.context("INDEXLOOM_MASTER_KEY is invalid")?;

    let store = Store::connect(&database_url)
        .await
        .context("failed to initialize IndexLoom storage")?;
    let catalog_source = CatalogSource::from_values(
        env::var("INDEXLOOM_CATALOG_DIR").ok(),
        &env_or(
            "INDEXLOOM_CATALOG_ARCHIVE_URL",
            "https://github.com/IndexLoom/Indexers/archive/refs/heads/main.tar.gz",
        ),
    )
    .context("INDEXLOOM catalog source is invalid")?;
    synchronize_catalog_or_retain(&store, &catalog_source).await?;
    let catalog_interval = env_or("INDEXLOOM_CATALOG_SYNC_SECONDS", "21600")
        .parse::<u64>()
        .context("INDEXLOOM_CATALOG_SYNC_SECONDS must be an integer")?;
    if catalog_interval != 0 {
        if catalog_interval < 300 {
            anyhow::bail!("INDEXLOOM_CATALOG_SYNC_SECONDS must be 0 or at least 300");
        }
        tokio::spawn(catalog_sync_loop(
            store.clone(),
            catalog_source,
            Duration::from_secs(catalog_interval),
        ));
    }
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;

    tracing::info!(%bind, "IndexLoom is ready");
    axum::serve(
        listener,
        router(AppState::new(store, vault, admin_token), web_root),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("IndexLoom HTTP server failed")?;

    Ok(())
}

async fn synchronize_catalog_or_retain(store: &Store, source: &CatalogSource) -> Result<()> {
    match synchronize(store, source).await {
        Ok(report) => {
            tracing::info!(
                definitions = report.definitions,
                new_indexers = report.new_indexers,
                "indexer catalog synchronized"
            );
            Ok(())
        }
        Err(error) => {
            let retained = store
                .definition_count()
                .await
                .context("failed to count retained indexer definitions")?;
            if retained == 0 {
                return Err(error).context("initial indexer catalog synchronization failed");
            }
            tracing::warn!(%error, retained, "catalog sync failed; retaining last valid catalog");
            Ok(())
        }
    }
}

async fn catalog_sync_loop(store: Store, source: CatalogSource, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if let Err(error) = synchronize(&store, &source).await {
            tracing::warn!(%error, "scheduled catalog synchronization failed");
        }
    }
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
