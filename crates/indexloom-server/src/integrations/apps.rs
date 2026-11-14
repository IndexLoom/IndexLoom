//! Schema-backed *arr adapters. Only recorded, identity-matching entries are owned.
use super::{
    ApiError, AppState, Integration, Remote,
    profile::{Profile, SyncMode},
    transport::failure,
};
use indexloom_definitions::{Protocol, parse_yaml};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) fn version(provider: &str) -> Option<&'static str> {
    match provider {
        "sonarr" | "radarr" | "whisparr" => Some("v3"),
        "lidarr" | "readarr" => Some("v1"),
        _ => None,
    }
}
fn family(provider: &str) -> u32 {
    match provider {
        "sonarr" => 5000,
        "radarr" => 2000,
        "whisparr" => 6000,
        "lidarr" => 3000,
        "readarr" => 7000,
        _ => 0,
    }
}
fn marker(integration: &str, local: &str) -> String {
    format!("IndexLoom [{integration}/{local}]")
}
fn owns(value: &Value, integration: &str, local: &str) -> bool {
    value["name"].as_str() == Some(marker(integration, local).as_str())
        && matches!(
            value["implementation"].as_str(),
            Some("Torznab" | "Newznab")
        )
}
fn set_field(body: &mut Value, name: &str, value: Value) -> Result<(), ApiError> {
    let fields = body["fields"].as_array_mut().ok_or_else(failure)?;
    let field = fields
        .iter_mut()
        .find(|f| f["name"] == name)
        .ok_or_else(|| ApiError::bad_request("remote schema lacks a required indexer field"))?;
    field["value"] = value;
    Ok(())
}
fn categories(provider: &str, p: &Profile) -> Vec<u32> {
    let base = family(provider);
    if p.categories.is_empty() {
        vec![base]
    } else {
        p.categories
            .iter()
            .copied()
            .filter(|c| *c / 1000 == base / 1000)
            .collect()
    }
}
#[async_trait::async_trait]
trait AppApi: Sync {
    async fn arr(
        &self,
        version: &str,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError>;
    fn secret(&self, key: &str) -> Result<&str, ApiError>;
}
#[async_trait::async_trait]
impl AppApi for Remote {
    async fn arr(
        &self,
        version: &str,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        Remote::arr(self, version, method, path, body).await
    }
    fn secret(&self, key: &str) -> Result<&str, ApiError> {
        Remote::secret(self, key)
    }
}
pub(super) async fn sync(
    state: &AppState,
    app: &Integration,
    remote: &Remote,
    p: &Profile,
) -> Result<Value, ApiError> {
    sync_with_api(state, app, remote, p).await
}
#[allow(
    clippy::too_many_lines,
    reason = "The staged remote sync is an ordered, ownership-fenced workflow"
)]
async fn sync_with_api(
    state: &AppState,
    app: &Integration,
    remote: &impl AppApi,
    p: &Profile,
) -> Result<Value, ApiError> {
    let version = version(&app.provider).ok_or_else(super::unsupported)?;
    let cats = categories(&app.provider, p);
    let current = remote.arr(version, Method::GET, "indexer", None).await?;
    let current = current.as_array().ok_or_else(failure)?;
    let schemas = remote
        .arr(version, Method::GET, "indexer/schema", None)
        .await?;
    let schemas = schemas.as_array().ok_or_else(failure)?;
    let mappings = state.store.managed_indexers(&app.id).await?;
    let mut wanted = BTreeSet::new();
    let mut added = 0;
    let mut updated = 0;
    let mut deleted = 0;
    // Preflight all local definitions and schemas before performing any remote mutation.
    let mut desired = Vec::new();
    for indexer in state.store.list_indexers(&state.vault).await? {
        if !p.selects(&indexer) || cats.is_empty() {
            continue;
        }
        let def = parse_yaml(&indexer.definition_yaml)
            .map_err(|_| ApiError::bad_request("selected indexer definition is invalid"))?;
        let capabilities = indexloom_definitions::capabilities::summarize(&def);
        let root = match app.provider.as_str() {
            "sonarr" => "TV",
            "radarr" => "Movies",
            "lidarr" => "Audio",
            "readarr" => "Books",
            "whisparr" => "XXX",
            _ => "",
        };
        if !capabilities.categories.is_empty()
            && !capabilities
                .categories
                .iter()
                .any(|c| c.split('/').next() == Some(root))
        {
            continue;
        }
        let implementation = if def.spec.protocol == Protocol::Usenet {
            "Newznab"
        } else {
            "Torznab"
        };
        let schema = schemas
            .iter()
            .find(|s| s["implementation"] == implementation)
            .ok_or_else(|| {
                ApiError::bad_request(
                    "remote application does not support the selected feed protocol",
                )
            })?;
        let mut body = schema.clone();
        body.as_object_mut().ok_or_else(failure)?.remove("id");
        body["name"] = json!(marker(&app.id, &indexer.id));
        body["enableRss"] = json!(true);
        body["enableAutomaticSearch"] = json!(true);
        body["enableInteractiveSearch"] = json!(true);
        body["tags"] = json!(p.remote_tags);
        body["priority"] = json!(if p.priority == 0 { 25 } else { p.priority });
        let feed = super::origin(
            p.feed_origin
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("feedOrigin required"))?,
        )?;
        set_field(
            &mut body,
            "baseUrl",
            json!(feed.origin().ascii_serialization()),
        )?;
        set_field(
            &mut body,
            "apiPath",
            json!(format!(
                "/api/v1/{}/{}",
                implementation.to_ascii_lowercase(),
                indexer.id
            )),
        )?;
        set_field(&mut body, "apiKey", json!(remote.secret("feedApiKey")?))?;
        set_field(&mut body, "categories", json!(cats))?;
        wanted.insert(indexer.id.clone());
        desired.push((indexer.id, body));
    }
    // Reject a replaced/reused remote ID instead of adopting or deleting someone else's entry.
    for (local, id) in &mappings {
        if let Some(existing) = current.iter().find(|v| v["id"].as_i64() == Some(*id))
            && !owns(existing, &app.id, local)
        {
            return Err(ApiError::conflict(
                "managed remote identity changed; sync refused",
            ));
        }
    }
    for (local, mut body) in desired {
        let existing = mappings
            .get(&local)
            .and_then(|id| current.iter().find(|v| v["id"].as_i64() == Some(*id)));
        if let Some(existing) = existing {
            if p.mode == SyncMode::FullSync {
                let id = existing["id"].as_i64().ok_or_else(failure)?;
                // Keep remote advanced settings while replacing fields managed by this profile.
                let mut merged = existing.clone();
                for key in [
                    "name",
                    "enableRss",
                    "enableAutomaticSearch",
                    "enableInteractiveSearch",
                    "tags",
                    "priority",
                ] {
                    merged[key] = body[key].clone();
                }
                for name in ["baseUrl", "apiPath", "apiKey", "categories"] {
                    let value = body["fields"]
                        .as_array()
                        .and_then(|fields| fields.iter().find(|f| f["name"] == name))
                        .ok_or_else(failure)?["value"]
                        .clone();
                    set_field(&mut merged, name, value)?;
                }
                merged["id"] = json!(id);
                body = merged;
                remote
                    .arr(version, Method::PUT, &format!("indexer/{id}"), Some(&body))
                    .await?;
                updated += 1;
            }
        } else {
            // A prior POST may have succeeded before a crash. Never duplicate/adopt an unrecorded entry.
            if current.iter().any(|v| v["name"] == body["name"]) {
                return Err(ApiError::conflict(
                    "unmapped IndexLoom entry exists; resolve it before retrying",
                ));
            }
            let created = remote
                .arr(version, Method::POST, "indexer", Some(&body))
                .await?;
            let id = created["id"]
                .as_i64()
                .filter(|v| *v > 0)
                .ok_or_else(failure)?;
            state.store.set_managed_indexer(&app.id, &local, id).await?;
            added += 1;
        }
    }
    if p.mode == SyncMode::FullSync {
        for (local, id) in mappings {
            if wanted.contains(&local) {
                continue;
            }
            if current.iter().any(|v| v["id"].as_i64() == Some(id)) {
                remote
                    .arr(version, Method::DELETE, &format!("indexer/{id}"), None)
                    .await?;
                deleted += 1;
            }
            state.store.forget_managed_indexer(&app.id, &local).await?;
        }
    }
    Ok(json!({"added":added,"updated":updated,"deleted":deleted}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_and_categories() {
        assert!(owns(
            &json!({"name":"IndexLoom [app/local]","implementation":"Torznab"}),
            "app",
            "local"
        ));
        assert!(!owns(
            &json!({"name":"User entry","implementation":"Torznab"}),
            "app",
            "local"
        ));
        let p = Profile {
            categories: vec![2000, 2040, 5000, 7000],
            ..Profile::default()
        };
        assert_eq!(categories("sonarr", &p), vec![5000]);
        assert_eq!(categories("radarr", &p), vec![2000, 2040]);
        assert_eq!(categories("readarr", &p), vec![7000]);
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    struct Fixture {
        entries: tokio::sync::Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl AppApi for Fixture {
        fn secret(&self, _: &str) -> Result<&str, ApiError> {
            Ok("fixture-only-key")
        }
        async fn arr(
            &self,
            version: &str,
            method: Method,
            path: &str,
            body: Option<&Value>,
        ) -> Result<Value, ApiError> {
            assert!(matches!(version, "v1" | "v3"));
            let mut entries = self.entries.lock().await;
            if path == "indexer/schema" {
                return Ok(
                    json!([{"implementation":"Torznab","fields":[{"name":"baseUrl"},{"name":"apiPath"},{"name":"apiKey"},{"name":"categories"}]}]),
                );
            }
            match method {
                Method::GET => Ok(json!(*entries)),
                Method::POST => {
                    let mut item = body.unwrap().clone();
                    item["id"] = json!(
                        i64::try_from(entries.len())
                            .unwrap_or(i64::MAX)
                            .saturating_add(100)
                    );
                    entries.push(item.clone());
                    Ok(item)
                }
                Method::PUT => {
                    let item = body.unwrap();
                    let old = entries.iter_mut().find(|v| v["id"] == item["id"]).unwrap();
                    *old = item.clone();
                    Ok(item.clone())
                }
                Method::DELETE => {
                    let id: i64 = path.rsplit('/').next().unwrap().parse().unwrap();
                    entries.retain(|v| v["id"] != id);
                    Ok(Value::Null)
                }
                _ => panic!("unexpected method"),
            }
        }
    }
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "End-to-end sync scenario intentionally tests every mode"
    )]
    async fn sync_preserves_unmanaged_and_respects_modes_without_sockets() {
        let store = indexloom_storage::Store::connect("sqlite::memory:")
            .await
            .unwrap();
        store
            .upsert_definition(
                "example",
                "test",
                None,
                "indexloom.io/v1alpha1",
                crate::tests::DEFINITION,
                &"0".repeat(64),
            )
            .await
            .unwrap();
        store
            .create_indexer("local", "example", "Local", true)
            .await
            .unwrap();
        let state = AppState::new(
            store.clone(),
            indexloom_storage::SecretVault::generate().unwrap(),
            crate::AdminToken::new("fixture-admin-token-at-least-32-bytes").unwrap(),
        );
        for provider in ["sonarr", "radarr", "lidarr", "readarr", "whisparr"] {
            let app = Integration {
                id: provider.into(),
                provider: provider.into(),
                name: provider.into(),
                origin: "http://127.0.0.1:8989".into(),
                enabled: true,
            };
            store
                .upsert_integration(&app, None, &state.vault)
                .await
                .unwrap();
            let remote = Fixture {
                entries: tokio::sync::Mutex::new(vec![
                    json!({"id":1,"name":"Unmanaged","implementation":"Torznab"}),
                ]),
            };
            let mut p = Profile {
                mode: SyncMode::AddOnly,
                feed_origin: Some("http://localhost:3000".into()),
                ..Profile::default()
            };
            assert_eq!(
                sync_with_api(&state, &app, &remote, &p).await.unwrap()["added"],
                1
            );
            assert_eq!(
                sync_with_api(&state, &app, &remote, &p).await.unwrap()["updated"],
                0
            );
            let mappings = store.managed_indexers(provider).await.unwrap();
            let id = mappings["local"];
            let get_field = |v: &Value, name: &str| {
                v["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|f| f["name"] == name)
                    .unwrap()["value"]
                    .clone()
            };
            {
                let mut entries = remote.entries.lock().await;
                let owned = entries.iter_mut().find(|v| v["id"] == id).unwrap();
                assert_eq!(get_field(owned, "categories"), json!([family(provider)]));
                assert_eq!(get_field(owned, "apiPath"), json!("/api/v1/torznab/local"));
                owned["customRemoteSetting"] = json!("preserve-me");
                owned["fields"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name":"additionalParameters","value":"preserve-this-too"}));
                owned["name"] = json!("Reused ID");
            }
            assert!(sync_with_api(&state, &app, &remote, &p).await.is_err());
            remote
                .entries
                .lock()
                .await
                .iter_mut()
                .find(|v| v["id"] == id)
                .unwrap()["name"] = json!(marker(provider, "local"));
            p.mode = SyncMode::FullSync;
            p.priority = 7;
            assert_eq!(
                sync_with_api(&state, &app, &remote, &p).await.unwrap()["updated"],
                1
            );
            assert_eq!(
                remote
                    .entries
                    .lock()
                    .await
                    .iter()
                    .find(|v| v["id"] == id)
                    .unwrap()["priority"],
                7
            );
            {
                let entries = remote.entries.lock().await;
                let owned = entries.iter().find(|v| v["id"] == id).unwrap();
                assert_eq!(owned["customRemoteSetting"], "preserve-me");
                assert_eq!(
                    get_field(owned, "additionalParameters"),
                    json!("preserve-this-too")
                );
            }
            p.indexer_ids = vec!["excluded".into()];
            p.mode = SyncMode::AddOnly;
            assert_eq!(
                sync_with_api(&state, &app, &remote, &p).await.unwrap()["deleted"],
                0
            );
            p.mode = SyncMode::FullSync;
            assert_eq!(
                sync_with_api(&state, &app, &remote, &p).await.unwrap()["deleted"],
                1
            );
            assert_eq!(
                remote.entries.lock().await.as_slice(),
                &[json!({"id":1,"name":"Unmanaged","implementation":"Torznab"})]
            );
        }
    }
}
