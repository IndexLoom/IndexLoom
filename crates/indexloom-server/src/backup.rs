//! Bounded snapshots and offline restore queue. No open database is replaced.
use crate::{ApiError, AppState};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path as ApiPath, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use indexloom_storage::Store;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};
const MAX: u64 = 256 * 1024 * 1024;
const MAX_BODY: usize = 256 * 1024 * 1024;
const CAP: usize = 10;
static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
fn lock() -> &'static tokio::sync::Mutex<()> {
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
fn directory() -> Result<PathBuf, ApiError> {
    let path = std::env::var_os("INDEXLOOM_BACKUP_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::bad_request("INDEXLOOM_BACKUP_DIR must be configured"))?;
    if !path.is_absolute() || path.parent().is_none() {
        return Err(ApiError::bad_request("backup directory must be absolute"));
    }
    std::fs::create_dir_all(&path).map_err(io)?;
    if std::fs::symlink_metadata(&path)
        .map_err(io)?
        .file_type()
        .is_symlink()
    {
        return Err(ApiError::bad_request(
            "backup directory cannot be a symlink",
        ));
    }
    Ok(path)
}
async fn backup_directory(store: &Store) -> Result<PathBuf, ApiError> {
    let dir = directory()?;
    let database = store.database_path().await?;
    let parent = database
        .parent()
        .ok_or_else(|| ApiError::bad_request("file database required"))?;
    if std::fs::canonicalize(&dir).map_err(io)? == std::fs::canonicalize(parent).map_err(io)? {
        return Err(ApiError::bad_request(
            "use a dedicated backup directory separate from the database directory",
        ));
    }
    private_directory(&dir)?;
    Ok(dir)
}
fn private_directory(path: &Path) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    Ok(())
}
fn io(error: std::io::Error) -> ApiError {
    let kind = error.kind();
    drop(error);
    match kind {
        std::io::ErrorKind::NotFound => ApiError::not_found("backup was not found"),
        std::io::ErrorKind::AlreadyExists => {
            ApiError::conflict("backup destination already exists")
        }
        _ => ApiError::internal("backup filesystem operation failed"),
    }
}
fn target(dir: &Path, id: &str) -> Result<PathBuf, ApiError> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| ApiError::bad_request("invalid backup ID"))?;
    if uuid.to_string() != id {
        return Err(ApiError::bad_request("invalid backup ID"));
    }
    Ok(dir.join(format!("{id}.db")))
}
fn regular(path: &Path) -> Result<(), ApiError> {
    let m = std::fs::symlink_metadata(path).map_err(io)?;
    if !m.is_file() || m.len() > MAX {
        return Err(ApiError::bad_request(
            "backup must be a regular file within 256 MiB",
        ));
    }
    Ok(())
}
fn entries(dir: &Path) -> Result<Vec<serde_json::Value>, ApiError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = name.strip_suffix(".db")
            && target(dir, id).is_ok()
        {
            regular(&entry.path())?;
            let m = entry.metadata().map_err(io)?;
            out.push(serde_json::json!({"id":id,"bytes":m.len()}));
        }
    }
    out.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    Ok(out)
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/backups", get(list).post(create))
        .route("/api/v1/backups/upload", post(upload))
        .route("/api/v1/backups/{id}", get(download).delete(delete))
        .route("/api/v1/backups/{id}/restore", post(queue))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}
