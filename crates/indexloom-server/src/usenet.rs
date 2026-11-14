//! Administrator-managed Newznab sources. API keys are encrypted in SQLite,
//! never included in the custom YAML definitions or returned through JSON.
use crate::{ApiError, AppState, validate_identifier};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, put},
};
use indexloom_definitions::{Protocol, parse_yaml};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use url::{Host, Url};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewznabInput {
    name: String,
    endpoint: String,
    api_key: Option<String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
}
const fn default_enabled() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NewznabView {
    id: String,
    name: String,
    endpoint: String,
    enabled: bool,
    key_configured: bool,
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/usenet/indexers", get(list))
        .route("/api/v1/usenet/indexers/{id}", put(save).delete(remove))
}
async fn list(State(state): State<AppState>) -> Result<Json<Vec<NewznabView>>, ApiError> {
    let mut sources = Vec::new();
    for indexer in state.store.list_indexers(&state.vault).await? {
        let Ok(def) = parse_yaml(&indexer.definition_yaml) else {
            continue;
        };
        if def.spec.protocol != Protocol::Usenet {
            continue;
        }
        sources.push(NewznabView {
            id: indexer.id,
            name: indexer.display_name,
            endpoint: def.spec.base_urls.first().cloned().unwrap_or_default(),
            enabled: indexer.enabled,
            key_configured: indexer.configured_secrets.iter().any(|v| v == "apiKey"),
        });
    }
    Ok(Json(sources))
}

fn sanitize_endpoint(raw: &str) -> Result<Url, ApiError> {
    if raw.len() > 1024 {
        return Err(ApiError::bad_request("Newznab endpoint is too long"));
    }
    let mut url = Url::parse(raw).map_err(|_| ApiError::bad_request("invalid Newznab endpoint"))?;
    if url.scheme() != "https"
        || !matches!(url.host(), Some(Host::Domain(_)))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(ApiError::bad_request(
            "Newznab requires a public HTTPS domain without credentials, query or fragment",
        ));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    #[allow(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "Host is normalized lowercase and checked by DNS suffix, not a local file extension."
    )]
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || !host.contains('.')
    {
        return Err(ApiError::bad_request(
            "private Newznab hostnames are not permitted",
        ));
    }
    if !url.path().ends_with('/') && !url.path().ends_with("/api") {
        let new_path = format!("{}/", url.path());
        url.set_path(&new_path);
    }
    Ok(url)
}
fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for value in bytes {
        write!(&mut output, "{value:02x}").expect("hex to String");
    }
    output
}
fn custom_definition(id: &str, name: &str, url: &Url) -> Result<(String, String), ApiError> {
    let zeros40 = "0".repeat(40);
    let zeros64 = "0".repeat(64);
    let payload = serde_json::json!({
        "apiVersion":"indexloom.io/v1alpha1",
        "kind":"Indexer",
        "metadata":{"id":id,"name":name,"description":"User-managed Newznab/Usenet source","language":"en"},
        "provenance":{"generatedBy":"indexloom-user/v1","source":{
            "project":"user","repository":"https://github.com/IndexLoom/IndexLoom",
            "revision":zeros40,"path":format!("user/newznab/{id}.json"),
            "sourceId":id,"license":"NOASSERTION",
            "fileSha256":zeros64,"contentSha256":zeros64
        }},
        "spec":{
            "protocol":"usenet","privacy":"private","baseUrls":[url.to_string()],
            "legacyUrls":[],"replacedIds":[],
            "implementation":{
                "engine":"newznab","encoding":"UTF-8","followRedirects":false,
                "testLinkTorrent":false,"requestDelaySeconds":1.0,"certificates":[],
                 "capabilities":{"categorymappings":[
                    {"id":"1000","cat":"Console"},
                    {"id":"2000","cat":"Movies"},
                    {"id":"3000","cat":"Audio"},
                    {"id":"4000","cat":"PC"},
                    {"id":"5000","cat":"TV"},
                    {"id":"6000","cat":"XXX"},
                    {"id":"7000","cat":"Books"},
                    {"id":"8000","cat":"Other"}
                ],"modes":{
                    "search":["q"],"tvsearch":["q","tvdbid","tvmazeid","season","ep"],
                    "movie":["q","imdbid","tmdbid"],"music":["artist","album"],
                    "book":["author","title"]
                }},
                "settings":[{"name":"apiKey","label":"Newznab API key","type":"password"}],
                "authentication":null,"download":null,"search":{}
            }
        }
    });
    let json = payload.to_string();
    parse_yaml(&json)
        .map_err(|_| ApiError::bad_request("generated Newznab definition is invalid"))?;
    let digest = Sha256::digest(json.as_bytes());
    Ok((json, hex(digest.as_slice())))
}
async fn save(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<NewznabInput>,
) -> Result<Json<NewznabView>, ApiError> {
    validate_identifier(&id)?;
    if !id.starts_with("usenet-") || id.len() > 100 {
        return Err(ApiError::bad_request(
            "custom Usenet id must begin with usenet-",
        ));
    }
    if request.name.len() < 2
        || request.name.len() > 100
        || request.name.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request("invalid Newznab display name"));
    }
    if request
        .api_key
        .as_ref()
        .is_some_and(|key| !(8..=512).contains(&key.len()) || key.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request("invalid Newznab API key"));
    }
    let url = sanitize_endpoint(&request.endpoint)?;
    let (definition, sha) = custom_definition(&id, &request.name, &url)?;
    state
        .store
        .upsert_newznab_indexer(
            &id,
            &request.name,
            &definition,
            &sha,
            request.enabled,
            request.api_key.as_deref(),
            &state.vault,
        )
        .await?;
    state.executor_cache.invalidate(&id).await;
    state.search_cache.clear().await;
    Ok(Json(NewznabView {
        id,
        name: request.name,
        endpoint: url.to_string(),
        enabled: request.enabled,
        key_configured: true,
    }))
}
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_identifier(&id)?;
    state.store.delete_newznab_indexer(&id).await?;
    state.executor_cache.invalidate(&id).await;
    state.search_cache.clear().await;
    Ok(Json(serde_json::json!({"deleted":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_internal_origins_and_drops_all_secrets_from_definition() {
        assert!(sanitize_endpoint("http://example.org/").is_err());
        assert!(sanitize_endpoint("https://127.0.0.1/api").is_err());
        assert!(sanitize_endpoint("https://example.org/?token=leak").is_err());
        let url = sanitize_endpoint("https://indexer.example/api").unwrap();
        let (json, _) = custom_definition("usenet-example", "Example", &url).unwrap();
        assert!(json.contains("newznab"));
        assert!(!json.contains("secret-password"));
    }
}
