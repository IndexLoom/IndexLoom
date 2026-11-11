use serde::Serialize;
use uuid::Uuid;

use super::{StorageError, Store, now};

/// One validated catalog definition ready for transactional synchronization.
#[derive(Clone, Debug)]
pub struct CatalogDefinitionInput {
    pub id: String,
    pub display_name: String,
    pub source: String,
    pub source_revision: Option<String>,
    pub schema_version: String,
    pub content_yaml: String,
    pub content_sha256: String,
}

/// Summary of one idempotent catalog synchronization.
#[derive(Clone, Copy, Debug)]
pub struct CatalogSyncReport {
    pub definitions: usize,
    pub new_indexers: usize,
    /// Whether the stored definitions changed during this call.
    pub updated: bool,
}


#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatus {
    pub total_definitions: i64,
    pub available_definitions: i64,
    pub retired_definitions: i64,
    pub latest_release_applied_at: Option<String>,
    pub latest_release_fingerprint: Option<String>,
}

impl Store {
    /// Read-only installed catalog statistics and the last applied release.
    ///
    /// # Errors
    ///
    /// Returns a storage error if any of the SQLite queries fail.
    pub async fn catalog_status(&self) -> Result<CatalogStatus, StorageError> {
        let (total, available): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(catalog_available), 0) FROM indexer_definitions",
        )
        .fetch_one(&self.pool)
        .await?;
        let last: Option<(String, String)> = sqlx::query_as(
            "SELECT updated_at, manifest_sha256 FROM catalog_release_state ORDER BY updated_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let (latest_release_applied_at, latest_release_fingerprint) =
            last.map_or((None, None), |(at, fingerprint)| (Some(at), Some(fingerprint)));
        Ok(CatalogStatus {
            total_definitions: total,
            available_definitions: available,
            retired_definitions: total - available,
            latest_release_applied_at,
            latest_release_fingerprint,
        })
    }

    /// Counts definitions retained in the local catalog.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when SQLite cannot execute the query.
    pub async fn definition_count(&self) -> Result<i64, StorageError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM indexer_definitions")
            .fetch_one(&self.pool)
            .await
            .map_err(StorageError::from)
    }

    /// Returns the previously verified release manifest digest for a source.
    ///
    /// # Errors
    ///
    /// Returns a storage error if querying SQLite fails.
    pub async fn catalog_release_fingerprint(
        &self,
        source_url: &str,
    ) -> Result<Option<String>, StorageError> {
        sqlx::query_scalar("SELECT manifest_sha256 FROM catalog_release_state WHERE source_url = ?")
            .bind(source_url)
            .fetch_optional(&self.pool)
            .await
            .map_err(StorageError::from)
    }

    /// Records an accepted, fully imported release snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error if persisting the digest fails.
    pub async fn record_catalog_release_fingerprint(
        &self,
        source_url: &str,
        fingerprint: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO catalog_release_state (source_url, manifest_sha256, updated_at) \
             VALUES (?, ?, ?) ON CONFLICT(source_url) DO UPDATE SET \
             manifest_sha256 = excluded.manifest_sha256, updated_at = excluded.updated_at",
        )
        .bind(source_url)
        .bind(fingerprint)
        .bind(now()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically synchronizes verified definitions and creates missing disabled instances.
    /// Existing names, state, credentials, groups, and routes are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when catalog data is invalid or the transaction fails.
    pub async fn sync_catalog(
        &self,
        definitions: &[CatalogDefinitionInput],
    ) -> Result<CatalogSyncReport, StorageError> {
        let mut transaction = self.pool.begin().await?;
        if definitions.is_empty() {
            return Err(StorageError::InvalidData(
                "refusing to replace the catalog with an empty snapshot".to_owned(),
            ));
        }
        let timestamp = now()?;
        let generation = Uuid::now_v7().to_string();
        let mut new_indexers = 0_usize;
        for definition in definitions {
            sqlx::query(
                "INSERT INTO indexer_definitions \
                     (id, source, source_revision, schema_version, content_yaml, \
                      content_sha256, imported_at, catalog_generation, catalog_available) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1) \
                 ON CONFLICT(id) DO UPDATE SET source = excluded.source, \
                     source_revision = excluded.source_revision, \
                     schema_version = excluded.schema_version, \
                     content_yaml = excluded.content_yaml, \
                     content_sha256 = excluded.content_sha256, \
                     imported_at = CASE WHEN indexer_definitions.content_sha256 != excluded.content_sha256 \
                         THEN excluded.imported_at ELSE indexer_definitions.imported_at END, \
                     catalog_generation = excluded.catalog_generation, catalog_available = 1",
            )
            .bind(&definition.id)
            .bind(&definition.source)
            .bind(&definition.source_revision)
            .bind(&definition.schema_version)
            .bind(&definition.content_yaml)
            .bind(&definition.content_sha256)
            .bind(&timestamp)
            .bind(&generation)
            .execute(&mut *transaction)
            .await?;

            let inserted = sqlx::query(
                "INSERT INTO indexers \
                     (id, definition_id, display_name, enabled, created_at, updated_at) \
                 VALUES (?, ?, ?, 0, ?, ?) ON CONFLICT(id) DO NOTHING",
            )
            .bind(&definition.id)
            .bind(&definition.id)
            .bind(&definition.display_name)
            .bind(&timestamp)
            .bind(&timestamp)
            .execute(&mut *transaction)
            .await?;
            new_indexers += usize::try_from(inserted.rows_affected()).unwrap_or(0);
        }
        // Keep retired definitions and all associated user state, but prevent
        // searches from using a definition removed from the verified catalog.
        sqlx::query(
            "UPDATE indexer_definitions SET catalog_available = 0 \
             WHERE catalog_generation IS NOT NULL \
               AND catalog_generation != ? AND catalog_available = 1",
        )
        .bind(&generation)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(CatalogSyncReport {
            definitions: definitions.len(),
            new_indexers,
            updated: true,
        })
    }
}
