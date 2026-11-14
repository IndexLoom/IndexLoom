use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use indexloom_storage::{SecretVault, Store};
use std::path::PathBuf;
use tower::ServiceExt;
const TOKEN: &str = "stage-d-test-token-at-least-32-characters";
async fn state() -> AppState {
    let store = Store::connect("sqlite::memory:").await.unwrap();
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
        .create_indexer("one", "example", "One", true)
        .await
        .unwrap();
    AppState::new(
        store,
        SecretVault::generate().unwrap(),
        crate::AdminToken::new(TOKEN).unwrap(),
    )
}
fn request(path: &str, method: &str, token: bool) -> Request<Body> {
    let mut builder = Request::builder().uri(path).method(method);
    if token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    }
    builder.body(Body::empty()).unwrap()
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
#[test]
fn classifies_without_formatting_errors() {
    assert_eq!(
        classify(&EngineError::UnexpectedStatus(401)),
        Failure::Authentication
    );
    assert_eq!(
        classify(&EngineError::UnexpectedStatus(429)),
        Failure::RateLimited
    );
    assert_eq!(classify(&EngineError::RequestTimeout), Failure::Network);
    assert_eq!(
        classify(&EngineError::UnexpectedStatus(503)),
        Failure::Upstream
    );
}
#[tokio::test]
async fn authenticated_routes_health_metrics_and_reset() {
    let state = state().await;
    for _ in 0..3 {
        state
            .store
            .observe(
                Some("one"),
                Operation::Search,
                Some(Failure::Authentication),
                20,
                0,
            )
            .await
            .unwrap();
    }
    let app = crate::router(state.clone(), PathBuf::from("missing"));
    for path in [
        "/api/v1/monitoring/indexers",
        "/api/v1/monitoring/indexers/one",
        "/api/v1/monitoring/events",
        "/api/v1/monitoring/health",
        "/api/v1/metrics",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(path, "GET", false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = app
            .clone()
            .oneshot(request(path, "GET", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let response = app
        .clone()
        .oneshot(request("/api/v1/monitoring/health", "GET", true))
        .await
        .unwrap();
    let health = body(response).await;
    assert_eq!(health["indexers"][0]["status"], "open");
    assert_eq!(health["components"]["clockSkew"]["status"], "unknown");
    let metrics = app
        .clone()
        .oneshot(request("/api/v1/metrics", "GET", true))
        .await
        .unwrap();
    let metrics =
        String::from_utf8(to_bytes(metrics.into_body(), 65536).await.unwrap().to_vec()).unwrap();
    assert!(metrics.contains("indexloom_search_requests_total"));
    assert!(metrics.contains("indexloom_durable_authentication_errors_total 3"));
    assert!(metrics.contains("indexloom_open_circuits 1"));
    let response = app
        .clone()
        .oneshot(request(
            "/api/v1/monitoring/indexers/one/reset",
            "POST",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.store.claim_indexer_dispatch("one").await.unwrap());
    assert_eq!(
        app.oneshot(request("/api/v1/monitoring/indexers/missing", "GET", true))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn open_circuit_skips_search_test_rss_and_preserves_counters() {
    let state = state().await;
    for _ in 0..3 {
        state
            .store
            .observe(Some("one"), Operation::Search, Some(Failure::Network), 5, 0)
            .await
            .unwrap();
    }
    let app = crate::router(state.clone(), PathBuf::from("missing"));
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/search")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"query":"private query never persist","selection":{"all":true}}"#,
        ))
        .unwrap();
    let response = body(app.clone().oneshot(req).await.unwrap()).await;
    assert!(
        response["indexers"][0]["error"]
            .as_str()
            .unwrap()
            .contains("circuit")
    );
    assert!(crate::execution::scan_indexer(&state, "one").await.is_err());
    let row = state.store.indexer_statistics().await.unwrap().remove(0);
    assert_eq!((row.searches, row.rss, row.consecutive_failures), (3, 0, 3));
    let events = state
        .store
        .monitoring_events(&EventFilter::default())
        .await
        .unwrap();
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains("private query"));
    assert!(!serialized.contains(TOKEN));
    // Global access-denied audit records contain no submitted credential.
    app.oneshot(request("/api/v1/monitoring/events", "GET", false))
        .await
        .unwrap();
    let events = state
        .store
        .monitoring_events(&EventFilter {
            category: Some("auth".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(events.events[0].message, "authentication");
}

#[tokio::test]
async fn open_circuit_skips_download_and_test_job() {
    let state = state().await;
    let executor = crate::execution::build_executor(&state, "one")
        .await
        .unwrap()
        .executor;
    for _ in 0..3 {
        state
            .store
            .observe(Some("one"), Operation::Search, Some(Failure::Network), 5, 0)
            .await
            .unwrap();
    }
    let release: indexloom_core::SearchResult=serde_json::from_value(json!({"id":uuid::Uuid::now_v7(),"indexerInstanceId":"one","definitionId":"example","title":"private-title"})).unwrap();
    let error = resolve_release(&state, &executor, &release)
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    let app = crate::router(state.clone(), PathBuf::from("missing"));
    let req = Request::builder()
        .uri("/api/v1/indexer-tests")
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"all":true}"#))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let job = body(response).await;
    let path = format!("/api/v1/indexer-tests/{}", job["id"].as_str().unwrap());
    let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let result = body(
                app.clone()
                    .oneshot(request(&path, "GET", true))
                    .await
                    .unwrap(),
            )
            .await;
            if result["status"] == "completed" {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed["results"][0]["success"], false);
    assert!(
        completed["results"][0]["message"]
            .as_str()
            .unwrap()
            .contains("circuit")
    );
    let indexer_metrics = state.store.indexer_statistics().await.unwrap().remove(0);
    assert_eq!(
        (
            indexer_metrics.downloads,
            indexer_metrics.response_count,
            indexer_metrics.consecutive_failures
        ),
        (0, 3, 3)
    );
}
