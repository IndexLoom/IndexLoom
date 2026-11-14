//! Authenticated own-origin manual tracker login. Request bodies and tracker
//! errors are deliberately excluded from logs and error responses.
use crate::{ApiError, AppState, auth, execution, validate_identifier};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use indexloom_engine::{CardigannExecutor, EngineError, LoginProgress};
use serde::Deserialize;
use std::collections::BTreeMap;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/indexers/{id}/login-challenge", post(begin))
        .route(
            "/api/v1/indexers/{id}/login-challenge/{challenge}",
            post(submit),
        )
        .route(
            "/api/v1/indexers/{id}/login-challenge/{challenge}/image",
            get(image),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Solution {
    answers: BTreeMap<String, String>,
}

async fn runtime(
    state: &AppState,
    id: &str,
) -> Result<std::sync::Arc<CardigannExecutor>, ApiError> {
    validate_identifier(id)?;
    execution::build_executor(state, id)
        .await
        .map(|h| h.executor)
        .map_err(|_| {
            ApiError::bad_request(
                "tracker configuration is unavailable; save valid credentials and routing first",
            )
        })
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Consuming errors through map_err deliberately maps all sensitive remote details to sanitized responses."
)]
fn challenge_error(error: EngineError) -> ApiError {
    match error {
        EngineError::LoginChallengeRateLimited => {
            ApiError::too_many_requests("too many tracker login requests; wait one minute")
        }
        EngineError::LoginChallengeUnavailable => ApiError::not_found(
            "tracker challenge expired or is unavailable; start tracker login again",
        ),
        EngineError::BrowserLoginRequired => ApiError::bad_request(
            "this CAPTCHA requires a browser; use a tracker session cookie or configure FlareSolverr for Cloudflare",
        ),
        EngineError::AuthenticationRejected => ApiError::bad_request(
            "tracker rejected the response or credentials; check credentials and start tracker login again",
        ),
        EngineError::InvalidLoginChallenge
        | EngineError::AuthenticationFormMissing
        | EngineError::UnsupportedAuthentication(_) => ApiError::bad_request(
            "tracker challenge format is unsupported or the response is invalid; start tracker login again",
        ),
        _ => ApiError::bad_request(
            "tracker login request failed; check routing and start tracker login again",
        ),
    }
}

async fn begin(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<LoginProgress>, ApiError> {
    let actor = auth::tracker_login_actor(&state, &headers).await?;
    let result = runtime(&state, &id)
        .await?
        .begin_interactive_login(&actor)
        .await;
    tracing::info!(
        event = "tracker_login_begin",
        success = result.is_ok(),
        "redacted tracker login event"
    );
    result.map(Json).map_err(challenge_error)
}

async fn submit(
    State(state): State<AppState>,
    Path((id, challenge)): Path<(String, String)>,
    headers: HeaderMap,
    Json(solution): Json<Solution>,
) -> Result<Json<LoginProgress>, ApiError> {
    let actor = auth::tracker_login_actor(&state, &headers).await?;
    validate_challenge(&challenge)?;
    if solution.answers.len() > 16
        || solution
            .answers
            .iter()
            .any(|(k, v)| k.len() > 128 || v.len() > 512)
    {
        // Let the engine consume a valid owned challenge even on malformed input.
        return runtime(&state, &id)
            .await?
            .submit_login_challenge(&actor, &challenge, BTreeMap::new())
            .await
            .map(Json)
            .map_err(challenge_error);
    }
    let result = runtime(&state, &id)
        .await?
        .submit_login_challenge(&actor, &challenge, solution.answers)
        .await;
    tracing::info!(
        event = "tracker_login_submit",
        success = result.is_ok(),
        "redacted tracker login event"
    );
    result.map(Json).map_err(challenge_error)
}

async fn image(
    State(state): State<AppState>,
    Path((id, challenge)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let actor = auth::tracker_login_actor(&state, &headers).await?;
    validate_challenge(&challenge)?;
    let (mime, bytes) = runtime(&state, &id)
        .await?
        .login_challenge_image(&actor, &challenge)
        .await
        .map_err(challenge_error)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CACHE_CONTROL, "no-store, private"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox",
            ),
            (
                header::HeaderName::from_static("cross-origin-resource-policy"),
                "same-origin",
            ),
        ],
        bytes,
    )
        .into_response())
}

fn validate_challenge(id: &str) -> Result<(), ApiError> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("tracker challenge is unavailable"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
