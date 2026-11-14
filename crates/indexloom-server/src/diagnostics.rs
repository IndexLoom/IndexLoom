use crate::{ApiError, AppState};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::Deserialize;
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    severity: Option<String>,
    component: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/diagnostics/logs", get(logs))
}
async fn logs(
    State(state): State<AppState>,
    Query(f): Query<Filter>,
) -> Result<Json<Vec<indexloom_storage::management::DiagnosticLog>>, ApiError> {
    if f.severity
        .as_deref()
        .is_some_and(|s| !matches!(s, "info" | "warn" | "error"))
        || f.component.as_ref().is_some_and(|s| s.len() > 80)
        || !(1..=200).contains(&f.limit.unwrap_or(100))
        || f.before.is_some_and(|n| n < 1)
    {
        return Err(ApiError::bad_request("invalid diagnostic filter"));
    }
    Ok(Json(
        state
            .store
            .diagnostic_logs(
                f.severity.as_deref(),
                f.component.as_deref(),
                f.before.unwrap_or(i64::MAX),
                f.limit.unwrap_or(100),
            )
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn filters_validate_and_messages_exclude_sensitive_input() {
        let store = indexloom_storage::Store::connect("sqlite::memory:")
            .await
            .unwrap();
        store
            .diagnostic(indexloom_storage::management::DiagnosticEvent::JobFailure)
            .await
            .unwrap();
        let token = "management-diagnostics-token-32-bytes";
        let state = AppState::new(
            store,
            indexloom_storage::SecretVault::generate().unwrap(),
            crate::AdminToken::new(token).unwrap(),
        );
        let app = crate::router(state, std::path::PathBuf::from("missing-web-root"));
        for query in [
            "severity=debug",
            "limit=0",
            "limit=201",
            "before=-1",
            "unknown=value",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/diagnostics/logs?{query}"))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/diagnostics/logs?severity=error&component=scheduler")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 10000).await.unwrap();
        let rows: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["message"], "Scheduled runtime job failed");
        assert!(!String::from_utf8_lossy(&bytes).contains(token));
    }
}
