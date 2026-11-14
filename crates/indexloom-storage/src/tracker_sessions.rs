//! Execution-only encrypted tracker sessions; never part of a management DTO.

use std::{collections::BTreeMap, fmt};

use sqlx::FromRow;
use time::OffsetDateTime;

use crate::{KEY_VERSION, SecretVault, StorageError, Store};

const MAX_SESSION_BYTES: usize = 1024 * 1024;

/// Opaque cookie-store snapshot. Cookie values and all cookie attributes are
/// encrypted together. Deliberately has no Serialize implementation.
pub struct TrackerSession {
    pub cookies_json: String,
    pub expires_at: i64,
}

impl fmt::Debug for TrackerSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrackerSession").finish_non_exhaustive()
    }
}

#[derive(FromRow)]
struct SessionRow {
    revision: i64,
    key_version: i64,
    expires_at: i64,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

fn scope(id: &str, revision: i64, expires_at: i64) -> String {
    // Separate from credential encryption, and authenticate every mutable
    // metadata field, including the eviction deadline.
    format!("indexloom:tracker-session:v{KEY_VERSION}:{id}:{revision}:{expires_at}")
}

impl Store {
    /// Loads only the current, unexpired session for this configuration revision.
    /// # Errors
    /// Fails closed on corrupted metadata, ciphertext or an incorrect key.
    pub async fn load_tracker_session(
        &self,
        id: &str,
        revision: i64,
        vault: &SecretVault,
    ) -> Result<Option<TrackerSession>, StorageError> {
        self.evict_expired_tracker_sessions().await?;
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT s.revision, s.key_version, s.expires_at, s.nonce, s.ciphertext \
             FROM indexer_sessions s JOIN indexers i ON i.id = s.indexer_id \
             WHERE s.indexer_id = ? AND s.revision = ? AND i.session_revision = s.revision",
        )
        .bind(id)
        .bind(revision)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        if row.key_version != KEY_VERSION {
            return Err(StorageError::Decryption);
        }
        let mut decrypted = vault.decrypt_scoped(
            &scope(id, row.revision, row.expires_at),
            &row.nonce,
            &row.ciphertext,
        )?;
        let cookies_json = decrypted
            .remove("cookies")
            .ok_or(StorageError::Decryption)?;
        Ok(Some(TrackerSession {
            cookies_json,
            expires_at: row.expires_at,
        }))
    }

    /// Replaces one session atomically, provided credentials have not changed.
    /// Returns false for an obsolete runtime; it cannot resurrect revoked data.
    /// # Errors
    /// Returns an error on encryption, size or database failures.
    pub async fn save_tracker_session(
        &self,
        id: &str,
        revision: i64,
        session: &TrackerSession,
        vault: &SecretVault,
    ) -> Result<bool, StorageError> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if session.cookies_json.len() > MAX_SESSION_BYTES
            || session.expires_at > now + 12 * 24 * 60 * 60
        {
            return Err(StorageError::InvalidData(
                "tracker session exceeds limits".into(),
            ));
        }
        if session.expires_at <= now {
            self.clear_tracker_session(id, revision).await?;
            return Ok(false);
        }
        let encrypted = vault.encrypt_scoped(
            &scope(id, revision, session.expires_at),
            &BTreeMap::from([("cookies".into(), session.cookies_json.clone())]),
        )?;
        let result = sqlx::query(
            "INSERT INTO indexer_sessions(indexer_id, revision, key_version, expires_at, nonce, ciphertext) \
             SELECT id, ?, ?, ?, ?, ? FROM indexers WHERE id = ? AND session_revision = ? \
             ON CONFLICT(indexer_id) DO UPDATE SET revision = excluded.revision, \
             key_version = excluded.key_version, expires_at = excluded.expires_at, \
             nonce = excluded.nonce, ciphertext = excluded.ciphertext",
        )
        .bind(revision).bind(KEY_VERSION).bind(session.expires_at)
        .bind(encrypted.nonce).bind(encrypted.ciphertext).bind(id).bind(revision)
        .execute(&self.pool).await?;
        Ok(result.rows_affected() != 0)
    }

    /// Revokes only the calling runtime's session, never a newer revision.
    /// # Errors
    /// Returns database errors.
    pub async fn clear_tracker_session(&self, id: &str, revision: i64) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM indexer_sessions WHERE indexer_id = ? AND revision = ?")
            .bind(id)
            .bind(revision)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Runs on opening storage and before session reads; no shutdown flush or
    /// background worker is required for durability or expiry enforcement.
    /// # Errors
    /// Returns database errors.
    pub async fn evict_expired_tracker_sessions(&self) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM indexer_sessions WHERE expires_at <= ?")
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
