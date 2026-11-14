//! Persistent, bounded release metadata cache. Never contains credentials.
use crate::{StorageError, Store};
impl Store {
    /// Read the last successful metadata refresh.
    ///
    /// # Errors
    /// Returns a storage error when SQLite cannot read the cache.
    pub async fn update_cache(
        &self,
        repository: &str,
    ) -> Result<Option<(i64, String)>, StorageError> {
        Ok(
            sqlx::query_as("SELECT checked_at, document FROM update_cache WHERE repository = ?")
                .bind(repository)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    /// Replace cached metadata atomically.
    ///
    /// # Errors
    /// Returns a storage error when SQLite rejects the bounded document.
    pub async fn save_update_cache(
        &self,
        repository: &str,
        checked_at: i64,
        document: &str,
    ) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO update_cache(repository, checked_at, document) VALUES (?, ?, ?) ON CONFLICT(repository) DO UPDATE SET checked_at=excluded.checked_at, document=excluded.document")
            .bind(repository).bind(checked_at).bind(document).execute(&self.pool).await?;
        Ok(())
    }
}