async fn list(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = backup_directory(&state.store).await?;
    Ok(Json(
        serde_json::json!({"backups":entries(&dir)?,"restoreQueued":dir.join("restore.json").exists(),"capacity":CAP,"maxBytes":MAX}),
    ))
}
fn capacity(dir: &Path) -> Result<(), ApiError> {
    if entries(dir)?.len() >= CAP {
        Err(ApiError::conflict(
            "backup capacity reached; delete a backup first",
        ))
    } else {
        Ok(())
    }
}
fn private(path: &Path) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    Ok(())
}
async fn snapshot(store: &Store, dir: &Path) -> Result<String, ApiError> {
    capacity(dir)?;
    let id = uuid::Uuid::now_v7().to_string();
    let stage = dir.join(format!(".{id}.tmp"));
    let result = async {
        store.snapshot(&stage).await?;
        regular(&stage)?;
        private(&stage)?;
        std::fs::File::open(&stage)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        std::fs::rename(&stage, target(dir, &id)?).map_err(io)?;
        std::fs::File::open(dir)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        Ok(id)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&stage);
        let _ = store
            .diagnostic(indexloom_storage::management::DiagnosticEvent::BackupFailure)
            .await;
    }
    result
}
async fn create(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let _guard = lock().lock().await;
    let dir = backup_directory(&state.store).await?;
    let id = snapshot(&state.store, &dir).await?;
    Ok(Json(serde_json::json!({"id":id})))
}
async fn upload(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _guard = lock().lock().await;
    let dir = backup_directory(&state.store).await?;
    capacity(&dir)?;
    let id = uuid::Uuid::now_v7().to_string();
    let upload_staging_path = dir.join(format!(".{id}.tmp"));
    let result = async {
        std::fs::write(&upload_staging_path, &body).map_err(io)?;
        private(&upload_staging_path)?;
        Store::verify_snapshot(&upload_staging_path)
            .await
            .map_err(|_| ApiError::bad_request("invalid IndexLoom SQLite backup"))?;
        std::fs::File::open(&upload_staging_path)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        std::fs::rename(&upload_staging_path, target(&dir, &id)?).map_err(io)?;
        std::fs::File::open(&dir)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        Ok(Json(serde_json::json!({"id":id})))
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(upload_staging_path);
    }
    result
}
async fn download(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> Result<Response, ApiError> {
    let _guard = lock().lock().await;
    let dir = backup_directory(&state.store).await?;
    let path = target(&dir, &id)?;
    regular(&path)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.sqlite3"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"indexloom-backup.db\"",
            ),
        ],
        std::fs::read(path).map_err(io)?,
    )
        .into_response())
}
async fn delete(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let _guard = lock().lock().await;
    let dir = backup_directory(&state.store).await?;
    let path = target(&dir, &id)?;
    regular(&path)?;
    std::fs::remove_file(path).map_err(io)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    confirmation: String,
}
async fn queue(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(c): Json<Confirmation>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if c.confirmation != "RESTORE ON RESTART" {
        return Err(ApiError::bad_request(
            "explicit restore confirmation required",
        ));
    }
    let _guard = lock().lock().await;
    let dir = backup_directory(&state.store).await?;
    if dir.join("restore.json").exists() {
        return Err(ApiError::conflict("a restore is already queued"));
    }
    let source = target(&dir, &id)?;
    regular(&source)?;
    Store::verify_snapshot(&source)
        .await
        .map_err(|_| ApiError::bad_request("backup integrity failed"))?;
    let database = state.store.database_path().await?;
    if !database.is_absolute() {
        return Err(ApiError::bad_request(
            "restore requires a file database with an absolute path",
        ));
    }
    let rollback = snapshot(&state.store, &dir).await?;
    let staged_temp = dir.join(format!(".restore-{}.tmp", uuid::Uuid::now_v7()));
    std::fs::copy(source, &staged_temp).map_err(io)?;
    private(&staged_temp)?;
    if let Err(error) = Store::verify_snapshot(&staged_temp).await {
        let _ = std::fs::remove_file(&staged_temp);
        return Err(error.into());
    }
    let restore_staging_path = dir.join("restore-stage.db");
    std::fs::rename(&staged_temp, &restore_staging_path).map_err(io)?;
    let marker = dir.join(".restore.json.tmp");
    std::fs::write(
        &marker,
        serde_json::to_vec(
            &serde_json::json!({"backupId":id,"rollbackId":rollback,"database":database}),
        )
        .expect("serialize marker"),
    )
    .map_err(io)?;
    private(&marker)?;
    std::fs::File::open(&restore_staging_path)
        .map_err(io)?
        .sync_all()
        .map_err(io)?;
    std::fs::File::open(&marker)
        .map_err(io)?
        .sync_all()
        .map_err(io)?;
    std::fs::rename(marker, dir.join("restore.json")).map_err(io)?;
    std::fs::File::open(&dir)
        .map_err(io)?
        .sync_all()
        .map_err(io)?;
    Ok(Json(
        serde_json::json!({"status":"queued","restartRequired":true,"rollbackId":rollback}),
    ))
}
/// Must run before creating any pool. Refuses WAL/SHM rather than discarding data.
/// # Errors
/// Returns an error when a queued restore is corrupt, unsafe or cannot be applied offline.
pub async fn consume_pending(database_url: &str) -> anyhow::Result<()> {
    let Some(dir) = std::env::var_os("INDEXLOOM_BACKUP_DIR").map(PathBuf::from) else {
        return Ok(());
    };
    if !dir.join("restore.json").exists() {
        return Ok(());
    }
    let dir = directory().map_err(|_| anyhow::anyhow!("invalid backup directory"))?;
    consume_from_directory(database_url, &dir).await
}

