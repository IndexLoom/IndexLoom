//! Transactional management operations. Never decrypts or rewrites credentials.
use crate::{StorageError, Store};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, Row};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkPatch {
    pub ids: Vec<String>,
    pub enabled: Option<bool>,
    pub tags: Option<TagPatch>,
    pub group_ids: Option<Vec<String>>,
    /// Omit to preserve; empty string clears the override.
    pub proxy_id: Option<String>,
    pub flaresolverr_instance_id: Option<String>,
    #[serde(default)]
    pub delete: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagPatch {
    pub mode: TagMode,
    pub values: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagMode {
    Append,
    Remove,
    Replace,
}
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid bulk operation".into())
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}
impl BulkPatch {
    /// # Errors
    /// Returns an error for duplicate IDs, invalid identifiers, or contradictory edits.
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.ids.is_empty()
            || self.ids.len() > 500
            || self.ids.iter().any(|s| !identifier(s))
            || self.ids.iter().collect::<BTreeSet<_>>().len() != self.ids.len()
        {
            return Err(invalid());
        }
        let changed = self.enabled.is_some()
            || self.tags.is_some()
            || self.group_ids.is_some()
            || self.proxy_id.is_some()
            || self.flaresolverr_instance_id.is_some();
        if self.delete == changed {
            return Err(invalid());
        }
        if let Some(tags) = &self.tags
            && (tags.values.len() > 50
                || tags.values.iter().any(|s| {
                    s.is_empty() || s.len() > 50 || s.trim() != s || s.chars().any(char::is_control)
                })
                || tags
                    .values
                    .iter()
                    .map(|s| s.to_lowercase())
                    .collect::<BTreeSet<_>>()
                    .len()
                    != tags.values.len())
        {
            return Err(invalid());
        }
        if let Some(groups) = &self.group_ids
            && (groups.len() > 50
                || groups.iter().any(|s| !identifier(s))
                || groups.iter().collect::<BTreeSet<_>>().len() != groups.len())
        {
            return Err(invalid());
        }
        for id in [&self.proxy_id, &self.flaresolverr_instance_id]
            .into_iter()
            .flatten()
        {
            if !id.is_empty() && !identifier(id) {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
impl Store {
    /// # Errors
    /// Returns errors for invalid edits, missing records, or failed SQLite transactions.
    pub async fn bulk_indexers(&self, patch: &BulkPatch) -> Result<usize, StorageError> {
        patch.validate()?;
        let mut tx = self.pool.begin().await?;
        // Validate every target before the first write, inside the same transaction.
        for id in &patch.ids {
            if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM indexers WHERE id=?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?
                != 1
            {
                return Err(invalid());
            }
        }
        for id in &patch.ids {
            if patch.delete {
                sqlx::query("DELETE FROM indexers WHERE id=?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                continue;
            }
            sqlx::query("UPDATE indexers SET enabled=coalesce(?,enabled), proxy_id=CASE WHEN ? IS NULL THEN proxy_id ELSE nullif(?,'') END, flaresolverr_instance_id=CASE WHEN ? IS NULL THEN flaresolverr_instance_id ELSE nullif(?,'') END, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?")
    .bind(patch.enabled).bind(&patch.proxy_id).bind(&patch.proxy_id).bind(&patch.flaresolverr_instance_id).bind(&patch.flaresolverr_instance_id).bind(id).execute(&mut *tx).await?;
            if let Some(groups) = &patch.group_ids {
                sqlx::query("DELETE FROM indexer_group_members WHERE indexer_id=?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                for group in groups {
                    sqlx::query(
                        "INSERT INTO indexer_group_members(indexer_id,group_id) VALUES (?,?)",
                    )
                    .bind(id)
                    .bind(group)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            if let Some(tags) = &patch.tags {
                if matches!(tags.mode, TagMode::Replace) {
                    sqlx::query("DELETE FROM indexer_tags WHERE indexer_id=?")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                }
                for tag in &tags.values {
                    let query = if matches!(tags.mode, TagMode::Remove) {
                        "DELETE FROM indexer_tags WHERE indexer_id=? AND tag=?"
                    } else {
                        "INSERT OR IGNORE INTO indexer_tags(indexer_id,tag) VALUES (?,?)"
                    };
                    sqlx::query(query)
                        .bind(id)
                        .bind(tag)
                        .execute(&mut *tx)
                        .await?;
                }
                let count: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM indexer_tags WHERE indexer_id=?")
                        .bind(id)
                        .fetch_one(&mut *tx)
                        .await?;
                if count > 50 {
                    return Err(invalid());
                }
            }
        }
        tx.commit().await?;
        Ok(patch.ids.len())
    }
    /// Only fixed, reviewed messages enter this sink; no error details or user input.
    /// # Errors
    /// Returns an error if the diagnostic SQLite write fails.
    pub async fn diagnostic(&self, event: DiagnosticEvent) -> Result<(), StorageError> {
        let (severity, component, message) = match event {
            DiagnosticEvent::CatalogFailure => {
                ("warn", "catalog", "Catalog synchronization failed")
            }
            DiagnosticEvent::BackupFailure => {
                ("error", "backups", "Database backup operation failed")
            }
            DiagnosticEvent::JobFailure => ("error", "scheduler", "Scheduled runtime job failed"),
            DiagnosticEvent::Started => ("info", "server", "Server storage initialized"),
        };
        sqlx::query("INSERT INTO diagnostic_logs(severity,component,message) VALUES (?,?,?)")
            .bind(severity)
            .bind(component)
            .bind(message)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// # Errors
    /// Returns an error if diagnostic queries fail.
    pub async fn diagnostic_logs(
        &self,
        severity: Option<&str>,
        component: Option<&str>,
        before: i64,
        limit: i64,
    ) -> Result<Vec<DiagnosticLog>, StorageError> {
        Ok(sqlx::query_as("SELECT id,timestamp,severity,component,message FROM diagnostic_logs WHERE (? IS NULL OR severity=?) AND (? IS NULL OR component=?) AND id<? ORDER BY id DESC LIMIT ?").bind(severity).bind(severity).bind(component).bind(component).bind(before).bind(limit).fetch_all(&self.pool).await?)
    }
    /// # Errors
    /// Returns an error if SQLite has no resolvable on-disk database.
    pub async fn database_path(&self) -> Result<std::path::PathBuf, StorageError> {
        let rows = sqlx::query("PRAGMA database_list")
            .fetch_all(&self.pool)
            .await?;
        let filename: String = rows
            .iter()
            .find(|row| row.get::<String, _>("name") == "main")
            .ok_or_else(invalid)?
            .get("file");
        if filename.is_empty() {
            return Err(invalid());
        }
        Ok(std::path::PathBuf::from(filename))
    }
    /// # Errors
    /// Returns an error when the snapshot cannot be created or verified.
    pub async fn snapshot(&self, path: &std::path::Path) -> Result<(), StorageError> {
        let pages: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await?;
        let size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await?;
        if pages.saturating_mul(size) > 256 * 1024 * 1024 {
            return Err(invalid());
        }

        sqlx::query("VACUUM INTO ?")
            .bind(path.to_str().ok_or_else(invalid)?)
            .execute(&self.pool)
            .await?;
        Self::verify_snapshot(path).await
    }
    /// # Errors
    /// Returns an error for unreadable, corrupt, or incompatible SQLite backups.
    pub async fn verify_snapshot(path: &std::path::Path) -> Result<(), StorageError> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .read_only(true);
        let mut conn = sqlx::SqliteConnection::connect_with(&options).await?;
        let rows = sqlx::query("PRAGMA integrity_check")
            .fetch_all(&mut conn)
            .await?;
        if rows.len() != 1 || rows[0].get::<String, _>(0) != "ok" {
            return Err(invalid());
        }
        // Reject arbitrary SQLite uploads; require the IndexLoom schema and migration history.
        sqlx::query("SELECT id,configuration_json FROM indexers LIMIT 0")
            .execute(&mut conn)
            .await?;
        let applied = sqlx::query("SELECT version, checksum, success FROM _sqlx_migrations")
            .fetch_all(&mut conn)
            .await?;
        if applied.is_empty() {
            return Err(invalid());
        }
        let known = sqlx::migrate!("../../migrations/sqlite");
        for row in applied {
            let version: i64 = row.get("version");
            let checksum: Vec<u8> = row.get("checksum");
            let success: bool = row.get("success");
            if !success
                || !known.iter().any(|migration| {
                    migration.version == version
                        && migration.checksum.as_ref() == checksum.as_slice()
                })
            {
                return Err(invalid());
            }
        }
        if !sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut conn)
            .await?
            .is_empty()
        {
            return Err(invalid());
        }
        conn.close().await?;
        Ok(())
    }
}
#[derive(Debug)]
pub enum DiagnosticEvent {
    CatalogFailure,
    BackupFailure,
    Started,
    JobFailure,
}
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DiagnosticLog {
    pub id: i64,
    pub timestamp: String,
    pub severity: String,
    pub component: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretVault;
    use serde_json::json;
    async fn populated() -> Store {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        store
            .upsert_definition("example", "test", None, "v1", "yaml", &"a".repeat(64))
            .await
            .unwrap();
        for id in ["first", "second"] {
            store.create_indexer(id, "example", id, true).await.unwrap();
        }
        let vault = SecretVault::generate().unwrap();
        store
            .save_indexer_configuration(
                "first",
                Some("agent"),
                None,
                &std::collections::BTreeMap::from([("other".into(), json!("preserve"))]),
                &std::collections::BTreeMap::from([("password".into(), "secret".into())]),
                &vault,
            )
            .await
            .unwrap();
        store
    }
    fn patch(value: serde_json::Value) -> BulkPatch {
        serde_json::from_value(value).unwrap()
    }
    #[tokio::test]
    async fn bulk_atomicity_and_secret_preservation() {
        let store = populated().await;
        let before: Vec<u8> =
            sqlx::query_scalar("SELECT ciphertext FROM indexer_secrets WHERE indexer_id='first'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let original: String =
            sqlx::query_scalar("SELECT configuration_json FROM indexers WHERE id='first'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        let invalid =
            patch(json!({"ids":["first","second"],"enabled":false,"groupIds":["missing"]}));
        assert!(store.bulk_indexers(&invalid).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM indexers WHERE enabled=1")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.bulk_indexers(&patch(json!({"ids":["first","second"],"enabled":false,"tags":{"mode":"append","values":["tag"]}}))).await.unwrap(),2);
        let after: Vec<u8> =
            sqlx::query_scalar("SELECT ciphertext FROM indexer_secrets WHERE indexer_id='first'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(before, after);
        let config: String =
            sqlx::query_scalar("SELECT configuration_json FROM indexers WHERE id='first'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(config, original);
        assert!(
            store
                .bulk_indexers(&patch(json!({"ids":["first","unknown"],"delete":true})))
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM indexers")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        store
            .bulk_indexers(&patch(json!({"ids":["first"],"delete":true})))
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM indexer_secrets")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn tag_modes_and_assignment_preserve_other_fields() {
        let store = populated().await;
        for (mode, values, expected) in [
            ("append", vec!["one", "two"], vec!["one", "two"]),
            ("remove", vec!["ONE"], vec!["two"]),
            ("replace", vec!["three"], vec!["three"]),
            ("replace", vec![], vec![]),
        ] {
            store
                .bulk_indexers(&patch(
                    json!({"ids":["first"],"tags":{"mode":mode,"values":values}}),
                ))
                .await
                .unwrap();
            let tags: Vec<String> = sqlx::query_scalar(
                "SELECT tag FROM indexer_tags WHERE indexer_id='first' ORDER BY tag",
            )
            .fetch_all(&store.pool)
            .await
            .unwrap();
            assert_eq!(tags, expected);
        }
        store
            .upsert_group(&crate::IndexerGroupInput {
                id: "group".into(),
                name: "Group".into(),
                description: String::new(),
                enabled: true,
                priority: 0,
                proxy_id: None,
            })
            .await
            .unwrap();
        store
            .bulk_indexers(&patch(
                json!({"ids":["first","second"],"groupIds":["group"]}),
            ))
            .await
            .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM indexer_group_members WHERE group_id='group'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
        assert!(
            store
                .bulk_indexers(&patch(
                    json!({"ids":["first","second"],"enabled":false,"proxyId":"unknown"})
                ))
                .await
                .is_err()
        );
        let enabled: i64 = sqlx::query_scalar("SELECT count(*) FROM indexers WHERE enabled=1")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(enabled, 2);
        assert!(store.bulk_indexers(&patch(json!({"ids":["first","second"],"enabled":false,"flaresolverrInstanceId":"unknown"}))).await.is_err());
        store
            .bulk_indexers(&patch(
                json!({"ids":["first"],"proxyId":"","flaresolverrInstanceId":"","groupIds":[]}),
            ))
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM indexer_group_members WHERE indexer_id='first'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
    #[test]
    fn invalid_ids_and_operations() {
        for value in [
            json!({"ids":[],"enabled":true}),
            json!({"ids":["first","first"],"delete":true}),
            json!({"ids":["x';DELETE"],"delete":true}),
            json!({"ids":["first"],"enabled":true,"delete":true}),
            json!({"ids":["first"]}),
            json!({"ids":["first"],"tags":{"mode":"replace","values":["a","A"]}}),
            json!({"ids":vec!["first";501],"enabled":true}),
        ] {
            assert!(patch(value).validate().is_err());
        }
    }
    #[tokio::test]
    async fn diagnostics_capacity_and_cursor() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        for _ in 0..2005 {
            store
                .diagnostic(DiagnosticEvent::CatalogFailure)
                .await
                .unwrap();
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM diagnostic_logs")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 2000);
        let logs = store
            .diagnostic_logs(Some("warn"), Some("catalog"), i64::MAX, 2)
            .await
            .unwrap();
        assert_eq!(logs.len(), 2);
        let older = store
            .diagnostic_logs(None, None, logs[1].id, 2)
            .await
            .unwrap();
        assert!(older[0].id < logs[1].id);
        assert!(
            store
                .diagnostic_logs(Some("error"), None, i64::MAX, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn file_snapshot_is_consistent_and_verified() {
        let dir =
            std::env::temp_dir().join(format!("indexloom-backup-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        let database = dir.join("live.db");
        let backup = dir.join("backup.db");
        let store = Store::connect(&format!("sqlite://{}", database.display()))
            .await
            .unwrap();
        store.diagnostic(DiagnosticEvent::Started).await.unwrap();
        store.snapshot(&backup).await.unwrap();
        assert!(store.snapshot(&backup).await.is_err());
        Store::verify_snapshot(&backup).await.unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&backup)
            .read_only(true);
        let mut conn = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM diagnostic_logs")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(count, 1);
        conn.close().await.unwrap();
        let incompatible = dir.join("incompatible.db");
        std::fs::copy(&backup, &incompatible).unwrap();
        let mut conn = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&incompatible),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00' WHERE version=1")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
        assert!(Store::verify_snapshot(&incompatible).await.is_err());
        let bad = dir.join("bad.db");
        std::fs::write(&bad, b"not sqlite").unwrap();
        assert!(Store::verify_snapshot(&bad).await.is_err());
        store.pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
