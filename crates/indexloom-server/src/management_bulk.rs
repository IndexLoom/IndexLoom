use crate::{ApiError, AppState};
use axum::{Json, Router, extract::State, routing::post};
use indexloom_storage::management::BulkPatch;
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/indexers/bulk", post(bulk))
}
async fn bulk(
    State(state): State<AppState>,
    Json(patch): Json<BulkPatch>,
) -> Result<Json<serde_json::Value>, ApiError> {
    patch
        .validate()
        .map_err(|_| ApiError::bad_request("invalid bulk operation"))?;
    let count = state
        .store
        .bulk_indexers(&patch)
        .await
        .map_err(|_| ApiError::bad_request("bulk operation failed; no changes applied"))?;
    state.executor_cache.clear().await;
    let _ = state
        .store
        .auth_audit(
            None,
            None,
            if patch.delete {
                "indexers-bulk-deleted"
            } else {
                "indexers-bulk-updated"
            },
        )
        .await;
    Ok(Json(serde_json::json!({"count":count})))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn rejects_invalid_ids_and_unauthenticated_requests() {
        let token = "management-bulk-test-token-32-bytes";
        let state = crate::AppState::new(
            indexloom_storage::Store::connect("sqlite::memory:")
                .await
                .unwrap(),
            indexloom_storage::SecretVault::generate().unwrap(),
            crate::AdminToken::new(token).unwrap(),
        );
        let app = crate::router(state, std::path::PathBuf::from("missing-web-root"));
        for body in [
            r#"{"ids":[],"delete":true}"#,
            r#"{"ids":["x","x"],"enabled":true}"#,
            r#"{"ids":["../live"],"delete":true}"#,
            r#"{"ids":["missing"],"enabled":true}"#,
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/indexers/bulk")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/indexers/bulk")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"ids":["missing"],"delete":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
