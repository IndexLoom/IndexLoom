//! Search preset CRUD, isolated by the stable user identity, not session token.
use crate::search_processing::ResultOptions;
use crate::{ApiError, AppState, auth, validate_identifier};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
};
use indexloom_engine::SearchParameters;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PresetDocument {
    pub filters: SearchParameters,
    pub result_options: ResultOptions,
    pub selection: PresetSelection,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PresetSelection {
    pub all: bool,
    pub indexer_ids: Vec<String>,
    pub group_ids: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresetInput {
    name: String,
    document: PresetDocument,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresetView {
    id: String,
    name: String,
    document: PresetDocument,
    created_at: String,
    updated_at: String,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/search/presets", get(list))
        .route(
            "/api/v1/search/presets/{id}",
            axum::routing::put(save).delete(remove),
        )
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PresetView>>, ApiError> {
    let owner = auth::search_preset_owner(&state, &headers).await?;
    let mut result = Vec::new();
    for record in state.store.list_search_presets(&owner).await? {
        let Ok(document) = serde_json::from_str(&record.document_json) else {
            continue;
        };
        result.push(PresetView {
            id: record.id,
            name: record.name,
            document,
            created_at: record.created_at,
            updated_at: record.updated_at,
        });
    }
    Ok(Json(result))
}
async fn save(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<PresetInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_identifier(&id)?;
    if id.len() > 80
        || request.name.is_empty()
        || request.name.len() > 100
        || request.name.chars().any(char::is_control)
        || request.document.selection.indexer_ids.len() > 256
        || request.document.selection.group_ids.len() > 64
        || request
            .document
            .selection
            .indexer_ids
            .iter()
            .chain(&request.document.selection.group_ids)
            .any(|v| v.len() > 100)
    {
        return Err(ApiError::bad_request("invalid search preset"));
    }
    request
        .document
        .result_options
        .validate()
        .map_err(ApiError::bad_request)?;
    let json = serde_json::to_string(&request.document)
        .map_err(|_| ApiError::bad_request("invalid preset encoding"))?;
    if json.len() > 32768 {
        return Err(ApiError::bad_request("search preset too large"));
    }
    let owner = auth::search_preset_owner(&state, &headers).await?;
    state
        .store
        .save_search_preset(&owner, &id, &request.name, &json)
        .await?;
    Ok(Json(serde_json::json!({"saved":true})))
}
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_identifier(&id)?;
    let owner = auth::search_preset_owner(&state, &headers).await?;
    state.store.delete_search_preset(&owner, &id).await?;
    Ok(Json(serde_json::json!({"deleted":true})))
}
