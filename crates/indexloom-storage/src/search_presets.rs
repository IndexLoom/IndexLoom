//! User-isolated saved search presets. Only typed JSON validated by the API is persisted.
use crate::{StorageError, Store, now};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SearchPresetRecord {
    pub id: String,
    pub name: String,
    pub document_json: String,
    pub created_at: String,
    pub updated_at: String,
}
impl Store {
    /// List a user's saved presets without exposing any other user entries.
    ///
    /// # Errors
    /// Returns a SQLite query error.
    pub async fn list_search_presets(
        &self,
        owner: &str,
    ) -> Result<Vec<SearchPresetRecord>, StorageError> {
        sqlx::query_as::<_, SearchPresetRecord>(
            "SELECT id,name,document_json,created_at,updated_at FROM search_presets
             WHERE owner=? ORDER BY updated_at DESC LIMIT 32",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await
        .map_err(StorageError::from)
    }

    /// Save one validated preset up to the per-owner bound of 32.
    ///
    /// # Errors
    /// Returns a SQLite error or a quota failure.
    pub async fn save_search_preset(
        &self,
        owner: &str,
        id: &str,
        name: &str,
        document_json: &str,
    ) -> Result<(), StorageError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM search_presets WHERE owner=? AND id!=?")
                .bind(owner)
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        if count >= 32
            && sqlx::query_scalar::<_, i64>("SELECT 1 FROM search_presets WHERE owner=? AND id=?")
                .bind(owner)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .is_none()
        {
            return Err(StorageError::InvalidData(
                "saved preset limit exceeded".into(),
            ));
        }
        let at = now()?;
        sqlx::query(
            "INSERT INTO search_presets(owner,id,name,document_json,created_at,updated_at)
             VALUES (?,?,?,?,?,?)
             ON CONFLICT(owner,id) DO UPDATE SET name=excluded.name,
             document_json=excluded.document_json,updated_at=excluded.updated_at",
        )
        .bind(owner)
        .bind(id)
        .bind(name)
        .bind(document_json)
        .bind(&at)
        .bind(&at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Remove only the requested owner's saved preset.
    ///
    /// # Errors
    /// Returns a SQLite error.
    pub async fn delete_search_preset(&self, owner: &str, id: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM search_presets WHERE owner=? AND id=?")
            .bind(owner)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
