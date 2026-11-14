//! An autonomous *foreground* runtime shared by native Android/iOS/desktop.
//! The HTTP adapter and web assets are still provided by indexloom-server;
//! no duplicate torrent search implementation is bundled into the client.
use anyhow::{Context, Result};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    AdminToken, AppState,
    catalog::{CatalogSource, synchronize},
    router,
};
use indexloom_storage::{SecretVault, Store};

#[derive(Debug)]
pub struct LocalInstance {
    pub endpoint: SocketAddr,
    server: JoinHandle<()>,
    catalog: JoinHandle<()>,
    automation: JoinHandle<()>,
}

impl LocalInstance {
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.endpoint.port())
    }
}

impl Drop for LocalInstance {
    fn drop(&mut self) {
        self.server.abort();
        self.catalog.abort();
        self.automation.abort();
    }
}

/// Starts a fully local `IndexLoom` HTTP/API server in the calling app's Tokio
/// runtime. The listener is ALWAYS loopback-only and uses an ephemeral port.
/// The caller manages per-app credential files and foreground lifecycle.
///
/// # Errors
/// Fails if SQLite, secrets or the loopback listener are unavailable.
pub async fn start(
    database_path: &Path,
    master_key_base64: &str,
    operator_token: &str,
) -> Result<LocalInstance> {
    let store = Store::connect_path(database_path)
        .await
        .context("could not open the local IndexLoom database")?;
    let vault =
        SecretVault::from_base64(master_key_base64).context("local encryption key is invalid")?;
    let admin_token = AdminToken::new(operator_token).map_err(anyhow::Error::msg)?;
    let state = AppState::new(store.clone(), vault, admin_token);
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .context("failed to bind localhost")?;
    let endpoint = listener.local_addr()?;
    let scheduler_state = state.clone();
    let server = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router(state, PathBuf::new())).await {
            tracing::error!(%error, "local IndexLoom runtime stopped");
        }
    });

    // Native execution is foreground-only. The scheduler is aborted with the
    // local instance, so iOS/Android suspension never promises background work.
    let catalog_source = CatalogSource::from_values(
        None,
        "https://github.com/IndexLoom/Indexers/releases/latest/download/manifest.json",
    )
    .context("built-in catalog URL is invalid")?;
    let initial_source = catalog_source.clone();
    let catalog = tokio::spawn(async move {
        if let Err(error) = synchronize(&store, &initial_source).await {
            tracing::warn!(%error, "local catalog refresh failed");
        }
    });
    let automation = tokio::spawn(crate::automation::run(
        scheduler_state,
        catalog_source,
        21600,
    ));
    Ok(LocalInstance {
        endpoint,
        server,
        catalog,
        automation,
    })
}
