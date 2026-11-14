//! Real reqwest + SQLite tests via a loopback HTTP proxy. The tracker hostname
//! is synthetic; no DNS lookup or external tracker request is made.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use indexloom_storage::{OutboundProxyInput, ProxyKind, ProxyPasswordUpdate, SecretVault, Store};
use serde_json::json;
use tower::ServiceExt;

use super::super::build_executor;
use crate::{AdminToken, AppState};

#[derive(Default)]
struct MockState {
    logins: BTreeMap<String, usize>,
    sessions: BTreeMap<String, String>,
    rotations: usize,
    reject_next_search: bool,
    search_cookies: Vec<String>,
}

fn response(status: StatusCode, body: String, cookies: &[String]) -> Response {
    let mut response = (status, body).into_response();
    for cookie in cookies {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie.parse().unwrap());
    }
    response
}

async fn mock_tracker(State(state): State<Arc<Mutex<MockState>>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let cookie = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = to_bytes(request.into_body(), 4096).await.unwrap();
    let fields = url::form_urlencoded::parse(&body)
        .into_owned()
        .collect::<BTreeMap<_, _>>();
    let mut state = state.lock().unwrap();
    if path == "/login" && method == "GET" {
        return response(StatusCode::OK,
            "<form action='/login'><input name='csrf' value='fixture-csrf'><input name='username'><input name='password'></form>".into(),
            &["csrf=fixture-csrf; Path=/; HttpOnly; Max-Age=3600".into(),
              "restricted=fixture-private; Path=/login; Max-Age=3600".into(),
              "foreign=fixture-bad; Domain=other.test; Path=/".into()]);
    }
    if path == "/login" && method == "POST" {
        if fields.get("csrf").map(String::as_str) != Some("fixture-csrf")
            || !cookie.contains("csrf=fixture-csrf")
            || !matches!(
                fields.get("password").map(String::as_str),
                Some("fixture-password" | "fixture-new-password")
            )
        {
            return response(
                StatusCode::FORBIDDEN,
                "invalid fixture credentials/CSRF".into(),
                &[],
            );
        }
        let user = fields["username"].clone();
        let logins = state.logins.entry(user.clone()).or_default();
        *logins += 1;
        let sid = format!("fixture-{user}-login-{logins}");
        state.sessions.insert(sid.clone(), user);
        return response(
            StatusCode::OK,
            "logged in".into(),
            &[
                format!("sid={sid}; Path=/; HttpOnly; SameSite=Strict; Max-Age=3600"),
                "csrf=deleted; Path=/; Max-Age=0".into(),
            ],
        );
    }
    let sid = cookie
        .split(';')
        .find_map(|part| part.trim().strip_prefix("sid="))
        .unwrap_or("")
        .to_owned();
    let Some(user) = state.sessions.get(&sid).cloned() else {
        // A 200 login page is a common expired-session response.
        return response(
            StatusCode::OK,
            "<form>please log in</form>".into(),
            &["sid=deleted; Path=/; Max-Age=0".into()],
        );
    };
    if path == "/account" {
        return response(
            StatusCode::OK,
            "<div class='authenticated'>yes</div>".into(),
            &[],
        );
    }
    if path == "/search" {
        state.search_cookies.push(cookie);
        if state.reject_next_search {
            state.reject_next_search = false;
            state.sessions.remove(&sid);
            return response(
                StatusCode::UNAUTHORIZED,
                "session rejected".into(),
                &["sid=deleted; Path=/; Max-Age=0".into()],
            );
        }
        state.sessions.remove(&sid);
        state.rotations += 1;
        let rotated = format!("fixture-{user}-rotation-{}", state.rotations);
        state.sessions.insert(rotated.clone(), user.clone());
        return response(
            StatusCode::OK,
            format!("<div class='result'><a class='title' href='/details'>{user}</a></div>"),
            &[format!("sid={rotated}; Path=/; HttpOnly; Max-Age=3600")],
        );
    }
    response(StatusCode::NOT_FOUND, "unexpected mock path".into(), &[])
}

async fn mock() -> (u16, Arc<Mutex<MockState>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(Mutex::new(MockState::default()));
    let app = Router::new()
        .fallback(any(mock_tracker))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, state, task)
}

