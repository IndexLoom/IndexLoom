//! Administrator-only Blackhole export to an explicitly configured watch directory.
//! Filenames are random ticket IDs, not upstream release titles or user-supplied paths.
use crate::{ApiError, AppState, download};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use indexloom_engine::DownloadArtifact;
use serde::Serialize;
use std::path::{Path as FsPath, PathBuf};
use tokio::io::AsyncWriteExt;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/blackhole/{ticket}", post(send))
        .route("/api/v1/blackhole/status", axum::routing::get(status))
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlackholeStatus {
    configured: bool,
    path: Option<String>,
}
async fn status() -> Json<BlackholeStatus> {
    // Do not disclose host filesystem paths or secret env content via API.
    Json(BlackholeStatus {
        configured: std::env::var_os("INDEXLOOM_BLACKHOLE_DIRECTORY").is_some(),
        path: None,
    })
}
async fn ensure_dir() -> Result<PathBuf, ApiError> {
    let path = std::env::var("INDEXLOOM_BLACKHOLE_DIRECTORY")
        .map_err(|_| ApiError::bad_request("Blackhole directory is not configured"))?;
    if path.is_empty()
        || path.len() > 2048
        || !FsPath::new(&path).is_absolute()
        || path.contains('\0')
    {
        return Err(ApiError::bad_request(
            "Blackhole directory configuration is invalid",
        ));
    }
    let directory = tokio::fs::canonicalize(&path)
        .await
        .map_err(|_| ApiError::bad_request("Blackhole directory is unavailable"))?;
    let metadata = tokio::fs::metadata(&directory)
        .await
        .map_err(|_| ApiError::bad_request("Blackhole directory cannot be opened"))?;
    if !metadata.is_dir() {
        return Err(ApiError::bad_request("Blackhole path must be a directory"));
    }
    Ok(directory)
}
async fn send(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if ticket.len() != 64 || !ticket.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("Blackhole ticket is invalid"));
    }
    let folder = ensure_dir().await?;
    let artifact = download::resolve_ticket_artifact(&state, &ticket).await?;
    let (extension, bytes) = match artifact {
        DownloadArtifact::Torrent(v) => ("torrent", v),
        DownloadArtifact::Nzb(v) => ("nzb", v),
        DownloadArtifact::Magnet(v) => ("magnet", v.into_bytes()),
    };
    if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
        return Err(ApiError::bad_request("Blackhole item exceeds size budget"));
    }
    let final_name = format!("indexloom-{ticket}.{extension}");
    let filename = folder.join(&final_name);
    let pending = folder.join(format!(".indexloom-{ticket}.pending"));
    // Refuse target file or pending-file collisions; never overwrite existing jobs.
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    let mut file = opts
        .open(&pending)
        .await
        .map_err(|_| ApiError::conflict("Blackhole already has this job or another writer"))?;
    let write = file.write_all(&bytes).await;
    let synced = if write.is_ok() {
        file.sync_all().await
    } else {
        write
    };
    drop(file);
    if synced.is_err() {
        let _ = tokio::fs::remove_file(&pending).await;
        return Err(ApiError::internal("Blackhole file write failed"));
    }
    let linked = tokio::fs::hard_link(&pending, &filename).await;
    let _ = tokio::fs::remove_file(&pending).await;
    if linked.is_err() {
        return Err(ApiError::conflict(
            "Blackhole file already exists or is inaccessible",
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "queued":true,"filename":final_name,"protocol":extension
        })),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn extension_names_are_fixed_for_blackhole() {
        let ticket = "a".repeat(64);
        let path = format!("indexloom-{ticket}.torrent");
        assert!(
            path.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        );
    }
}
