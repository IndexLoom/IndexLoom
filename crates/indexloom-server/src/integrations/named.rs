//! Ownership-fenced synchronization for command-based named providers.
use super::{
    ApiError, AppState, Integration, Remote,
    profile::{Profile, SyncMode},
    transport::failure,
};
use indexloom_definitions::{Protocol, parse_yaml};
use indexloom_storage::integrations::ManagedProviderName;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct Entry {
    kind: String,
    name: String,
    id: Option<i64>,
    host: String,
    key: String,
    categories: String,
    enabled: bool,
}
pub(super) fn entries(data: &Value, lazy: bool) -> Result<Vec<Entry>, ApiError> {
    let mut result = Vec::new();
    for (kind, field) in [
        ("torznab", if lazy { "Torznabs" } else { "torznabs" }),
        ("newznab", if lazy { "Newznabs" } else { "newznabs" }),
    ] {
        for item in data[field].as_array().ok_or_else(failure)? {
            let text = |field: &str| item[field].as_str().map(str::to_owned).ok_or_else(failure);
            let name = if lazy {
                text("Dispname")?
            } else {
                text("name")?
            };
            // Empty hosts are cleared slots returned after LazyLibrarian deletion.
            let host = text(if lazy { "Host" } else { "host" })?;
            if lazy && host.is_empty() {
                continue;
            }
            let id = if lazy {
                None
            } else {
                Some(item["id"].as_i64().filter(|v| *v > 0).ok_or_else(failure)?)
            };
            if result
                .iter()
                .any(|e: &Entry| e.name == name || (id.is_some() && e.kind == kind && e.id == id))
            {
                return Err(failure());
            }
            let enabled = if lazy {
                match &item["Enabled"] {
                    Value::Bool(value) => *value,
                    value if value.as_i64() == Some(1) => true,
                    value if value.as_i64() == Some(0) => false,
                    _ => return Err(failure()),
                }
            } else {
                true
            };
            result.push(Entry {
                kind: kind.into(),
                name,
                id,
                host,
                key: text(if lazy { "Apikey" } else { "apikey" })?,
                categories: text(if lazy { "Categories" } else { "categories" })?,
                enabled,
            });
        }
    }
    Ok(result)
}
#[async_trait::async_trait]
pub(super) trait AppApi: Sync {
    async fn command(&self, command: &str, params: &[(String, String)]) -> Result<Value, ApiError>;
    fn feed_key(&self) -> Result<&str, ApiError>;
}
#[async_trait::async_trait]
impl AppApi for Remote {
    async fn command(&self, command: &str, params: &[(String, String)]) -> Result<Value, ApiError> {
        self.send(
            self.request(reqwest::Method::GET, "/api")?
                .query(&[("cmd", command), ("apikey", self.secret("apiKey")?)])
                .query(params),
        )
        .await
    }
    fn feed_key(&self) -> Result<&str, ApiError> {
        self.secret("feedApiKey")
    }
}
fn lazy(app: &Integration) -> bool {
    app.provider == "lazylibrarian"
}
async fn list(app: &Integration, api: &impl AppApi) -> Result<Vec<Entry>, ApiError> {
    let value = api
        .command(
            if lazy(app) {
                "listNabProviders"
            } else {
                "listProviders"
            },
            &[],
        )
        .await?;
    if lazy(app) {
        super::lazylibrarian::parse(&value)
    } else {
        super::mylar::parse(&value)
    }
}
pub(super) async fn test(app: &Integration, api: &impl AppApi) -> Result<(), ApiError> {
    list(app, api).await.map(|_| ())
}
fn marker(app: &Integration, local: &str) -> String {
    format!("IndexLoom [{}/{local}]", app.id)
}
fn conflict() -> ApiError {
    ApiError::conflict("named provider ownership mismatch or orphan; sync refused")
}
fn owned<'a>(
    app: &Integration,
    m: &ManagedProviderName,
    current: &'a [Entry],
) -> Result<Option<&'a Entry>, ApiError> {
    if m.remote_name != marker(app, &m.local_indexer_id) || (!lazy(app) && m.remote_id.is_none()) {
        return Err(conflict());
    }
    let candidate = current.iter().find(|e| {
        e.name == m.remote_name
            || (m.remote_id.is_some() && e.kind == m.provider_type && e.id == m.remote_id)
    });
    if candidate.is_some_and(|e| {
        e.name != m.remote_name || e.kind != m.provider_type || e.id != m.remote_id
    }) {
        return Err(conflict());
    }
    Ok(candidate)
}
async fn mutate(
    app: &Integration,
    api: &impl AppApi,
    cmd: &str,
    params: &[(String, String)],
) -> Result<(), ApiError> {
    let value = api.command(cmd, params).await?;
    if value[if lazy(app) { "Success" } else { "success" }] != true {
        return Err(failure());
    }
    Ok(())
}
fn categories(app: &Integration, p: &Profile) -> Vec<u32> {
    if p.categories.is_empty() {
        return vec![if lazy(app) { 7000 } else { 7030 }];
    }
    p.categories
        .iter()
        .copied()
        .filter(|c| {
            if lazy(app) {
                c / 1000 == 7 || c / 1000 == 3
            } else {
                *c == 7000 || *c == 7030
            }
        })
        .collect()
}
#[allow(
    clippy::too_many_lines,
    reason = "Preflight ownership precedes ordered remote mutations and durable mapping writes"
)]
pub(super) async fn sync(
    state: &AppState,
    app: &Integration,
    api: &impl AppApi,
    p: &Profile,
) -> Result<Value, ApiError> {
    if !app.enabled || p.mode == SyncMode::Disabled {
        return Ok(json!({"skipped":true,"reason":"disabled"}));
    }
    let mut current = list(app, api).await?;
    let mappings = state.store.managed_provider_names(&app.id).await?;
    for m in &mappings {
        // A missing display name may have been renamed by a user. Refuse to
        // recreate or forget it automatically, especially without numeric IDs.
        if owned(app, m, &current)?.is_none() {
            return Err(conflict());
        }
    }
    // Refuse every orphan for this integration, including deselected entries.
    let prefix = format!("IndexLoom [{}/", app.id);
    if current
        .iter()
        .any(|e| e.name.starts_with(&prefix) && !mappings.iter().any(|m| m.remote_name == e.name))
    {
        return Err(conflict());
    }
    let cats = categories(app, p);
    let mut desired = Vec::new();
    let mut wanted = BTreeSet::new();
    for indexer in state.store.list_indexers(&state.vault).await? {
        if !p.selects(&indexer) || cats.is_empty() {
            continue;
        }
        let def = parse_yaml(&indexer.definition_yaml)
            .map_err(|_| ApiError::bad_request("selected indexer definition is invalid"))?;
        let capabilities = indexloom_definitions::capabilities::summarize(&def);
        if !capabilities.categories.is_empty()
            && !capabilities.categories.iter().any(|c| {
                c.split('/').next().is_some_and(|root| {
                    (root == "Books" && cats.iter().any(|c| c / 1000 == 7))
                        || (lazy(app) && root == "Audio" && cats.iter().any(|c| c / 1000 == 3))
                })
            })
        {
            continue;
        }
        let kind = if def.spec.protocol == Protocol::Usenet {
            "newznab"
        } else {
            "torznab"
        };
        if mappings
            .iter()
            .any(|m| m.local_indexer_id == indexer.id && m.provider_type != kind)
        {
            return Err(conflict());
        }
        let feed = super::origin(
            p.feed_origin
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("feedOrigin required"))?,
        )?;
        let host = format!(
            "{}/api/v1/{kind}/{}",
            feed.origin().ascii_serialization(),
            indexer.id
        );
        wanted.insert(indexer.id.clone());
        desired.push((indexer.id, kind, host));
    }
    let key = api.feed_key()?;
    let csv = cats
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let (mut added, mut updated, mut deleted) = (0, 0, 0);
    for (local, kind, host) in desired {
        let mapping = mappings.iter().find(|m| m.local_indexer_id == local);
        let existing = mapping
            .map(|m| owned(app, m, &current))
            .transpose()?
            .flatten();
        if existing.is_some() && p.mode == SyncMode::AddOnly {
            continue;
        }
        let name = marker(app, &local);
        let mut params = vec![
            ("providertype".into(), kind.into()),
            ("name".into(), name.clone()),
            ("host".into(), host.clone()),
            ("prov_apikey".into(), key.into()),
            ("categories".into(), csv.clone()),
            ("enabled".into(), "true".into()),
        ];
        if let Some(id) = existing.and_then(|e| e.id) {
            params.push(("prov_id".into(), id.to_string()));
        }
        if lazy(app) {
            params.push(("type".into(), kind.into()));
        }
        let updating = existing.is_some();
        mutate(
            app,
            api,
            if updating {
                "changeProvider"
            } else {
                "addProvider"
            },
            &params,
        )
        .await?;
        current = list(app, api).await?;
        let created = current
            .iter()
            .find(|e| e.name == name && e.kind == kind)
            .ok_or_else(failure)?;
        if created.host != host
            || created.key != key
            || created.categories != csv
            || !created.enabled
            || (updating && mapping.is_some_and(|m| m.remote_id != created.id))
        {
            return Err(failure());
        }
        state
            .store
            .set_managed_provider_name(
                &app.id,
                &ManagedProviderName {
                    local_indexer_id: local,
                    provider_type: kind.into(),
                    remote_name: name,
                    remote_id: created.id,
                },
            )
            .await?;
        if updating {
            updated += 1;
        } else {
            added += 1;
        }
    }
    if p.mode == SyncMode::FullSync {
        for m in &mappings {
            if wanted.contains(&m.local_indexer_id) {
                continue;
            }
            if owned(app, m, &current)?.is_some() {
                let mut params = vec![
                    ("providertype".into(), m.provider_type.clone()),
                    ("name".into(), m.remote_name.clone()),
                ];
                if let Some(id) = m.remote_id {
                    params.push(("prov_id".into(), id.to_string()));
                }
                mutate(app, api, "delProvider", &params).await?;
                current = list(app, api).await?;
                if owned(app, m, &current)?.is_some() {
                    return Err(failure());
                }
                deleted += 1;
            }
            state
                .store
                .forget_managed_provider_name(&app.id, &m.local_indexer_id)
                .await?;
        }
    }
    Ok(json!({"added":added,"updated":updated,"deleted":deleted}))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        lazy: bool,
        entries: tokio::sync::Mutex<Vec<Value>>,
        fail: std::sync::atomic::AtomicBool,
        ignore: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl AppApi for Fixture {
        fn feed_key(&self) -> Result<&str, ApiError> {
            Ok("feed-only")
        }
        async fn command(&self, cmd: &str, params: &[(String, String)]) -> Result<Value, ApiError> {
            use std::sync::atomic::Ordering::Relaxed;
            if self.fail.load(Relaxed) {
                return Ok(json!({"unexpected":true}));
            }
            let mut entries = self.entries.lock().await;
            let get = |name: &str| {
                params
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.as_str())
                    .unwrap()
            };
            let field = if self.lazy { "Dispname" } else { "name" };
            if !cmd.starts_with("list") {
                if !self.ignore.load(Relaxed) {
                    let name = get("name");
                    if cmd == "delProvider" {
                        if self.lazy {
                            entries.iter_mut().find(|v| v[field] == name).unwrap()["Host"] =
                                json!("");
                        } else {
                            entries.retain(|v| v[field] != name);
                        }
                    } else {
                        let kind = get("providertype");
                        let id = entries
                            .iter()
                            .find(|v| v[field] == name)
                            .map_or(42, |v| v["id"].as_i64().unwrap_or(42));
                        let value = if self.lazy {
                            json!({"Name":"Torznab_9","Dispname":name,"Host":get("host"),"Apikey":get("prov_apikey"),"Categories":get("categories"),"Enabled":1,"kind":kind})
                        } else {
                            json!({"id":id,"name":name,"host":get("host"),"apikey":get("prov_apikey"),"categories":get("categories"),"kind":kind})
                        };
                        if cmd == "changeProvider" {
                            *entries.iter_mut().find(|v| v[field] == name).unwrap() = value;
                        } else {
                            entries.push(value);
                        }
                    }
                }
                return Ok(if self.lazy {
                    json!({"Success":true})
                } else {
                    json!({"success":true})
                });
            }
            let tor: Vec<_> = entries
                .iter()
                .filter(|v| v["kind"] == "torznab")
                .cloned()
                .collect();
            let new: Vec<_> = entries
                .iter()
                .filter(|v| v["kind"] == "newznab")
                .cloned()
                .collect();
            Ok(if self.lazy {
                json!({"Success":true,"Data":{"Torznabs":tor,"Newznabs":new}})
            } else {
                json!({"success":true,"data":{"torznabs":tor,"newznabs":new}})
            })
        }
    }
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "Shared fixture verifies both authoritative dialects through every sync mode"
    )]
    async fn both_apps_ownership_modes_and_verification() {
        use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
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
        for provider in ["mylar", "lazylibrarian"] {
            let app = Integration {
                id: provider.into(),
                provider: provider.into(),
                name: provider.into(),
                origin: "http://localhost:8080".into(),
                enabled: true,
            };
            store
                .upsert_integration(&app, None, &state.vault)
                .await
                .unwrap();
            let api = Fixture {
                lazy: lazy(&app),
                entries: tokio::sync::Mutex::new(vec![]),
                fail: AtomicBool::new(false),
                ignore: AtomicBool::new(false),
            };
            let mut p = Profile {
                mode: SyncMode::Disabled,
                feed_origin: Some("http://localhost:3000".into()),
                ..Profile::default()
            };
            api.fail.store(true, Relaxed);
            assert_eq!(sync(&state, &app, &api, &p).await.unwrap()["skipped"], true);
            assert!(test(&app, &api).await.is_err());
            api.fail.store(false, Relaxed);
            test(&app, &api).await.unwrap();
            p.mode = SyncMode::AddOnly;
            api.ignore.store(true, Relaxed);
            assert!(sync(&state, &app, &api, &p).await.is_err());
            assert!(
                store
                    .managed_provider_names(provider)
                    .await
                    .unwrap()
                    .is_empty()
            );
            api.ignore.store(false, Relaxed);
            assert_eq!(sync(&state, &app, &api, &p).await.unwrap()["added"], 1);
            assert_eq!(sync(&state, &app, &api, &p).await.unwrap()["updated"], 0);
            let mapping = store
                .managed_provider_names(provider)
                .await
                .unwrap()
                .remove(0);
            assert_eq!(mapping.remote_name, marker(&app, "local"));
            assert_eq!(mapping.remote_id, if lazy(&app) { None } else { Some(42) });
            let name_field = if lazy(&app) { "Dispname" } else { "name" };
            api.entries.lock().await[0][name_field] = json!("User renamed");
            assert!(sync(&state, &app, &api, &p).await.is_err());
            api.entries.lock().await[0][name_field] = json!(mapping.remote_name);
            api.entries.lock().await[0]["kind"] = json!("newznab");
            assert!(sync(&state, &app, &api, &p).await.is_err());
            api.entries.lock().await[0]["kind"] = json!("torznab");
            p.mode = SyncMode::FullSync;
            assert_eq!(sync(&state, &app, &api, &p).await.unwrap()["updated"], 1);
            let parsed = list(&app, &api).await.unwrap();
            assert_eq!(parsed[0].host, "http://localhost:3000/api/v1/torznab/local");
            assert_eq!(parsed[0].key, "feed-only");
            assert_eq!(
                parsed[0].categories,
                if lazy(&app) { "7000" } else { "7030" }
            );
            let unmanaged = if lazy(&app) {
                json!({"Name":"Newznab_1","Dispname":"User provider","Host":"https://example.invalid","Apikey":"user-key","Categories":"7000","Enabled":1,"kind":"newznab"})
            } else {
                json!({"id":99,"name":"User provider","host":"https://example.invalid","apikey":"user-key","categories":"7030","kind":"newznab"})
            };
            api.entries.lock().await.push(unmanaged.clone());
            store
                .forget_managed_provider_name(provider, "local")
                .await
                .unwrap();
            assert!(sync(&state, &app, &api, &p).await.is_err());
            store
                .set_managed_provider_name(provider, &mapping)
                .await
                .unwrap();
            p.indexer_ids = vec!["excluded".into()];
            p.mode = SyncMode::AddOnly;
            assert_eq!(sync(&state, &app, &api, &p).await.unwrap()["deleted"], 0);
            p.mode = SyncMode::FullSync;
            api.ignore.store(true, Relaxed);
            assert!(sync(&state, &app, &api, &p).await.is_err());
            assert_eq!(
                store.managed_provider_names(provider).await.unwrap().len(),
                1
            );
            api.ignore.store(false, Relaxed);
            assert_eq!(sync(&state, &app, &api, &p).await.unwrap()["deleted"], 1);
            assert!(
                store
                    .managed_provider_names(provider)
                    .await
                    .unwrap()
                    .is_empty()
            );
            let remaining = list(&app, &api).await.unwrap();
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].name, "User provider");
            assert_eq!(api.entries.lock().await.last().unwrap(), &unmanaged);
            p.categories = vec![3000, 7000, 7030, 2000];
            assert_eq!(
                categories(&app, &p),
                if lazy(&app) {
                    vec![3000, 7000, 7030]
                } else {
                    vec![7000, 7030]
                }
            );
        }
    }
    #[tokio::test]
    async fn named_command_http_query_fixture() {
        use std::{
            collections::BTreeMap,
            sync::{Arc, atomic::AtomicBool},
        };
        async fn handle(
            axum::extract::State(api): axum::extract::State<Arc<Fixture>>,
            axum::extract::Query(mut query): axum::extract::Query<BTreeMap<String, String>>,
        ) -> axum::Json<Value> {
            assert_eq!(query.remove("apikey").as_deref(), Some("app &+key"));
            let cmd = query.remove("cmd").unwrap();
            axum::Json(
                api.command(&cmd, &query.into_iter().collect::<Vec<_>>())
                    .await
                    .unwrap(),
            )
        }
        for provider in ["mylar", "lazylibrarian"] {
            let api = Arc::new(Fixture {
                lazy: provider == "lazylibrarian",
                entries: tokio::sync::Mutex::new(vec![]),
                fail: AtomicBool::new(false),
                ignore: AtomicBool::new(false),
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let routes = axum::Router::new()
                .route("/api", axum::routing::get(handle))
                .with_state(api);
            let server = tokio::spawn(async move {
                axum::serve(listener, routes).await.unwrap();
            });
            let app = Integration {
                id: provider.into(),
                provider: provider.into(),
                name: provider.into(),
                origin: format!("http://{endpoint}"),
                enabled: true,
            };
            let remote = Remote::new(
                &app,
                indexloom_storage::integrations::IntegrationSecrets(BTreeMap::from([
                    ("apiKey".into(), "app &+key".into()),
                    ("feedApiKey".into(), "feed &+key".into()),
                ])),
            )
            .await
            .unwrap();
            test(&app, &remote).await.unwrap();
            let name = marker(&app, "local");
            mutate(
                &app,
                &remote,
                "addProvider",
                &[
                    ("providertype".into(), "newznab".into()),
                    ("type".into(), "newznab".into()),
                    ("name".into(), name.clone()),
                    (
                        "host".into(),
                        "http://localhost:3000/api/v1/newznab/local".into(),
                    ),
                    ("prov_apikey".into(), remote.feed_key().unwrap().into()),
                    ("categories".into(), "7000".into()),
                    ("enabled".into(), "true".into()),
                ],
            )
            .await
            .unwrap();
            let entries = list(&app, &remote).await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, name);
            assert_eq!(entries[0].key, "feed &+key");
            server.abort();
        }
    }
    #[test]
    fn strict_dialects_and_both_protocols() {
        assert!(super::super::mylar::parse(&json!({"success":true,"data":{}})).is_err());
        assert!(super::super::lazylibrarian::parse(&json!({"Success":true,"Data":{}})).is_err());
        let m = json!({"id":7,"name":"IndexLoom [a/b]","host":"http://localhost/feed","apikey":"fixture","categories":"7030"});
        let v = json!({"success":true,"data":{"torznabs":[],"newznabs":[m]}});
        assert_eq!(super::super::mylar::parse(&v).unwrap()[0].kind, "newznab");
        assert!(super::super::lazylibrarian::parse(&v).is_err());
        let l = json!({"Name":"Newznab_1","Dispname":"IndexLoom [a/b]","Host":"http://localhost/feed","Apikey":"fixture","Categories":"3000","Enabled":1});
        let v = json!({"Success":true,"Data":{"Torznabs":[],"Newznabs":[l]}});
        let entries = super::super::lazylibrarian::parse(&v).unwrap();
        assert_eq!(entries[0].kind, "newznab");
        assert_eq!(entries[0].name, "IndexLoom [a/b]");
    }
}
