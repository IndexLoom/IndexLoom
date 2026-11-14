//! Encrypted channels and a durable leased notification outbox.
use crate::{SecretVault, StorageError, Store};
use serde::Serialize;
use sqlx::FromRow;
use std::collections::BTreeMap;

// Debug deliberately omits public configuration and decrypted secrets; never Serialize this type.
pub struct Channel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub enabled: bool,
    pub config: String,
    pub events: String,
    pub secrets: BTreeMap<String, String>,
    pub revision: i64,
}
#[derive(FromRow)]
struct ChannelRow {
    id: String,
    name: String,
    provider: String,
    enabled: bool,
    config: String,
    events: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    revision: i64,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub scope: String,
    pub event: String,
    pub title: String,
    pub body: String,
}
#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub channel_id: String,
    pub event: String,
    pub status: String,
    pub attempts: i64,
    pub next_attempt: i64,
    pub summary: Option<String>,
    pub duration_ms: Option<i64>,
    pub is_test: bool,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(FromRow)]
pub struct Pending {
    pub channel_revision: i64,
    pub id: String,
    pub channel_id: String,
    pub payload: String,
    pub attempts: i64,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("credentials", &"<redacted>")
            .finish_non_exhaustive()
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.secrets.values_mut() {
            value.zeroize();
        }
    }
}
impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("id", &self.id)
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Atomically applies the per-channel test cooldown.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn claim_notification_test(&self, id: &str) -> Result<bool, StorageError> {
        Ok(sqlx::query("UPDATE notification_channels SET last_test_at=unixepoch() WHERE id=? AND (last_test_at IS NULL OR last_test_at<=unixepoch()-15)").bind(id).execute(&self.pool).await?.rows_affected()>0)
    }
    /// Loads channels and decrypts credentials for internal delivery.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn notification_channels(
        &self,
        vault: &SecretVault,
    ) -> Result<Vec<Channel>, StorageError> {
        let rows = sqlx::query_as::<_, ChannelRow>("SELECT id,name,provider,enabled,config,events,nonce,ciphertext,revision FROM notification_channels ORDER BY created_at,id").fetch_all(&self.pool).await?;
        rows.into_iter().map(|r| decode(r, vault)).collect()
    }
    /// Loads one channel with its scoped credentials.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn notification_channel(
        &self,
        id: &str,
        vault: &SecretVault,
    ) -> Result<Option<Channel>, StorageError> {
        sqlx::query_as::<_, ChannelRow>("SELECT id,name,provider,enabled,config,events,nonce,ciphertext,revision FROM notification_channels WHERE id=?").bind(id).fetch_optional(&self.pool).await?.map(|r| decode(r,vault)).transpose()
    }
    /// Optimistic revision prevents a concurrent edit from restoring stale secrets.
    /// Saves a revision and cancels messages queued for its former configuration.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn save_notification_channel(
        &self,
        c: &Channel,
        vault: &SecretVault,
    ) -> Result<bool, StorageError> {
        let encrypted =
            vault.encrypt_scoped(&format!("indexloom:notification:v1:{}", c.id), &c.secrets)?;
        let mut tx = self.pool.begin().await?;
        let changed = if c.revision == 0 {
            sqlx::query("INSERT INTO notification_channels(id,name,provider,enabled,config,events,nonce,ciphertext) SELECT ?,?,?,?,?,?,?,? WHERE (SELECT count(*) FROM notification_channels)<100 ON CONFLICT(id) DO NOTHING")
                .bind(&c.id).bind(&c.name).bind(&c.provider).bind(c.enabled).bind(&c.config).bind(&c.events).bind(encrypted.nonce).bind(encrypted.ciphertext).execute(&mut *tx).await?.rows_affected()
        } else {
            sqlx::query("UPDATE notification_channels SET name=?,provider=?,enabled=?,config=?,events=?,nonce=?,ciphertext=?,revision=revision+1,updated_at=unixepoch() WHERE id=? AND revision=?")
                .bind(&c.name).bind(&c.provider).bind(c.enabled).bind(&c.config).bind(&c.events).bind(encrypted.nonce).bind(encrypted.ciphertext).bind(&c.id).bind(c.revision).execute(&mut *tx).await?.rows_affected()
        };
        if changed > 0 {
            // An edit invalidates queued content rendered for the old destination/settings.
            sqlx::query("UPDATE notification_deliveries SET status='cancelled',payload='{}',summary='Channel configuration changed',updated_at=unixepoch() WHERE channel_id=? AND status IN ('pending','retry')").bind(&c.id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(changed > 0)
    }
    /// Deletes a channel, its overrides and delivery history atomically.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn delete_notification_channel(&self, id: &str) -> Result<bool, StorageError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM notification_templates WHERE scope=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let n = sqlx::query("DELETE FROM notification_channels WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }
    /// Lists global templates and channel overrides.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn notification_templates(&self) -> Result<Vec<Template>, StorageError> {
        Ok(sqlx::query_as(
            "SELECT scope,event,title,body FROM notification_templates ORDER BY scope,event",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    /// Upserts a validated template.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn save_notification_template(&self, t: &Template) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO notification_templates(scope,event,title,body) VALUES(?,?,?,?) ON CONFLICT(scope,event) DO UPDATE SET title=excluded.title,body=excluded.body").bind(&t.scope).bind(&t.event).bind(&t.title).bind(&t.body).execute(&self.pool).await?;
        Ok(())
    }
    /// Removes an override so its fallback applies.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn reset_notification_template(
        &self,
        scope: &str,
        event: &str,
    ) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM notification_templates WHERE scope=? AND event=?")
            .bind(scope)
            .bind(event)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// One row per channel/event key. Enqueue also checks revision and a hard pending cap.
    /// Enqueues one delivery subject to revision, idempotency and pending limits.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn enqueue_notification(
        &self,
        channel: &Channel,
        event: &str,
        key: &str,
        payload: &str,
        test: bool,
    ) -> Result<Option<String>, StorageError> {
        let id = uuid::Uuid::now_v7().to_string();
        let n = sqlx::query("INSERT INTO notification_deliveries(id,channel_id,event,event_key,payload,is_test,channel_revision) SELECT ?,?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM notification_channels WHERE id=? AND revision=?) AND (SELECT count(*) FROM notification_deliveries WHERE status IN ('pending','retry','sending'))<10000 ON CONFLICT(channel_id,event_key) DO NOTHING")
            .bind(&id).bind(&channel.id).bind(event).bind(key).bind(payload).bind(test).bind(channel.revision).bind(&channel.id).bind(channel.revision).execute(&self.pool).await?.rows_affected();
        Ok((n > 0).then_some(id))
    }
    /// UPDATE RETURNING is atomic across worker processes. Expired leases recover on restart.
    /// Acquires a due delivery with a crash recovery lease.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn claim_notification(
        &self,
        lease: &str,
        id: Option<&str>,
    ) -> Result<Option<Pending>, StorageError> {
        Ok(sqlx::query_as("UPDATE notification_deliveries SET status='sending',attempts=attempts+1,lease=?,lease_until=unixepoch()+60,updated_at=unixepoch() WHERE id=(SELECT id FROM notification_deliveries WHERE ((status IN ('pending','retry') AND next_attempt<=unixepoch()) OR (status='sending' AND lease_until<unixepoch())) AND attempts<5 AND (? IS NULL AND is_test=0 OR id=?) ORDER BY next_attempt,id LIMIT 1) RETURNING id,channel_id,payload,attempts,channel_revision")
            .bind(lease).bind(id).bind(id).fetch_optional(&self.pool).await?)
    }
    /// Records a safe outcome only for the active lease owner.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn finish_notification(
        &self,
        id: &str,
        lease: &str,
        status: &str,
        summary: &str,
        duration: i64,
        retry_seconds: i64,
    ) -> Result<(), StorageError> {
        sqlx::query("UPDATE notification_deliveries SET status=?,summary=?,duration_ms=?,next_attempt=unixepoch()+?,lease=NULL,lease_until=NULL,payload=CASE WHEN ?='retry' THEN payload ELSE '{}' END,updated_at=unixepoch() WHERE id=? AND lease=? AND status='sending'")
            .bind(status).bind(summary).bind(duration).bind(retry_seconds).bind(status).bind(id).bind(lease).execute(&self.pool).await?;
        Ok(())
    }
    /// Returns a capped page without message contents or credentials.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn notification_deliveries(
        &self,
        before: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Delivery>, StorageError> {
        Ok(sqlx::query_as("SELECT id,channel_id,event,status,attempts,next_attempt,summary,duration_ms,is_test,created_at,updated_at FROM notification_deliveries WHERE (? IS NULL OR id<?) ORDER BY id DESC LIMIT ?")
            .bind(before).bind(before).bind(limit.clamp(1,200)).fetch_all(&self.pool).await?)
    }
    /// Recovers abandoned tests and enforces terminal log retention.
    ///
    /// # Errors
    /// Returns a storage error if SQLite fails or stored credentials cannot be decoded.
    pub async fn prune_notifications(&self) -> Result<(), StorageError> {
        // Failed final leases and abandoned synchronous tests must not stay stuck forever.
        sqlx::query("UPDATE notification_deliveries SET status='failed',payload='{}',summary='Delivery lease expired',updated_at=unixepoch() WHERE (status='sending' AND lease_until<unixepoch() AND (attempts>=5 OR is_test=1)) OR (is_test=1 AND status='pending' AND created_at<unixepoch()-60)").execute(&self.pool).await?;
        sqlx::query("DELETE FROM notification_deliveries WHERE status IN ('sent','failed','cancelled') AND (created_at<unixepoch()-2592000 OR id IN (SELECT id FROM notification_deliveries WHERE status IN ('sent','failed','cancelled') ORDER BY id DESC LIMIT -1 OFFSET 2000))").execute(&self.pool).await?;
        Ok(())
    }
}
fn decode(r: ChannelRow, vault: &SecretVault) -> Result<Channel, StorageError> {
    let secrets = vault.decrypt_scoped(
        &format!("indexloom:notification:v1:{}", r.id),
        &r.nonce,
        &r.ciphertext,
    )?;
    Ok(Channel {
        id: r.id,
        name: r.name,
        provider: r.provider,
        enabled: r.enabled,
        config: r.config,
        events: r.events,
        secrets,
        revision: r.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn setup() -> (Store, SecretVault, Channel) {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let vault = SecretVault::generate().unwrap();
        let c = Channel {
            id: "test".into(),
            name: "Test".into(),
            provider: "webhook".into(),
            enabled: true,
            config: "{}".into(),
            events: "[]".into(),
            secrets: BTreeMap::from([("url".into(), "https://secret.example/token".into())]),
            revision: 0,
        };
        assert!(store.save_notification_channel(&c, &vault).await.unwrap());
        let c = store
            .notification_channel("test", &vault)
            .await
            .unwrap()
            .unwrap();
        (store, vault, c)
    }
    #[tokio::test]
    async fn credentials_are_encrypted_and_bound_to_channel_identity() {
        let (s, v, c) = setup().await;
        let row: ChannelRow = sqlx::query_as("SELECT * FROM notification_channels WHERE id='test'")
            .fetch_one(&s.pool)
            .await
            .unwrap();
        assert!(!row.ciphertext.windows(6).any(|w| w == b"secret"));
        assert_eq!(row.nonce.len(), 24);
        assert!(
            v.decrypt_scoped(
                "indexloom:notification:v1:other",
                &row.nonce,
                &row.ciphertext
            )
            .is_err()
        );
        assert_eq!(c.secrets["url"], "https://secret.example/token");
        assert!(!format!("{c:?}").contains("secret.example"));
        sqlx::query("UPDATE notification_channels SET ciphertext=x'00' WHERE id='test'")
            .execute(&s.pool)
            .await
            .unwrap();
        assert!(s.notification_channel("test", &v).await.is_err());
    }
    #[tokio::test]
    async fn expired_lease_retry_limit_and_retention_are_durable() {
        let (s, _, c) = setup().await;
        let id = s
            .enqueue_notification(&c, "auth.login", "key", "{}", false)
            .await
            .unwrap()
            .unwrap();
        assert!(
            s.enqueue_notification(&c, "auth.login", "key", "{}", false)
                .await
                .unwrap()
                .is_none()
        );
        for attempt in 1..=5 {
            let lease = format!("lease-{attempt}");
            let pending = s.claim_notification(&lease, None).await.unwrap().unwrap();
            assert_eq!(pending.attempts, attempt);
            assert!(
                s.claim_notification("parallel", None)
                    .await
                    .unwrap()
                    .is_none()
            );
            // A stale completion cannot overwrite another worker's lease.
            s.finish_notification(&id, "wrong", "sent", "wrong", 1, 0)
                .await
                .unwrap();
            assert_eq!(
                s.notification_deliveries(None, 10).await.unwrap()[0].status,
                "sending"
            );
            sqlx::query("UPDATE notification_deliveries SET lease_until=unixepoch()-1 WHERE id=?")
                .bind(&id)
                .execute(&s.pool)
                .await
                .unwrap();
        }
        assert!(s.claim_notification("sixth", None).await.unwrap().is_none());
        s.prune_notifications().await.unwrap();
        assert_eq!(
            s.notification_deliveries(None, 10).await.unwrap()[0].status,
            "failed"
        );
        sqlx::query("UPDATE notification_deliveries SET created_at=unixepoch()-2592001")
            .execute(&s.pool)
            .await
            .unwrap();
        s.prune_notifications().await.unwrap();
        assert!(
            s.notification_deliveries(None, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn edits_cancel_queued_messages_and_delete_cascades_templates_and_logs() {
        let (s, v, mut c) = setup().await;
        s.enqueue_notification(&c, "auth.login", "key", "{\"body\":\"old message\"}", false)
            .await
            .unwrap();
        s.save_notification_template(&Template {
            scope: c.id.clone(),
            event: "auth.login".into(),
            title: "test".into(),
            body: "test".into(),
        })
        .await
        .unwrap();
        c.name = "Updated".into();
        assert!(s.save_notification_channel(&c, &v).await.unwrap());
        assert!(!s.save_notification_channel(&c, &v).await.unwrap());
        assert_eq!(
            s.notification_deliveries(None, 10).await.unwrap()[0].status,
            "cancelled"
        );
        assert!(
            s.enqueue_notification(&c, "auth.login", "stale-channel", "{}", false)
                .await
                .unwrap()
                .is_none()
        );
        assert!(s.delete_notification_channel(&c.id).await.unwrap());
        assert!(
            s.notification_deliveries(None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(s.notification_templates().await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[tokio::test]
    async fn terminal_history_has_a_hard_count_bound_and_pending_rows_survive() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO notification_channels(id,name,provider,config,events,nonce,ciphertext) VALUES('channel','test','webhook','{}','[]',x'00',x'00')").execute(&store.pool).await.unwrap();
        sqlx::query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<2005) INSERT INTO notification_deliveries(id,channel_id,event,event_key,channel_revision,payload,status) SELECT printf('%08d',x),'channel','test',CAST(x AS TEXT),1,'{}','sent' FROM n").execute(&store.pool).await.unwrap();
        sqlx::query("INSERT INTO notification_deliveries(id,channel_id,event,event_key,channel_revision,payload) VALUES('pending','channel','test','pending',1,'{}')").execute(&store.pool).await.unwrap();
        store.prune_notifications().await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notification_deliveries WHERE status='sent'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(count, 2000);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notification_deliveries WHERE status='pending'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    #[tokio::test]
    async fn encrypted_channels_and_outbox_survive_database_reopen() {
        let path = std::env::temp_dir().join(format!(
            "indexloom-notification-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let vault = SecretVault::generate().unwrap();
        let store = Store::connect_path(&path).await.unwrap();
        let c = Channel {
            id: "persistent".into(),
            name: "Restart fixture".into(),
            provider: "webhook".into(),
            enabled: true,
            config: "{}".into(),
            events: "[]".into(),
            secrets: BTreeMap::from([("url".into(), "https://example.invalid/dummy".into())]),
            revision: 0,
        };
        store.save_notification_channel(&c, &vault).await.unwrap();
        let c = store
            .notification_channel(&c.id, &vault)
            .await
            .unwrap()
            .unwrap();
        let id = store
            .enqueue_notification(&c, "auth.login", "persistent-event", "{}", false)
            .await
            .unwrap()
            .unwrap();
        store
            .claim_notification("crashed-worker", None)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE notification_deliveries SET lease_until=unixepoch()-1")
            .execute(&store.pool)
            .await
            .unwrap();
        store.pool.close().await;
        let reopened = Store::connect_path(&path).await.unwrap();
        let restored = reopened
            .notification_channel(&c.id, &vault)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.secrets, c.secrets);
        let pending = reopened
            .claim_notification("restarted-worker", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.id, id);
        assert_eq!(pending.attempts, 2);
        reopened.pool.close().await;
        std::fs::remove_file(path).unwrap();
    }
}
