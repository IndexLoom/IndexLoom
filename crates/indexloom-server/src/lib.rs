//! Authenticated HTTP surface for `IndexLoom`.

use std::{collections::BTreeMap, fmt, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use indexloom_definitions::{
    ConfigurationField, ConfigurationFieldKind, ConfigurationSchema, Privacy, parse_yaml,
};
use indexloom_storage::{
    FlareSolverrInput, FlareSolverrInstance, IndexerConfigurationRecord, LastIndexerTest,
    SecretVault, StorageError, Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
#[cfg(not(any(feature = "embedded-web", feature = "headless")))]
use tower_http::services::ServeDir;

#[cfg(all(feature = "embedded-web", feature = "headless"))]
compile_error!("embedded-web and headless are mutually exclusive");

#[cfg(feature = "embedded-web")]
mod embedded_web {
    include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
}

use url::Url;

pub mod auth;
pub mod catalog;
mod download;
mod execution;
pub mod local;
mod management;
pub mod remote;
mod torznab;
mod tracker_login;
mod tracker_sessions;

const MAX_CONFIGURATION_BODY: usize = 128 * 1024;
const MAX_USER_AGENT_BYTES: usize = 512;
const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_CONFIGURATION_FIELDS: usize = 128;

/// Shared application state.
#[derive(Clone, Debug)]
pub struct AppState {
    store: Store,
    vault: SecretVault,
    admin_token: Option<AdminToken>,
    auth_settings: auth::AuthSettings,
    mail_sender: Option<auth::MailSender>,
    test_jobs: execution::TestJobRegistry,
    executor_cache: execution::ExecutorCache,
    downloads: download::DownloadRegistry,
    search_permits: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    /// Creates application state around storage, encryption, and authorization.
    #[must_use]
    pub fn new(store: Store, vault: SecretVault, admin_token: AdminToken) -> Self {
        Self {
            store,
            vault,
            admin_token: Some(admin_token),
            auth_settings: auth::AuthSettings::legacy(),
            mail_sender: None,
            test_jobs: execution::TestJobRegistry::default(),
            executor_cache: execution::ExecutorCache::default(),
            downloads: download::DownloadRegistry::new(),
            search_permits: execution::search_permits(),
        }
    }

    /// Runtime-selectable authentication and mail configuration.
    #[must_use]
    pub fn with_auth(
        store: Store,
        vault: SecretVault,
        admin_token: Option<AdminToken>,
        auth_settings: auth::AuthSettings,
        mail_sender: Option<auth::MailSender>,
    ) -> Self {
        Self {
            store,
            vault,
            admin_token,
            auth_settings,
            mail_sender,
            test_jobs: execution::TestJobRegistry::default(),
            executor_cache: execution::ExecutorCache::default(),
            downloads: download::DownloadRegistry::new(),
            search_permits: execution::search_permits(),
        }
    }

    /// Drops cached authenticated indexer runtimes after a catalog refresh.
    pub async fn clear_execution_runtimes(&self) {
        self.executor_cache.clear().await;
    }
}

/// Bearer token protecting configuration and credential endpoints.
#[derive(Clone)]
pub struct AdminToken(Arc<[u8]>);

impl AdminToken {
    /// Accepts an administrator token with at least 32 bytes of input.
    ///
    /// # Errors
    ///
    /// Returns a static validation message when the token is too short or
    /// contains surrounding whitespace.
    pub fn new(value: &str) -> Result<Self, &'static str> {
        if value.len() < 32 {
            return Err("INDEXLOOM_ADMIN_TOKEN must be at least 32 bytes");
        }
        if value.trim() != value {
            return Err("INDEXLOOM_ADMIN_TOKEN must not contain surrounding whitespace");
        }
        Ok(Self(Arc::from(value.as_bytes())))
    }

    fn authorizes(&self, headers: &HeaderMap) -> bool {
        let Some(candidate) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        let expected = self.0.as_ref();
        expected.len() == candidate.len() && bool::from(expected.ct_eq(candidate.as_bytes()))
    }
}

impl fmt::Debug for AdminToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdminToken(<redacted>)")
    }
}

