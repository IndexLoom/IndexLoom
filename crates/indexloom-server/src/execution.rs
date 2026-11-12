use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use indexloom_core::SearchResult;
use indexloom_definitions::parse_yaml;
use indexloom_engine::{
    CardigannExecutor, FlareSolverrClient, IndexerExecutionConfig, IndexerSearchBatch,
    IndexerTestOutcome, ProxyRoute, SearchParameters,
};
use indexloom_storage::{
    FlareSolverrInstance, IndexerConfigurationRecord, IndexerTestRecord,
    ResolvedIndexerConfiguration, ResolvedOutboundProxy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    sync::{RwLock, Semaphore},
    task::JoinSet,
};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

use super::{ApiError, AppState, validate_identifier};

const TEST_CONCURRENCY: usize = 16;
const SEARCH_CONCURRENCY: usize = 12;
const MAX_SEARCH_INDEXERS: usize = 256;
const MAX_SELECTION_IDENTIFIERS: usize = 20_000;
const MAX_RUNNING_TEST_JOBS: usize = 8;
const MAX_COMPLETED_TEST_JOBS: usize = 256;
const TEST_JOB_TTL: Duration = Duration::from_hours(24);
const EXECUTOR_CACHE_TTL: Duration = Duration::from_mins(30);
const MAX_EXECUTOR_CACHE_ENTRIES: usize = 512;

#[derive(Clone, Debug)]
pub(crate) struct TestJobRegistry {
    jobs: Arc<RwLock<BTreeMap<String, TestJob>>>,
    permits: Arc<Semaphore>,
}

impl Default for TestJobRegistry {
    fn default() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(BTreeMap::new())),
            permits: Arc::new(Semaphore::new(TEST_CONCURRENCY)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ExecutorCache {
    entries: Arc<RwLock<HashMap<String, CachedExecutor>>>,
}

#[derive(Clone, Debug)]
struct CachedExecutor {
    fingerprint: [u8; 32],
    executor: Arc<CardigannExecutor>,
    last_used: Instant,
}

#[derive(Clone, Debug)]
struct ExecutorHandle {
    executor: Arc<CardigannExecutor>,
    display_name: String,
    definition_id: String,
}

pub(crate) fn search_permits() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(SEARCH_CONCURRENCY))
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/indexer-tests", post(start_test_job))
        .route("/api/v1/indexer-tests/{id}", get(get_test_job))
        .route("/api/v1/search", post(search))
        .route("/api/v1/search/stream", post(search_stream))
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
    #[serde(default)]
    filters: SearchParameters,
    #[serde(default)]
    languages: Vec<String>,
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
    #[serde(skip)]
    finished_monotonic: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TestJobStatus {
    Running,
    Completed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchIndexerView {
    indexer_instance_id: String,
    definition_id: Option<String>,
    indexer_name: String,
    response_time_ms: u64,
    results: Vec<SearchResult>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    query: String,
    indexers: Vec<SearchIndexerView>,
    result_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SearchStreamEvent {
    Started {
        query: String,
        total: usize,
    },
    Indexer {
        indexer: SearchIndexerView,
    },
    Finished {
        query: String,
        completed: usize,
        total: usize,
        result_count: usize,
    },
}

#[derive(Debug)]
struct PreparedSearch {
    query: String,
    indexer_ids: Vec<String>,
    limit_per_indexer: usize,
    filters: SearchParameters,
}

impl TestJobRegistry {
    async fn insert(&self, job: TestJob) -> Result<(), ApiError> {
        let mut jobs = self.jobs.write().await;
        prune_test_jobs(&mut jobs);
        if jobs
            .values()
            .filter(|candidate| candidate.status == TestJobStatus::Running)
            .count()
            >= MAX_RUNNING_TEST_JOBS
        {
            return Err(ApiError::too_many_requests(
                "too many indexer test jobs are already running",
            ));
        }
        jobs.insert(job.id.clone(), job);
        Ok(())
    }

    async fn get(&self, id: &str) -> Option<TestJob> {
        let mut jobs = self.jobs.write().await;
        prune_test_jobs(&mut jobs);
        jobs.get(id).cloned()
    }

    async fn record_outcome(&self, job_id: &str, outcome: IndexerTestOutcome) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
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
                job.finished_monotonic = Some(Instant::now());
            }
        }
        prune_test_jobs(&mut jobs);
    }
}

