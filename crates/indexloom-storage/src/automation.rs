//! Durable quotas, leased jobs and transactional RSS baselines.
use crate::{StorageError, Store};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchQuota {
    pub indexer_id: String,
    pub hourly_limit: i64,
    pub daily_limit: i64,
    #[serde(default)]
    pub hour_used: i64,
    #[serde(default)]
    pub day_used: i64,
    #[serde(default)]
    #[sqlx(default)]
    pub retry_after_seconds: i64,
    #[serde(default)]
    #[sqlx(default)]
    pub next_allowed_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledJob {
    pub id: String,
    pub kind: String,
    pub target: Option<String>,
    pub interval_seconds: i64,
    pub enabled: bool,
    #[serde(default)]
    pub next_run: i64,
    #[serde(default)]
    pub last_run: Option<i64>,
    #[serde(default)]
    pub last_outcome: Option<String>,
}
#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RssHistory {
    pub id: i64,
    pub indexer_id: String,
    pub title: String,
    pub created_at: i64,
}
#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RssState {
    pub indexer_id: String,
    pub baseline_at: i64,
    pub last_scan: i64,
}
fn quota_retry_seconds(
    now: i64,
    hourly_limit: i64,
    hour_used: i64,
    daily_limit: i64,
    day_used: i64,
) -> i64 {
    let hour = if hourly_limit > 0 && hour_used >= hourly_limit {
        3600 - now.rem_euclid(3600)
    } else {
        0
    };
    let day = if daily_limit > 0 && day_used >= daily_limit {
        86400 - now.rem_euclid(86400)
    } else {
        0
    };
    hour.max(day)
}

