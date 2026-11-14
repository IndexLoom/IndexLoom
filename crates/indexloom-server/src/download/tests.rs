use super::*;
use crate::{AdminToken, router};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use indexloom_storage::{SecretVault, Store};
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "test-download-token-with-at-least-32-bytes";
const YAML: &str =
    include_str!("../../../indexloom-engine/src/cardigann/download/fixtures/indexer.yml");

async fn fixture() -> (AppState, Arc<CardigannExecutor>, SearchResult, HeaderMap) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    store
        .upsert_definition(
            "grab-fixture",
            "fixture",
            None,
            "indexloom.io/v1alpha1",
            YAML,
            &"0".repeat(64),
        )
        .await
        .unwrap();
    store
        .create_indexer("instance-1", "grab-fixture", "fixture", true)
        .await
        .unwrap();
    let state = AppState::new(
        store,
        SecretVault::generate().unwrap(),
        AdminToken::new(TOKEN).unwrap(),
    );
    let executor = crate::execution::build_executor(&state, "instance-1")
        .await
        .unwrap()
        .executor;
    let release = SearchResult {
        id: Uuid::now_v7(),
        indexer_instance_id: "instance-1".to_owned(),
        definition_id: "grab-fixture".to_owned(),
        source_id: Some("17".to_owned()),
        source_guid: Some("https://primary.example/guid?passkey=private".to_owned()),
        info_hash: None,
        title: "Fixture".to_owned(),
        details_url: Some("https://primary.example/details?passkey=private".to_owned()),
        download_url: Some("https://primary.example/download?passkey=private".to_owned()),
        magnet_url: Some(format!("magnet:?xt=urn:btih:{}", "a".repeat(40))),
        category: None,
        size: None,
        seeders: None,
        leechers: None,
        published_at: None,
        protocol: indexloom_core::ReleaseProtocol::Torrent,
        attributes: indexloom_core::ReleaseAttributes::default(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {TOKEN}").parse().unwrap(),
    );
    (state, executor, release, headers)
}
async fn publish(
    state: &AppState,
    executor: &Arc<CardigannExecutor>,
    release: SearchResult,
    headers: &HeaderMap,
) -> (String, SearchResult) {
    let mut results = vec![release];
    state
        .downloads
        .publish(executor, &mut results, Some(credential_scope(headers)))
        .await;
    (results[0].download_url.clone().unwrap(), results.remove(0))
}

#[tokio::test]
async fn publish_hides_private_urls_and_issues_unpredictable_bound_handles() {
    let (state, executor, release, headers) = fixture().await;
    let (path, result) = publish(&state, &executor, release.clone(), &headers).await;
    assert!(path.starts_with("/api/v1/downloads/"));
    assert_eq!(path.rsplit('/').next().unwrap().len(), 64);
    let body = serde_json::to_string(&result).unwrap();
    assert!(!body.contains("passkey"));
    assert!(!body.contains("primary.example"));
    assert!(result.details_url.is_none());
    assert!(result.magnet_url.is_none());
    let (other, _) = publish(&state, &executor, release, &headers).await;
    assert_ne!(path, other);
}

#[tokio::test]
async fn endpoint_requires_auth_and_is_single_use() {
    let (state, executor, release, headers) = fixture().await;
    let (path, _) = publish(&state, &executor, release, &headers).await;
    let app = router(state, "missing-web-root".into());
    let unauthorized = app
        .clone()
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(
                    header::AUTHORIZATION,
                    headers[header::AUTHORIZATION].clone(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let data: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 32768).await.unwrap()).unwrap();
    assert!(
        data["magnetUrl"]
            .as_str()
            .unwrap()
            .starts_with("magnet:?xt=")
    );
    let repeat = app
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(
                    header::AUTHORIZATION,
                    headers[header::AUTHORIZATION].clone(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeat.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wrong_credential_cannot_consume_or_destroy_handle() {
    let (state, executor, release, headers) = fixture().await;
    let (path, _) = publish(&state, &executor, release, &headers).await;
    let handle = path.rsplit('/').next().unwrap();
    assert!(state.downloads.take(handle, [0; 32]).await.is_err());
    assert!(
        state
            .downloads
            .take(handle, credential_scope(&headers))
            .await
            .is_ok()
    );
    assert!(
        state
            .downloads
            .take(handle, credential_scope(&headers))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn expired_handles_and_replaced_runtime_are_rejected() {
    let (state, executor, release, headers) = fixture().await;
    let (path, _) = publish(&state, &executor, release.clone(), &headers).await;
    let handle = path.rsplit('/').next().unwrap();
    state
        .downloads
        .entries
        .lock()
        .await
        .get_mut(handle)
        .unwrap()
        .created = Instant::now()
        .checked_sub(TTL + Duration::from_secs(1))
        .unwrap();
    assert!(
        state
            .downloads
            .take(handle, credential_scope(&headers))
            .await
            .is_err()
    );
    let (path, _) = publish(&state, &executor, release, &headers).await;
    state.executor_cache.invalidate("instance-1").await;
    let response = router(state, "missing-web-root".into())
        .oneshot(
            Request::builder()
                .uri(path)
                .header(
                    header::AUTHORIZATION,
                    headers[header::AUTHORIZATION].clone(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn arbitrary_urls_and_unknown_handles_are_not_a_proxy() {
    let (state, _, _, headers) = fixture().await;
    let app = router(state, "missing-web-root".into());
    for path in [
        "/api/v1/downloads/https%3A%2F%2F127.0.0.1",
        "/api/v1/downloads/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?url=http://127.0.0.1/",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(
                        header::AUTHORIZATION,
                        headers[header::AUTHORIZATION].clone(),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn feed_search_rows_do_not_publish_tracker_download_urls() {
    let (state, executor, release, _) = fixture().await;
    let mut releases = vec![release];
    state
        .downloads
        .publish(&executor, &mut releases, None)
        .await;
    assert!(releases[0].download_url.is_none());
    assert!(releases[0].magnet_url.is_none());
    assert!(state.downloads.entries.lock().await.is_empty());
}

#[tokio::test]
async fn rss_guid_does_not_reveal_upstream_tokens_and_is_stable() {
    use super::feed_guid;
    let (_state, _executor, mut release, _headers) = fixture().await;
    release.source_guid = Some("https://tracker.example/download?passkey=secret-value".into());
    release.title = "Test Release".into();
    let first = feed_guid(&release);
    let second = feed_guid(&release);
    assert_eq!(first, second);
    assert_eq!(first.len(), 64);
    assert!(!first.contains("passkey"));
    release.title = "Changed Title".into();
    assert_ne!(feed_guid(&release), first);
}
