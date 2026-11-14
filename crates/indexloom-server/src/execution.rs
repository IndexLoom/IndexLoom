use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
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
// Process-local aggregate counts. Never attach query text or user-identifying labels.
static SEARCH_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SEARCH_COMPLETED: AtomicU64 = AtomicU64::new(0);
static SEARCH_RELEASES: AtomicU64 = AtomicU64::new(0);
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
pub(crate) struct ExecutorHandle {
    pub(crate) executor: Arc<CardigannExecutor>,
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
        .route("/api/v1/metrics", get(metrics))
}

/// Authenticated Prometheus text exposition for coarse-grained runtime counters.
async fn metrics() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        format!(
            "# HELP indexloom_search_requests_total Accepted federated search requests since process start\n# TYPE indexloom_search_requests_total counter\nindexloom_search_requests_total {}\n# HELP indexloom_search_completed_total Completed searches since process start\n# TYPE indexloom_search_completed_total counter\nindexloom_search_completed_total {}\n# HELP indexloom_search_results_total Result rows fetched since process start\n# TYPE indexloom_search_results_total counter\nindexloom_search_results_total {}\n",
            SEARCH_REQUESTS.load(Ordering::Relaxed),
            SEARCH_COMPLETED.load(Ordering::Relaxed),
            SEARCH_RELEASES.load(Ordering::Relaxed)
        ),
    )
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
    #[serde(default)]
    result_options: crate::search_processing::ResultOptions,
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
    #[serde(flatten)]
    processed: crate::search_processing::ProcessedResults,
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
        #[serde(flatten)]
        processed: crate::search_processing::ProcessedResults,
        query: String,
        completed: usize,
        total: usize,
        result_count: usize,
    },
}

