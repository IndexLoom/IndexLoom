//! Administrator APIs and a persistent, leased scheduler. Nothing is sent to download clients.
mod rss;
use crate::{
    ApiError, AppState,
    catalog::{CatalogSource, synchronize},
    validate_identifier,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use indexloom_storage::automation::{RssHistory, RssState, ScheduledJob, SearchQuota};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/automation/jobs", get(jobs))
        .route(
            "/api/v1/automation/jobs/{id}",
            axum::routing::put(save_job).delete(delete_job),
        )
        .route(
            "/api/v1/automation/jobs/{id}/run",
            axum::routing::post(queue_job_now),
        )
        .route("/api/v1/automation/quotas", get(quotas))
        .route(
            "/api/v1/automation/quotas/{id}",
            axum::routing::put(save_quota),
        )
        .route("/api/v1/automation/rss/history", get(history))
        .route("/api/v1/automation/rss/state", get(scan_states))
}
async fn jobs(State(s): State<AppState>) -> Result<Json<Vec<ScheduledJob>>, ApiError> {
    Ok(Json(s.store.scheduled_jobs().await?))
}
async fn quotas(State(s): State<AppState>) -> Result<Json<Vec<SearchQuota>>, ApiError> {
    Ok(Json(s.store.search_quotas().await?))
}
async fn scan_states(State(s): State<AppState>) -> Result<Json<Vec<RssState>>, ApiError> {
    Ok(Json(s.store.rss_scan_states().await?))
}
async fn history(State(s): State<AppState>) -> Result<Json<Vec<RssHistory>>, ApiError> {
    Ok(Json(s.store.rss_history().await?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuotaInput {
    hourly_limit: i64,
    daily_limit: i64,
}
async fn save_quota(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(q): Json<QuotaInput>,
) -> Result<Json<Value>, ApiError> {
    validate_identifier(&id)?;
    if q.hourly_limit < 0 || q.daily_limit < 0 {
        return Err(ApiError::bad_request(
            "quota limits must be nonnegative; zero means unlimited",
        ));
    }
    s.store.get_indexer(&id, &s.vault).await?;
    s.store
        .save_search_quota(&id, q.hourly_limit, q.daily_limit)
        .await?;
    Ok(Json(json!({"saved":true})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JobInput {
    kind: String,
    target: Option<String>,
    interval_seconds: i64,
    enabled: bool,
}
async fn save_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<JobInput>,
) -> Result<Json<Value>, ApiError> {
    validate_identifier(&id)?;
    if !(60..=2_592_000).contains(&input.interval_seconds) {
        return Err(ApiError::bad_request(
            "job interval must be 60..2592000 seconds",
        ));
    }
    match input.kind.as_str() {
        "app-sync" => {
            let target = input
                .target
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("app-sync requires target"))?;
            validate_identifier(target)?;
            if s.store.get_integration(target).await?.is_none() {
                return Err(ApiError::not_found("integration not found"));
            }
        }
        "rss" => {
            let target = input
                .target
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("RSS requires target"))?;
            validate_identifier(target)?;
            s.store.get_indexer(target, &s.vault).await?;
        }
        "maintenance" | "catalog" if input.target.is_none() => {}
        _ => return Err(ApiError::bad_request("unsupported job kind or target")),
    }
    let job = ScheduledJob {
        id,
        kind: input.kind,
        target: input.target,
        interval_seconds: input.interval_seconds,
        enabled: input.enabled,
        next_run: 0,
        last_run: None,
        last_outcome: None,
    };
    s.store
        .save_scheduled_job(&job, false)
        .await
        .map_err(|error| match error {
            indexloom_storage::StorageError::InvalidData(_) => {
                ApiError::conflict("job is running; wait for completion before editing")
            }
            other => ApiError::from(other),
        })?;
    Ok(Json(json!({"saved":true})))
}
async fn delete_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_identifier(&id)?;
    if !s.store.delete_scheduled_job(&id).await? {
        return Err(ApiError::conflict("job is running or does not exist"));
    }
    Ok(Json(json!({"deleted":true})))
}
async fn queue_job_now(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_identifier(&id)?;
    if !s.store.queue_scheduled_job_now(&id).await? {
        return Err(ApiError::conflict("job is disabled, running, or missing"));
    }
    Ok(Json(json!({"queued":true})))
}

/// Run from the server executable once. Timed-out/crashed jobs become eligible after a 300s lease.
pub async fn run(state: AppState, source: CatalogSource, catalog_interval: u64) {
    for (id, kind, interval) in [
        ("system-maintenance", "maintenance", 3600),
        ("system-catalog", "catalog", catalog_interval),
    ] {
        if interval == 0 {
            continue;
        }
        let job = ScheduledJob {
            id: id.into(),
            kind: kind.into(),
            target: None,
            interval_seconds: i64::try_from(interval.min(2_592_000)).unwrap_or(2_592_000),
            enabled: true,
            next_run: 0,
            last_run: None,
            last_outcome: None,
        };
        if state.store.save_scheduled_job(&job, true).await.is_err() {
            tracing::warn!("could not initialize scheduler job");
        }
    }
    let mut ticker = tokio::time::interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;
        let Ok(jobs) = state.store.scheduled_jobs().await else {
            tracing::warn!("could not load scheduled jobs");
            continue;
        };
        for job in jobs {
            let lease = uuid::Uuid::now_v7().to_string();
            if !state
                .store
                .claim_scheduled_job(&job.id, &lease)
                .await
                .unwrap_or(false)
            {
                continue;
            }
            // Reload after claiming: an administrator could have edited the due job since listing.
            let active = state
                .store
                .scheduled_jobs()
                .await
                .ok()
                .and_then(|jobs| jobs.into_iter().find(|j| j.id == job.id));
            let success = if let Some(active) = active {
                tokio::time::timeout(Duration::from_secs(240), execute(&state, &source, &active))
                    .await
                    .is_ok_and(|r| r.is_ok())
            } else {
                false
            };
            if state
                .store
                .finish_scheduled_job(&job.id, &lease, success)
                .await
                .is_err()
            {
                tracing::warn!("could not finish scheduled job");
            }
        }
    }
}
async fn execute(
    state: &AppState,
    source: &CatalogSource,
    job: &ScheduledJob,
) -> Result<(), ApiError> {
    match job.kind.as_str() {
        "app-sync" => {
            crate::integrations::sync_id(
                state,
                job.target
                    .as_deref()
                    .ok_or_else(|| ApiError::bad_request("missing target"))?,
            )
            .await?;
        }
        "rss" => {
            rss::scan(
                state,
                job.target
                    .as_deref()
                    .ok_or_else(|| ApiError::bad_request("missing target"))?,
            )
            .await?;
        }
        "maintenance" => state.store.automation_maintenance().await?,
        "catalog" => {
            let report = synchronize(&state.store, source)
                .await
                .map_err(|_| ApiError::bad_gateway("catalog synchronization failed"))?;
            if report.updated {
                state.clear_execution_runtimes().await;
            }
        }
        _ => return Err(ApiError::bad_request("unknown job kind")),
    }
    Ok(())
}
