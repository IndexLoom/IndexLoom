//! Stage C management API. All routes inherit the existing administrator middleware.
mod apps;
mod lazylibrarian;
mod mylar;
mod named;
mod profile;
mod transport;
use crate::{ApiError, AppState, validate_identifier};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use indexloom_storage::integrations::{Integration, IntegrationSecrets};
use profile::Profile;
use serde::Deserialize;
use serde_json::{Value, json};
use transport::Remote;
// Serializes configuration and remote mutations so a stale FullSync cannot race a profile edit.
static MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    provider: String,
    name: String,
    origin: String,
    enabled: bool,
    secrets: Option<IntegrationSecrets>,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/integrations", get(list))
        .route(
            "/api/v1/integrations/{id}",
            get(get_one).put(save).delete(remove),
        )
        .route(
            "/api/v1/integrations/{id}/profile",
            get(get_profile).put(save_profile),
        )
        .route("/api/v1/integrations/{id}/test", post(test))
        .route("/api/v1/integrations/{id}/sync", post(sync))
}
pub(super) fn allow_lan() -> bool {
    std::env::var("INDEXLOOM_INTEGRATION_ALLOW_LAN").is_ok_and(|value| value == "true")
}

fn lan_http_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            ip.octets()[0] == 10
                || (ip.octets()[0] == 172 && (16..=31).contains(&ip.octets()[1]))
                || (ip.octets()[0..2] == [192, 168])
        }
        Some(url::Host::Ipv6(ip)) => (ip.segments()[0] & 0xfe00) == 0xfc00,
        Some(url::Host::Domain(host)) => {
            !host.contains('.')
                || host
                    .rsplit('.')
                    .next()
                    .is_some_and(|suffix| matches!(suffix, "local" | "internal"))
        }
        _ => false,
    }
}

