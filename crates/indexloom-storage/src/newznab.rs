//! Custom Newznab indexer storage. User sources are never retired by catalog sync.
use super::{SecretVault, StorageError, Store, now, save_secrets};
use sqlx::Row;
use std::collections::BTreeMap;

impl Store {
    /// Atomically creates or updates a user-owned Newznab definition and encrypted API key.
    ///
    /// # Errors
    /// Returns a database or validation error; never returns the saved key.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_newznab_indexer(
        &self,
        id: &str,
        name: &str,
        definition_json: &str,
        sha256: &str,
        enabled: bool,
        api_key: Option<&str>,
        vault: &SecretVault,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query("SELECT source FROM indexer_definitions WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(row) = &existing {
            let source: String = row.try_get("source")?;
            if source != "user-newznab" {
                return Err(StorageError::InvalidData(
                    "cannot modify a catalog definition".into(),
                ));
            }
        } else if api_key.is_none() {
            return Err(StorageError::InvalidData(
                "API key is required for a new Newznab source".into(),
            ));
        }
        let timestamp = now()?;
        sqlx::query(
            "INSERT INTO indexer_definitions
             (id,source,source_revision,schema_version,content_yaml,content_sha256,imported_at,catalog_generation,catalog_available)
             VALUES (?,'user-newznab',NULL,'indexloom.io/v1alpha1',?,?,?,NULL,1)
             ON CONFLICT(id) DO UPDATE SET content_yaml=excluded.content_yaml,
                  content_sha256=excluded.content_sha256, imported_at=excluded.imported_at,
                  catalog_available=1, catalog_generation=NULL"
        ).bind(id).bind(definition_json).bind(sha256).bind(&timestamp)
            .execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO indexers (id,definition_id,display_name,enabled,created_at,updated_at)
             VALUES (?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,
                  enabled=excluded.enabled,updated_at=excluded.updated_at,
                  session_revision=session_revision+1",
        )
        .bind(id)
        .bind(id)
        .bind(name)
        .bind(enabled)
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(&mut *tx)
        .await?;
        if let Some(key) = api_key {
            save_secrets(
                &mut tx,
                id,
                &BTreeMap::from([("apiKey".to_owned(), key.to_owned())]),
                vault,
                &timestamp,
            )
            .await?;
        }
        sqlx::query("DELETE FROM indexer_sessions WHERE indexer_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Delete only custom Newznab instances and their related encrypted secrets.
    ///
    /// # Errors
    /// Returns an error on missing or non-user-defined indexers.
    pub async fn delete_newznab_indexer(&self, id: &str) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT source FROM indexer_definitions WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Err(StorageError::NotFound("Newznab source not found".into()));
        };
        let source: String = row.try_get("source")?;
        if source != "user-newznab" {
            return Err(StorageError::InvalidData(
                "cannot delete a catalog source".into(),
            ));
        }
        sqlx::query("DELETE FROM indexer_sessions WHERE indexer_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM indexer_secrets WHERE indexer_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM indexers WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM indexer_definitions WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
