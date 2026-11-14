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

/// Public endpoint: the random 256-bit ticket *is* the authorization.
/// Tracker-specific URLs remain encrypted in the database until redeemed.
pub(crate) fn public_routes() -> Router<AppState> {
    Router::new().route("/api/v1/dl/{handle}", get(stable_download))
}

/// Create URL-stable, encrypted-at-rest tickets for Torznab/Newznab clients.
/// Original private fields must never enter the RSS response.
/// Stable feed identity uses a digest, never the tracker passkey, source URL or
/// a random per-request UUID. Delimit inputs to avoid ambiguous concatenation.
fn feed_guid(item: &SearchResult) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"indexloom-rss-guid-v1");
    for part in [
        item.indexer_instance_id.as_bytes(),
        item.source_guid
            .as_deref()
            .or(item.source_id.as_deref())
            .or(item.info_hash.as_deref())
            .unwrap_or(&item.title)
            .as_bytes(),
        item.title.as_bytes(),
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.update(item.size.unwrap_or(0).to_be_bytes());
    let mut output = String::with_capacity(64);
    for b in hasher.finalize() {
        write!(&mut output, "{b:02x}").expect("hex write");
    }
    output
}

pub(crate) async fn publish_feed(
    state: &AppState,
    executor: &Arc<CardigannExecutor>,
    results: &mut [SearchResult],
) {
    for release in results {
        let original = release.clone();
        let guid = feed_guid(&original);
        release.download_url = None;
        release.magnet_url = None;
        release.source_guid = Some(guid.clone());
        release.details_url = None;
        if !executor.can_resolve_release(&original) {
            continue;
        }
        let Ok(raw) = serde_json::to_string(&original) else {
            continue;
        };
        if raw.len() > MAX_RELEASE_BYTES {
            continue;
        }
        if let Ok(token) = state
            .store
            .create_download_ticket(&original.indexer_instance_id, &raw, &guid, &state.vault)
            .await
        {
            release.download_url = Some(format!("/api/v1/dl/{token}"));
        }
    }
}

/// Resolve an encrypted source-bound ticket through the current configured runtime.
pub(crate) async fn resolve_ticket_artifact(
    state: &AppState,
    handle: &str,
) -> Result<DownloadArtifact, ApiError> {
    if handle.len() != 64 || !handle.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("download ticket was not found"));
    }
    let _permit = state
        .downloads
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::too_many_requests("too many active downloads"))?;
    let (indexer_id, raw) = state
        .store
        .load_download_ticket(handle, &state.vault)
        .await?
        .ok_or_else(|| ApiError::not_found("download ticket expired or was revoked"))?;
    let release: SearchResult = serde_json::from_str(&raw)
        .map_err(|_| ApiError::not_found("download ticket is invalid"))?;
    if release.indexer_instance_id != indexer_id {
        return Err(ApiError::not_found("download ticket was revoked"));
    }
    let current = super::execution::build_executor(state, &indexer_id)
        .await
        .map_err(|_| ApiError::not_found("indexer is unavailable"))?;
    if !current.executor.can_resolve_release(&release) {
        return Err(ApiError::not_found("indexer release is unavailable"));
    }
    let artifact = super::monitoring::resolve_release(state, &current.executor, &release).await?;
    Ok(artifact)
}

async fn stable_download(
    State(state): State<AppState>,
    Path(handle): Path<String>,
) -> Result<Response, ApiError> {
    let artifact = resolve_ticket_artifact(&state, &handle).await?;
    let mut response = match artifact {
        DownloadArtifact::Torrent(bytes) => (
            [
                (header::CONTENT_TYPE, "application/x-bittorrent"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"indexloom.torrent\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        DownloadArtifact::Nzb(bytes) => (
            [
                (header::CONTENT_TYPE, "application/x-nzb"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"indexloom.nzb\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        DownloadArtifact::Magnet(uri) => {
            let mut res = StatusCode::FOUND.into_response();
            res.headers_mut().insert(
                header::LOCATION,
                axum::http::HeaderValue::from_str(&uri)
                    .map_err(|_| ApiError::bad_gateway("invalid magnet redirect"))?,
            );
            res
        }
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private,no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    Ok(response)
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
    let artifact =
        super::monitoring::resolve_release(&state, &entry.executor, &entry.release).await?;
    let mut response = match artifact {
        DownloadArtifact::Nzb(bytes) => (
            [
                (header::CONTENT_TYPE, "application/x-nzb"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"download.nzb\"",
                ),
            ],
            bytes,
        )
            .into_response(),
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
