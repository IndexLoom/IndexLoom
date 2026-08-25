//! HTTP surface for `IndexLoom`.

use std::path::PathBuf;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use indexloom_storage::Store;
use serde::Serialize;
use tower_http::services::ServeDir;

/// Shared application state.
#[derive(Clone, Debug)]
pub struct AppState {
    store: Store,
}

impl AppState {
    /// Creates application state around an initialized storage backend.
    #[must_use]
    pub const fn new(store: Store) -> Self {
        Self { store }
    }
}

/// Builds the API and frontend router.
pub fn router(state: AppState, web_root: PathBuf) -> Router {
    let web = ServeDir::new(web_root).append_index_html_on_directories(true);

    Router::new()
        .route("/api/v1/health", get(health))
        .fallback_service(web)
        .with_state(state)
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    database: &'static str,
}

async fn health(
    State(state): State<AppState>,
) -> Result<Json<HealthResponse>, (StatusCode, Json<HealthResponse>)> {
    match state.store.ping().await {
        Ok(()) => Ok(Json(HealthResponse {
            status: "ok",
            version: env!("CARGO_PKG_VERSION"),
            database: "ok",
        })),
        Err(error) => {
            tracing::error!(%error, "database health check failed");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    version: env!("CARGO_PKG_VERSION"),
                    database: "unavailable",
                }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use indexloom_storage::Store;
    use tower::ServiceExt;

    use super::{AppState, router};

    #[tokio::test]
    async fn health_endpoint_reports_ready_database() {
        let store = Store::connect("sqlite::memory:")
            .await
            .expect("in-memory database should initialize");
        let response = router(AppState::new(store), PathBuf::from("missing-web-root"))
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
}
