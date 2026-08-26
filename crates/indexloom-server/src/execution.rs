use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use indexloom_definitions::parse_yaml;
use indexloom_engine::{
    CardigannExecutor, FlareSolverrClient, IndexerExecutionConfig, IndexerSearchBatch,
    IndexerTestOutcome, ProxyRoute,
};
use indexloom_storage::IndexerTestRecord;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    sync::{RwLock, Semaphore},
    task::JoinSet,
};
use url::Url;
use uuid::Uuid;

use super::{ApiError, AppState, validate_identifier};

const TEST_CONCURRENCY: usize = 16;
const SEARCH_CONCURRENCY: usize = 12;
const MAX_SEARCH_INDEXERS: usize = 256;

#[derive(Clone, Debug, Default)]
pub(crate) struct TestJobRegistry {
    jobs: Arc<RwLock<BTreeMap<String, TestJob>>>,
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/indexer-tests", post(start_test_job))
        .route("/api/v1/indexer-tests/{id}", get(get_test_job))
        .route("/api/v1/search", post(search))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IndexerSelection {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    indexer_ids: Vec<String>,
    #[serde(default)]
    group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestRequest {
    #[serde(flatten)]
    selection: IndexerSelection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchRequest {
    query: String,
    #[serde(default)]
    selection: IndexerSelection,
    #[serde(default = "default_result_limit")]
    limit_per_indexer: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestJob {
    id: String,
    status: TestJobStatus,
    total: usize,
    completed: usize,
    results: Vec<IndexerTestOutcome>,
    created_at: String,
    finished_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TestJobStatus {
    Running,
    Completed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchIndexerView {
    indexer_instance_id: String,
    definition_id: Option<String>,
    indexer_name: String,
    response_time_ms: u64,
    results: Vec<indexloom_core::SearchResult>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    query: String,
    indexers: Vec<SearchIndexerView>,
    result_count: usize,
}

async fn start_test_job(
    State(state): State<AppState>,
    Json(request): Json<TestRequest>,
) -> Result<impl IntoResponse, ApiError> {
    validate_selection(&request.selection)?;
    let indexer_ids = state
        .store
        .select_indexer_ids(
            request.selection.all,
            &request.selection.indexer_ids,
            &request.selection.group_ids,
            true,
        )
        .await?;
    if indexer_ids.is_empty() {
        return Err(ApiError::bad_request("indexer selection is empty"));
    }
    let id = Uuid::now_v7().to_string();
    let job = TestJob {
        id: id.clone(),
        status: TestJobStatus::Running,
        total: indexer_ids.len(),
        completed: 0,
        results: Vec::new(),
        created_at: now(),
        finished_at: None,
    };
    state
        .test_jobs
        .jobs
        .write()
        .await
        .insert(id.clone(), job.clone());
    tokio::spawn(run_test_job(state, id, indexer_ids));
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn get_test_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TestJob>, ApiError> {
    let job = state
        .test_jobs
        .jobs
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("indexer test job was not found"))?;
    Ok(Json(job))
}

async fn run_test_job(state: AppState, job_id: String, indexer_ids: Vec<String>) {
    let semaphore = Arc::new(Semaphore::new(TEST_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for indexer_id in indexer_ids {
        let state = state.clone();
        let semaphore = semaphore.clone();
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            test_one(state, indexer_id).await
        });
    }
    while let Some(result) = tasks.join_next().await {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => IndexerTestOutcome {
                indexer_id: "worker".to_owned(),
                indexer_name: "Worker".to_owned(),
                success: false,
                response_time_ms: 0,
                message: format!("test worker failed: {error}"),
            },
        };
        if outcome.indexer_id != "worker"
            && let Err(error) = state
                .store
                .record_indexer_tests(&[IndexerTestRecord {
                    indexer_id: outcome.indexer_id.clone(),
                    success: outcome.success,
                    response_time_ms: Some(outcome.response_time_ms),
                    message: outcome.message.clone(),
                }])
                .await
        {
            tracing::error!(%error, indexer_id = %outcome.indexer_id, "failed to persist indexer test");
        }
        let mut jobs = state.test_jobs.jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.completed += 1;
            job.results.push(outcome);
            job.results.sort_by(|left, right| {
                left.indexer_name
                    .to_lowercase()
                    .cmp(&right.indexer_name.to_lowercase())
                    .then_with(|| left.indexer_id.cmp(&right.indexer_id))
            });
            if job.completed == job.total {
                job.status = TestJobStatus::Completed;
                job.finished_at = Some(now());
            }
        }
    }
}

async fn test_one(state: AppState, indexer_id: String) -> IndexerTestOutcome {
    match build_executor(&state, &indexer_id).await {
        Ok((executor, display_name, _definition_id)) => {
            let mut outcome = executor.test().await;
            outcome.indexer_id = indexer_id;
            outcome.indexer_name = display_name;
            outcome
        }
        Err(message) => IndexerTestOutcome {
            indexer_id,
            indexer_name: "Unavailable indexer".to_owned(),
            success: false,
            response_time_ms: 0,
            message,
        },
    }
}

async fn search(
    State(state): State<AppState>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    validate_selection(&request.selection)?;
    let query = request.query.trim();
    if query.is_empty() || query.len() > 500 {
        return Err(ApiError::bad_request(
            "search query must contain 1 to 500 bytes",
        ));
    }
    if !(1..=200).contains(&request.limit_per_indexer) {
        return Err(ApiError::bad_request(
            "limitPerIndexer must be between 1 and 200",
        ));
    }
    let indexer_ids = state
        .store
        .select_indexer_ids(
            request.selection.all,
            &request.selection.indexer_ids,
            &request.selection.group_ids,
            true,
        )
        .await?;
    if indexer_ids.is_empty() {
        return Err(ApiError::bad_request(
            "selection contains no enabled indexers",
        ));
    }
    if indexer_ids.len() > MAX_SEARCH_INDEXERS {
        return Err(ApiError::bad_request(
            "search selection exceeds 256 enabled indexers",
        ));
    }
    let semaphore = Arc::new(Semaphore::new(SEARCH_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for indexer_id in indexer_ids {
        let state = state.clone();
        let semaphore = semaphore.clone();
        let query = query.to_owned();
        let limit = request.limit_per_indexer;
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            search_one(&state, &indexer_id, &query, limit).await
        });
    }
    let mut indexers = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(result) => indexers.push(result),
            Err(error) => indexers.push(SearchIndexerView {
                indexer_instance_id: "worker".to_owned(),
                definition_id: None,
                indexer_name: "Worker".to_owned(),
                response_time_ms: 0,
                results: Vec::new(),
                error: Some(format!("search worker failed: {error}")),
            }),
        }
    }
    indexers.sort_by(|left, right| {
        left.indexer_name
            .to_lowercase()
            .cmp(&right.indexer_name.to_lowercase())
            .then_with(|| left.indexer_instance_id.cmp(&right.indexer_instance_id))
    });
    let result_count = indexers.iter().map(|indexer| indexer.results.len()).sum();
    Ok(Json(SearchResponse {
        query: query.to_owned(),
        indexers,
        result_count,
    }))
}

async fn search_one(
    state: &AppState,
    indexer_id: &str,
    query: &str,
    limit: usize,
) -> SearchIndexerView {
    match build_executor(state, indexer_id).await {
        Ok((executor, display_name, definition_id)) => match executor.search(query, limit).await {
            Ok(IndexerSearchBatch {
                indexer_instance_id,
                definition_id,
                response_time_ms,
                results,
                ..
            }) => SearchIndexerView {
                indexer_instance_id,
                definition_id: Some(definition_id),
                indexer_name: display_name,
                response_time_ms,
                results,
                error: None,
            },
            Err(error) => SearchIndexerView {
                indexer_instance_id: indexer_id.to_owned(),
                definition_id: Some(definition_id),
                indexer_name: display_name,
                response_time_ms: 0,
                results: Vec::new(),
                error: Some(error.to_string()),
            },
        },
        Err(message) => SearchIndexerView {
            indexer_instance_id: indexer_id.to_owned(),
            definition_id: None,
            indexer_name: "Unavailable indexer".to_owned(),
            response_time_ms: 0,
            results: Vec::new(),
            error: Some(message),
        },
    }
}

async fn build_executor(
    state: &AppState,
    indexer_id: &str,
) -> Result<(CardigannExecutor, String, String), String> {
    let record = state
        .store
        .get_indexer(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?;
    let resolved = state
        .store
        .resolve_indexer_configuration(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?;
    let definition = parse_yaml(&record.definition_yaml).map_err(|error| error.to_string())?;
    let proxy = state
        .store
        .effective_proxy_for_indexer(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?
        .map(|proxy| ProxyRoute {
            url: proxy.transport_url(),
            id: proxy.id,
            enabled: proxy.enabled,
            username: proxy.username,
            password: proxy.password,
        });
    let flaresolverr = if let Some(connector_id) = resolved.flaresolverr_instance_id.as_deref() {
        let connector = state
            .store
            .list_flaresolverr()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|connector| connector.id == connector_id)
            .ok_or_else(|| "assigned FlareSolverr connector was not found".to_owned())?;
        if !connector.enabled {
            return Err("assigned FlareSolverr connector is disabled".to_owned());
        }
        Some(
            FlareSolverrClient::new(
                Url::parse(&connector.endpoint).map_err(|error| error.to_string())?,
                Duration::from_secs(
                    u64::try_from(connector.timeout_seconds)
                        .map_err(|_| "invalid FlareSolverr timeout".to_owned())?,
                ),
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let definition_id = definition.metadata.id.clone();
    CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: indexer_id.to_owned(),
        definition,
        values: resolved.values,
        secrets: resolved.secrets,
        user_agent: resolved.user_agent,
        proxy,
        flaresolverr,
    })
    .map(|executor| (executor, record.display_name, definition_id))
    .map_err(|error| error.to_string())
}

fn validate_selection(selection: &IndexerSelection) -> Result<(), ApiError> {
    if selection.indexer_ids.len() > 553 || selection.group_ids.len() > 100 {
        return Err(ApiError::bad_request("indexer selection is too large"));
    }
    if !selection.all && selection.indexer_ids.is_empty() && selection.group_ids.is_empty() {
        return Err(ApiError::bad_request(
            "select all, at least one group, or at least one indexer",
        ));
    }
    for id in selection.indexer_ids.iter().chain(&selection.group_ids) {
        validate_identifier(id)?;
    }
    Ok(())
}

const fn default_result_limit() -> usize {
    50
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
