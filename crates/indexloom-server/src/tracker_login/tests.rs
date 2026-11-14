use super::*;
use crate::{AdminToken, router};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::Method,
    routing::any,
};
use indexloom_storage::{OutboundProxyInput, ProxyKind, ProxyPasswordUpdate, SecretVault, Store};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tower::ServiceExt;

const ALICE: &str = "alice-test-session-token-00000000000000000000000000000000";
const BOB: &str = "bob-test-session-token-0000000000000000000000000000000000";
const VIEWER: &str = "viewer-session-token-000000000000000000000000000000000000";
const OPERATOR: &str = "operator-test-token-0000000000000000000000000000000000000";

async fn mock(request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let cookie = request
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = to_bytes(request.into_body(), 65536).await.unwrap();
    let values = url::form_urlencoded::parse(&body)
        .into_owned()
        .collect::<BTreeMap<_, _>>();
    match path.as_str() {
        "/login" => ([("set-cookie", "session=tracker-secret; Path=/; HttpOnly")], "<form id=login action=/submit><input type=hidden name=csrf value=hidden-secret><img id=captcha src=/image><input name=answer><input name=username><input name=password></form>").into_response(),
        "/image" => {
            assert!(cookie.contains("session=tracker-secret"));
            ([("content-type", "image/png")], b"\x89PNG\r\n\x1a\nprivate-raster".to_vec()).into_response()
        }
        "/submit" => {
            assert!(cookie.contains("session=tracker-secret"));
            assert_eq!(values.get("csrf").map(String::as_str), Some("hidden-secret"));
            if values.get("answer").is_some_and(|v| v == "correct") {
                ([("set-cookie", "authenticated=1; Path=/")], "logged in").into_response()
            } else { "<div class=error>wrong</div>".into_response() }
        }
        "/test" => if cookie.contains("authenticated=1") { "<span class=authenticated>ok</span>".into_response() } else { "no".into_response() },
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn fixture() -> (AppState, Router, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(any(mock)))
            .await
            .unwrap();
    });
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    store
        .upsert_definition(
            "challenge-test",
            "fixture",
            None,
            "indexloom.io/v1alpha1",
            include_str!("../../../indexloom-engine/tests/fixtures/interactive.yml"),
            &"0".repeat(64),
        )
        .await
        .unwrap();
    for id in ["first", "second"] {
        store
            .create_indexer(id, "challenge-test", id, true)
            .await
            .unwrap();
        store
            .save_indexer_configuration(
                id,
                None,
                None,
                &BTreeMap::from([("username".into(), serde_json::json!("test-user"))]),
                &BTreeMap::from([("password".into(), "secret-password".into())]),
                &vault,
            )
            .await
            .unwrap();
    }
    store
        .upsert_proxy(
            &OutboundProxyInput {
                id: "mock".into(),
                name: "Offline HTTP mock".into(),
                kind: ProxyKind::Http,
                host: "127.0.0.1".into(),
                port,
                enabled: true,
                username: None,
            },
            ProxyPasswordUpdate::Clear,
            &vault,
        )
        .await
        .unwrap();
    store.set_global_proxy_id(Some("mock")).await.unwrap();
    for (id, token, role) in [
        ("alice", ALICE, "admin"),
        ("bob", BOB, "admin"),
        ("viewer", VIEWER, "viewer"),
    ] {
        store
            .add_local_user(
                id,
                id,
                &format!("{id}@example.invalid"),
                "unused-password-hash",
                true,
                role,
            )
            .await
            .unwrap();
        let hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        store.create_session(&hash, id, 3600).await.unwrap();
    }
    let mut state = AppState::new(store, vault, AdminToken::new(OPERATOR).unwrap());
    state.auth_settings.mode = auth::AuthMode::Local;
    let app = router(state.clone(), PathBuf::from("missing"));
    (state, app, task)
}
async fn request(
    app: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: &str,
    site: Option<&str>,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(site) = site {
        builder = builder.header("sec-fetch-site", site);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}
async fn json(response: Response) -> serde_json::Value {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "API integration exercises the complete login challenge state machine."
)]
async fn api_auth_csrf_image_isolation_submit_and_credential_revocation() {
    let (state, app, task) = fixture().await;
    let base = "/api/v1/indexers/first/login-challenge";
    assert_eq!(
        request(&app, Method::POST, base, None, "{}", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, Method::POST, base, Some(VIEWER), "{}", None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            Method::POST,
            base,
            Some(ALICE),
            "{}",
            Some("cross-site")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let descriptor = json(
        request(
            &app,
            Method::POST,
            base,
            Some(ALICE),
            "{}",
            Some("same-origin"),
        )
        .await,
    )
    .await;
    let id = descriptor["challenge"]["id"].as_str().unwrap();
    assert_eq!(descriptor["status"], "pending");
    let image = format!("{base}/{id}/image");
    assert_eq!(
        request(&app, Method::GET, &image, None, "", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, Method::GET, &image, Some(VIEWER), "", None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, Method::GET, &image, Some(BOB), "", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = request(
        &app,
        Method::GET,
        &image,
        Some(ALICE),
        "",
        Some("same-origin"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "no-store, private"
    );
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    let wrong_indexer = image.replace("/first/", "/second/");
    assert_eq!(
        request(&app, Method::GET, &wrong_indexer, Some(ALICE), "", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let result = json(
        request(
            &app,
            Method::POST,
            &format!("{base}/{id}"),
            Some(ALICE),
            r#"{"answers":{"answer":"correct"}}"#,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(result["status"], "complete");
    assert_eq!(
        request(&app, Method::GET, &image, Some(ALICE), "", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        execution::build_executor(&state, "first")
            .await
            .unwrap()
            .executor
            .test()
            .await
            .success
    );
    let next = json(request(&app, Method::POST, base, Some(ALICE), "{}", None).await).await;
    let next_id = next["challenge"]["id"].as_str().unwrap();
    // The actual configuration API invalidates the executor and its pending nonce.
    let saved = request(&app, Method::PUT, "/api/v1/indexers/first/configuration", Some(ALICE), r#"{"values":{"username":"changed"},"secrets":{"password":"changed-password"},"userAgent":null,"flaresolverrInstanceId":null}"#, None).await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        request(
            &app,
            Method::POST,
            &format!("{base}/{next_id}"),
            Some(ALICE),
            r#"{"answers":{"answer":"correct"}}"#,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    task.abort();
}

#[tokio::test]
async fn api_rejects_cookie_auth_arbitrary_image_urls_wrong_answers_and_rate_excess() {
    let (_, app, task) = fixture().await;
    let base = "/api/v1/indexers/first/login-challenge";
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(base)
                .header("cookie", format!("session={ALICE}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let p = json(request(&app, Method::POST, base, Some(ALICE), "{}", None).await).await;
    let id = p["challenge"]["id"].as_str().unwrap();
    let solution = format!("{base}/{id}");
    let wrong = request(
        &app,
        Method::POST,
        &solution,
        Some(ALICE),
        r#"{"answers":{"answer":"wrong"}}"#,
        None,
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(wrong.into_body(), 65536).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    for secret in [
        "secret-password",
        "tracker-secret",
        "hidden-secret",
        "tracker.invalid",
    ] {
        assert!(!body.contains(secret));
    }
    assert_eq!(
        request(
            &app,
            Method::POST,
            &solution,
            Some(ALICE),
            r#"{"answers":{"answer":"correct"}}"#,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            Method::GET,
            &format!("{base}/http%3A%2F%2F127.0.0.1/image"),
            Some(ALICE),
            "",
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..5 {
        assert_eq!(
            request(&app, Method::POST, base, Some(ALICE), "{}", None)
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        request(&app, Method::POST, base, Some(ALICE), "{}", None)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    task.abort();
}

#[tokio::test]
async fn disabled_auth_still_cannot_expose_challenge_images_or_start_login_anonymously() {
    let (mut state, _, task) = fixture().await;
    state.auth_settings.mode = auth::AuthMode::Disabled;
    let app = router(state, PathBuf::from("missing"));
    assert_eq!(
        request(
            &app,
            Method::POST,
            "/api/v1/indexers/first/login-challenge",
            None,
            "{}",
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    task.abort();
}
