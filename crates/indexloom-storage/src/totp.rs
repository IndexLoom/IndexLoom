//! Account-scoped, encrypted RFC 6238 factor storage and one-time recovery.
//! Never serialize ciphertext, secrets or challenge hashes to the public API.

use sqlx::FromRow;
use time::OffsetDateTime;

use super::{SecretVault, StorageError, Store};

fn epoch() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

#[derive(Clone, Debug, FromRow)]
pub struct TotpChallenge {
    pub user_id: String,
    pub remember_me: bool,
}

impl Store {
    /// True when the user has an enabled authenticator.
    ///
    /// # Errors
    /// Returns a database error if SQLite cannot be read.
    pub async fn totp_enabled(&self, user_id: &str) -> Result<bool, StorageError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM auth_totp_credentials WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    /// Number of unused offline recovery codes.
    ///
    /// # Errors
    /// Returns a database error if SQLite cannot be read.
    pub async fn totp_recovery_count(&self, user_id: &str) -> Result<i64, StorageError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM auth_totp_recovery WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Encrypts and saves a single expiring authenticator enrollment.
    ///
    /// # Errors
    /// Returns an error on encryption or database failure.
    pub async fn totp_start_pending(
        &self,
        user_id: &str,
        secret: &str,
        vault: &SecretVault,
    ) -> Result<bool, StorageError> {
        if self.totp_enabled(user_id).await? {
            return Ok(false);
        }
        let encrypted = vault.encrypt_totp_secret(user_id, secret)?;
        let result = sqlx::query(
            "INSERT INTO auth_totp_pending(user_id, nonce, ciphertext, expires_at)
             SELECT ?, ?, ?, ? WHERE NOT EXISTS
             (SELECT 1 FROM auth_totp_credentials WHERE user_id = ?)
             ON CONFLICT(user_id) DO UPDATE SET
                 nonce = excluded.nonce, ciphertext = excluded.ciphertext,
                 expires_at = excluded.expires_at",
        )
        .bind(user_id)
        .bind(encrypted.nonce)
        .bind(encrypted.ciphertext)
        .bind(epoch() + 600)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
    }

