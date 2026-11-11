//! Optional second factor for local passwords: RFC 6238 SHA-256 and offline recovery.
//! All authentication secrets are encrypted at rest and only shown at enrollment.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use indexloom_storage::auth::AuthUser;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

use super::{
    ApiError, AppState, audit_action, digest, issue_session, new_token, user_for_bearer,
    verify_password,
};

type HmacSha256 = Hmac<Sha256>;

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const PERIOD: i64 = 30;
const CODE_DIGITS: u32 = 6;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FactorRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DisableRequest {
    password: String,
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompleteRequest {
    challenge_token: String,
    code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginChallenge {
    pub requires_totp: bool,
    pub challenge_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse {
    secret: String,
    otpauth_uri: String,
    expires_in_seconds: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmResponse {
    recovery_codes: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    enabled: bool,
    recovery_codes_remaining: i64,
}

pub(super) fn public_routes() -> Router<AppState> {
    Router::new().route("/api/v1/auth/totp/complete", post(complete_login))
}

pub(super) fn private_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/totp", get(status))
        .route("/api/v1/auth/totp/start", post(start_setup))
        .route("/api/v1/auth/totp/confirm", post(confirm_setup))
        .route("/api/v1/auth/totp/disable", post(disable))
}

fn encode_base32(input: &[u8]) -> String {
    let mut encoded = String::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for &byte in input {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(char::from(
                BASE32_ALPHABET[((buffer >> bits) & 31) as usize],
            ));
        }
        buffer &= (1_u32 << bits) - 1;
    }
    if bits != 0 {
        encoded.push(char::from(
            BASE32_ALPHABET[((buffer << (5 - bits)) & 31) as usize],
        ));
    }
    encoded
}

fn decode_base32(encoded: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in encoded.bytes() {
        let value = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'2'..=b'7' => u32::from(byte - b'2') + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            decoded.push(((buffer >> bits) & 255) as u8);
        }
        buffer &= (1_u32 << bits) - 1;
    }
    if bits != 0 && buffer != 0 {
        return None;
    }
    Some(decoded)
}

fn totp_for_step(secret: &[u8], step: i64) -> Result<String, ApiError> {
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|_| ApiError::internal("invalid TOTP key"))?;
    mac.update(&step.to_be_bytes());
    let bytes = mac.finalize().into_bytes();
    let offset = usize::from(bytes[bytes.len() - 1] & 0x0f);
    let number = ((u32::from(bytes[offset] & 0x7f)) << 24)
        | (u32::from(bytes[offset + 1]) << 16)
        | (u32::from(bytes[offset + 2]) << 8)
        | u32::from(bytes[offset + 3]);
    Ok(format!(
        "{:0width$}",
        number % 10_u32.pow(CODE_DIGITS),
        width = CODE_DIGITS as usize
    ))
}

fn valid_totp_step(secret: &str, code: &str) -> Result<Option<i64>, ApiError> {
    if code.len() != CODE_DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let secret =
        decode_base32(secret).ok_or_else(|| ApiError::internal("invalid stored factor"))?;
    let step = OffsetDateTime::now_utc()
        .unix_timestamp()
        .div_euclid(PERIOD);
    // Both back and forward by one step; SQLite prevents reusing an accepted step.
    for offset in [0_i64, -1, 1] {
        let candidate_step = step + offset;
        let expected = totp_for_step(&secret, candidate_step)?;
        if bool::from(expected.as_bytes().ct_eq(code.as_bytes())) {
            return Ok(Some(candidate_step));
        }
    }
    Ok(None)
}

fn normalize_factor(code: &str) -> Option<String> {
    if code.len() > 48 {
        return None;
    }
    let code = code.trim().replace(['-', ' '], "").to_ascii_uppercase();
    if code.len() == CODE_DIGITS as usize && code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(code);
    }
    if code.len() == 16 && code.bytes().all(|byte| BASE32_ALPHABET.contains(&byte)) {
        return Some(code);
    }
    None
}

fn recovery_digest(user_id: &str, code: &str) -> String {
    digest(&format!("indexloom:totp-recovery:{user_id}:{code}"))
}