fn prune_test_jobs(jobs: &mut BTreeMap<String, TestJob>) {
    jobs.retain(|_, job| {
        job.status == TestJobStatus::Running
            || job
                .finished_monotonic
                .is_some_and(|finished| finished.elapsed() <= TEST_JOB_TTL)
    });
    let mut completed = jobs
        .iter()
        .filter_map(|(id, job)| {
            (job.status == TestJobStatus::Completed).then_some((id.clone(), job.finished_monotonic))
        })
        .collect::<Vec<_>>();
    if completed.len() <= MAX_COMPLETED_TEST_JOBS {
        return;
    }
    completed.sort_by_key(|(id, finished)| (*finished, id.clone()));
    let excess = completed.len() - MAX_COMPLETED_TEST_JOBS;
    for (id, _) in completed.into_iter().take(excess) {
        jobs.remove(&id);
    }
}

impl ExecutorCache {
    pub(crate) async fn invalidate(&self, id: &str) {
        self.entries.write().await.remove(id);
    }

    pub(crate) async fn clear(&self) {
        self.entries.write().await.clear();
    }

    async fn get(&self, id: &str, fingerprint: &[u8; 32]) -> Option<Arc<CardigannExecutor>> {
        let mut entries = self.entries.write().await;
        prune_executor_cache(&mut entries);
        let cached = entries.get_mut(id)?;
        if &cached.fingerprint != fingerprint {
            entries.remove(id);
            return None;
        }
        cached.last_used = Instant::now();
        Some(cached.executor.clone())
    }

    async fn insert(
        &self,
        id: String,
        fingerprint: [u8; 32],
        executor: Arc<CardigannExecutor>,
    ) -> Arc<CardigannExecutor> {
        let mut entries = self.entries.write().await;
        prune_executor_cache(&mut entries);
        if let Some(cached) = entries.get_mut(&id)
            && cached.fingerprint == fingerprint
        {
            cached.last_used = Instant::now();
            return cached.executor.clone();
        }
        if entries.len() >= MAX_EXECUTOR_CACHE_ENTRIES
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| id.clone())
        {
            entries.remove(&oldest);
        }
        entries.insert(
            id,
            CachedExecutor {
                fingerprint,
                executor: executor.clone(),
                last_used: Instant::now(),
            },
        );
        executor
    }
}

fn prune_executor_cache(entries: &mut HashMap<String, CachedExecutor>) {
    entries.retain(|_, entry| entry.last_used.elapsed() <= EXECUTOR_CACHE_TTL);
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
        finished_monotonic: None,
    };
    state.test_jobs.insert(job.clone()).await?;
    tokio::spawn(run_test_job(state, id, indexer_ids));
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn get_test_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TestJob>, ApiError> {
    let job = state
        .test_jobs
        .get(&id)
        .await
        .ok_or_else(|| ApiError::not_found("indexer test job was not found"))?;
    Ok(Json(job))
}

async fn run_test_job(state: AppState, job_id: String, indexer_ids: Vec<String>) {
    let mut tasks = JoinSet::new();
    for indexer_id in indexer_ids {
        let state = state.clone();
        let semaphore = state.test_jobs.permits.clone();
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
        state.test_jobs.record_outcome(&job_id, outcome).await;
    }
}