#[derive(Debug)]
struct PreparedSearch {
    result_options: crate::search_processing::ResultOptions,
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
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    let prepared = prepare_search(&state, request).await?;
    SEARCH_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let mut tasks = spawn_search_tasks(
        &state,
        &prepared,
        Some(crate::download::credential_scope(&headers)),
        false,
    );
    let mut indexers = Vec::new();
    while let Some(result) = tasks.join_next().await {
        indexers.push(search_join_result(result));
    }
    sort_search_indexers(&mut indexers);
    let result_count: usize = indexers.iter().map(|indexer| indexer.results.len()).sum();
    let processed = crate::search_processing::process(
        indexers
            .iter()
            .flat_map(|indexer| indexer.results.iter().cloned()),
        &prepared.result_options,
    );
    SEARCH_RELEASES.fetch_add(
        u64::try_from(result_count).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    SEARCH_COMPLETED.fetch_add(1, Ordering::Relaxed);
    Ok(Json(SearchResponse {
        query: prepared.query,
        indexers,
        result_count,
        processed,
    }))
}

async fn search_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Response, ApiError> {
    let prepared = prepare_search(&state, request).await?;
    SEARCH_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let total = prepared.indexer_ids.len();
    let query = prepared.query.clone();
    let result_options = prepared.result_options.clone();
    let mut tasks = spawn_search_tasks(
        &state,
        &prepared,
        Some(crate::download::credential_scope(&headers)),
        false,
    );
    let output = async_stream::stream! {
        yield Ok::<String, Infallible>(ndjson_line(&SearchStreamEvent::Started {
            query: query.clone(),
            total,
        }));
        let mut completed = 0_usize;
        let mut result_count = 0_usize;
        let mut all_results = Vec::new();
        while let Some(result) = tasks.join_next().await {
            let indexer = search_join_result(result);
            completed += 1;
            result_count += indexer.results.len();
            all_results.extend(indexer.results.iter().cloned());
            yield Ok(ndjson_line(&SearchStreamEvent::Indexer { indexer }));
        }
        SEARCH_RELEASES.fetch_add(
            u64::try_from(result_count).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        SEARCH_COMPLETED.fetch_add(1, Ordering::Relaxed);
        let processed = crate::search_processing::process(all_results, &result_options);
        yield Ok(ndjson_line(&SearchStreamEvent::Finished {
            processed,
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
    let structured = has_structured_query(&request.filters);
    if (query.is_empty() && !structured) || query.len() > 500 {
        return Err(ApiError::bad_request(
            "search requires keywords or a recognized structured field (max 500 bytes)",
        ));
    }
    validate_search_filters(&request.filters)?;
    request
        .result_options
        .validate()
        .map_err(ApiError::bad_request)?;

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
        let requested: Vec<_> = request
            .languages
            .iter()
            .map(|l| l.to_ascii_lowercase())
            .collect();
        indexer_ids = state
            .store
            .list_indexers(&state.vault)
            .await?
            .into_iter()
            .filter(|record| selected.contains(&record.id))
            .filter(|record| {
                let Ok(definition) = indexloom_definitions::parse_yaml(&record.definition_yaml)
                else {
                    return false;
                };
                let tag = indexloom_definitions::capabilities::summarize(&definition)
                    .language
                    .to_ascii_lowercase();
                requested
                    .iter()
                    .any(|code| tag == *code || tag.starts_with(&format!("{code}-")))
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
        result_options: request.result_options,
        query: query.to_owned(),
        indexer_ids,
        limit_per_indexer: request.limit_per_indexer,
        filters: request.filters,
    })
}

/// Bounded credential-free feed search with externally usable encrypted tickets.
/// A caller may select one configured indexer, otherwise all enabled sources.
pub(crate) async fn search_for_feed(
    state: &AppState,
    query: String,
    filters: SearchParameters,
    only_indexer: Option<String>,
    limit: usize,
) -> Result<Vec<SearchResult>, ApiError> {
    if let Some(id) = only_indexer.as_deref() {
        validate_identifier(id)?;
    }
    let result_options = crate::search_processing::ResultOptions {
        min_size: filters.min_size,
        max_size: filters.max_size,
        min_seeders: filters.min_seeders,
        max_age_days: filters.max_age_days,
        ..crate::search_processing::ResultOptions::default()
    };
    let request = SearchRequest {
        result_options: result_options.clone(),
        query,
        selection: IndexerSelection {
            all: only_indexer.is_none(),
            indexer_ids: only_indexer.into_iter().collect(),
            group_ids: Vec::new(),
        },
        limit_per_indexer: limit.clamp(1, 200),
        filters,
        languages: Vec::new(),
    };
    let prepared = prepare_search(state, request).await?;
    SEARCH_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let mut tasks = spawn_search_tasks(state, &prepared, None, true);
    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let batch = search_join_result(result);
        results.extend(batch.results);
    }
    let mut processed = crate::search_processing::process(results, &result_options).unique_results;
    processed.truncate(limit.min(200));
    // Mint encrypted tickets only for final, deduplicated releases. Doing this
    // per-indexer would otherwise create up to 50,000 SQL writes per RSS query.
    // Never return raw upstream URLs if the indexer disappeared meanwhile.
    for release in &mut processed {
        let id = release.indexer_instance_id.clone();
        if let Ok(handle) = build_executor(state, &id).await {
            crate::download::publish_feed(state, &handle.executor, std::slice::from_mut(release))
                .await;
        } else {
            release.download_url = None;
            release.details_url = None;
            release.magnet_url = None;
            release.source_guid = None;
        }
    }
    processed.retain(|r| {
        r.download_url
            .as_deref()
            .is_some_and(|link| link.starts_with("/api/v1/dl/"))
    });
    SEARCH_RELEASES.fetch_add(
        u64::try_from(processed.len()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    SEARCH_COMPLETED.fetch_add(1, Ordering::Relaxed);
    Ok(processed)
}

fn spawn_search_tasks(
    state: &AppState,
    prepared: &PreparedSearch,
    download_owner: Option<[u8; 32]>,
    feed: bool,
) -> JoinSet<SearchIndexerView> {
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
            search_one(
                &state,
                &indexer_id,
                &query,
                limit,
                &filters,
                download_owner,
                feed,
            )
            .await
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
    download_owner: Option<[u8; 32]>,
    feed: bool,
) -> SearchIndexerView {
    match build_executor(state, indexer_id).await {
        Ok(handle) => {
            let cached_or_fetched = if let Some(cached) = state
                .search_cache
                .get(
                    indexer_id,
                    query,
                    limit,
                    filters,
                    download_owner,
                    &handle.executor,
                )
                .await
            {
                Ok(cached)
            } else {
                let fetched = handle
                    .executor
                    .search_with_parameters(query, limit, filters)
                    .await;
                if let Ok(batch) = &fetched {
                    state
                        .search_cache
                        .put(
                            indexer_id,
                            query,
                            limit,
                            filters,
                            download_owner,
                            handle.executor.clone(),
                            batch.clone(),
                        )
                        .await;
                }
                fetched
            };
            match cached_or_fetched {
                Ok(IndexerSearchBatch {
                    indexer_instance_id,
                    definition_id,
                    response_time_ms,
                    mut results,
                    ..
                }) => {
                    if !feed {
                        state
                            .downloads
                            .publish(&handle.executor, &mut results, download_owner)
                            .await;
                    }
                    SearchIndexerView {
                        indexer_instance_id,
                        definition_id: Some(definition_id),
                        indexer_name: handle.display_name,
                        response_time_ms,
                        results,
                        error: None,
                    }
                }
                Err(error) => SearchIndexerView {
                    indexer_instance_id: indexer_id.to_owned(),
                    definition_id: Some(handle.definition_id),
                    indexer_name: handle.display_name,
                    response_time_ms: 0,
                    results: Vec::new(),
                    error: Some(error.to_string()),
                },
            }
        }
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

#[allow(
    clippy::too_many_lines,
    reason = "Keep execution-profile resolution and its revision-bound session backend together."
)]
pub(crate) async fn build_executor(
    state: &AppState,
    indexer_id: &str,
) -> Result<ExecutorHandle, String> {
    let record = state
        .store
        .get_indexer(indexer_id, &state.vault)
        .await
        .map_err(|error| error.to_string())?;
    if !record.enabled || !record.catalog_available {
        return Err("configured indexer is disabled or unavailable".to_owned());
    }
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
        .map_err(|error| error.to_string())?
        .with_session_persistence(Arc::new(
            super::tracker_sessions::StoredTrackerSession {
                store: state.store.clone(),
                vault: state.vault.clone(),
                indexer_id: indexer_id.to_owned(),
                revision: resolved.session_revision,
            },
        )),
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
    hash_part(&mut hasher, &resolved.session_revision.to_be_bytes());
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

/// When specialized clients supply a real identifier, never silently replace it
/// with the literal asterisk (which would turn an ID lookup into a wildcard).
fn has_structured_query(filters: &SearchParameters) -> bool {
    [
        filters.imdb_id.as_deref(),
        filters.tmdb_id.as_deref(),
        filters.tvdb_id.as_deref(),
        filters.tvmaze_id.as_deref(),
        filters.trakt_id.as_deref(),
        filters.douban_id.as_deref(),
        filters.tvrage_id.as_deref(),
        filters.artist.as_deref(),
        filters.album.as_deref(),
        filters.author.as_deref(),
        filters.title.as_deref(),
        filters.group.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}

fn validate_search_filters(filters: &SearchParameters) -> Result<(), ApiError> {
    for value in [
        filters.mode.as_deref(),
        filters.category.as_deref(),
        filters.genre.as_deref(),
        filters.imdb_id.as_deref(),
        filters.tmdb_id.as_deref(),
        filters.tvdb_id.as_deref(),
        filters.tvmaze_id.as_deref(),
        filters.trakt_id.as_deref(),
        filters.douban_id.as_deref(),
        filters.tvrage_id.as_deref(),
        filters.artist.as_deref(),
        filters.album.as_deref(),
        filters.author.as_deref(),
        filters.title.as_deref(),
        filters.publisher.as_deref(),
        filters.label.as_deref(),
        filters.track.as_deref(),
        filters.group.as_deref(),
        filters.episode_code.as_deref(),
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
        || filters.max_age_days.is_some_and(|age| age > 36500)
        || filters.min_seeders.is_some_and(|s| s > 10_000_000)
        || filters
            .min_size
            .zip(filters.max_size)
            .is_some_and(|(a, b)| a > b)
    {
        return Err(ApiError::bad_request(
            "search numeric filter is out of range",
        ));
    }
    Ok(())
}

fn validate_search_languages(languages: &[String]) -> Result<(), ApiError> {
    if languages.len() > 16
        || languages.iter().any(|language| {
            language.len() < 2
                || language.len() > 35
                || !language
                    .bytes()
                    .all(|character| character.is_ascii_alphanumeric() || character == b'-')
                || language.starts_with('-')
                || language.ends_with('-')
        })
    {
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
#[path = "execution/tests.rs"]
mod tests;
