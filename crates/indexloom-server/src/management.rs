use std::collections::BTreeSet;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
};
use indexloom_storage::{
    CatalogStatus, IndexerGroup, IndexerGroupInput, OutboundProxy, OutboundProxyInput, ProxyKind,
    ProxyPasswordUpdate,
};
use serde::{Deserialize, Serialize};
use url::Host;

use super::{ApiError, AppState, IndexerView, indexer_view, validate_identifier};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/catalog/status", get(catalog_status))
        .route("/api/v1/proxies", get(list_proxies))
        .route(
            "/api/v1/proxies/{id}",
            put(upsert_proxy).delete(delete_proxy),
        )
        .route("/api/v1/groups", get(list_groups))
        .route(
            "/api/v1/groups/{id}",
            put(upsert_group).delete(delete_group),
        )
        .route("/api/v1/routing", get(get_routing).put(save_routing))
        .route(
            "/api/v1/indexers/{id}/organization",
            put(save_indexer_organization),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyRequest {
    name: String,
    kind: ProxyKind,
    host: String,
    port: u16,
    enabled: bool,
    username: Option<String>,
    password: Option<String>,
    #[serde(default)]
    clear_password: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GroupRequest {
    name: String,
    #[serde(default)]
    description: String,
    enabled: bool,
    priority: i64,
    proxy_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoutingRequest {
    global_proxy_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoutingView {
    global_proxy_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IndexerOrganizationRequest {
    display_name: String,
    enabled: bool,
    proxy_id: Option<String>,
    #[serde(default)]
    group_ids: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

async fn catalog_status(State(state): State<AppState>) -> Result<Json<CatalogStatus>, ApiError> {
    Ok(Json(state.store.catalog_status().await?))
}

async fn list_proxies(State(state): State<AppState>) -> Result<Json<Vec<OutboundProxy>>, ApiError> {
    Ok(Json(state.store.list_proxies(&state.vault).await?))
}

async fn upsert_proxy(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ProxyRequest>,
) -> Result<Json<OutboundProxy>, ApiError> {
    validate_identifier(&id)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::bad_request(
            "proxy name must contain 1 to 100 bytes",
        ));
    }
    let host = request.host.trim().trim_matches(['[', ']']);
    if host.is_empty()
        || host.len() > 253
        || host.contains(['/', '@', '#', '?'])
        || Host::parse(host).is_err()
    {
        return Err(ApiError::bad_request(
            "proxy host must be an IP address or DNS hostname without a scheme or path",
        ));
    }
    let username = request
        .username
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if username.as_ref().is_some_and(|value| value.len() > 256) {
        return Err(ApiError::bad_request(
            "proxy username must not exceed 256 bytes",
        ));
    }
    if request.clear_password && request.password.is_some() {
        return Err(ApiError::bad_request(
            "password and clearPassword cannot be supplied together",
        ));
    }
    let password_update = match request.password {
        Some(password) if password.is_empty() || password.len() > 4096 => {
            return Err(ApiError::bad_request(
                "proxy password must contain 1 to 4096 bytes",
            ));
        }
        Some(password) => ProxyPasswordUpdate::Replace(password),
        None if request.clear_password => ProxyPasswordUpdate::Clear,
        None => ProxyPasswordUpdate::Preserve,
    };
    let input = OutboundProxyInput {
        id,
        name: name.to_owned(),
        kind: request.kind,
        host: host.to_owned(),
        port: request.port,
        enabled: request.enabled,
        username,
    };
    let proxy = state
        .store
        .upsert_proxy(&input, password_update, &state.vault)
        .await?;
    state.executor_cache.clear().await;
    Ok(Json(proxy))
}

async fn delete_proxy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_identifier(&id)?;
    state.store.delete_proxy(&id).await?;
    state.executor_cache.clear().await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_groups(State(state): State<AppState>) -> Result<Json<Vec<IndexerGroup>>, ApiError> {
    Ok(Json(state.store.list_groups().await?))
}

async fn upsert_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<GroupRequest>,
) -> Result<Json<IndexerGroup>, ApiError> {
    validate_identifier(&id)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 || request.description.len() > 1000 {
        return Err(ApiError::bad_request(
            "group name must contain 1 to 100 bytes and description at most 1000 bytes",
        ));
    }
    if !(-10_000..=10_000).contains(&request.priority) {
        return Err(ApiError::bad_request(
            "group priority must be between -10000 and 10000",
        ));
    }
    validate_proxy_reference(&state, request.proxy_id.as_deref()).await?;
    let input = IndexerGroupInput {
        id,
        name: name.to_owned(),
        description: request.description.trim().to_owned(),
        enabled: request.enabled,
        priority: request.priority,
        proxy_id: request.proxy_id,
    };
    let group = state.store.upsert_group(&input).await?;
    state.executor_cache.clear().await;
    Ok(Json(group))
}

async fn delete_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_identifier(&id)?;
    state.store.delete_group(&id).await?;
    state.executor_cache.clear().await;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_routing(State(state): State<AppState>) -> Result<Json<RoutingView>, ApiError> {
    Ok(Json(RoutingView {
        global_proxy_id: state.store.global_proxy_id().await?,
    }))
}

async fn save_routing(
    State(state): State<AppState>,
    Json(request): Json<RoutingRequest>,
) -> Result<Json<RoutingView>, ApiError> {
    validate_proxy_reference(&state, request.global_proxy_id.as_deref()).await?;
    state
        .store
        .set_global_proxy_id(request.global_proxy_id.as_deref())
        .await?;
    state.executor_cache.clear().await;
    Ok(Json(RoutingView {
        global_proxy_id: request.global_proxy_id,
    }))
}

async fn save_indexer_organization(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<IndexerOrganizationRequest>,
) -> Result<Json<IndexerView>, ApiError> {
    validate_identifier(&id)?;
    let display_name = request.display_name.trim();
    if display_name.is_empty() || display_name.len() > 160 {
        return Err(ApiError::bad_request(
            "indexer display name must contain 1 to 160 bytes",
        ));
    }
    validate_proxy_reference(&state, request.proxy_id.as_deref()).await?;
    let available_groups = state
        .store
        .list_groups()
        .await?
        .into_iter()
        .map(|group| group.id)
        .collect::<BTreeSet<_>>();
    let group_ids = deduplicate_identifiers(request.group_ids)?;
    if group_ids.iter().any(|id| !available_groups.contains(id)) {
        return Err(ApiError::bad_request(
            "selected indexer group was not found",
        ));
    }
    let tags = normalize_tags(request.tags)?;
    state
        .store
        .save_indexer_organization(
            &id,
            display_name,
            request.enabled,
            request.proxy_id.as_deref(),
            &group_ids,
            &tags,
        )
        .await?;
    state.executor_cache.invalidate(&id).await;
    Ok(Json(indexer_view(
        state.store.get_indexer(&id, &state.vault).await?,
    )?))
}

async fn validate_proxy_reference(
    state: &AppState,
    proxy_id: Option<&str>,
) -> Result<(), ApiError> {
    let Some(proxy_id) = proxy_id else {
        return Ok(());
    };
    validate_identifier(proxy_id)?;
    if state
        .store
        .list_proxies(&state.vault)
        .await?
        .iter()
        .any(|proxy| proxy.id == proxy_id)
    {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "selected outbound proxy was not found",
        ))
    }
}

fn deduplicate_identifiers(values: Vec<String>) -> Result<Vec<String>, ApiError> {
    if values.len() > 100 {
        return Err(ApiError::bad_request(
            "an indexer cannot belong to more than 100 groups",
        ));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        validate_identifier(&value)?;
        unique.insert(value);
    }
    Ok(unique.into_iter().collect())
}

fn normalize_tags(values: Vec<String>) -> Result<Vec<String>, ApiError> {
    if values.len() > 100 {
        return Err(ApiError::bad_request(
            "an indexer cannot have more than 100 tags",
        ));
    }
    let mut tags = BTreeSet::new();
    for value in values {
        let value = value.trim().to_lowercase();
        if value.is_empty()
            || value.chars().count() > 48
            || value.chars().any(|character| {
                !(character.is_alphanumeric() || matches!(character, '-' | '_' | '.' | ' '))
            })
        {
            return Err(ApiError::bad_request(
                "tags must contain 1 to 48 letters, numbers, spaces, dots, underscores, or dashes",
            ));
        }
        tags.insert(value);
    }
    Ok(tags.into_iter().collect())
}