async fn test_one(state: AppState, indexer_id: String) -> IndexerTestOutcome {
    match build_executor(&state, &indexer_id).await {
        Ok(handle) => {
            let mut outcome = handle.executor.test().await;
            outcome.indexer_id = indexer_id;
            outcome.indexer_name = handle.display_name;
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
    let prepared = prepare_search(&state, request).await?;
    let mut tasks = spawn_search_tasks(&state, &prepared);
    let mut indexers = Vec::new();
    while let Some(result) = tasks.join_next().await {
        indexers.push(search_join_result(result));
    }
    sort_search_indexers(&mut indexers);
    let result_count = indexers.iter().map(|indexer| indexer.results.len()).sum();
    Ok(Json(SearchResponse {
        query: prepared.query,
        indexers,
        result_count,
    }))
}

async fn search_stream(
    State(state): State<AppState>,
    Json(request): Json<SearchRequest>,
) -> Result<Response, ApiError> {
    let prepared = prepare_search(&state, request).await?;
    let total = prepared.indexer_ids.len();
    let query = prepared.query.clone();
    let mut tasks = spawn_search_tasks(&state, &prepared);
    let output = async_stream::stream! {
        yield Ok::<String, Infallible>(ndjson_line(&SearchStreamEvent::Started {
            query: query.clone(),
            total,
        }));
        let mut completed = 0_usize;
        let mut result_count = 0_usize;
        while let Some(result) = tasks.join_next().await {
            let indexer = search_join_result(result);
            completed += 1;
            result_count += indexer.results.len();
            yield Ok(ndjson_line(&SearchStreamEvent::Indexer { indexer }));
        }
        yield Ok(ndjson_line(&SearchStreamEvent::Finished {
            query,
            completed,
            total,
            result_count,
        }));
    };
    let mut response = Body::from_stream(output).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}

async fn prepare_search(
    state: &AppState,
    request: SearchRequest,
) -> Result<PreparedSearch, ApiError> {
    validate_selection(&request.selection)?;
    let query = request.query.trim();
    if query.is_empty() || query.len() > 500 {
        return Err(ApiError::bad_request(
            "search query must contain 1 to 500 bytes",
        ));
    }
    validate_search_filters(&request.filters)?;
    validate_search_languages(&request.languages)?;
    if !(1..=200).contains(&request.limit_per_indexer) {
        return Err(ApiError::bad_request(
            "limitPerIndexer must be between 1 and 200",
        ));
    }
    let mut indexer_ids = state
        .store
        .select_indexer_ids(
            request.selection.all,
            &request.selection.indexer_ids,
            &request.selection.group_ids,
            true,
        )
        .await?;
    if !request.languages.is_empty() {
        let selected: std::collections::HashSet<_> = indexer_ids.into_iter().collect();
        let requested: Vec<_> = request.languages.iter().map(|l| l.to_ascii_lowercase()).collect();
        indexer_ids = state.store.list_indexers(&state.vault).await?
            .into_iter()
            .filter(|record| selected.contains(&record.id))
            .filter(|record| {
                let Ok(definition) = indexloom_definitions::parse_yaml(&record.definition_yaml) else {
                    return false;
                };
                let tag = indexloom_definitions::capabilities::summarize(&definition).language.to_ascii_lowercase();
                requested.iter().any(|code| tag == *code || tag.starts_with(&format!("{code}-")))
            })
            .map(|record| record.id)
            .collect();
    }
    if indexer_ids.is_empty() {
        return Err(ApiError::bad_request(
            "selection contains no enabled indexers matching the requested languages",
        ));
    }
    if indexer_ids.len() > MAX_SEARCH_INDEXERS {
        return Err(ApiError::bad_request(
            "search selection exceeds 256 enabled indexers",
        ));
    }
    Ok(PreparedSearch {
        query: query.to_owned(),
        indexer_ids,
        limit_per_indexer: request.limit_per_indexer,
        filters: request.filters,
    })
}

fn spawn_search_tasks(state: &AppState, prepared: &PreparedSearch) -> JoinSet<SearchIndexerView> {
    let mut tasks = JoinSet::new();
    for indexer_id in &prepared.indexer_ids {
        let state = state.clone();
        let semaphore = state.search_permits.clone();
        let indexer_id = indexer_id.clone();
        let query = prepared.query.clone();
        let limit = prepared.limit_per_indexer;
        let filters = prepared.filters.clone();
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            search_one(&state, &indexer_id, &query, limit, &filters).await
        });
    }
    tasks
}

fn search_join_result(
    result: Result<SearchIndexerView, tokio::task::JoinError>,
) -> SearchIndexerView {
    match result {
        Ok(result) => result,
        Err(error) => SearchIndexerView {
            indexer_instance_id: "worker".to_owned(),
            definition_id: None,
            indexer_name: "Worker".to_owned(),
            response_time_ms: 0,
            results: Vec::new(),
            error: Some(format!("search worker failed: {error}")),
        },
    }
}