/// Builds the public health route, authenticated management API, and frontend.
pub fn router(state: AppState, web_root: PathBuf) -> Router {
    let protected = Router::new()
        .route("/api/v1/indexers", get(list_indexers))
        .route(
            "/api/v1/indexers/{id}/configuration",
            put(save_indexer_configuration),
        )
        .route("/api/v1/flaresolverr", get(list_flaresolverr))
        .route(
            "/api/v1/flaresolverr/{id}",
            put(upsert_flaresolverr).delete(delete_flaresolverr),
        )
        .merge(management::routes())
        .merge(execution::routes())
        .merge(tracker_login::routes())
        .merge(download::routes())
        .merge(auth::private_routes())
        .layer(DefaultBodyLimit::max(MAX_CONFIGURATION_BODY))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_admin));

    let api = Router::new()
        .route("/api/v1/health", get(health))
        .merge(auth::public_routes())
        .merge(torznab::routes())
        .merge(protected)
        .layer(middleware::from_fn(security_headers));

    #[cfg(feature = "headless")]
    {
        let _ = web_root;
        api.with_state(state)
    }
    #[cfg(feature = "embedded-web")]
    {
        let _ = web_root;
        api.fallback(serve_embedded_web).with_state(state)
    }
    #[cfg(not(any(feature = "headless", feature = "embedded-web")))]
    {
        let web = ServeDir::new(web_root).append_index_html_on_directories(true);
        api.fallback_service(web).with_state(state)
    }
}

async fn security_headers(request: Request, next: Next) -> Response {
    let auth_endpoint = request.uri().path().starts_with("/api/v1/auth/")
        || request.uri().path().contains("/login-challenge");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if auth_endpoint {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, private"),
        );
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    response
}

