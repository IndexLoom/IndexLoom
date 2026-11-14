//! Durable, privacy-preserving observations. Text is selected from enums, never upstream input.
use crate::{StorageError, Store};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

pub mod circuit;
#[cfg(test)]
mod tests;

pub const EVENT_LIMIT: i64 = 10_000;
pub const EVENT_RETENTION_SECONDS: i64 = 30 * 86400;

#[derive(Clone, Copy, Debug)]
pub enum Operation {
    Search,
    Test,
    Download,
    Auth,
    AuthPending,
    Rss,
    Reset,
}
impl Operation {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Test => "test",
            Self::Download => "download",
            Self::Auth | Self::AuthPending => "auth",
            Self::Rss => "rss",
            Self::Reset => "reset",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Authentication,
    RateLimited,
    Network,
    Upstream,
    Configuration,
    Quota,
    Request,
}
impl Failure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::RateLimited => "rate-limited",
            Self::Network => "network",
            Self::Upstream => "upstream",
            Self::Configuration => "configuration",
            Self::Quota => "quota",
            Self::Request => "request",
        }
    }
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct IndexerStatistics {
    pub indexer_id: String,
    pub display_name: String,
    pub enabled: bool,
    pub searches: i64,
    pub downloads: i64,
    pub failed_queries: i64,
    pub auth_errors: i64,
    pub rss: i64,
    pub response_count: i64,
    pub response_sum_ms: i64,
    pub average_response_ms: f64,
    pub results: i64,
    pub consecutive_failures: i64,
    pub open_until: i64,
    pub probe_until: i64,
    pub failure_class: Option<String>,
    pub last_activity: Option<i64>,
    pub last_success: Option<i64>,
}
#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub id: i64,
    pub timestamp: i64,
    pub category: String,
    pub indexer_id: Option<String>,
    pub status: String,
    pub message: String,
    pub count: i64,
}
/// Cursor pagination remains stable when new events are appended. All values are bound.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventFilter {
    pub before: Option<i64>,
    pub indexer_id: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: Option<i64>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub events: Vec<ActivityEvent>,
    pub next_before: Option<i64>,
}