async fn configure(store: &Store, vault: &SecretVault, port: u16) {
    let definition = crate::tests::DEFINITION
        .replace(
            "baseUrls: [https://example.invalid/]",
            "baseUrls: [http://tracker.test/]",
        )
        .replace(
            "    search: { paths: [{ path: /search }], rows: { selector: .result }, fields: {} }",
            r"
    authentication:
      method: form
      path: /login
      inputs: { username: '{{ .Config.username }}', password: '{{ .Config.password }}' }
      test: { path: /account, selector: .authenticated }
    search:
      paths: [{ path: /search }]
      rows: { selector: .result }
      fields:
        title: { selector: .title }
        details: { selector: .title, attribute: href }
",
        );
    store
        .upsert_definition(
            "example",
            "test",
            None,
            "indexloom.io/v1alpha1",
            &definition,
            &"0".repeat(64),
        )
        .await
        .unwrap();
    store
        .upsert_proxy(
            &OutboundProxyInput {
                id: "mock".into(),
                name: "Mock".into(),
                kind: ProxyKind::Http,
                host: "127.0.0.1".into(),
                port,
                enabled: true,
                username: None,
            },
            ProxyPasswordUpdate::Clear,
            vault,
        )
        .await
        .unwrap();
    store.set_global_proxy_id(Some("mock")).await.unwrap();
    for user in ["alice", "bob"] {
        store
            .create_indexer(user, "example", user, true)
            .await
            .unwrap();
        store
            .save_indexer_configuration(
                user,
                None,
                None,
                &BTreeMap::from([("username".into(), json!(user))]),
                &BTreeMap::from([("password".into(), "fixture-password".into())]),
                vault,
            )
            .await
            .unwrap();
    }
}

fn state(store: Store, vault: SecretVault) -> AppState {
    AppState::new(
        store,
        vault,
        AdminToken::new("fixture-admin-token-at-least-32-bytes").unwrap(),
    )
}

async fn search(state: &AppState, user: &str) {
    let handle = build_executor(state, user).await.unwrap();
    let batch = handle.executor.search("fixture", 10).await.unwrap();
    assert_eq!(batch.results.len(), 1);
    assert_eq!(batch.results[0].title, user);
}

#[tokio::test]
async fn persists_rotated_sessions_across_restart_and_isolates_same_tracker_instances() {
    let (port, mock_state, task) = mock().await;
    let path = std::env::temp_dir().join(format!("indexloom-runtime-{}.db", uuid::Uuid::now_v7()));
    let vault = SecretVault::generate().unwrap();
    let store = Store::connect_path(&path).await.unwrap();
    configure(&store, &vault, port).await;
    let app = state(store.clone(), vault.clone());
    let ((), ()) = tokio::join!(search(&app, "alice"), search(&app, "bob"));
    // Read through the management GET API, checking that runtime cookies never
    // become part of redacted configuration or response payloads.
    let response = crate::router(app.clone(), "missing-web-root".into())
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/indexers")
                .header(
                    header::AUTHORIZATION,
                    "Bearer fixture-admin-token-at-least-32-bytes",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("fixture-password"));
    assert!(!text.contains("fixture-alice-rotation"));
    drop(app);
    drop(store);
    // A newly opened DB, AppState, client and jar, using the same master key.
    let reopened = Store::connect_path(&path).await.unwrap();
    let app = state(reopened, vault);
    search(&app, "alice").await;
    search(&app, "bob").await;
    {
        let observed = mock_state.lock().unwrap();
        assert_eq!(observed.logins["alice"], 1);
        assert_eq!(observed.logins["bob"], 1);
        assert_eq!(observed.rotations, 4);
        assert!(
            observed
                .search_cookies
                .iter()
                .all(|cookies| !cookies.contains("csrf=")
                    && !cookies.contains("restricted=")
                    && !cookies.contains("foreign="))
        );
    }
    drop(app);
    task.abort();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn rejects_expired_remote_sessions_reauthenticates_and_invalidates_credential_changes() {
    let (port, mock_state, task) = mock().await;
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    configure(&store, &vault, port).await;
    let app = state(store.clone(), vault.clone());
    search(&app, "alice").await;
    // Force a new executor, as on restart; a 200 login form fails the auth test.
    mock_state.lock().unwrap().sessions.clear();
    app.clear_execution_runtimes().await;
    search(&app, "alice").await;
    assert_eq!(mock_state.lock().unwrap().logins["alice"], 2);
    // A server-side rejection of a warm session retries once with a fresh login.
    mock_state.lock().unwrap().reject_next_search = true;
    search(&app, "alice").await;
    assert_eq!(mock_state.lock().unwrap().logins["alice"], 3);
    let old = build_executor(&app, "alice").await.unwrap();
    store
        .save_indexer_configuration(
            "alice",
            None,
            None,
            &BTreeMap::from([("username".into(), json!("alice"))]),
            &BTreeMap::from([("password".into(), "fixture-new-password".into())]),
            &vault,
        )
        .await
        .unwrap();
    // An in-flight executor holding old credentials cannot save its snapshot.
    assert!(matches!(
        old.executor.search("fixture", 10).await,
        Err(indexloom_engine::EngineError::SessionPersistence)
    ));
    search(&app, "alice").await;
    assert_eq!(mock_state.lock().unwrap().logins["alice"], 4);
    task.abort();
}

#[tokio::test]
async fn test_endpoint_authentication_is_durable_without_a_shutdown_flush() {
    let (port, mock_state, task) = mock().await;
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    configure(&store, &vault, port).await;
    let app = state(store.clone(), vault.clone());
    let executor = build_executor(&app, "alice").await.unwrap().executor;
    assert!(executor.test().await.success);
    // A separate application state must restore the immediately saved login.
    let restarted = state(store, vault);
    search(&restarted, "alice").await;
    assert_eq!(mock_state.lock().unwrap().logins["alice"], 1);
    task.abort();
}

#[tokio::test]
async fn fixture_builds_revision_bound_isolated_executors_without_network() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    configure(&store, &vault, 1).await;
    let app = state(store, vault);
    let alice = build_executor(&app, "alice").await.unwrap().executor;
    let again = build_executor(&app, "alice").await.unwrap().executor;
    let bob = build_executor(&app, "bob").await.unwrap().executor;
    assert!(Arc::ptr_eq(&alice, &again));
    assert!(!Arc::ptr_eq(&alice, &bob));
    assert!(!format!("{alice:?}").contains("fixture-password"));
}

