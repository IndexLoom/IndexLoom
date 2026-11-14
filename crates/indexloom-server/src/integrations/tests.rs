use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;
type Entries = Arc<tokio::sync::Mutex<Vec<Value>>>;
const TOKEN: &str = "integration-tests-admin-token-32-bytes";
async fn mock_list(State(s): State<Entries>) -> Json<Value> {
    Json(json!(*s.lock().await))
}
async fn mock_schema() -> Json<Value> {
    Json(
        json!([{"implementation":"Torznab","configContract":"TorznabSettings","fields":[{"name":"baseUrl"},{"name":"apiPath"},{"name":"apiKey"},{"name":"categories"}]}]),
    )
}
async fn mock_add(State(s): State<Entries>, Json(mut body): Json<Value>) -> Json<Value> {
    let mut entries = s.lock().await;
    body["id"] = json!(
        entries
            .iter()
            .filter_map(|v| v["id"].as_i64())
            .max()
            .unwrap_or(0)
            + 1
    );
    entries.push(body.clone());
    Json(body)
}
async fn mock_update(
    State(s): State<Entries>,
    Path((_version, id)): Path<(String, i64)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut entries = s.lock().await;
    let old = entries.iter_mut().find(|v| v["id"] == id).unwrap();
    *old = body.clone();
    Json(body)
}
async fn mock_delete(
    State(s): State<Entries>,
    Path((_version, id)): Path<(String, i64)>,
) -> Json<Value> {
    s.lock().await.retain(|v| v["id"] != id);
    Json(Value::Null)
}
async fn request(app: &Router, path: &str, method: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .method(method)
                .header("Authorization", format!("Bearer {TOKEN}"))
                .header("Content-Type", "application/json")
                .body(if body.is_null() {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Integrated API scenario exercises all supported app adapters"
)]
async fn rest_crud_sync_modes_and_unmanaged_entries() {
    let entries: Entries = Arc::new(tokio::sync::Mutex::new(vec![
        json!({"id":10,"name":"Unmanaged","implementation":"Torznab"}),
    ]));
    let mock = Router::new()
        .route("/api/{version}/indexer", get(mock_list).post(mock_add))
        .route("/api/{version}/indexer/schema", get(mock_schema))
        .route(
            "/api/{version}/indexer/{id}",
            axum::routing::put(mock_update).delete(mock_delete),
        )
        .route(
            "/api/{version}/system/status",
            get(|| async { Json(json!({"version":"test"})) }),
        )
        .with_state(entries.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
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
    store
        .create_indexer("excluded", "example", "Excluded", false)
        .await
        .unwrap();
    let state = AppState::new(
        store.clone(),
        indexloom_storage::SecretVault::generate().unwrap(),
        crate::AdminToken::new(TOKEN).unwrap(),
    );
    let app = crate::router(state, PathBuf::from("missing-web-root"));
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/integrations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    for provider in ["sonarr", "radarr", "lidarr", "readarr", "whisparr"] {
        let base = format!("/api/v1/integrations/{provider}");
        let saved=request(&app,&base,"PUT",json!({"provider":provider,"name":provider,"origin":origin,"enabled":true,"secrets":{"apiKey":"private-key","feedApiKey":"feed-key"}})).await;
        assert_eq!(saved.0, StatusCode::OK, "{saved:?}");
        assert!(!saved.1.to_string().contains("private-key"));
        assert_eq!(
            request(&app, &format!("{base}/test"), "POST", Value::Null)
                .await
                .0,
            StatusCode::OK
        );
        let profile =
            json!({"mode":"AddOnly","feedOrigin":"http://127.0.0.1:3000","indexerIds":["local"]});
        assert_eq!(
            request(&app, &format!("{base}/profile"), "PUT", profile.clone())
                .await
                .0,
            StatusCode::OK
        );
        let first = request(&app, &format!("{base}/sync"), "POST", Value::Null).await;
        assert_eq!(first.0, StatusCode::OK, "{first:?}");
        assert_eq!(first.1["added"], 1);
        let again = request(&app, &format!("{base}/sync"), "POST", Value::Null).await;
        assert_eq!(again.1["added"], 0);
        assert_eq!(again.1["updated"], 0);
        let mapped = store.managed_indexers(provider).await.unwrap();
        assert_eq!(mapped.len(), 1);
        let id = mapped["local"];
        entries
            .lock()
            .await
            .iter_mut()
            .find(|v| v["id"] == id)
            .unwrap()["name"] = json!("Replaced by user");
        assert_eq!(
            request(&app, &format!("{base}/sync"), "POST", Value::Null)
                .await
                .0,
            StatusCode::CONFLICT
        );
        entries
            .lock()
            .await
            .iter_mut()
            .find(|v| v["id"] == id)
            .unwrap()["name"] = json!(format!("IndexLoom [{provider}/local]"));
        assert_eq!(request(&app,&format!("{base}/profile"),"PUT",json!({"mode":"FullSync","feedOrigin":"http://127.0.0.1:3000","indexerIds":["local"],"priority":10})).await.0,StatusCode::OK);
        assert_eq!(
            request(&app, &format!("{base}/sync"), "POST", Value::Null)
                .await
                .1["updated"],
            1
        );
        assert_eq!(request(&app,&format!("{base}/profile"),"PUT",json!({"mode":"AddOnly","feedOrigin":"http://127.0.0.1:3000","indexerIds":["excluded"]})).await.0,StatusCode::OK);
        assert_eq!(
            request(&app, &format!("{base}/sync"), "POST", Value::Null)
                .await
                .1["deleted"],
            0
        );
        assert_eq!(request(&app,&format!("{base}/profile"),"PUT",json!({"mode":"FullSync","feedOrigin":"http://127.0.0.1:3000","indexerIds":["excluded"]})).await.0,StatusCode::OK);
        assert_eq!(
            request(&app, &format!("{base}/sync"), "POST", Value::Null)
                .await
                .1["deleted"],
            1
        );
        assert!(store.managed_indexers(provider).await.unwrap().is_empty());
        assert_eq!(
            request(&app, &base, "GET", Value::Null).await.0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, &base, "DELETE", Value::Null).await.0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, &base, "GET", Value::Null).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        entries.lock().await.as_slice(),
        &[json!({"id":10,"name":"Unmanaged","implementation":"Torznab"})]
    );
    assert_eq!(
        request(
            &app,
            "/api/v1/integrations/unsupported",
            "PUT",
            json!({"provider":"unsupported","name":"Unknown","origin":origin,"enabled":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    server.abort();
}

#[tokio::test]
async fn rest_configuration_validation_without_sockets() {
    let store = indexloom_storage::Store::connect("sqlite::memory:")
        .await
        .unwrap();
    let state = AppState::new(
        store,
        indexloom_storage::SecretVault::generate().unwrap(),
        crate::AdminToken::new(TOKEN).unwrap(),
    );
    let app = crate::router(state, PathBuf::from("missing-web-root"));
    let base = "/api/v1/integrations/app";
    let mut connection = json!({"provider":"sonarr","name":"App","origin":"http://localhost:8989","enabled":true,"secrets":{"apiKey":"fixture-secret"}});
    assert_eq!(
        request(&app, base, "PUT", connection.clone()).await.0,
        StatusCode::OK
    );
    let got = request(&app, base, "GET", Value::Null).await;
    assert!(!got.1.to_string().contains("fixture-secret"));
    assert!(got.1.get("secrets").is_none());
    connection["origin"] = json!("http://localhost:9999");
    assert_eq!(
        request(&app, base, "PUT", connection.clone()).await.0,
        StatusCode::CONFLICT
    );
    connection["origin"] = json!("http://192.168.1.2:8989");
    assert_eq!(
        request(&app, base, "PUT", connection.clone()).await.0,
        StatusCode::BAD_REQUEST
    );
    for selector in ["indexerIds", "groupIds", "tags"] {
        let mut profile = json!({"mode":"Disabled"});
        profile[selector] = json!(["missing"]);
        assert_eq!(
            request(&app, &format!("{base}/profile"), "PUT", profile)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            &app,
            &format!("{base}/profile"),
            "PUT",
            json!({"mode":"Disabled"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, &format!("{base}/sync"), "POST", Value::Null)
            .await
            .1["skipped"],
        true
    );
    assert_eq!(
        request(&app, base, "DELETE", Value::Null).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, base, "GET", Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn named_provider_crud_disabled_and_redaction() {
    let store = indexloom_storage::Store::connect("sqlite::memory:")
        .await
        .unwrap();
    let state = AppState::new(
        store,
        indexloom_storage::SecretVault::generate().unwrap(),
        crate::AdminToken::new(TOKEN).unwrap(),
    );
    let app = crate::router(state, PathBuf::from("missing-web-root"));
    for provider in ["mylar", "lazylibrarian"] {
        let base = format!("/api/v1/integrations/{provider}");
        let saved = request(&app,&base,"PUT",json!({"provider":provider,"name":provider,"origin":"http://localhost:8090","enabled":true,"secrets":{"apiKey":"app-secret","feedApiKey":"feed-secret"}})).await;
        assert_eq!(saved.0, StatusCode::OK);
        assert!(saved.1.get("secrets").is_none());
        let fetched = request(&app, &base, "GET", Value::Null).await;
        assert!(fetched.1.get("secrets").is_none());
        assert_eq!(
            request(&app, &format!("{base}/sync"), "POST", Value::Null)
                .await
                .1["skipped"],
            true
        );
        assert_eq!(
            request(&app, &base, "DELETE", Value::Null).await.0,
            StatusCode::OK
        );
    }
}
