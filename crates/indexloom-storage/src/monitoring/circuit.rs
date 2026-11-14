//! Three failures trip the circuit; capped exponential backoff and one leased recovery probe.
use super::{Failure, Operation, timestamp};
use crate::{StorageError, Store};
use sqlx::{Sqlite, Transaction};

pub(super) async fn complete(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    failure: Option<Failure>,
    now: i64,
) -> Result<(), StorageError> {
    if matches!(failure, Some(Failure::Quota | Failure::Request)) {
        sqlx::query("UPDATE indexer_monitoring SET probe_until=0 WHERE indexer_id=?")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }
    if let Some(failure) = failure {
        sqlx::query("UPDATE indexer_monitoring SET consecutive_failures=MIN(consecutive_failures+1,30),failure_class=?,probe_until=0,open_until=CASE WHEN consecutive_failures+1 >= 3 THEN ? + MIN(3600, ? * (1 << MIN(MAX(consecutive_failures-2,0),7))) ELSE 0 END WHERE indexer_id=?")
            .bind(failure.code()).bind(now).bind(if failure == Failure::RateLimited {120} else {30}).bind(id).execute(&mut **tx).await?;
    } else {
        sqlx::query("UPDATE indexer_monitoring SET consecutive_failures=0,open_until=0,probe_until=0,failure_class=NULL WHERE indexer_id=?").bind(id).execute(&mut **tx).await?;
    }
    Ok(())
}
impl Store {
    /// Claims a single recovery probe after backoff; abandoned probes expire in five minutes.
    /// # Errors
    /// Returns database errors; dispatch must fail closed when storage is unavailable.
    pub async fn claim_indexer_dispatch(&self, id: &str) -> Result<bool, StorageError> {
        self.claim_dispatch_at(id, timestamp()).await
    }
    pub(super) async fn claim_dispatch_at(&self, id: &str, now: i64) -> Result<bool, StorageError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO indexer_monitoring(indexer_id) VALUES(?) ON CONFLICT DO NOTHING")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let changed = sqlx::query("UPDATE indexer_monitoring SET probe_until=CASE WHEN consecutive_failures>=3 THEN ?+300 ELSE 0 END WHERE indexer_id=? AND open_until<=? AND probe_until<=?").bind(now).bind(id).bind(now).bind(now).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(changed == 1)
    }
    /// Whether the admitted operation must bypass cached data to test upstream recovery.
    /// # Errors
    /// Returns database errors.
    pub async fn indexer_recovery_probe(&self, id: &str) -> Result<bool, StorageError> {
        Ok(sqlx::query_scalar(
            "SELECT COALESCE((SELECT probe_until>0 FROM indexer_monitoring WHERE indexer_id=?),0)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?)
    }
    /// Resets automatic state without changing the user's enabled preference.
    /// # Errors
    /// Returns database errors or `NotFound` for an unknown indexer.
    pub async fn reset_indexer_circuit(&self, id: &str) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM indexers WHERE id=?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        if !exists {
            return Err(StorageError::NotFound("indexer".into()));
        }
        sqlx::query("UPDATE indexer_monitoring SET consecutive_failures=0,open_until=0,probe_until=0,failure_class=NULL WHERE indexer_id=?").bind(id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO monitoring_events(timestamp,category,indexer_id,status,message,count) VALUES(?, ?, ?, 'success','circuit reset',1)").bind(timestamp()).bind(Operation::Reset.category()).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