async fn use_factor(state: &AppState, user_id: &str, submitted: &str) -> Result<bool, ApiError> {
    let Some(code) = normalize_factor(submitted) else {
        return Ok(false);
    };
    if code.len() == 16 {
        return Ok(state
            .store
            .totp_consume_recovery(user_id, &recovery_digest(user_id, &code))
            .await?);
    }
    let secret = state
        .store
        .totp_current_secret(user_id, &state.vault)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let Some(step) = valid_totp_step(&secret, &code)? else {
        return Ok(false);
    };
    Ok(state.store.totp_consume_step(user_id, step).await?)
}

pub(super) async fn start_login_challenge(
    state: &AppState,
    user: &AuthUser,
    remember_me: bool,
) -> Result<LoginChallenge, ApiError> {
    let token = new_token()?;
    let expires = state
        .store
        .totp_create_challenge(&digest(&token), &user.id, remember_me)
        .await?;
    Ok(LoginChallenge {
        requires_totp: true,
        challenge_token: token,
        expires_at: expires,
    })
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StatusResponse>, ApiError> {
    let user = user_for_bearer(&state, &headers).await?;
    let enabled = state.store.totp_enabled(&user.id).await?;
    let recovery_codes_remaining = state.store.totp_recovery_count(&user.id).await?;
    Ok(Json(StatusResponse {
        enabled,
        recovery_codes_remaining,
    }))
}

async fn start_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> Result<Json<StartResponse>, ApiError> {
    if !state.auth_settings.mode.local() {
        return Err(ApiError::not_found(
            "local password authentication is disabled",
        ));
    }
    let user = user_for_bearer(&state, &headers).await?;
    let password_hash = user
        .password_hash
        .as_ref()
        .ok_or_else(ApiError::unauthorized)?;
    if !verify_password(
        request.password,
        password_hash.clone(),
        Arc::clone(&state.auth_settings.hash_permits),
    )
    .await
    {
        return Err(ApiError::unauthorized());
    }
    let mut bytes = [0_u8; 20];
    getrandom::fill(&mut bytes).map_err(|_| ApiError::internal("random generator unavailable"))?;
    let secret = encode_base32(&bytes);
    if !state
        .store
        .totp_start_pending(&user.id, &secret, &state.vault)
        .await?
    {
        return Err(ApiError::conflict("TOTP is already enabled"));
    }
    let mut uri = url::Url::parse("otpauth://totp/IndexLoom")
        .map_err(|_| ApiError::internal("failed to create provisioning URI"))?;
    uri.set_path(&format!("/IndexLoom:{}", user.username));
    uri.query_pairs_mut()
        .append_pair("secret", &secret)
        .append_pair("issuer", "IndexLoom")
        .append_pair("algorithm", "SHA256")
        .append_pair("digits", "6")
        .append_pair("period", "30");
    Ok(Json(StartResponse {
        secret,
        otpauth_uri: uri.to_string(),
        expires_in_seconds: 600,
    }))
}

async fn confirm_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FactorRequest>,
) -> Result<Json<ConfirmResponse>, ApiError> {
    let user = user_for_bearer(&state, &headers).await?;
    let secret = state
        .store
        .totp_pending_secret(&user.id, &state.vault)
        .await?
        .ok_or_else(|| ApiError::bad_request("no pending TOTP enrollment"))?;
    let rate_key = digest(&format!("totp-enrollment:{}", user.id));
    if state.store.is_login_locked(&rate_key).await? {
        return Err(ApiError::too_many_requests(
            "too many authenticator enrollment attempts",
        ));
    }
    let normalized = normalize_factor(&request.code);
    let step = match normalized.as_deref() {
        Some(code) => valid_totp_step(&secret, code)?,
        None => None,
    };
    let Some(step) = step else {
        state.store.record_login_failure(&rate_key).await?;
        return Err(ApiError::unauthorized());
    };
    let mut codes = Vec::with_capacity(10);
    let mut digests = Vec::with_capacity(10);
    for _ in 0..10 {
        let mut bytes = [0_u8; 10];
        getrandom::fill(&mut bytes)
            .map_err(|_| ApiError::internal("random generator unavailable"))?;
        let code = encode_base32(&bytes);
        digests.push(recovery_digest(&user.id, &code));
        codes.push(code);
    }
    if !state.store.totp_confirm_pending(&user.id, &digests).await? {
        return Err(ApiError::bad_request(
            "enrollment has expired or factor already exists",
        ));
    }
    // The setup code must also be single use; no immediate reuse for sign-in.
    state.store.totp_consume_step(&user.id, step).await?;
    state.store.clear_login_failures(&rate_key).await?;
    // Revoke old sessions after successful enrollment: authenticate with new factor.
    state.store.revoke_all_sessions(&user.id).await?;
    audit_action(&state, Some(&user.id), Some(&user.id), "totp-enabled").await;
    Ok(Json(ConfirmResponse {
        recovery_codes: codes,
    }))
}