#[tokio::test]
async fn credential_changes_fence_server_backends_and_replace_cached_executors_without_network() {
    use indexloom_engine::{CookieSnapshot, EngineError, SessionPersistence};

    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    configure(&store, &vault, 1).await;
    let app = state(store.clone(), vault.clone());
    let old_executor = build_executor(&app, "alice").await.unwrap().executor;
    let backend = |revision| crate::tracker_sessions::StoredTrackerSession {
        store: store.clone(),
        vault: vault.clone(),
        indexer_id: "alice".into(),
        revision,
    };
    let snapshot = |value: &str| CookieSnapshot {
        cookies_json: value.into(),
        expires_at: time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
    };
    let revision = store
        .resolve_indexer_configuration("alice", &vault)
        .await
        .unwrap()
        .session_revision;
    let old = backend(revision);
    old.save(snapshot("fixture-old-session")).await.unwrap();
    store
        .save_indexer_configuration(
            "alice",
            None,
            None,
            &BTreeMap::from([("username".into(), json!("alice"))]),
            &BTreeMap::from([("password".into(), "fixture-new-password".into())]),
            &vault,
        )
        .await
        .unwrap();
    assert!(old.load().await.unwrap().is_none());
    let current = backend(revision + 1);
    assert!(current.load().await.unwrap().is_none());
    current
        .save(snapshot("fixture-current-session"))
        .await
        .unwrap();
    assert!(matches!(
        old.save(snapshot("fixture-stale-session")).await,
        Err(EngineError::SessionPersistence)
    ));
    old.clear().await.unwrap();
    assert_eq!(
        current.load().await.unwrap().unwrap().cookies_json,
        "fixture-current-session"
    );
    let new_executor = build_executor(&app, "alice").await.unwrap().executor;
    assert!(!Arc::ptr_eq(&old_executor, &new_executor));
    assert!(!format!("{new_executor:?}").contains("fixture-new-password"));
}

#[tokio::test]
async fn mock_fixture_checks_csrf_and_rotates_sessions_without_sockets() {
    let observed = Arc::new(Mutex::new(MockState::default()));
    let app = Router::new()
        .fallback(any(mock_tracker))
        .with_state(observed.clone());
    let request = |cookie: &str, body: &str| {
        axum::http::Request::builder()
            .method("POST")
            .uri("http://tracker.test/login")
            .header(header::COOKIE, cookie)
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let body = "username=alice&password=fixture-password&csrf=fixture-csrf";
    assert_eq!(
        app.clone()
            .oneshot(request("", body))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let login = app
        .clone()
        .oneshot(request("csrf=fixture-csrf", body))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let sid = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let search = app
        .oneshot(
            axum::http::Request::builder()
                .uri("http://tracker.test/search")
                .header(header::COOKIE, sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);
    assert!(
        search.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("rotation")
    );
    assert_eq!(observed.lock().unwrap().logins["alice"], 1);
}

#[tokio::test]
async fn stage_d_monitoring_real_search_rss_and_test() {
    let (port, _, task) = mock().await;
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    configure(&store, &vault, port).await;
    let app = state(store.clone(), vault);
    let batch = super::super::search_one(
        &app,
        "alice",
        "private-search-text",
        10,
        &indexloom_engine::SearchParameters::default(),
        None,
        false,
    )
    .await;
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.results.len(), 1);
    let rss = crate::execution::scan_indexer(&app, "alice").await.unwrap();
    assert_eq!(rss.len(), 1);
    let outcome = super::super::test_one(app, "alice".into()).await;
    assert!(outcome.success, "{}", outcome.message);
    let stats = store.indexer_statistics().await.unwrap();
    let alice = stats.iter().find(|r| r.indexer_id == "alice").unwrap();
    assert_eq!(
        (
            alice.searches,
            alice.rss,
            alice.response_count,
            alice.results
        ),
        (1, 1, 3, 2)
    );
    assert!(alice.response_sum_ms > 0);
    let events = store
        .monitoring_events(&indexloom_storage::monitoring::EventFilter::default())
        .await
        .unwrap();
    let audit = serde_json::to_string(&events).unwrap();
    assert!(!audit.contains("private-search-text"));
    assert!(!audit.contains("fixture-password"));
    assert!(!audit.contains("fixture-csrf"));
    task.abort();
}