pub fn timestamp() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl Store {
    /// Atomically updates counters, circuit state and the bounded audit log.
    /// # Errors
    /// Returns database errors; callers must not substitute unsanitized error messages.
    pub async fn observe(
        &self,
        id: Option<&str>,
        operation: Operation,
        failure: Option<Failure>,
        elapsed_ms: u64,
        count: usize,
    ) -> Result<(), StorageError> {
        self.observe_at(id, operation, failure, elapsed_ms, count, timestamp())
            .await
    }
    async fn observe_at(
        &self,
        id: Option<&str>,
        operation: Operation,
        failure: Option<Failure>,
        elapsed_ms: u64,
        count: usize,
        now: i64,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        if let Some(id) = id {
            sqlx::query(
                "INSERT INTO indexer_monitoring(indexer_id) VALUES (?) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE indexer_monitoring SET searches=searches+?,downloads=downloads+?,failed_queries=failed_queries+?,auth_errors=auth_errors+?,rss=rss+?,response_count=response_count+1,response_sum_ms=response_sum_ms+?,results=results+?,last_activity=?,last_success=CASE WHEN ? THEN ? ELSE last_success END WHERE indexer_id=?")
                .bind(matches!(operation, Operation::Search)).bind(matches!(operation, Operation::Download))
                .bind(failure.is_some() && matches!(operation, Operation::Search | Operation::Rss))
                .bind(failure == Some(Failure::Authentication)).bind(matches!(operation, Operation::Rss))
                .bind(i64::try_from(elapsed_ms).unwrap_or(i64::MAX / 1000).min(86_400_000))
                .bind(i64::try_from(count).unwrap_or(i64::MAX / 1000)).bind(now).bind(failure.is_none() && !matches!(operation,Operation::AuthPending)).bind(now).bind(id)
                .execute(&mut *tx).await?;
            if !matches!(operation, Operation::AuthPending) {
                circuit::complete(&mut tx, id, failure, now).await?;
            }
        }
        if id.is_none() {
            sqlx::query("INSERT INTO monitoring_global(category,operations,failures,response_sum_ms) VALUES(?,1,?,?) ON CONFLICT(category) DO UPDATE SET operations=operations+1, failures=failures+excluded.failures,response_sum_ms=response_sum_ms+excluded.response_sum_ms")
                .bind(operation.category()).bind(failure.is_some()).bind(i64::try_from(elapsed_ms).unwrap_or(86_400_000).min(86_400_000)).execute(&mut *tx).await?;
        }
        let status = if failure.is_some() {
            "error"
        } else if matches!(operation, Operation::AuthPending) {
            "pending"
        } else {
            "success"
        };
        let message = failure.map_or("operation completed", Failure::code);
        sqlx::query("INSERT INTO monitoring_events(timestamp,category,indexer_id,status,message,count) VALUES(?,?,?,?,?,?)")
            .bind(now).bind(operation.category()).bind(id).bind(status).bind(message).bind(1_i64).execute(&mut *tx).await?;
        if failure.is_some()
            || (count > 0 && matches!(operation, Operation::Search | Operation::Rss))
        {
            sqlx::query("INSERT INTO monitoring_events(timestamp,category,indexer_id,status,message,count) VALUES(?,?,?,?,?,?)")
                .bind(now).bind(if failure.is_some() {"errors"} else {"results"}).bind(id).bind(status).bind(message)
                .bind(if failure.is_some() {1} else {i64::try_from(count).unwrap_or(i64::MAX / 1000)}).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// # Errors
    /// Returns database errors.
    pub async fn indexer_statistics(&self) -> Result<Vec<IndexerStatistics>, StorageError> {
        Ok(sqlx::query_as("SELECT i.id AS indexer_id,i.display_name,i.enabled,COALESCE(m.searches,0) searches,COALESCE(m.downloads,0) downloads,COALESCE(m.failed_queries,0) failed_queries,COALESCE(m.auth_errors,0) auth_errors,COALESCE(m.rss,0) rss,COALESCE(m.response_count,0) response_count,COALESCE(m.response_sum_ms,0) response_sum_ms,COALESCE(1.0*m.response_sum_ms/NULLIF(m.response_count,0),0.0) average_response_ms,COALESCE(m.results,0) results,COALESCE(m.consecutive_failures,0) consecutive_failures,COALESCE(m.open_until,0) open_until,COALESCE(m.probe_until,0) probe_until,m.failure_class,m.last_activity,m.last_success FROM indexers i LEFT JOIN indexer_monitoring m ON m.indexer_id=i.id ORDER BY i.id").fetch_all(&self.pool).await?)
    }
    /// # Errors
    /// Returns database errors. The page size is always clamped to 1..200.
    pub async fn monitoring_events(&self, filter: &EventFilter) -> Result<EventPage, StorageError> {
        let limit = filter.limit.unwrap_or(50).clamp(1, 200);
        let mut events: Vec<ActivityEvent> = sqlx::query_as("SELECT * FROM monitoring_events WHERE timestamp >= ? AND (? IS NULL OR timestamp <= ?) AND (? IS NULL OR id < ?) AND (? IS NULL OR indexer_id=?) AND (? IS NULL OR category=?) AND (? IS NULL OR status=?) ORDER BY id DESC LIMIT ?")
            .bind(filter.since.unwrap_or(0).max(timestamp()-EVENT_RETENTION_SECONDS))
            .bind(filter.until).bind(filter.until)
            .bind(filter.before).bind(filter.before).bind(&filter.indexer_id).bind(&filter.indexer_id)
            .bind(&filter.category).bind(&filter.category).bind(&filter.status).bind(&filter.status).bind(limit+1).fetch_all(&self.pool).await?;
        let more = events.len() > usize::try_from(limit).unwrap_or(200);
        events.truncate(usize::try_from(limit).unwrap_or(200));
        let next_before = if more {
            events.last().map(|e| e.id)
        } else {
            None
        };
        Ok(EventPage {
            events,
            next_before,
        })
    }
}

impl Store {
    /// Permanent failed global authentication operations, without actor identifiers.
    /// # Errors
    /// Returns database errors.
    pub async fn global_authentication_errors(&self) -> Result<i64, StorageError> {
        Ok(sqlx::query_scalar(
            "SELECT COALESCE((SELECT failures FROM monitoring_global WHERE category='auth'),0)",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    /// Curated component metadata only: never endpoints, settings or credentials.
    /// # Errors
    /// Returns database errors.
    pub async fn monitoring_components(&self) -> Result<serde_json::Value, StorageError> {
        let proxies: Vec<(String, bool)> =
            sqlx::query_as("SELECT id,enabled FROM outbound_proxies ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        let applications: Vec<(String,bool,Option<String>,Option<i64>)> = sqlx::query_as("SELECT i.id,i.enabled,(SELECT outcome FROM integration_events e WHERE e.integration_id=i.id ORDER BY e.id DESC LIMIT 1),(SELECT unixepoch(created_at) FROM integration_events e WHERE e.integration_id=i.id ORDER BY e.id DESC LIMIT 1) FROM integrations i ORDER BY i.id").fetch_all(&self.pool).await?;
        Ok(serde_json::json!({
            "proxies":proxies.into_iter().map(|(id,enabled)|serde_json::json!({"id":id,"enabled":enabled,"status":"unknown"})).collect::<Vec<_>>(),
            "applications":applications.into_iter().map(|(id,enabled,outcome,at)|serde_json::json!({"id":id,"enabled":enabled,"status":if !enabled {"disabled"} else if at.is_some_and(|t| timestamp().saturating_sub(t)>86400) {"stale"} else {match outcome.as_deref(){Some("success")=>"ok",Some(_)=>"degraded",None=>"unknown"}},"lastCheckedAt":at,"ageSeconds":at.map(|t|timestamp().saturating_sub(t))})).collect::<Vec<_>>()
        }))
    }
}
