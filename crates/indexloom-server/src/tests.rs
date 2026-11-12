//! Tests for the parent module; extracted for easier maintenance.
use std::path::PathBuf;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use indexloom_storage::{SecretVault, Store};
use tower::ServiceExt;

use super::{AdminToken, AppState, normalize_flaresolverr_endpoint, router};

const TOKEN: &str = "test-admin-token-with-at-least-32-bytes";
const DEFINITION: &str = r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata: { id: example, name: Example, description: Example, language: en-US }
provenance:
  generatedBy: indexloom-importer/v1
  source:
    project: jackett
    repository: https://github.com/Jackett/Jackett
    revision: "0000000000000000000000000000000000000000"
    path: src/Jackett.Common/Definitions/example.yml
    sourceId: example
    license: GPL-2.0-only
    fileSha256: "0000000000000000000000000000000000000000000000000000000000000000"
    contentSha256: "1111111111111111111111111111111111111111111111111111111111111111"
spec:
  protocol: torrent
  privacy: private
  baseUrls: [https://example.invalid/]
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities: {}
    settings:
      - { name: username, type: text, label: Username }
      - { name: password, type: password, label: Password }
      - { name: cookie, type: text, label: Cookie }
      - { name: info_flaresolverr, type: info_flaresolverr }
    search: { paths: [{ path: /search }], rows: { selector: .result }, fields: {} }
"#;

async fn test_state() -> AppState {
    let store = Store::connect("sqlite::memory:")
        .await
        .expect("in-memory database should initialize");
    store
        .upsert_definition(
            "example",
            "test",
            None,
            "indexloom.io/v1alpha1",
            DEFINITION,
            &"0".repeat(64),
        )
        .await
        .expect("definition should persist");
    store
        .create_indexer("example-main", "example", "Example", true)
        .await
        .expect("indexer should persist");
    AppState::new(
        store,
        SecretVault::generate().expect("vault should initialize"),
        AdminToken::new(TOKEN).expect("token should validate"),
    )
}

#[tokio::test]
async fn health_endpoint_reports_ready_database() {
    let response = router(test_state().await, PathBuf::from("missing-web-root"))
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("health request should complete");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn local_bootstrap_requires_operator_secret_and_sessions_are_revoked() {
    let mut state = test_state().await;
    state.auth_settings.mode = super::auth::AuthMode::Local;
    let app = router(state, PathBuf::from("missing-web-root"));

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let payload = r#"{"username":"alice","email":"alice@example.org","password":"very-long-secret-password"}"#;
    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header(header::AUTHORIZATION, "Bearer invalid")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let data: serde_json::Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 32_768).await.unwrap()).unwrap();
    let access_token = data["accessToken"]
        .as_str()
        .expect("session token is returned");
    assert!(access_token.len() >= 40);
    assert_eq!(data["user"]["username"], "alice");

    let repeat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeat.status(), StatusCode::CONFLICT);

    let accessible = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accessible.status(), StatusCode::OK);

    let logged_out = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logged_out.status(), StatusCode::NO_CONTENT);

    let rejected_again = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected_again.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn disabled_mode_explicitly_removes_auth_from_management_api() {
    let mut state = test_state().await;
    state.auth_settings.mode = super::auth::AuthMode::Disabled;
    let response = router(state, PathBuf::from("missing-web-root"))
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn management_api_requires_bearer_token() {
    let response = router(test_state().await, PathBuf::from("missing-web-root"))
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should complete");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn saves_and_redacts_secret_fields() {
    let app = router(test_state().await, PathBuf::from("missing-web-root"));
    let body = r#"{
          "userAgent":"Mozilla/5.0",
          "flaresolverrInstanceId":null,
          "values":{"username":"alice"},
          "secrets":{"password":"correct horse","cookie":"session=secret"}
        }"#;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/indexers/example-main/configuration")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("save request should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("response should read");
    let text = String::from_utf8(bytes.to_vec()).expect("response should be UTF-8");
    assert!(text.contains("configuredSecrets"));
    assert!(!text.contains("correct horse"));
    assert!(!text.contains("session=secret"));

    let preserve = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/indexers/example-main/configuration")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"userAgent":null,"flaresolverrInstanceId":null,"values":{"username":"bob"},"secrets":{}}"#,
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("preserve request should complete");
    let preserved = to_bytes(preserve.into_body(), 256 * 1024)
        .await
        .expect("response should read");
    let preserved = String::from_utf8(preserved.to_vec()).expect("response should be UTF-8");
    assert!(preserved.contains(r#""configuredSecrets":["cookie","password"]"#));

    let clear = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/indexers/example-main/configuration")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"userAgent":null,"flaresolverrInstanceId":null,"values":{"username":"bob"},"secrets":{"cookie":null}}"#,
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("clear request should complete");
    let cleared = to_bytes(clear.into_body(), 256 * 1024)
        .await
        .expect("response should read");
    let cleared = String::from_utf8(cleared.to_vec()).expect("response should be UTF-8");
    assert!(cleared.contains(r#""configuredSecrets":["password"]"#));
}

#[tokio::test]
async fn manages_proxy_groups_and_indexer_organization() {
    let app = router(test_state().await, PathBuf::from("missing-web-root"));
    let requests = [
        (
            "/api/v1/proxies/private-route",
            r#"{"name":"Private route","kind":"socks5","host":"proxy.invalid","port":1080,"enabled":true,"username":"alice","password":"secret","clearPassword":false}"#,
        ),
        (
            "/api/v1/groups/private",
            r#"{"name":"Private","description":"Private trackers","enabled":true,"priority":10,"proxyId":"private-route"}"#,
        ),
        (
            "/api/v1/indexers/example-main/organization",
            r#"{"displayName":"Example Private","enabled":true,"proxyId":null,"groupIds":["private"],"tags":["private","anime"]}"#,
        ),
    ];
    for (uri, body) in requests {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("management request should complete");
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let body = to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("response should read");
        assert!(!String::from_utf8_lossy(&body).contains(r#""password":"secret""#));
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("list request should complete");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("response should read");
    let text = String::from_utf8(body.to_vec()).expect("response should be UTF-8");
    assert!(text.contains(r#""displayName":"Example Private""#));
    assert!(text.contains(r#""groupIds":["private"]"#));
    assert!(text.contains(r#""tags":["anime","private"]"#));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn local_bootstrap_login_logout_and_legacy_token_isolation() {
    let mut state = test_state().await;
    state.auth_settings.mode = super::auth::AuthMode::Local;
    let app = router(state, PathBuf::from("missing-web-root"));
    let bootstrap_json =
        r#"{"username":"alice","email":"alice@example.org","password":"correct-horse-battery"}"#;
    let without_operator = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bootstrap_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(without_operator.status(), StatusCode::UNAUTHORIZED);

    let bootstrap = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::from(bootstrap_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::OK);
    let body = to_bytes(bootstrap.into_body(), 4096).await.unwrap();
    let session: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token = session["accessToken"].as_str().unwrap();
    assert_eq!(session["user"]["username"], "alice");

    let legacy = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(legacy.status(), StatusCode::UNAUTHORIZED);

    let authorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);

    let repeated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bootstrap_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::CONFLICT);

    let login_bad = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"login":"alice","password":"wrong-password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_bad.status(), StatusCode::UNAUTHORIZED);

    let login_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"login":"alice@example.org","password":"correct-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_ok.status(), StatusCode::OK);

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let revoked = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn explicitly_disabled_mode_bypasses_admin_gate() {
    let mut state = test_state().await;
    state.auth_settings.mode = super::auth::AuthMode::Disabled;
    let response = router(state, PathBuf::from("missing-web-root"))
        .oneshot(
            Request::builder()
                .uri("/api/v1/indexers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn role_policies_protect_direct_api_requests_not_just_frontend_controls() {
    use axum::http::Method;

    const OWNER: &str = "owner-session-token-longer-than-forty-characters";
    const ADMIN: &str = "admin-session-token-longer-than-forty-characters";
    const VIEWER: &str = "viewer-session-token-longer-than-forty-characters";

    async fn request_as(
        app: axum::Router,
        method: Method,
        path: &'static str,
        token: &'static str,
        body: &'static str,
    ) -> StatusCode {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    let mut state = test_state().await;
    state.auth_settings.mode = super::auth::AuthMode::Local;
    state
        .store
        .bootstrap_admin("owner-id", "owner", "owner@example.org", "hash", true)
        .await
        .unwrap();
    state
        .store
        .add_local_user(
            "admin-id",
            "admin",
            "admin@example.org",
            "hash",
            true,
            "admin",
        )
        .await
        .unwrap();
    state
        .store
        .add_local_user(
            "viewer-id",
            "viewer",
            "viewer@example.org",
            "hash",
            true,
            "viewer",
        )
        .await
        .unwrap();
    for (token, id) in [
        (OWNER, "owner-id"),
        (ADMIN, "admin-id"),
        (VIEWER, "viewer-id"),
    ] {
        state
            .store
            .create_session(&super::auth::digest(token), id, 120)
            .await
            .unwrap();
    }
    let app = router(state.clone(), PathBuf::from("missing-web-root"));
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/indexers", VIEWER, "").await,
        StatusCode::OK
    );
    assert_eq!(
        request_as(app.clone(), Method::POST, "/api/v1/search", VIEWER, "{}").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request_as(
            app.clone(),
            Method::PUT,
            "/api/v1/flaresolverr/abc",
            VIEWER,
            "{}"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/users", VIEWER, "").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/mail", VIEWER, "").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/audit", ADMIN, "").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/users", ADMIN, "").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request_as(
            app.clone(),
            Method::PUT,
            "/api/v1/flaresolverr/abc",
            ADMIN,
            "{}"
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/users", OWNER, "").await,
        StatusCode::OK
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/audit", OWNER, "").await,
        StatusCode::OK
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/indexers", TOKEN, "").await,
        StatusCode::UNAUTHORIZED
    );

    let elevate = request_as(
        app.clone(),
        Method::PUT,
        "/api/v1/auth/users/viewer-id/role",
        OWNER,
        r#"{"role":"admin"}"#,
    )
    .await;
    assert_eq!(elevate, StatusCode::OK);
    assert_eq!(
        request_as(
            app.clone(),
            Method::PUT,
            "/api/v1/flaresolverr/abc",
            VIEWER,
            "{}"
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request_as(app.clone(), Method::GET, "/api/v1/auth/users", VIEWER, "").await,
        StatusCode::FORBIDDEN
    );
    let demote = request_as(
        app.clone(),
        Method::PUT,
        "/api/v1/auth/users/viewer-id/role",
        OWNER,
        r#"{"role":"viewer"}"#,
    )
    .await;
    assert_eq!(demote, StatusCode::OK);
    assert_eq!(
        request_as(app, Method::PUT, "/api/v1/flaresolverr/abc", VIEWER, "{}").await,
        StatusCode::FORBIDDEN
    );
}

#[test]
fn normalizes_flaresolverr_base_url() {
    assert_eq!(
        normalize_flaresolverr_endpoint("http://flaresolverr:8191").expect("URL should validate"),
        "http://flaresolverr:8191/v1"
    );
    assert!(normalize_flaresolverr_endpoint("file:///etc/passwd").is_err());
    assert!(normalize_flaresolverr_endpoint("http://user:pass@host/v1").is_err());
}
