//! Expiring, single-use downloads bound to the authenticated search credential.
//! Neither a tracker URL nor caller-supplied network target is accepted here.
use super::{ApiError, AppState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use indexloom_core::SearchResult;
use indexloom_engine::{CardigannExecutor, DownloadArtifact};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

const TTL: Duration = Duration::from_mins(10);
const MAX_HANDLES: usize = 4096;
const MAX_RELEASE_BYTES: usize = 32 * 1024;

#[derive(Clone)]
pub(crate) struct DownloadRegistry {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    // Download work is independent of searches and globally bounded.
    permits: Arc<Semaphore>,
}
impl std::fmt::Debug for DownloadRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadRegistry").finish_non_exhaustive()
    }
}
struct Entry {
    created: Instant,
    owner: [u8; 32],
    release: SearchResult,
    executor: Arc<CardigannExecutor>,
}

pub(crate) fn credential_scope(headers: &HeaderMap) -> [u8; 32] {
    Sha256::digest(
        headers
            .get(header::AUTHORIZATION)
            .map_or(&[][..], |v| v.as_bytes()),
    )
    .into()
}

impl DownloadRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Arc::default(),
            permits: Arc::new(Semaphore::new(4)),
        }
    }

    /// Keeps the execution row private and publishes only a random capability.
    /// Feed searches have no interactive credential, so do not mint unusable links.
    pub(crate) async fn publish(
        &self,
        executor: &Arc<CardigannExecutor>,
        results: &mut [SearchResult],
        owner: Option<[u8; 32]>,
    ) {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, entry| entry.created.elapsed() < TTL);
        for release in results {
            let original = release.clone();
            release.download_url = None;
            release.magnet_url = None;
            // Details/GUID fields can themselves be passkey-bearing URLs.
            release.details_url = None;
            release.source_guid = None;
            let Some(owner) = owner else {
                continue;
            };
            if !executor.can_resolve_release(&original) {
                continue;
            }
            // Retained tracker data is bounded as well as the handle count.
            if serde_json::to_vec(&original).map_or(true, |bytes| bytes.len() > MAX_RELEASE_BYTES) {
                continue;
            }
            if entries.len() >= MAX_HANDLES
                && let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.created)
                    .map(|(id, _)| id.clone())
            {
                entries.remove(&oldest);
            }
            let mut random = [0_u8; 32];
            if getrandom::fill(&mut random).is_err() {
                continue;
            }
            let mut handle = String::with_capacity(64);
            for byte in random {
                write!(&mut handle, "{byte:02x}")
                    .expect("formatting hexadecimal bytes cannot fail");
            }
            release.download_url = Some(format!("/api/v1/downloads/{handle}"));
            entries.insert(
                handle,
                Entry {
                    created: Instant::now(),
                    owner,
                    release: original,
                    executor: executor.clone(),
                },
            );
        }
    }

    async fn take(&self, handle: &str, owner: [u8; 32]) -> Result<Entry, ApiError> {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, entry| entry.created.elapsed() < TTL);
        let entry = entries
            .get(handle)
            .filter(|entry| entry.owner == owner)
            .ok_or_else(|| {
                ApiError::not_found("download handle is unknown, expired, or already used")
            })?;
        let _ = entry;
        Ok(entries.remove(handle).expect("entry exists under lock"))
    }
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/downloads/{handle}", get(download))
}

async fn download(
    State(state): State<AppState>,
    Path(handle): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if handle.len() != 64 || !handle.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("download handle was not found"));
    }
    let _permit = state
        .downloads
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::too_many_requests("too many active downloads"))?;
    let entry = state
        .downloads
        .take(&handle, credential_scope(&headers))
        .await?;
    // Recheck enabled/configuration state after search; revoked credentials and
    // changed routes must invalidate already-issued capabilities.
    let current = super::execution::build_executor(&state, &entry.release.indexer_instance_id)
        .await
        .map_err(|_| ApiError::not_found("configured indexer is no longer available"))?;
    if !Arc::ptr_eq(&current.executor, &entry.executor) {
        return Err(ApiError::not_found(
            "indexer configuration changed; search again",
        ));
    }
    let artifact = entry.executor.resolve_release(&entry.release).await
        .map_err(|_| ApiError { status: StatusCode::BAD_GATEWAY, body: super::ErrorBody { code: "DOWNLOAD_FAILED", message: "tracker download failed validation or could not be retrieved; search again to retry" } })?;
    let mut response = match artifact {
        DownloadArtifact::Torrent(bytes) => (
            [
                (header::CONTENT_TYPE, "application/x-bittorrent"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"download.torrent\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        // Returning magnet material as JSON avoids tracker-controlled redirects
        // and keeps the authentication credential out of the URI.
        DownloadArtifact::Magnet(uri) => {
            Json(serde_json::json!({"magnetUrl": uri})).into_response()
        }
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    Ok(response)
}

#[cfg(test)]
mod tests;