#[cfg(feature = "embedded-web")]
async fn serve_embedded_web(uri: axum::http::Uri) -> Response {
    let path = uri.path();
    let asset = embedded_web::embedded_asset(path).or_else(|| {
        if path.rsplit('/').next().unwrap_or("").contains('.') {
            None
        } else {
            embedded_web::embedded_asset("/index.html")
        }
    });
    match asset {
        Some((bytes, mime)) => {
            (StatusCode::OK, [(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    database: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexerView {
    id: String,
    definition_id: String,
    display_name: String,
    enabled: bool,
    catalog_available: bool,
    privacy: Privacy,
    user_agent: Option<String>,
    flaresolverr_instance_id: Option<String>,
    proxy_id: Option<String>,
    group_ids: Vec<String>,
    tags: Vec<String>,
    last_test: Option<LastIndexerTest>,
    values: BTreeMap<String, Value>,
    configured_secrets: Vec<String>,
    configuration_schema: ConfigurationSchema,
    capabilities: indexloom_definitions::capabilities::IndexerCapabilities,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveConfigurationRequest {
    user_agent: Option<String>,
    flaresolverr_instance_id: Option<String>,
    #[serde(default)]
    values: BTreeMap<String, Value>,
    /// A missing key is unchanged, a string replaces it, and `null` clears it.
    #[serde(default)]
    secrets: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FlareSolverrRequest {
    name: String,
    endpoint: String,
    enabled: bool,
    timeout_seconds: i64,
}

async fn health(
    State(state): State<AppState>,
) -> Result<Json<HealthResponse>, (StatusCode, Json<HealthResponse>)> {
    match state.store.ping().await {
        Ok(()) => Ok(Json(HealthResponse {
            status: "ok",
            version: env!("CARGO_PKG_VERSION"),
            database: "ok",
        })),
        Err(error) => {
            tracing::error!(%error, "database health check failed");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    version: env!("CARGO_PKG_VERSION"),
                    database: "unavailable",
                }),
            ))
        }
    }
}

async fn require_admin(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Err(error) = auth::authorize_action(
        &state,
        request.headers(),
        request.method(),
        request.uri().path(),
    )
    .await
    {
        return error.into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

async fn list_indexers(State(state): State<AppState>) -> Result<Json<Vec<IndexerView>>, ApiError> {
    let records = state.store.list_indexers(&state.vault).await?;
    let views = records
        .into_iter()
        .map(indexer_view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(views))
}

async fn save_indexer_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SaveConfigurationRequest>,
) -> Result<Json<IndexerView>, ApiError> {
    validate_identifier(&id)?;
    let record = state.store.get_indexer(&id, &state.vault).await?;
    let definition = parse_yaml(&record.definition_yaml)
        .map_err(|_| ApiError::internal("stored indexer definition is invalid"))?;
    let schema = definition.configuration_schema();
    validate_configuration(&request, &schema)?;

    if let Some(connector_id) = request.flaresolverr_instance_id.as_deref() {
        validate_identifier(connector_id)?;
        let connectors = state.store.list_flaresolverr().await?;
        if !connectors
            .iter()
            .any(|connector| connector.id == connector_id)
        {
            return Err(ApiError::bad_request(
                "selected FlareSolverr connector was not found",
            ));
        }
    }

    let mut resolved = state
        .store
        .resolve_indexer_configuration(&id, &state.vault)
        .await?;
    for (name, update) in request.secrets {
        if let Some(value) = update {
            resolved.secrets.insert(name, value);
        } else {
            resolved.secrets.remove(&name);
        }
    }
    state
        .store
        .save_indexer_configuration(
            &id,
            request.user_agent.as_deref(),
            request.flaresolverr_instance_id.as_deref(),
            &request.values,
            &resolved.secrets,
            &state.vault,
        )
        .await?;
    state.executor_cache.invalidate(&id).await;

    let refreshed = state.store.get_indexer(&id, &state.vault).await?;
    Ok(Json(indexer_view(refreshed)?))
}

async fn list_flaresolverr(
    State(state): State<AppState>,
) -> Result<Json<Vec<FlareSolverrInstance>>, ApiError> {
    Ok(Json(state.store.list_flaresolverr().await?))
}

async fn upsert_flaresolverr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<FlareSolverrRequest>,
) -> Result<Json<FlareSolverrInstance>, ApiError> {
    validate_identifier(&id)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::bad_request(
            "FlareSolverr name must contain 1 to 100 bytes",
        ));
    }
    if !(5..=300).contains(&request.timeout_seconds) {
        return Err(ApiError::bad_request(
            "FlareSolverr timeout must be between 5 and 300 seconds",
        ));
    }
    let endpoint = normalize_flaresolverr_endpoint(&request.endpoint)?;
    let input = FlareSolverrInput {
        id,
        name: name.to_owned(),
        endpoint,
        enabled: request.enabled,
        timeout_seconds: request.timeout_seconds,
    };
    let connector = state.store.upsert_flaresolverr(&input).await?;
    state.executor_cache.clear().await;
    Ok(Json(connector))
}

async fn delete_flaresolverr(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_identifier(&id)?;
    state.store.delete_flaresolverr(&id).await?;
    state.executor_cache.clear().await;
    Ok(StatusCode::NO_CONTENT)
}

fn indexer_view(record: IndexerConfigurationRecord) -> Result<IndexerView, ApiError> {
    let definition = parse_yaml(&record.definition_yaml)
        .map_err(|_| ApiError::internal("stored indexer definition is invalid"))?;
    Ok(IndexerView {
        id: record.id,
        definition_id: record.definition_id,
        display_name: record.display_name,
        enabled: record.enabled,
        catalog_available: record.catalog_available,
        privacy: definition.spec.privacy,
        user_agent: record.user_agent,
        flaresolverr_instance_id: record.flaresolverr_instance_id,
        proxy_id: record.proxy_id,
        group_ids: record.group_ids,
        tags: record.tags,
        last_test: record.last_test,
        values: record.values,
        configured_secrets: record.configured_secrets,
        configuration_schema: definition.configuration_schema(),
        capabilities: indexloom_definitions::capabilities::summarize(&definition),
    })
}

fn validate_configuration(
    request: &SaveConfigurationRequest,
    schema: &ConfigurationSchema,
) -> Result<(), ApiError> {
    if request.values.len() > MAX_CONFIGURATION_FIELDS
        || request.secrets.len() > MAX_CONFIGURATION_FIELDS
    {
        return Err(ApiError::bad_request("too many configuration fields"));
    }
    if let Some(user_agent) = request.user_agent.as_deref()
        && (user_agent.is_empty()
            || user_agent.len() > MAX_USER_AGENT_BYTES
            || user_agent.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request(
            "User-Agent must contain 1 to 512 bytes without control characters",
        ));
    }

    let fields = schema
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    for (name, value) in &request.values {
        let field = fields
            .get(name.as_str())
            .ok_or_else(|| ApiError::bad_request("unknown configuration field"))?;
        if field.secret {
            return Err(ApiError::bad_request(
                "secret fields must be sent through the secrets object",
            ));
        }
        validate_public_value(field, value)?;
    }
    for (name, update) in &request.secrets {
        let field = fields
            .get(name.as_str())
            .ok_or_else(|| ApiError::bad_request("unknown secret field"))?;
        if !field.secret {
            return Err(ApiError::bad_request(
                "non-secret fields must be sent through the values object",
            ));
        }
        if let Some(value) = update
            && (value.is_empty() || value.len() > MAX_SECRET_BYTES)
        {
            return Err(ApiError::bad_request(
                "secret values must contain 1 to 65536 bytes; use null to clear",
            ));
        }
    }
    Ok(())
}

fn validate_public_value(field: &ConfigurationField, value: &Value) -> Result<(), ApiError> {
    match field.kind {
        ConfigurationFieldKind::Text if value.is_string() => Ok(()),
        ConfigurationFieldKind::Checkbox if value.is_boolean() => Ok(()),
        ConfigurationFieldKind::Select => {
            let Some(selected) = scalar_json_string(value) else {
                return Err(ApiError::bad_request("select values must be scalar"));
            };
            if field.options.iter().any(|option| option.value == selected) {
                Ok(())
            } else {
                Err(ApiError::bad_request("invalid select option"))
            }
        }
        ConfigurationFieldKind::Password => Err(ApiError::bad_request(
            "password fields must be stored as secrets",
        )),
        _ => Err(ApiError::bad_request(
            "configuration value has the wrong type",
        )),
    }
}

fn scalar_json_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn normalize_flaresolverr_endpoint(value: &str) -> Result<String, ApiError> {
    let mut url = Url::parse(value.trim())
        .map_err(|_| ApiError::bad_request("FlareSolverr endpoint is not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(ApiError::bad_request(
            "FlareSolverr endpoint must be an HTTP(S) URL without credentials, query, or fragment",
        ));
    }
    match url.path().trim_end_matches('/') {
        "" | "/v1" => url.set_path("/v1"),
        _ => {
            return Err(ApiError::bad_request(
                "FlareSolverr endpoint path must be /v1",
            ));
        }
    }
    Ok(url.to_string())
}

fn validate_identifier(value: &str) -> Result<(), ApiError> {
    let valid = !value.is_empty()
        && value.len() <= 100
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "identifier must be lowercase ASCII kebab-case",
        ))
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    const fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorBody {
                code: "UNAUTHORIZED",
                message: "valid authentication is required",
            },
        }
    }

    const fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            body: ErrorBody {
                code: "FORBIDDEN",
                message: "your role does not permit this operation",
            },
        }
    }

    const fn conflict(message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            body: ErrorBody {
                code: "CONFLICT",
                message,
            },
        }
    }

    const fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorBody {
                code: "INVALID_CONFIGURATION",
                message,
            },
        }
    }

    const fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: ErrorBody {
                code: "NOT_FOUND",
                message,
            },
        }
    }

    const fn too_many_requests(message: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: ErrorBody {
                code: "TOO_MANY_REQUESTS",
                message,
            },
        }
    }

    const fn internal(message: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ErrorBody {
                code: "INTERNAL_ERROR",
                message,
            },
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        if matches!(error, StorageError::NotFound(_)) {
            return Self::not_found("requested record was not found");
        }
        tracing::error!(%error, "management API storage failure");
        Self::internal("storage operation failed")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body)).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
            .headers_mut()
            .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
        response
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
