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
}

impl Store {
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
        let timestamp = now()?;
        let mut new_indexers = 0_usize;
        for definition in definitions {
            sqlx::query(
                "INSERT INTO indexer_definitions \
                     (id, source, source_revision, schema_version, content_yaml, \
                      content_sha256, imported_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET source = excluded.source, \
                     source_revision = excluded.source_revision, \
                     schema_version = excluded.schema_version, \
                     content_yaml = excluded.content_yaml, \
                     content_sha256 = excluded.content_sha256, \
                     imported_at = excluded.imported_at",
            )
            .bind(&definition.id)
            .bind(&definition.source)
            .bind(&definition.source_revision)
            .bind(&definition.schema_version)
            .bind(&definition.content_yaml)
            .bind(&definition.content_sha256)
            .bind(&timestamp)
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
        transaction.commit().await?;
        Ok(CatalogSyncReport {
            definitions: definitions.len(),
            new_indexers,
        })
    }
}
