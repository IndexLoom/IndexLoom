//! Opaque durable tickets encrypt tracker-specific release links before SQLite storage.
use super::{SecretVault, StorageError, Store};
use sqlx::Row;
use std::{collections::BTreeMap, fmt::Write};
use time::OffsetDateTime;

const TTL_SECONDS: i64 = 7 * 24 * 60 * 60;
const MAX_PENDING: i64 = 10000;

fn scope(id: &str) -> String {
    format!("indexloom:download-ticket:v1:{id}")
}
fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
fn fresh_id() -> Result<String, StorageError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| StorageError::RandomSource)?;
    let mut result = String::with_capacity(64);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("hex write into String");
    }
    Ok(result)
}
impl Store {
    /// Encrypt one bounded release row and issue a reusable 7-day ticket.
    /// The token itself grants access, so callers must only expose it to authorized clients.
    ///
    /// # Errors
    /// Returns a storage or encryption failure; no plaintext link is stored.
    pub async fn create_download_ticket(
        &self,
        indexer_id: &str,
        release_json: &str,
        guid: &str,
        vault: &SecretVault,
    ) -> Result<String, StorageError> {
        if guid.len() != 64 || !guid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidData(
                "invalid stable release GUID".into(),
            ));
        }
        if release_json.len() > 32 * 1024 {
            return Err(StorageError::InvalidData(
                "download release exceeds limit".into(),
            ));
        }
        let id = fresh_id()?;
        let sealed = vault.encrypt_scoped(
            &scope(&id),
            &BTreeMap::from([("release".to_owned(), release_json.to_owned())]),
        )?;
        let now = now();
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM download_tickets WHERE expires_at<?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM download_tickets WHERE id IN
             (SELECT id FROM download_tickets ORDER BY created_at ASC LIMIT
             (SELECT MAX(0, COUNT(*) - ?) FROM download_tickets))",
        )
        .bind(MAX_PENDING - 1)
        .execute(&mut *tx)
        .await?;
        let row=sqlx::query(
            "INSERT INTO download_tickets(id,guid,indexer_id,revision,expires_at,nonce,ciphertext,created_at)
             SELECT ?,?,id,session_revision,?,?,?,? FROM indexers WHERE id=? AND enabled=1"
        ).bind(&id).bind(guid).bind(now+TTL_SECONDS).bind(sealed.nonce).bind(sealed.ciphertext).bind(now).bind(indexer_id)
            .execute(&mut *tx).await?;
        if row.rows_affected() != 1 {
            return Err(StorageError::NotFound(
                "configured indexer is disabled or missing".into(),
            ));
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Resolve a stable RSS GUID to the newest still-valid opaque download ticket.
    ///
    /// # Errors
    /// Returns a SQLite error.
    pub async fn download_ticket_for_guid(
        &self,
        guid: &str,
    ) -> Result<Option<String>, StorageError> {
        if guid.len() != 64 || !guid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT t.id FROM download_tickets t
             JOIN indexers i ON i.id=t.indexer_id
             JOIN indexer_definitions d ON d.id=i.definition_id
             WHERE t.guid=? AND t.expires_at>? AND i.enabled=1
             AND d.catalog_available=1 AND t.revision=i.session_revision
             ORDER BY t.created_at DESC, t.id DESC LIMIT 1",
        )
        .bind(guid)
        .bind(now())
        .fetch_optional(&self.pool)
        .await
        .map_err(StorageError::from)
    }

    /// Opens a non-expired ticket only if the indexer and encryption revision still match.
    ///
    /// # Errors
    /// Returns a storage or decryption error for invalid state.
    pub async fn load_download_ticket(
        &self,
        id: &str,
        vault: &SecretVault,
    ) -> Result<Option<(String, String)>, StorageError> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT t.indexer_id,t.nonce,t.ciphertext FROM download_tickets t
             JOIN indexers i ON i.id=t.indexer_id
             JOIN indexer_definitions d ON d.id=i.definition_id
             WHERE t.id=? AND t.expires_at>? AND i.enabled=1
             AND d.catalog_available=1 AND t.revision=i.session_revision",
        )
        .bind(id)
        .bind(now())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let indexer_id: String = row.try_get("indexer_id")?;
        let nonce: Vec<u8> = row.try_get("nonce")?;
        let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
        let release = vault
            .decrypt_scoped(&scope(id), &nonce, &ciphertext)?
            .remove("release")
            .ok_or_else(|| StorageError::InvalidData("ticket content missing".into()))?;
        Ok(Some((indexer_id, release)))
    }
}

#[cfg(test)]
mod tests;