async fn consume_from_directory(database_url: &str, dir: &Path) -> anyhow::Result<()> {
    let marker = dir.join("restore.json");
    anyhow::ensure!(
        std::fs::symlink_metadata(&marker)?.is_file(),
        "invalid restore marker"
    );
    let options: sqlx::sqlite::SqliteConnectOptions = database_url.parse()?;
    let database = options.get_filename();
    let metadata = std::fs::metadata(&marker)?;
    anyhow::ensure!(metadata.len() <= 4096, "invalid restore marker size");
    let pending: serde_json::Value = serde_json::from_slice(&std::fs::read(&marker)?)?;
    anyhow::ensure!(
        pending["database"]
            .as_str()
            .is_some_and(|p| Path::new(p) == database),
        "pending restore belongs to another database"
    );
    for field in ["backupId", "rollbackId"] {
        target(
            dir,
            pending[field]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid restore marker"))?,
        )
        .map_err(|_| anyhow::anyhow!("invalid restore marker"))?;
    }

    anyhow::ensure!(
        database.is_absolute() && !database.is_symlink(),
        "restore requires an absolute regular database path"
    );
    private_directory(dir).map_err(|_| anyhow::anyhow!("backup directory permissions"))?;
    for suffix in ["-wal", "-shm"] {
        anyhow::ensure!(
            !PathBuf::from(format!("{}{suffix}", database.display())).exists(),
            "restore pending: WAL/SHM remain; stop and checkpoint database offline before restarting"
        );
    }
    let stage = dir.join("restore-stage.db");
    regular(&stage).map_err(|_| anyhow::anyhow!("invalid restore stage"))?;
    Store::verify_snapshot(&stage).await?;
    // Keep an exact offline rollback as well, including changes made after queueing.
    let rollback = database.with_extension(format!("before-restore-{}.db", uuid::Uuid::now_v7()));
    std::fs::copy(database, &rollback)?;
    private(&rollback).map_err(|_| anyhow::anyhow!("rollback permissions"))?;
    std::fs::File::open(&rollback)?.sync_all()?;
    let replacement = database.with_extension("restore-tmp");
    anyhow::ensure!(
        !replacement.exists(),
        "restore temporary file already exists"
    );
    std::fs::copy(&stage, &replacement)?;
    private(&replacement).map_err(|_| anyhow::anyhow!("restore permissions"))?;
    Store::verify_snapshot(&replacement).await?;
    std::fs::File::open(&replacement)?.sync_all()?;
    std::fs::rename(&replacement, database)?;
    std::fs::File::open(
        database
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing database parent"))?,
    )?
    .sync_all()?;
    std::fs::remove_file(dir.join("restore.json"))?;
    std::fs::File::open(dir)?.sync_all()?;
    std::fs::remove_file(stage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    #[test]
    fn opaque_ids_reject_traversal_and_alternate_forms() {
        let dir = Path::new("/tmp/backups");
        for id in [
            "../live",
            "/tmp/live",
            "a.db",
            "",
            "00000000000000000000000000000000",
            "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA",
        ] {
            assert!(target(dir, id).is_err());
        }
        let id = uuid::Uuid::now_v7().to_string();
        assert_eq!(target(dir, &id).unwrap().parent().unwrap(), dir);
    }

    #[test]
    fn capacity_refuses_creation_without_deleting_snapshots_and_size_is_bounded() {
        let dir =
            std::env::temp_dir().join(format!("indexloom-capacity-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        for _ in 0..CAP {
            let path = target(&dir, &uuid::Uuid::now_v7().to_string()).unwrap();
            std::fs::write(path, b"snapshot").unwrap();
        }
        assert!(capacity(&dir).is_err());
        assert_eq!(entries(&dir).unwrap().len(), CAP);
        let oversized = dir.join("oversized");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX + 1)
            .unwrap();
        assert!(regular(&oversized).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn restore_validates_integrity_preserves_current_and_refuses_wal() {
        let dir =
            std::env::temp_dir().join(format!("indexloom-restore-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        let live = dir.join("live.db");
        let stage = dir.join("restore-stage.db");
        // Production backups require a persistent SQLite database; use the
        // same file-backed mode here, not the in-memory test connection.
        let seed = dir.join("seed.db");
        let store = Store::connect_path(&seed).await.unwrap();
        store.snapshot(&stage).await.unwrap();
        std::fs::copy(&stage, &live).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&live);
        let mut conn = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("INSERT INTO diagnostic_logs(severity,component,message) VALUES ('info','server','Current state')").execute(&mut conn).await.unwrap();
        conn.close().await.unwrap();
        let marker = dir.join("restore.json");
        std::fs::write(&marker,serde_json::to_vec(&serde_json::json!({"database":live,"backupId":uuid::Uuid::now_v7().to_string(),"rollbackId":uuid::Uuid::now_v7().to_string()})).unwrap()).unwrap();
        let url = format!("sqlite://{}", live.display());
        let wal = PathBuf::from(format!("{}-wal", live.display()));
        std::fs::write(&wal, b"do not discard WAL").unwrap();
        assert!(consume_from_directory(&url, &dir).await.is_err());
        assert!(marker.exists());
        std::fs::remove_file(wal).unwrap();
        let valid = std::fs::read(&stage).unwrap();
        std::fs::write(&stage, b"corrupt database").unwrap();
        assert!(consume_from_directory(&url, &dir).await.is_err());
        assert!(marker.exists());
        std::fs::write(&stage, valid).unwrap();
        consume_from_directory(&url, &dir).await.unwrap();
        assert!(!marker.exists());
        assert!(!stage.exists());
        Store::verify_snapshot(&live).await.unwrap();
        let mut conn = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM diagnostic_logs")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
        conn.close().await.unwrap();
        let rollback = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.to_string_lossy().contains("before-restore-"))
            .unwrap();
        let mut conn = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&rollback),
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM diagnostic_logs")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(count, 1);
        conn.close().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn backup_symlinks_are_rejected() {
        let dir =
            std::env::temp_dir().join(format!("indexloom-symlink-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        let source = dir.join("source");
        std::fs::write(&source, b"database").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(regular(&link).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