impl Store {
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn search_quotas(&self) -> Result<Vec<SearchQuota>, StorageError> {
        let mut quotas: Vec<SearchQuota> = sqlx::query_as("SELECT indexer_id,hourly_limit,daily_limit,CASE WHEN hour_bucket=unixepoch()/3600 THEN hour_used ELSE 0 END AS hour_used,CASE WHEN day_bucket=unixepoch()/86400 THEN day_used ELSE 0 END AS day_used FROM indexer_search_quotas ORDER BY indexer_id").fetch_all(&self.pool).await?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        for quota in &mut quotas {
            quota.retry_after_seconds = quota_retry_seconds(
                now,
                quota.hourly_limit,
                quota.hour_used,
                quota.daily_limit,
                quota.day_used,
            );
            quota.next_allowed_at =
                (quota.retry_after_seconds > 0).then_some(now + quota.retry_after_seconds);
        }
        Ok(quotas)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn save_search_quota(
        &self,
        id: &str,
        hour: i64,
        day: i64,
    ) -> Result<(), StorageError> {
        if hour < 0 || day < 0 {
            return Err(StorageError::InvalidData("negative quota".into()));
        }
        sqlx::query("INSERT INTO indexer_search_quotas(indexer_id,hourly_limit,daily_limit) VALUES(?,?,?) ON CONFLICT(indexer_id) DO UPDATE SET hourly_limit=excluded.hourly_limit,daily_limit=excluded.daily_limit").bind(id).bind(hour).bind(day).execute(&self.pool).await?;
        Ok(())
    }
    /// One SQL statement reserves both budgets under the SQLite write lock. Zero means unlimited.
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn reserve_search_quota(&self, id: &str) -> Result<bool, StorageError> {
        let result=sqlx::query("UPDATE indexer_search_quotas SET hour_used=CASE WHEN hour_bucket=unixepoch()/3600 THEN hour_used+1 ELSE 1 END,day_used=CASE WHEN day_bucket=unixepoch()/86400 THEN day_used+1 ELSE 1 END,hour_bucket=unixepoch()/3600,day_bucket=unixepoch()/86400 WHERE indexer_id=? AND (hourly_limit=0 OR hour_bucket!=unixepoch()/3600 OR hour_used<hourly_limit) AND (daily_limit=0 OR day_bucket!=unixepoch()/86400 OR day_used<daily_limit)").bind(id).execute(&self.pool).await?;
        if result.rows_affected() > 0 {
            return Ok(true);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM indexer_search_quotas WHERE indexer_id=?)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(!exists)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn scheduled_jobs(&self) -> Result<Vec<ScheduledJob>, StorageError> {
        Ok(sqlx::query_as("SELECT id,kind,target,interval_seconds,enabled,next_run,last_run,last_outcome FROM scheduled_jobs ORDER BY id").fetch_all(&self.pool).await?)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn save_scheduled_job(
        &self,
        j: &ScheduledJob,
        only_if_missing: bool,
    ) -> Result<(), StorageError> {
        let sql = if only_if_missing {
            "INSERT INTO scheduled_jobs(id,kind,target,interval_seconds,enabled,next_run) VALUES(?,?,?,?,?,unixepoch()+?) ON CONFLICT(id) DO NOTHING"
        } else {
            "INSERT INTO scheduled_jobs(id,kind,target,interval_seconds,enabled,next_run) VALUES(?,?,?,?,?,unixepoch()+?) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,target=excluded.target,interval_seconds=excluded.interval_seconds,enabled=excluded.enabled,next_run=excluded.next_run WHERE scheduled_jobs.lease_until<=unixepoch()"
        };
        let result = sqlx::query(sql)
            .bind(&j.id)
            .bind(&j.kind)
            .bind(&j.target)
            .bind(j.interval_seconds)
            .bind(j.enabled)
            .bind(j.interval_seconds)
            .execute(&self.pool)
            .await?;
        if !only_if_missing && result.rows_affected() == 0 {
            return Err(StorageError::InvalidData("job is currently running".into()));
        }
        Ok(())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn delete_scheduled_job(&self, id: &str) -> Result<bool, StorageError> {
        Ok(
            sqlx::query("DELETE FROM scheduled_jobs WHERE id=? AND lease_until<=unixepoch()")
                .bind(id)
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }
    /// Schedules one enabled idle job for its next polling cycle.
    /// # Errors
    /// Returns a database error if the scheduled job cannot be updated.
    pub async fn queue_scheduled_job_now(&self, id: &str) -> Result<bool, StorageError> {
        let update = sqlx::query(
            "UPDATE scheduled_jobs SET next_run=0 WHERE id=? AND enabled=1 AND lease_until<=unixepoch()",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(update.rows_affected() == 1)
    }

    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn claim_scheduled_job(&self, id: &str, token: &str) -> Result<bool, StorageError> {
        Ok(sqlx::query("UPDATE scheduled_jobs SET lease_until=unixepoch()+300,lease_token=? WHERE id=? AND enabled=1 AND next_run<=unixepoch() AND lease_until<=unixepoch()")
   .bind(token).bind(id).execute(&self.pool).await?.rows_affected()==1)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn finish_scheduled_job(
        &self,
        id: &str,
        token: &str,
        success: bool,
    ) -> Result<(), StorageError> {
        sqlx::query("UPDATE scheduled_jobs SET lease_until=0,lease_token=NULL,last_run=unixepoch(),last_outcome=?,next_run=unixepoch()+interval_seconds WHERE id=? AND lease_token=?")
   .bind(if success {"success"} else {"failure"}).bind(id).bind(token).execute(&self.pool).await?;
        Ok(())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn record_rss_scan(
        &self,
        id: &str,
        entries: &[(String, String)],
    ) -> Result<usize, StorageError> {
        let mut tx = self.pool.begin().await?;
        // Establish baseline under a write lock before examining seen identities.
        let baseline=sqlx::query("INSERT INTO rss_scan_state(indexer_id,baseline_at,last_scan) VALUES(?,unixepoch(),unixepoch()) ON CONFLICT(indexer_id) DO NOTHING").bind(id).execute(&mut *tx).await?.rows_affected()==1;
        let mut added = 0;
        for (identity, title) in entries {
            let new=sqlx::query("INSERT INTO rss_seen(indexer_id,identity,first_seen) VALUES(?,?,unixepoch()) ON CONFLICT DO NOTHING").bind(id).bind(identity).execute(&mut *tx).await?.rows_affected()==1;
            if new && !baseline {
                sqlx::query("INSERT INTO rss_history(indexer_id,identity,title,created_at) VALUES(?,?,?,unixepoch()) ON CONFLICT DO NOTHING").bind(id).bind(identity).bind(title).execute(&mut *tx).await?;
                added += 1;
            }
        }
        sqlx::query("UPDATE rss_scan_state SET last_scan=unixepoch() WHERE indexer_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(added)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn rss_scan_states(&self) -> Result<Vec<RssState>, StorageError> {
        Ok(sqlx::query_as(
            "SELECT indexer_id,baseline_at,last_scan FROM rss_scan_state ORDER BY indexer_id",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn rss_history(&self) -> Result<Vec<RssHistory>, StorageError> {
        Ok(sqlx::query_as(
            "SELECT id,indexer_id,title,created_at FROM rss_history ORDER BY id DESC LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn automation_maintenance(&self) -> Result<(), StorageError> {
        self.purge_expired_auth_state().await?;
        sqlx::query("DELETE FROM integration_events WHERE id NOT IN (SELECT id FROM integration_events ORDER BY id DESC LIMIT 10000)").execute(&self.pool).await?;
        sqlx::query("DELETE FROM rss_history WHERE id NOT IN (SELECT id FROM rss_history ORDER BY id DESC LIMIT 10000)").execute(&self.pool).await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn store() -> Store {
        let s = Store::connect("sqlite::memory:").await.unwrap();
        s.upsert_definition("d", "test", None, "v1", "yaml", "hash")
            .await
            .unwrap();
        s.create_indexer("i", "d", "Indexer", true).await.unwrap();
        s
    }
    #[test]
    fn hourly_and_daily_quota_recovery_time_is_maximum_reset_boundary() {
        assert_eq!(quota_retry_seconds(3601, 0, 0, 0, 0), 0);
        assert_eq!(quota_retry_seconds(3601, 1, 1, 0, 0), 3599);
        assert_eq!(quota_retry_seconds(3601, 1, 1, 2, 2), 82799);
        assert_eq!(quota_retry_seconds(86400, 4, 0, 1, 1), 86400);
    }

    #[tokio::test]
    async fn budgets_and_baseline() {
        let s = store().await;
        s.save_search_quota("i", 1, 2).await.unwrap();
        let (a, b) = tokio::join!(s.reserve_search_quota("i"), s.reserve_search_quota("i"));
        assert_ne!(a.unwrap(), b.unwrap());
        assert_eq!(
            s.record_rss_scan("i", &[("a".into(), "A".into())])
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            s.record_rss_scan("i", &[("a".into(), "A".into()), ("b".into(), "B".into())])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            s.record_rss_scan("i", &[("b".into(), "B".into())])
                .await
                .unwrap(),
            0
        );
        assert_eq!(s.rss_history().await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod durability_tests {
    use super::*;
    #[tokio::test]
    async fn leases_and_rss_state_survive_reconnect() {
        let path =
            std::env::temp_dir().join(format!("indexloom-stage-c-{}.db", uuid::Uuid::now_v7()));
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let store = Store::connect(&url).await.unwrap();
        let job = ScheduledJob {
            id: "test".into(),
            kind: "maintenance".into(),
            target: None,
            interval_seconds: 60,
            enabled: true,
            next_run: 0,
            last_run: None,
            last_outcome: None,
        };
        store.save_scheduled_job(&job, false).await.unwrap();
        sqlx::query("UPDATE scheduled_jobs SET next_run=0")
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(store.claim_scheduled_job("test", "lease-1").await.unwrap());
        assert!(!store.claim_scheduled_job("test", "lease-2").await.unwrap());
        assert!(store.save_scheduled_job(&job, false).await.is_err());
        store
            .finish_scheduled_job("test", "wrong-lease", true)
            .await
            .unwrap();
        assert!(!store.claim_scheduled_job("test", "lease-2").await.unwrap());
        store.pool.close().await;
        let store = Store::connect(&url).await.unwrap();
        assert!(!store.claim_scheduled_job("test", "lease-2").await.unwrap());
        sqlx::query("UPDATE scheduled_jobs SET lease_until=0")
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(store.claim_scheduled_job("test", "lease-2").await.unwrap());
        store
            .finish_scheduled_job("test", "lease-1", true)
            .await
            .unwrap();
        assert_eq!(store.scheduled_jobs().await.unwrap()[0].last_outcome, None);
        store
            .finish_scheduled_job("test", "lease-2", false)
            .await
            .unwrap();
        assert_eq!(
            store.scheduled_jobs().await.unwrap()[0]
                .last_outcome
                .as_deref(),
            Some("failure")
        );
        assert!(!store.claim_scheduled_job("test", "lease-3").await.unwrap());
        store.pool.close().await;
        std::fs::remove_file(path).unwrap();
    }
}
