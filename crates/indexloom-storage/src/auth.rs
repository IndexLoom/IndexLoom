//! Persistent user identity, opaque session and notification configuration storage.
//! Secrets and one-time tokens are indexed exclusively by SHA-256 digests.

use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;

use super::{StorageError, Store};

fn epoch() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

#[derive(Clone, FromRow)]
pub struct AuthUser {
    pub id: String,
    pub username: String,
    pub email: String,
    pub password_hash: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_subject: Option<String>,
    pub email_verified: bool,
    pub disabled: bool,
}

impl std::fmt::Debug for AuthUser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AuthUser")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("email_verified", &self.email_verified)
            .field("disabled", &self.disabled)
            .field("password_hash", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUserView {
    pub id: String,
    pub username: String,
    pub email: String,
    pub email_verified: bool,
    pub has_password: bool,
    pub has_oidc: bool,
    pub disabled: bool,
}

impl AuthUser {
    #[must_use]
    pub fn view(&self) -> AuthUserView {
        AuthUserView {
            id: self.id.clone(),
            username: self.username.clone(),
            email: self.email.clone(),
            email_verified: self.email_verified,
            has_password: self.password_hash.is_some(),
            has_oidc: self.oidc_subject.is_some(),
            disabled: self.disabled,
        }
    }
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailTemplate {
    pub kind: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailEventSetting {
    pub kind: String,
    pub enabled: bool,
}

impl Store {
    /// Count existing accounts. Initial administrator creation is guarded by a one-time insert.
    pub async fn auth_user_count(&self) -> Result<i64, StorageError> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_users")
            .fetch_one(&self.pool).await?)
    }

    /// Insert exactly one initial account; concurrent setup requests cannot create a second owner.
    pub async fn bootstrap_admin(
        &self, id: &str, username: &str, email: &str, password_hash: &str,
        email_verified: bool,
    ) -> Result<bool, StorageError> {
        let now = epoch();
        let result = sqlx::query(
            "INSERT INTO auth_users(id, username, email, password_hash, email_verified, created_at, updated_at) \
             SELECT ?, ?, ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM auth_users)"
        )
        .bind(id).bind(username).bind(email).bind(password_hash).bind(email_verified)
        .bind(now).bind(now)
        .execute(&self.pool).await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn add_local_user(
        &self, id: &str, username: &str, email: &str,
        password_hash: &str, email_verified: bool,
    ) -> Result<(), StorageError> {
        let now = epoch();
        sqlx::query(
            "INSERT INTO auth_users(id, username, email, password_hash, email_verified, created_at, updated_at) \\
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        ).bind(id).bind(username).bind(email).bind(password_hash)
            .bind(email_verified).bind(now).bind(now)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_auth_users(&self) -> Result<Vec<AuthUserView>, StorageError> {
        let users = sqlx::query_as::<_, AuthUser>(
            "SELECT id, username, email, password_hash, oidc_issuer, oidc_subject, \\
             email_verified, disabled FROM auth_users ORDER BY username COLLATE NOCASE"
        ).fetch_all(&self.pool).await?;
        Ok(users.iter().map(AuthUser::view).collect())
    }

    /// Never disable the last active user; a disabled account loses all sessions.
    pub async fn set_user_disabled(
        &self, user_id: &str, disabled: bool,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let enabled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_users WHERE disabled = 0")
            .fetch_one(&mut *transaction).await?;
        let target: Option<bool> = sqlx::query_scalar("SELECT disabled FROM auth_users WHERE id = ?")
            .bind(user_id).fetch_optional(&mut *transaction).await?;
        if target.is_none() || (disabled && target == Some(false) && enabled <= 1) {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query("UPDATE auth_users SET disabled = ?, updated_at = ? WHERE id = ?")
            .bind(disabled).bind(epoch()).bind(user_id)
            .execute(&mut *transaction).await?;
        if disabled {
            sqlx::query("DELETE FROM auth_sessions WHERE user_id = ?")
                .bind(user_id).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn auth_user_by_login(&self, login: &str) -> Result<Option<AuthUser>, StorageError> {
        Ok(sqlx::query_as::<_, AuthUser>(
            "SELECT id, username, email, password_hash, oidc_issuer, oidc_subject, email_verified, disabled \
             FROM auth_users WHERE username = ? OR email = ? LIMIT 1"
        ).bind(login).bind(login).fetch_optional(&self.pool).await?)
    }

    pub async fn auth_user_by_id(&self, id: &str) -> Result<Option<AuthUser>, StorageError> {
        Ok(sqlx::query_as::<_, AuthUser>(
            "SELECT id, username, email, password_hash, oidc_issuer, oidc_subject, email_verified, disabled \
             FROM auth_users WHERE id = ? LIMIT 1"
        ).bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn auth_user_by_oidc(
        &self, issuer: &str, subject: &str,
    ) -> Result<Option<AuthUser>, StorageError> {
        Ok(sqlx::query_as::<_, AuthUser>(
            "SELECT id, username, email, password_hash, oidc_issuer, oidc_subject, email_verified, disabled \
             FROM auth_users WHERE oidc_issuer = ? AND oidc_subject = ? LIMIT 1"
        ).bind(issuer).bind(subject).fetch_optional(&self.pool).await?)
    }

    /// OIDC subjects remain bound to the issuer; email collisions never auto-link accounts.
    pub async fn add_oidc_user(
        &self, id: &str, username: &str, email: &str, issuer: &str, subject: &str,
    ) -> Result<(), StorageError> {
        let now = epoch();
        sqlx::query(
            "INSERT INTO auth_users(id, username, email, oidc_issuer, oidc_subject, email_verified, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)"
        )
        .bind(id).bind(username).bind(email).bind(issuer).bind(subject).bind(now).bind(now)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn create_session(
        &self, digest: &str, user_id: &str, valid_for_seconds: i64,
    ) -> Result<i64, StorageError> {
        let issued = epoch();
        let expires = issued + valid_for_seconds;
        sqlx::query(
            "INSERT INTO auth_sessions(token_sha256, user_id, issued_at, expires_at) VALUES (?, ?, ?, ?)"
        ).bind(digest).bind(user_id).bind(issued).bind(expires)
            .execute(&self.pool).await?;
        Ok(expires)
    }

    pub async fn auth_user_for_session(&self, digest: &str) -> Result<Option<AuthUser>, StorageError> {
        Ok(sqlx::query_as::<_, AuthUser>(
            "SELECT u.id, u.username, u.email, u.password_hash, u.oidc_issuer, u.oidc_subject, \
             u.email_verified, u.disabled FROM auth_sessions s \
             JOIN auth_users u ON u.id = s.user_id \
             WHERE s.token_sha256 = ? AND s.expires_at > ? AND u.disabled = 0 LIMIT 1"
        ).bind(digest).bind(epoch()).fetch_optional(&self.pool).await?)
    }

    pub async fn revoke_session(&self, digest: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM auth_sessions WHERE token_sha256 = ?")
            .bind(digest).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn revoke_all_sessions(&self, user_id: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM auth_sessions WHERE user_id = ?")
            .bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn create_one_time_token(
        &self, digest: &str, user_id: &str, purpose: &str, validity_seconds: i64,
    ) -> Result<(), StorageError> {
        let issued = epoch();
        sqlx::query("DELETE FROM auth_one_time_tokens WHERE user_id = ? AND purpose = ?")
            .bind(user_id).bind(purpose).execute(&self.pool).await?;
        sqlx::query(
            "INSERT INTO auth_one_time_tokens(token_sha256, user_id, purpose, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?)"
        ).bind(digest).bind(user_id).bind(purpose)
            .bind(issued + validity_seconds).bind(issued)
            .execute(&self.pool).await?;
        Ok(())
    }

    /// A single DELETE RETURNING enforces one-time use across concurrent requests.
    pub async fn consume_one_time_token(
        &self, digest: &str, purpose: &str,
    ) -> Result<Option<String>, StorageError> {
        Ok(sqlx::query_scalar::<_, String>(
            "DELETE FROM auth_one_time_tokens \
             WHERE token_sha256 = ? AND purpose = ? AND expires_at > ? RETURNING user_id"
        ).bind(digest).bind(purpose).bind(epoch())
            .fetch_optional(&self.pool).await?)
    }

    pub async fn verify_email(&self, user_id: &str) -> Result<(), StorageError> {
        sqlx::query("UPDATE auth_users SET email_verified = 1, updated_at = ? WHERE id = ?")
            .bind(epoch()).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn change_password(
        &self, user_id: &str, password_hash: &str,
    ) -> Result<(), StorageError> {
        sqlx::query("UPDATE auth_users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(password_hash).bind(epoch()).bind(user_id)
            .execute(&self.pool).await?;
        self.revoke_all_sessions(user_id).await
    }

    pub async fn store_oidc_flow(
        &self, state_digest: &str, nonce: &str, verifier: &str,
    ) -> Result<(), StorageError> {
        let issued = epoch();
        sqlx::query(
            "INSERT INTO auth_oidc_flows(state_sha256, nonce, pkce_verifier, issued_at, expires_at) \
             VALUES (?, ?, ?, ?, ?)"
        ).bind(state_digest).bind(nonce).bind(verifier)
            .bind(issued).bind(issued + 600)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn consume_oidc_flow(
        &self, state_digest: &str,
    ) -> Result<Option<(String, String)>, StorageError> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "DELETE FROM auth_oidc_flows WHERE state_sha256 = ? AND expires_at > ? \
             RETURNING nonce, pkce_verifier"
        ).bind(state_digest).bind(epoch()).fetch_optional(&self.pool).await?)
    }

    pub async fn is_login_locked(&self, digest: &str) -> Result<bool, StorageError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT locked_until FROM auth_rate_limits WHERE identity_sha256 = ?"
        ).bind(digest).fetch_optional(&self.pool).await?
            .is_some_and(|until| until > epoch()))
    }

    pub async fn record_login_failure(&self, digest: &str) -> Result<(), StorageError> {
        let now = epoch();
        sqlx::query(
            "INSERT INTO auth_rate_limits(identity_sha256, failures, locked_until, updated_at) \
             VALUES (?, 1, 0, ?) \
             ON CONFLICT(identity_sha256) DO UPDATE SET \
             failures = CASE WHEN updated_at < ? THEN 1 ELSE failures + 1 END, \
             locked_until = CASE WHEN updated_at >= ? AND failures >= 4 THEN ? ELSE 0 END, \
             updated_at = ?"
        ).bind(digest).bind(now).bind(now - 900).bind(now - 900)
            .bind(now + 900).bind(now).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn clear_login_failures(&self, digest: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM auth_rate_limits WHERE identity_sha256 = ?")
            .bind(digest).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn mail_templates(&self) -> Result<Vec<MailTemplate>, StorageError> {
        Ok(sqlx::query_as::<_, MailTemplate>(
            "SELECT kind, subject, text_body, html_body FROM mail_templates ORDER BY kind"
        ).fetch_all(&self.pool).await?)
    }

    pub async fn mail_template(&self, kind: &str) -> Result<Option<MailTemplate>, StorageError> {
        Ok(sqlx::query_as::<_, MailTemplate>(
            "SELECT kind, subject, text_body, html_body FROM mail_templates WHERE kind = ?"
        ).bind(kind).fetch_optional(&self.pool).await?)
    }

    pub async fn save_mail_template(&self, template: &MailTemplate) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO mail_templates(kind, subject, text_body, html_body, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(kind) DO UPDATE SET subject = excluded.subject, \
             text_body = excluded.text_body, html_body = excluded.html_body, \
             updated_at = excluded.updated_at"
        ).bind(&template.kind).bind(&template.subject)
            .bind(&template.text_body).bind(&template.html_body).bind(epoch())
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn mail_event_settings(&self) -> Result<Vec<MailEventSetting>, StorageError> {
        Ok(sqlx::query_as::<_, MailEventSetting>(
            "SELECT kind, enabled FROM mail_event_settings ORDER BY kind"
        ).fetch_all(&self.pool).await?)
    }

    pub async fn is_mail_event_enabled(&self, kind: &str) -> Result<bool, StorageError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT enabled FROM mail_event_settings WHERE kind = ?"
        ).bind(kind).fetch_optional(&self.pool).await?.unwrap_or(false))
    }

    pub async fn set_mail_event_enabled(
        &self, kind: &str, enabled: bool,
    ) -> Result<(), StorageError> {
        sqlx::query("UPDATE mail_event_settings SET enabled = ? WHERE kind = ?")
            .bind(enabled).bind(kind).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Store;

    #[tokio::test]
    async fn bootstrap_is_one_time_and_sessions_are_revocable() {
        let store = Store::connect("sqlite::memory:").await.expect("create test database");
        assert_eq!(store.auth_user_count().await.unwrap(), 0);
        assert!(store.bootstrap_admin("alice", "alice", "alice@example.org", "hash", true).await.unwrap());
        assert!(!store.bootstrap_admin("other", "other", "other@example.org", "hash", true).await.unwrap());
        let user = store.auth_user_by_login("ALICE@example.org").await.unwrap().unwrap();
        assert_eq!(user.username, "alice");
        store.create_session("digest", &user.id, 60).await.unwrap();
        assert!(store.auth_user_for_session("digest").await.unwrap().is_some());
        store.revoke_session("digest").await.unwrap();
        assert!(store.auth_user_for_session("digest").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn one_time_token_is_consumed_only_once() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        store.bootstrap_admin("id", "alice", "alice@example.org", "hash", false).await.unwrap();
        store.create_one_time_token("digest", "id", "verify-email", 60).await.unwrap();
        assert_eq!(store.consume_one_time_token("digest", "verify-email").await.unwrap().as_deref(), Some("id"));
        assert!(store.consume_one_time_token("digest", "verify-email").await.unwrap().is_none());
    }
}
