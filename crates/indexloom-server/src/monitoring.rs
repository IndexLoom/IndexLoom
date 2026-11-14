//! Authenticated dashboard and execution observation boundary.
use super::{ApiError, AppState, validate_identifier};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use indexloom_engine::EngineError;
use indexloom_storage::monitoring::{
    EventFilter, EventPage, Failure, IndexerStatistics, Operation,
};
use serde_json::{Value, json};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/monitoring/indexers", get(indexers))
        .route("/api/v1/monitoring/indexers/{id}", get(indexer))
        .route("/api/v1/monitoring/indexers/{id}/reset", post(reset))
        .route("/api/v1/monitoring/events", get(events))
        .route("/api/v1/monitoring/health", get(health))
}
async fn indexers(State(state): State<AppState>) -> Result<Json<Vec<IndexerStatistics>>, ApiError> {
    Ok(Json(state.store.indexer_statistics().await?))
}
async fn indexer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<IndexerStatistics>, ApiError> {
    validate_identifier(&id)?;
    let row = state
        .store
        .indexer_statistics()
        .await?
        .into_iter()
        .find(|r| r.indexer_id == id)
        .ok_or_else(|| ApiError::not_found("indexer was not found"))?;
    Ok(Json(row))
}
async fn reset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_identifier(&id)?;
    state.store.reset_indexer_circuit(&id).await?;
    Ok(Json(json!({"status":"reset"})))
}
async fn events(
    State(state): State<AppState>,
    Query(filter): Query<EventFilter>,
) -> Result<Json<EventPage>, ApiError> {
    Ok(Json(state.store.monitoring_events(&filter).await?))
}
async fn health(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let now = indexloom_storage::monitoring::timestamp();
    let mut warnings = Vec::new();
    let database = if state.store.ping().await.is_ok() {
        "ok"
    } else {
        "unavailable"
    };
    if database != "ok" {
        return Ok(Json(
            json!({"status":"degraded","database":database,"warnings":["Database unavailable"],"checkedAt":now}),
        ));
    }
    let statistics = state.store.indexer_statistics().await?;
    let indexers: Vec<_> = statistics.iter().map(|s| {
        let age = s.last_activity.map(|last|now.saturating_sub(last));
        let status = if !s.enabled {"disabled"} else if s.open_until>now {"open"} else if s.consecutive_failures>=3 {"half-open"} else if s.consecutive_failures>0 {"degraded"} else if s.last_activity.is_none() {"unknown"} else if age.is_some_and(|a|a>86400) {"stale"} else {"ok"};
        if s.enabled && matches!(status,"open"|"half-open"|"degraded"|"stale") { warnings.push(format!("Indexer {} is {status}",s.indexer_id)); }
        json!({"indexerId":s.indexer_id,"status":status,"ageSeconds":age,"retryAt":s.open_until.max(s.probe_until),"failureClass":s.failure_class})
    }).collect();
    let mut components = state.store.monitoring_components().await?;
    if let Some(applications) = components["applications"].as_array() {
        for app in applications {
            if matches!(app["status"].as_str(), Some("degraded" | "stale")) {
                warnings.push(format!(
                    "Integration {} needs attention",
                    app["id"].as_str().unwrap_or("unknown")
                ));
            }
        }
    }
    let jobs = state.store.scheduled_jobs().await?;
    let rss: Vec<_> = jobs.iter().filter(|j|j.kind=="rss").map(|j| {
        let overdue = j.enabled && now > j.next_run.saturating_add(j.interval_seconds);
        if overdue { warnings.push("RSS scheduler has overdue work".into()); }
        let failed = j.enabled && j.last_outcome.as_deref()==Some("failure");
        if failed {warnings.push("RSS scheduler last execution failed".into());}
        json!({"id":j.id,"enabled":j.enabled,"lastRun":j.last_run,"nextRun":j.next_run,"status":if !j.enabled {"disabled"} else if overdue {"overdue"} else if failed {"degraded"} else if j.last_run.is_none() {"unknown"} else {"scheduled"}})
    }).collect();
    if rss.is_empty() {
        warnings.push("RSS scheduler is unconfigured".into());
    }
    components["rssScheduler"] = json!(rss);
    components["mail"] = json!({"status":if state.mail_sender.is_some() {"configured"} else {"unconfigured"},"delivery":"unknown"});
    let mail_events = state.store.mail_event_settings().await?;
    components["notifications"] = json!({"status":if mail_events.iter().any(|e| e.enabled) {"configured"} else {"unconfigured"}, "events":mail_events});
    components["clockSkew"] =
        json!({"status":"unknown","reason":"No independent reference clock configured"});
    Ok(Json(
        json!({"status":if warnings.is_empty(){"ok"}else{"degraded"},"database":database,"checkedAt":now,"indexers":indexers,"components":components,"warnings":warnings}),
    ))
}