async fn disable(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DisableRequest>,
) -> Result<StatusCode, ApiError> {
    let user = user_for_bearer(&state, &headers).await?;
    if !state.store.totp_enabled(&user.id).await? {
        return Err(ApiError::bad_request("TOTP is not enabled"));
    }
    let hash = user.password_hash.ok_or_else(ApiError::unauthorized)?;
    let rate_key = digest(&format!("totp-disable:{}", user.id));
    if state.store.is_login_locked(&rate_key).await? {
        return Err(ApiError::too_many_requests(
            "too many authenticator removal attempts",
        ));
    }
    if !verify_password(
        request.password,
        hash,
        Arc::clone(&state.auth_settings.hash_permits),
    )
    .await
        || !use_factor(&state, &user.id, &request.code).await?
    {
        state.store.record_login_failure(&rate_key).await?;
        return Err(ApiError::unauthorized());
    }
    state.store.totp_disable(&user.id).await?;
    state.store.clear_login_failures(&rate_key).await?;
    audit_action(&state, Some(&user.id), Some(&user.id), "totp-disabled").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn complete_login(
    State(state): State<AppState>,
    Json(request): Json<CompleteRequest>,
) -> Result<Json<super::SessionView>, ApiError> {
    if !state.auth_settings.mode.local() {
        return Err(ApiError::not_found("local authentication is disabled"));
    }
    if !(40..=128).contains(&request.challenge_token.len()) {
        return Err(ApiError::unauthorized());
    }
    let token_hash = digest(&request.challenge_token);
    let challenge = state
        .store
        .totp_challenge(&token_hash)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let user = state
        .store
        .auth_user_by_id(&challenge.user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    if user.disabled || (state.auth_settings.require_email_verification && !user.email_verified) {
        return Err(ApiError::unauthorized());
    }
    let rate_key = digest(&format!("totp-attempts:{}", user.id));
    if state.store.is_login_locked(&rate_key).await? {
        return Err(ApiError::too_many_requests(
            "too many second-factor attempts",
        ));
    }
    if !use_factor(&state, &user.id, &request.code).await? {
        state.store.totp_reject_challenge(&token_hash).await?;
        state.store.record_login_failure(&rate_key).await?;
        return Err(ApiError::unauthorized());
    }
    if !state
        .store
        .totp_consume_challenge(&token_hash, &user.id)
        .await?
    {
        return Err(ApiError::unauthorized());
    }
    state.store.clear_login_failures(&rate_key).await?;
    let session = issue_session(&state, &user, challenge.remember_me).await?;
    audit_action(&state, Some(&user.id), Some(&user.id), "login-totp").await;
    super::send_login_notification(&state, &user);
    Ok(Json(session))
}

#[cfg(test)]
mod tests {
    use super::{decode_base32, encode_base32, normalize_factor, totp_for_step};
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn totp_login_requires_second_factor_and_recovery_is_one_use() {
        use std::path::PathBuf;
        use axum::{
            body::{Body, to_bytes},
            http::{Request, StatusCode, header},
        };
        use indexloom_storage::{SecretVault, Store};
        use serde_json::Value;
        use tower::ServiceExt;

        use crate::{AdminToken, AppState, router};
        use super::{super::{AuthMode, AuthSettings}, decode_base32, totp_for_step};

        const OPERATOR: &str = "test-operator-token-with-at-least-32-bytes";
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let vault = SecretVault::generate().unwrap();
        let mut settings = AuthSettings::legacy();
        settings.mode = AuthMode::Local;
        let state = AppState::with_auth(
            store.clone(), vault, Some(AdminToken::new(OPERATOR).unwrap()),
            settings, None,
        );
        let app = router(state, PathBuf::from("nonexistent-web-directory"));

        let bootstrap = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/bootstrap")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {OPERATOR}"))
                .body(Body::from(r#"{"username":"alice","email":"alice@example.org","password":"correct-horse-battery"}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(bootstrap.status(), StatusCode::OK);
        let reply: Value = serde_json::from_slice(
            &to_bytes(bootstrap.into_body(), 4096).await.unwrap(),
        ).unwrap();
        let first_token = reply["accessToken"].as_str().unwrap().to_owned();

        let begin = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/totp/start")
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"correct-horse-battery"}"#)).unwrap(),
        ).await.unwrap();
        assert_eq!(begin.status(), StatusCode::OK);
        let begin_data: Value = serde_json::from_slice(
            &to_bytes(begin.into_body(), 4096).await.unwrap(),
        ).unwrap();
        let secret = decode_base32(begin_data["secret"].as_str().unwrap()).unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp().div_euclid(30);
        let code = totp_for_step(&secret, now).unwrap();

        let confirm = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/totp/confirm")
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({"code": code}).to_string())).unwrap(),
        ).await.unwrap();
        assert_eq!(confirm.status(), StatusCode::OK);
        let confirmed: Value = serde_json::from_slice(
            &to_bytes(confirm.into_body(), 4096).await.unwrap(),
        ).unwrap();
        assert_eq!(confirmed["recoveryCodes"].as_array().unwrap().len(), 10);
        let backup = confirmed["recoveryCodes"][0].as_str().unwrap().to_owned();

        let revoked = app.clone().oneshot(
            Request::builder().uri("/api/v1/indexers")
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

        let login = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"login":"alice","password":"correct-horse-battery"}"#)).unwrap(),
        ).await.unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let challenge_data: Value = serde_json::from_slice(
            &to_bytes(login.into_body(), 4096).await.unwrap(),
        ).unwrap();
        assert_eq!(challenge_data["requiresTotp"], true);
        assert!(challenge_data.get("accessToken").is_none());
        let challenge = challenge_data["challengeToken"].as_str().unwrap().to_owned();

        let invalid = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/totp/complete")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({
                    "challengeToken": challenge, "code": "invalid",
                }).to_string())).unwrap(),
        ).await.unwrap();
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);

        let completed = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/totp/complete")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({
                    "challengeToken": challenge, "code": backup,
                }).to_string())).unwrap(),
        ).await.unwrap();
        assert_eq!(completed.status(), StatusCode::OK);
        let completed_data: Value = serde_json::from_slice(
            &to_bytes(completed.into_body(), 4096).await.unwrap(),
        ).unwrap();
        let session = completed_data["accessToken"].as_str().unwrap();
        assert!(session.len() >= 40);
        assert_eq!(store.totp_recovery_count("unused").await.unwrap(), 0);
        assert_eq!(store.auth_user_count().await.unwrap(), 1);
        let user = store.auth_user_by_login("alice").await.unwrap().unwrap();
        assert_eq!(store.totp_recovery_count(&user.id).await.unwrap(), 9);
        let replay = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/auth/totp/complete")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({
                    "challengeToken": challenge, "code": backup,
                }).to_string())).unwrap(),
        ).await.unwrap();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn base32_round_trip_and_rfc6238_sha256_vector() {
        let secret = b"12345678901234567890123456789012";
        assert_eq!(decode_base32(&encode_base32(secret)).unwrap(), secret);
        // RFC 6238 appendix B: SHA256, T=59, eight digits 46119246 -> six 119246.
        assert_eq!(totp_for_step(secret, 1).unwrap(), "119246");
        assert_eq!(
            normalize_factor("abcD-EFGH IJKL-MNOP").as_deref(),
            Some("ABCDEFGHIJKLMNOP")
        );
    }
}