fn sort_search_indexers(indexers: &mut [SearchIndexerView]) {
    indexers.sort_by(|left, right| {
        left.indexer_name
            .to_lowercase()
            .cmp(&right.indexer_name.to_lowercase())
            .then_with(|| left.indexer_instance_id.cmp(&right.indexer_instance_id))
    });
}

fn ndjson_line(event: &SearchStreamEvent) -> String {
    let mut line = serde_json::to_string(event)
        .expect("search stream events contain only infallibly serializable values");
    line.push('\n');
    line
}

async fn search_one(
    state: &AppState,
    indexer_id: &str,
    query: &str,
    limit: usize,
    filters: &SearchParameters,
) -> SearchIndexerView {
    match build_executor(state, indexer_id).await {
        Ok(handle) => match handle
            .executor
            .search_with_parameters(query, limit, filters)
            .await
        {
            Ok(IndexerSearchBatch {
                indexer_instance_id,
                definition_id,
                response_time_ms,
                results,
                ..
            }) => SearchIndexerView {
                indexer_instance_id,
                definition_id: Some(definition_id),
                indexer_name: handle.display_name,
                response_time_ms,
                results,
                error: None,
            },
            Err(error) => SearchIndexerView {
                indexer_instance_id: indexer_id.to_owned(),
                definition_id: Some(handle.definition_id),
                indexer_name: handle.display_name,
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

async fn build_executor(state: &AppState, indexer_id: &str) -> Result<ExecutorHandle, String> {
    let record = state
        .store
        .get_indexer(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?;
    let mut resolved = state
        .store
        .resolve_indexer_configuration(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?;
    let mut resolved_proxy = state
        .store
        .effective_proxy_for_indexer(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?;
    let connector = if let Some(connector_id) = resolved.flaresolverr_instance_id.as_deref() {
        Some(
            state
                .store
                .list_flaresolverr()
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|connector| connector.id == connector_id)
                .ok_or_else(|| "assigned FlareSolverr connector was not found".to_owned())?,
        )
    } else {
        None
    };
    if connector
        .as_ref()
        .is_some_and(|connector| !connector.enabled)
    {
        return Err("assigned FlareSolverr connector is disabled".to_owned());
    }
    let fingerprint = execution_fingerprint(
        &record,
        &resolved,
        resolved_proxy.as_ref(),
        connector.as_ref(),
    )?;
    if let Some(executor) = state.executor_cache.get(indexer_id, &fingerprint).await {
        zeroize_resolved_credentials(&mut resolved, &mut resolved_proxy);
        return Ok(ExecutorHandle {
            executor,
            display_name: record.display_name,
            definition_id: record.definition_id,
        });
    }

    let definition = parse_yaml(&record.definition_yaml).map_err(|error| error.to_string())?;
    let proxy = resolved_proxy.map(|proxy| ProxyRoute {
        url: proxy.transport_url(),
        id: proxy.id,
        enabled: proxy.enabled,
        username: proxy.username,
        password: proxy.password,
    });
    let flaresolverr = if let Some(connector) = connector {
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
    let executor = Arc::new(
        CardigannExecutor::new(IndexerExecutionConfig {
            indexer_instance_id: indexer_id.to_owned(),
            definition,
            values: resolved.values,
            secrets: resolved.secrets,
            user_agent: resolved.user_agent,
            proxy,
            flaresolverr,
        })
        .map_err(|error| error.to_string())?,
    );
    let executor = state
        .executor_cache
        .insert(indexer_id.to_owned(), fingerprint, executor)
        .await;
    Ok(ExecutorHandle {
        executor,
        display_name: record.display_name,
        definition_id,
    })
}

fn execution_fingerprint(
    record: &IndexerConfigurationRecord,
    resolved: &ResolvedIndexerConfiguration,
    proxy: Option<&ResolvedOutboundProxy>,
    connector: Option<&FlareSolverrInstance>,
) -> Result<[u8; 32], String> {
    let mut hasher = Sha256::new();
    hash_part(&mut hasher, b"runtime-v1");
    hash_part(&mut hasher, record.definition_yaml.as_bytes());
    hash_optional(&mut hasher, resolved.user_agent.as_deref());
    hash_optional(&mut hasher, resolved.flaresolverr_instance_id.as_deref());
    hash_part(
        &mut hasher,
        &serde_json::to_vec(&resolved.values).map_err(|error| error.to_string())?,
    );
    for (name, value) in &resolved.secrets {
        hash_part(&mut hasher, name.as_bytes());
        hash_part(&mut hasher, value.as_bytes());
    }
    if let Some(proxy) = proxy {
        hash_part(&mut hasher, proxy.id.as_bytes());
        hash_part(&mut hasher, proxy.transport_url().as_bytes());
        hash_part(&mut hasher, &[u8::from(proxy.enabled)]);
        hash_optional(&mut hasher, proxy.username.as_deref());
        hash_optional(&mut hasher, proxy.password.as_deref());
    } else {
        hash_part(&mut hasher, b"no-proxy");
    }
    if let Some(connector) = connector {
        hash_part(&mut hasher, connector.id.as_bytes());
        hash_part(&mut hasher, connector.endpoint.as_bytes());
        hash_part(&mut hasher, &[u8::from(connector.enabled)]);
        hash_part(&mut hasher, &connector.timeout_seconds.to_be_bytes());
        hash_part(&mut hasher, connector.updated_at.as_bytes());
    } else {
        hash_part(&mut hasher, b"no-flaresolverr");
    }
    Ok(hasher.finalize().into())
}

fn hash_part(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

fn hash_optional(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_part(hasher, value.as_bytes());
        }
        None => hasher.update([0]),
    }
}

fn zeroize_resolved_credentials(
    resolved: &mut ResolvedIndexerConfiguration,
    proxy: &mut Option<ResolvedOutboundProxy>,
) {
    for value in resolved.secrets.values_mut() {
        value.zeroize();
    }
    if let Some(password) = proxy.as_mut().and_then(|proxy| proxy.password.as_mut()) {
        password.zeroize();
    }
}

fn validate_search_filters(filters: &SearchParameters) -> Result<(), ApiError> {
    for value in [
        filters.mode.as_deref(),
        filters.category.as_deref(),
        filters.genre.as_deref(),
        filters.imdb_id.as_deref(),
        filters.tmdb_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.len() > 100 || value.chars().any(char::is_control) {
            return Err(ApiError::bad_request(
                "search filter is too long or invalid",
            ));
        }
    }
    if filters
        .year
        .is_some_and(|year| !(1800..=2100).contains(&year))
        || filters.season.is_some_and(|season| season > 1000)
        || filters.episode.is_some_and(|episode| episode > 10_000)
    {
        return Err(ApiError::bad_request(
            "search numeric filter is out of range",
        ));
    }
    Ok(())
}

fn validate_search_languages(languages: &[String]) -> Result<(), ApiError> {
    if languages.len() > 16 || languages.iter().any(|language| {
        language.len() < 2 || language.len() > 35
            || !language.bytes().all(|character| character.is_ascii_alphanumeric() || character == b'-')
            || language.starts_with('-') || language.ends_with('-')
    }) {
        return Err(ApiError::bad_request("invalid indexer language filter"));
    }
    Ok(())
}

fn validate_selection(selection: &IndexerSelection) -> Result<(), ApiError> {
    if selection.indexer_ids.len() > MAX_SELECTION_IDENTIFIERS || selection.group_ids.len() > 100 {
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, time::Instant};

    use indexloom_storage::{
        IndexerConfigurationRecord, ProxyKind, ResolvedIndexerConfiguration, ResolvedOutboundProxy,
    };
    use serde_json::json;

    use super::{
        MAX_COMPLETED_TEST_JOBS, MAX_RUNNING_TEST_JOBS, TEST_JOB_TTL, TestJob, TestJobRegistry,
        TestJobStatus, execution_fingerprint, prune_test_jobs, validate_search_languages,
    };

    #[test]
    fn validates_international_indexer_language_tags() {
        assert!(validate_search_languages(&["ru".to_owned(), "ja-JP".to_owned(), "zh-Hans".to_owned()]).is_ok());
        assert!(validate_search_languages(&[]).is_ok());
        for invalid in ["", "a", "en_US", "../etc/passwd", "-ru", "fr-", "ру"] {
            assert!(validate_search_languages(&[invalid.to_owned()]).is_err(), "{invalid} should be rejected");
        }
        assert!(validate_search_languages(&vec!["de".to_owned(); 17]).is_err());
    }

    #[test]
    fn prunes_expired_and_oldest_completed_jobs_but_keeps_running_jobs() {
        let mut jobs = BTreeMap::new();
        jobs.insert(
            "expired".to_owned(),
            completed_job("expired", TEST_JOB_TTL + std::time::Duration::from_secs(1)),
        );
        jobs.insert("running".to_owned(), running_job("running"));
        for index in 0..(MAX_COMPLETED_TEST_JOBS + 5) {
            let id = format!("completed-{index:04}");
            let age = std::time::Duration::from_secs(
                u64::try_from(MAX_COMPLETED_TEST_JOBS + 5 - index).unwrap(),
            );
            jobs.insert(id.clone(), completed_job(&id, age));
        }

        prune_test_jobs(&mut jobs);

        assert!(!jobs.contains_key("expired"));
        assert!(jobs.contains_key("running"));
        assert_eq!(
            jobs.values()
                .filter(|job| job.status == TestJobStatus::Completed)
                .count(),
            MAX_COMPLETED_TEST_JOBS
        );
        assert!(!jobs.contains_key("completed-0000"));
        assert!(jobs.contains_key(&format!("completed-{:04}", MAX_COMPLETED_TEST_JOBS + 4)));
    }

    #[tokio::test]
    async fn rejects_more_than_the_running_test_job_budget() {
        let registry = TestJobRegistry::default();
        for index in 0..MAX_RUNNING_TEST_JOBS {
            registry
                .insert(running_job(&format!("running-{index}")))
                .await
                .expect("job within the active budget should be accepted");
        }
        let error = registry
            .insert(running_job("one-too-many"))
            .await
            .expect_err("active job budget must be enforced");
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn runtime_fingerprint_changes_with_credentials_and_effective_proxy() {
        let record = IndexerConfigurationRecord {
            id: "example-main".to_owned(),
            definition_id: "example".to_owned(),
            display_name: "Example".to_owned(),
            enabled: true,
            catalog_available: true,
            user_agent: None,
            flaresolverr_instance_id: None,
            proxy_id: None,
            group_ids: Vec::new(),
            tags: Vec::new(),
            last_test: None,
            values: BTreeMap::new(),
            configured_secrets: Vec::new(),
            definition_yaml: "definition-v1".to_owned(),
        };
        let mut resolved = ResolvedIndexerConfiguration {
            user_agent: None,
            flaresolverr_instance_id: None,
            proxy_id: None,
            values: BTreeMap::from([("ratio".to_owned(), json!(1))]),
            secrets: BTreeMap::from([("cookie".to_owned(), "first".to_owned())]),
        };
        let direct = execution_fingerprint(&record, &resolved, None, None).unwrap();
        resolved
            .secrets
            .insert("cookie".to_owned(), "second".to_owned());
        let changed_secret = execution_fingerprint(&record, &resolved, None, None).unwrap();
        assert_ne!(direct, changed_secret);

        let proxy = ResolvedOutboundProxy {
            id: "proxy-main".to_owned(),
            kind: ProxyKind::Socks5,
            host: "proxy.example".to_owned(),
            port: 1080,
            enabled: true,
            username: Some("alice".to_owned()),
            password: Some("secret".to_owned()),
        };
        let proxied = execution_fingerprint(&record, &resolved, Some(&proxy), None).unwrap();
        assert_ne!(changed_secret, proxied);
    }

    fn running_job(id: &str) -> TestJob {
        TestJob {
            id: id.to_owned(),
            status: TestJobStatus::Running,
            total: 1,
            completed: 0,
            results: Vec::new(),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            finished_at: None,
            finished_monotonic: None,
        }
    }

    fn completed_job(id: &str, age: std::time::Duration) -> TestJob {
        TestJob {
            id: id.to_owned(),
            status: TestJobStatus::Completed,
            total: 1,
            completed: 1,
            results: Vec::new(),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            finished_at: Some("2026-08-26T00:00:01Z".to_owned()),
            finished_monotonic: Some(
                Instant::now()
                    .checked_sub(age)
                    .expect("test age is bounded"),
            ),
        }
    }
}