pub(crate) fn classify(error: &EngineError) -> Failure {
    match error {
        EngineError::UnsupportedSearchMode(_)
        | EngineError::UnsupportedSearchCategory(_)
        | EngineError::EmptySearchTerm
        | EngineError::UnexpectedStatus(400) => Failure::Request,
        EngineError::UnexpectedStatus(401 | 403)
        | EngineError::AuthenticationRejected
        | EngineError::AuthenticationFormMissing
        | EngineError::LoginChallengeRequired
        | EngineError::BrowserLoginRequired => Failure::Authentication,
        EngineError::LoginChallengeUnavailable | EngineError::InvalidLoginChallenge => {
            Failure::Request
        }
        EngineError::LoginChallengeRateLimited => Failure::Quota,
        EngineError::UnexpectedStatus(429) => Failure::RateLimited,
        EngineError::RequestTimeout => Failure::Network,
        EngineError::HttpClient(e) => match e.status().map(|s| s.as_u16()) {
            Some(401 | 403) => Failure::Authentication,
            Some(429) => Failure::RateLimited,
            _ => Failure::Network,
        },
        EngineError::UnexpectedStatus(_) => Failure::Upstream,
        _ => Failure::Configuration,
    }
}
pub(crate) async fn record(
    state: &AppState,
    id: Option<&str>,
    op: Operation,
    failure: Option<Failure>,
    elapsed: std::time::Duration,
    count: usize,
) {
    if state
        .store
        .observe(
            id,
            op,
            failure,
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            count,
        )
        .await
        .is_err()
    {
        tracing::error!("failed to persist monitoring observation");
    }
}
/// Downloads use the same classification and accounting for stable and session tickets.
pub(crate) async fn resolve_release(
    state: &AppState,
    executor: &indexloom_engine::CardigannExecutor,
    release: &indexloom_core::SearchResult,
) -> Result<indexloom_engine::DownloadArtifact, ApiError> {
    let id = &release.indexer_instance_id;
    if !state.store.claim_indexer_dispatch(id).await? {
        return Err(ApiError::too_many_requests("indexer circuit is open"));
    }
    let start = std::time::Instant::now();
    let result = executor.resolve_release(release).await;
    record(
        state,
        Some(id),
        Operation::Download,
        result.as_ref().err().map(classify),
        start.elapsed(),
        usize::from(result.is_ok()),
    )
    .await;
    result.map_err(|_| ApiError::bad_gateway("tracker download failed"))
}

pub(crate) async fn prometheus(state: &AppState) -> Result<String, ApiError> {
    use std::fmt::Write;
    let rows = state.store.indexer_statistics().await?;
    let mut output = String::new();
    let _ = writeln!(
        output,
        "# TYPE indexloom_global_authentication_errors_total counter\nindexloom_global_authentication_errors_total {}",
        state.store.global_authentication_errors().await?
    );
    // Aggregate without labels: instance names and IDs need not enter metric storage.
    for (name, value) in [
        (
            "search_requests",
            rows.iter().map(|r| r.searches).sum::<i64>(),
        ),
        ("downloads", rows.iter().map(|r| r.downloads).sum()),
        (
            "failed_queries",
            rows.iter().map(|r| r.failed_queries).sum(),
        ),
        (
            "authentication_errors",
            rows.iter().map(|r| r.auth_errors).sum(),
        ),
        ("rss_operations", rows.iter().map(|r| r.rss).sum()),
        ("results", rows.iter().map(|r| r.results).sum()),
    ] {
        let _ = writeln!(
            output,
            "# TYPE indexloom_durable_{name}_total counter\nindexloom_durable_{name}_total {value}"
        );
    }
    let duration_count: i64 = rows.iter().map(|r| r.response_count).sum();
    let duration_ms: i64 = rows.iter().map(|r| r.response_sum_ms).sum();
    let whole_seconds = duration_ms.div_euclid(1000);
    let fractional_ms = duration_ms.rem_euclid(1000);
    let _ = writeln!(
        output,
        "# TYPE indexloom_durable_response_duration_seconds summary\nindexloom_durable_response_duration_seconds_count {duration_count}\nindexloom_durable_response_duration_seconds_sum {whole_seconds}.{fractional_ms:03}"
    );
    let now = indexloom_storage::monitoring::timestamp();
    let _ = writeln!(
        output,
        "# TYPE indexloom_open_circuits gauge\nindexloom_open_circuits {}",
        rows.iter()
            .filter(|r| r.open_until > now || r.probe_until > now)
            .count()
    );
    Ok(output)
}

#[cfg(test)]
mod tests;