    /// Reads a still-live pending factor, using user-scoped AEAD associated data.
    ///
    /// # Errors
    /// Returns an error if storage or authenticated decryption fails.
    pub async fn totp_pending_secret(
        &self,
        user_id: &str,
        vault: &SecretVault,
    ) -> Result<Option<String>, StorageError> {
        let encrypted: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT nonce, ciphertext FROM auth_totp_pending
             WHERE user_id = ? AND expires_at > ?",
        )
        .bind(user_id)
        .bind(epoch())
        .fetch_optional(&self.pool)
        .await?;
        encrypted
            .map(|(nonce, cipher)| vault.decrypt_totp_secret(user_id, &nonce, &cipher))
            .transpose()
    }

    /// Atomically promotes a verified pending factor, then saves hashed backup codes.
    ///
    /// # Errors
    /// Returns an error if SQLite cannot commit the enrollment.
    pub async fn totp_confirm_pending(
        &self,
        user_id: &str,
        code_hashes: &[String],
    ) -> Result<bool, StorageError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO auth_totp_credentials(user_id, nonce, ciphertext, last_used_step, created_at)
             SELECT user_id, nonce, ciphertext, -1, ?
             FROM auth_totp_pending WHERE user_id = ? AND expires_at > ?
             AND NOT EXISTS(SELECT 1 FROM auth_totp_credentials WHERE user_id = ?)",
        ).bind(epoch()).bind(user_id).bind(epoch()).bind(user_id)
            .execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query("DELETE FROM auth_totp_pending WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for digest in code_hashes {
            sqlx::query(
                "INSERT INTO auth_totp_recovery(code_sha256, user_id, created_at)
                 VALUES (?, ?, ?)",
            )
            .bind(digest)
            .bind(user_id)
            .bind(epoch())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Decrypts an enabled factor without exposing it through any API response.
    ///
    /// # Errors
    /// Returns an error if storage or authenticated decryption fails.
    pub async fn totp_current_secret(
        &self,
        user_id: &str,
        vault: &SecretVault,
    ) -> Result<Option<String>, StorageError> {
        let encrypted: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT nonce, ciphertext FROM auth_totp_credentials WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        encrypted
            .map(|(nonce, cipher)| vault.decrypt_totp_secret(user_id, &nonce, &cipher))
            .transpose()
    }

    /// Rejects repeated use of an otherwise correct time step across sessions.
    ///
    /// # Errors
    /// Returns a database error if the step cannot be marked used.
    pub async fn totp_consume_step(
        &self,
        user_id: &str,
        time_step: i64,
    ) -> Result<bool, StorageError> {
        Ok(sqlx::query(
            "UPDATE auth_totp_credentials SET last_used_step = ?
             WHERE user_id = ? AND last_used_step < ?",
        )
        .bind(time_step)
        .bind(user_id)
        .bind(time_step)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    /// Atomically consumes one SHA-256 recovery-code digest exactly once.
    ///
    /// # Errors
    /// Returns a database error if the code cannot be checked.
    pub async fn totp_consume_recovery(
        &self,
        user_id: &str,
        digest: &str,
    ) -> Result<bool, StorageError> {
        Ok(
            sqlx::query("DELETE FROM auth_totp_recovery WHERE user_id = ? AND code_sha256 = ?")
                .bind(user_id)
                .bind(digest)
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }

    /// Persists a short-lived, hashed password-authenticated login challenge.
    ///
    /// # Errors
    /// Returns a storage error if the challenge cannot be created.
    pub async fn totp_create_challenge(
        &self,
        digest: &str,
        user_id: &str,
        remember_me: bool,
    ) -> Result<i64, StorageError> {
        let now = epoch();
        let mut transaction = self.pool.begin().await?;
        // A successful password check starts a single outstanding challenge.
        // Replacing it prevents unbounded challenge storage from password reuse.
        sqlx::query("DELETE FROM auth_totp_challenges WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO auth_totp_challenges
             (token_sha256, user_id, remember_me, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(digest)
        .bind(user_id)
        .bind(remember_me)
        .bind(now + 300)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(now + 300)
    }

    /// Retrieves a still-valid challenge with fewer than five failed attempts.
    ///
    /// # Errors
    /// Returns a database error if SQLite cannot check the challenge.
    pub async fn totp_challenge(
        &self,
        digest: &str,
    ) -> Result<Option<TotpChallenge>, StorageError> {
        Ok(sqlx::query_as::<_, TotpChallenge>(
            "SELECT user_id, remember_me FROM auth_totp_challenges
             WHERE token_sha256 = ? AND expires_at > ? AND attempts < 5",
        )
        .bind(digest)
        .bind(epoch())
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Counts a failed challenge attempt, invalidating it after five tries.
    ///
    /// # Errors
    /// Returns a database error if SQLite cannot update the attempt counter.
    pub async fn totp_reject_challenge(&self, digest: &str) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE auth_totp_challenges SET attempts = attempts + 1
             WHERE token_sha256 = ? AND attempts < 5",
        )
        .bind(digest)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically deletes a valid challenge after successful factor verification.
    ///
    /// # Errors
    /// Returns a database error if SQLite cannot consume the challenge.
    pub async fn totp_consume_challenge(
        &self,
        digest: &str,
        user_id: &str,
    ) -> Result<bool, StorageError> {
        Ok(sqlx::query(
            "DELETE FROM auth_totp_challenges
             WHERE token_sha256 = ? AND user_id = ? AND expires_at > ? AND attempts < 5",
        )
        .bind(digest)
        .bind(user_id)
        .bind(epoch())
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    /// Disables the factor and recovery codes and revokes all existing sessions.
    ///
    /// # Errors
    /// Returns an error if the transaction cannot be committed.
    pub async fn totp_disable(&self, user_id: &str) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM auth_totp_credentials WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM auth_totp_pending WHERE user_id = ?")
            .bind(user_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM auth_totp_challenges WHERE user_id = ?")
            .bind(user_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM auth_totp_recovery WHERE user_id = ?")
            .bind(user_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM auth_sessions WHERE user_id = ?")
            .bind(user_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretVault, Store};

    #[tokio::test]
    async fn pending_secret_is_encrypted_and_recovery_can_be_used_once() {
        let db = Store::connect("sqlite::memory:").await.unwrap();
        let vault = SecretVault::generate().unwrap();
        db.bootstrap_admin("owner", "owner", "owner@example.org", "hash", true)
            .await
            .unwrap();
        assert!(
            db.totp_start_pending("owner", "JBSWY3DPEHPK3PXP", &vault)
                .await
                .unwrap()
        );
        assert_eq!(
            db.totp_pending_secret("owner", &vault)
                .await
                .unwrap()
                .as_deref(),
            Some("JBSWY3DPEHPK3PXP")
        );
        assert!(
            db.totp_confirm_pending("owner", &["recovery-sha".into()])
                .await
                .unwrap()
        );
        assert!(db.totp_enabled("owner").await.unwrap());
        assert_eq!(db.totp_recovery_count("owner").await.unwrap(), 1);
        assert!(
            db.totp_consume_recovery("owner", "recovery-sha")
                .await
                .unwrap()
        );
        assert!(
            !db.totp_consume_recovery("owner", "recovery-sha")
                .await
                .unwrap()
        );
        assert!(db.totp_consume_step("owner", 42).await.unwrap());
        assert!(!db.totp_consume_step("owner", 42).await.unwrap());
        assert_eq!(
            db.totp_current_secret("owner", &vault)
                .await
                .unwrap()
                .as_deref(),
            Some("JBSWY3DPEHPK3PXP")
        );
    }

    #[tokio::test]
    async fn challenge_expires_or_is_single_use() {
        let db = Store::connect("sqlite::memory:").await.unwrap();
        db.bootstrap_admin("owner", "owner", "owner@example.org", "hash", true)
            .await
            .unwrap();
        db.totp_create_challenge("hashed", "owner", false)
            .await
            .unwrap();
        assert!(db.totp_challenge("hashed").await.unwrap().is_some());
        assert!(db.totp_consume_challenge("hashed", "owner").await.unwrap());
        assert!(!db.totp_consume_challenge("hashed", "owner").await.unwrap());
    }
}