pub(super) fn origin(raw: &str) -> Result<url::Url, ApiError> {
    let url =
        url::Url::parse(raw).map_err(|_| ApiError::bad_request("invalid integration origin"))?;
    if raw.len() > 1024
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(ApiError::bad_request(
            "integration URL must be an HTTP(S) origin without path, credentials, query or fragment",
        ));
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        _ => false,
    };
    if url.scheme() != "https" && !loopback && !(allow_lan() && lan_http_host(&url)) {
        return Err(ApiError::bad_request(
            "HTTP integrations require loopback, or explicit LAN opt-in with a private host",
        ));
    }
    Ok(url)
}
fn supported(provider: &str) -> bool {
    apps::version(provider).is_some() || matches!(provider, "mylar" | "lazylibrarian")
}
fn unsupported() -> ApiError {
    ApiError::bad_request("unsupported application")
}
async fn item(state: &AppState, id: &str) -> Result<Integration, ApiError> {
    validate_identifier(id)?;
    state
        .store
        .get_integration(id)
        .await?
        .ok_or_else(|| ApiError::not_found("integration not found"))
}
async fn remote(state: &AppState, item: &Integration) -> Result<Remote, ApiError> {
    if !item.enabled {
        return Err(ApiError::conflict("integration is disabled"));
    }
    Remote::new(
        item,
        state.store.integration_secrets(item, &state.vault).await?,
    )
    .await
}
async fn list(State(s): State<AppState>) -> Result<Json<Vec<Integration>>, ApiError> {
    Ok(Json(s.store.list_integrations().await?))
}
async fn get_one(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Integration>, ApiError> {
    Ok(Json(item(&s, &id).await?))
}
async fn save(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Input>,
) -> Result<Json<Integration>, ApiError> {
    let _guard = MUTATION.lock().await;
    validate_identifier(&id)?;
    if !supported(&input.provider) {
        return Err(unsupported());
    }
    if input.name.is_empty() || input.name.len() > 100 || input.name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("invalid integration name"));
    }
    let origin = origin(&input.origin)?.origin().ascii_serialization();
    if let Some(secrets) = &input.secrets
        && (secrets.0.len() > 6
            || secrets.0.iter().any(|(k, v)| {
                !matches!(k.as_str(), "apiKey" | "feedApiKey")
                    || v.len() > 8192
                    || v.chars().any(char::is_control)
            }))
    {
        return Err(ApiError::bad_request("invalid integration credentials"));
    }
    let entry = Integration {
        id,
        provider: input.provider,
        name: input.name,
        origin,
        enabled: input.enabled,
    };
    if let Some(old) = s.store.get_integration(&entry.id).await?
        && (old.origin != entry.origin || old.provider != entry.provider)
    {
        return Err(ApiError::conflict(
            "provider and origin are immutable; delete and recreate integration",
        ));
    }
    s.store
        .upsert_integration(&entry, input.secrets.as_ref(), &s.vault)
        .await?;
    Ok(Json(entry))
}
async fn remove(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _guard = MUTATION.lock().await;
    item(&s, &id).await?;
    s.store.delete_integration(&id).await?;
    Ok(Json(json!({"deleted":true})))
}
async fn profile(s: &AppState, id: &str) -> Result<Profile, ApiError> {
    s.store
        .integration_profile(id)
        .await?
        .map_or(Ok(Profile::default()), |v| {
            serde_json::from_str(&v)
                .map_err(|_| ApiError::internal("invalid stored integration profile"))
        })
}
async fn get_profile(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Profile>, ApiError> {
    item(&s, &id).await?;
    Ok(Json(profile(&s, &id).await?))
}
async fn save_profile(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(p): Json<Profile>,
) -> Result<Json<Profile>, ApiError> {
    let _guard = MUTATION.lock().await;
    item(&s, &id).await?;
    p.validate_selectors(&s).await?;
    s.store
        .save_integration_profile(
            &id,
            &serde_json::to_string(&p).map_err(|_| ApiError::bad_request("invalid profile"))?,
        )
        .await?;
    Ok(Json(p))
}
async fn test(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let _guard = MUTATION.lock().await;
    let i = item(&s, &id).await?;
    let r = remote(&s, &i).await?;
    let result = if let Some(version) = apps::version(&i.provider) {
        r.arr(version, reqwest::Method::GET, "system/status", None)
            .await
            .map(|_| ())
    } else if supported(&i.provider) {
        named::test(&i, &r).await
    } else {
        return Err(unsupported());
    };
    s.store
        .integration_event(&id, "test", result.is_ok())
        .await?;
    result?;
    Ok(Json(json!({"success":true})))
}
async fn sync(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(sync_id(&s, &id).await?))
}
pub(crate) async fn sync_id(s: &AppState, id: &str) -> Result<Value, ApiError> {
    let _guard = MUTATION.lock().await;
    let i = item(s, id).await?;
    let p = profile(s, id).await?;
    p.validate()?;
    if !supported(&i.provider) {
        return Err(unsupported());
    }
    if !i.enabled || p.mode == profile::SyncMode::Disabled {
        return Ok(json!({"skipped":true,"reason":"disabled"}));
    }
    let r = remote(s, &i).await?;
    let result = if apps::version(&i.provider).is_some() {
        apps::sync(s, &i, &r, &p).await
    } else {
        named::sync(s, &i, &r, &p).await
    };
    s.store
        .integration_event(id, "sync", result.is_ok())
        .await?;
    result
}
#[cfg(test)]
mod validation_tests {
    use super::*;
    #[test]
    fn origin_validation() {
        for raw in [
            "file:///tmp/x",
            "http://u:p@localhost",
            "http://localhost/api",
            "http://localhost/?key=secret",
            "http://localhost/#x",
            "https://other//evil",
        ] {
            assert!(origin(raw).is_err(), "{raw}");
        }
        assert!(origin("http://127.0.0.1:8080").is_ok());
        assert!(supported("mylar"));
        assert!(!supported("flood"));
    }
}

#[cfg(test)]
mod tests;
