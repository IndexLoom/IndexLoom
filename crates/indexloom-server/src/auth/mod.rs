//! Opt-in authentication: legacy token, Argon2id local users, verified OIDC or
//! explicitly disabled. Opaque sessions and one-time token digests live in SQLite.

#![allow(clippy::too_many_lines)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::struct_excessive_bools)]

mod mail;
mod oidc;
mod password_policy;
mod totp;

use std::{env, io::Read, path::Path as FsPath, sync::Arc};

use argon2::Argon2;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, Method, StatusCode, header},
    routing::{get, post, put},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use indexloom_storage::auth::{
    AuthAuditEvent, AuthUser, AuthUserView, MailEventSetting, MailTemplate,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::{ApiError, AppState};
pub use mail::MailSender;
pub use oidc::OidcSettings;

const SESSION_SECONDS: i64 = 12 * 3600;
const REMEMBER_SESSION_SECONDS: i64 = 7 * 24 * 3600;
const RESET_SECONDS: i64 = 30 * 60;
const VERIFY_SECONDS: i64 = 24 * 3600;

/// Reads an optional transport secret from exactly one environment or file source.
/// Large or invalid secrets fail closed before entering SMTP/OIDC configuration.
fn read_optional_secret(name: &str) -> anyhow::Result<Option<String>> {
    let direct = env::var(name).ok().filter(|value| !value.is_empty());
    let path = env::var(format!("{name}_FILE"))
        .ok()
        .filter(|value| !value.is_empty());
    match (direct, path) {
        (Some(_), Some(_)) => anyhow::bail!("set either {name} or {name}_FILE, not both"),
        (Some(value), None) => Ok(Some(value)),
        (None, None) => Ok(None),
        (None, Some(file_path)) => {
            let path = FsPath::new(&file_path);
            let file = std::fs::File::open(path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.len() > 64 * 1024 {
                anyhow::bail!("{name}_FILE must be a regular file no larger than 64 KiB");
            }
            let mut data = Vec::new();
            file.take(64 * 1024 + 1).read_to_end(&mut data)?;
            if data.len() > 64 * 1024 {
                anyhow::bail!("{name}_FILE exceeds the 64 KiB limit");
            }
            let secret = String::from_utf8(data)?;
            let secret = secret
                .strip_suffix("\r\n")
                .or_else(|| secret.strip_suffix('\n'))
                .unwrap_or(&secret);
            if secret.is_empty() {
                anyhow::bail!("{name}_FILE must not be empty");
            }
            Ok(Some(secret.to_owned()))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    Token,
    Local,
    Oidc,
    Hybrid,
    Disabled,
}

impl AuthMode {
    pub fn from_env() -> Result<Self, String> {
        let value = env::var("INDEXLOOM_AUTH_MODE").unwrap_or_else(|_| "token".to_owned());
        match value.as_str() {
            "token" => Ok(Self::Token),
            "local" => Ok(Self::Local),
            "oidc" => Ok(Self::Oidc),
            "hybrid" => Ok(Self::Hybrid),
            "disabled" => {
                if matches!(
                    env::var("INDEXLOOM_ALLOW_UNSAFE_NO_AUTH").as_deref(),
                    Ok("true")
                ) {
                    Ok(Self::Disabled)
                } else {
                    Err("disabled auth requires INDEXLOOM_ALLOW_UNSAFE_NO_AUTH=true".to_owned())
                }
            }
            _ => {
                Err("INDEXLOOM_AUTH_MODE must be token, local, oidc, hybrid or disabled".to_owned())
            }
        }
    }

    const fn local(self) -> bool {
        matches!(self, Self::Local | Self::Hybrid)
    }
    const fn oidc(self) -> bool {
        matches!(self, Self::Oidc | Self::Hybrid)
    }
    const fn legacy_token(self) -> bool {
        matches!(self, Self::Token | Self::Hybrid)
    }
}

#[derive(Clone, Debug)]
pub struct AuthSettings {
    pub mode: AuthMode,
    pub require_email_verification: bool,
    pub oidc: Option<OidcSettings>,
    pub hash_permits: Arc<Semaphore>,
}

impl AuthSettings {
    pub fn from_env(mail_enabled: bool) -> anyhow::Result<Self> {
        let mode = AuthMode::from_env().map_err(anyhow::Error::msg)?;
        let oidc = if mode.oidc() {
            OidcSettings::from_env()?
        } else {
            None
        };
        if mode.oidc() && oidc.is_none() {
            anyhow::bail!("OIDC mode requires configured OIDC issuer, client and allowlist");
        }
        if mode == AuthMode::Oidc
            && oidc
                .as_ref()
                .is_some_and(|settings| !settings.has_owner_email())
        {
            anyhow::bail!(
                "OIDC-only mode requires INDEXLOOM_OIDC_OWNER_EMAIL to designate the owner"
            );
        }
        let require_email_verification =
            env::var("INDEXLOOM_REQUIRE_VERIFIED_EMAIL").as_deref() == Ok("true");
        if require_email_verification && !mail_enabled && mode.local() {
            anyhow::bail!("email verification requires enabled SMTP");
        }
        Ok(Self {
            mode,
            oidc,
            require_email_verification,
            hash_permits: Arc::new(Semaphore::new(4)),
        })
    }

    pub fn legacy() -> Self {
        Self {
            mode: AuthMode::Token,
            require_email_verification: false,
            oidc: None,
            hash_permits: Arc::new(Semaphore::new(4)),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuthConfig {
    mode: AuthMode,
    local_enabled: bool,
    oidc_enabled: bool,
    mail_enabled: bool,
    email_verification_required: bool,
    bootstrap_available: bool,
    oidc_issuer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Credentials {
    login: String,
    password: String,
    #[serde(default)]
    remember_me: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BootstrapRequest {
    username: String,
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenRequest {
    token: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetRequest {
    token: String,
    password: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetEmailRequest {
    email: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OidcCallbackRequest {
    code: String,
    state: String,
    #[serde(default)]
    remember_me: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionView {
    access_token: String,
    expires_at: i64,
    user: AuthUserView,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum LoginReply {
    Session(SessionView),
    Challenge(totp::LoginChallenge),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentSession {
    user: Option<AuthUserView>,
    method: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MailPanel {
    enabled: bool,
    templates: Vec<MailTemplate>,
    events: Vec<MailEventSetting>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MailTemplateInput {
    subject: String,
    text_body: String,
    html_body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MailEventInput {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddUserInput {
    username: String,
    email: String,
    password: String,
    #[serde(default = "viewer_role")]
    role: String,
}

fn viewer_role() -> String {
    "viewer".to_owned()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoleInput {
    role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DisabledInput {
    disabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestMailInput {
    to: String,
}

pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/config", get(public_config))
        .route("/api/v1/auth/password-policy", get(get_password_policy))
        .route("/api/v1/auth/bootstrap", post(bootstrap))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/oidc/start", post(start_oidc))
        .route("/api/v1/auth/oidc/complete", post(complete_oidc))
        .route("/api/v1/auth/email/request-reset", post(request_reset))
        .route("/api/v1/auth/email/reset", post(reset_password))
        .route("/api/v1/auth/email/verify", post(verify_email))
        .merge(totp::public_routes())
        .layer(DefaultBodyLimit::max(16 * 1024))
}

pub fn private_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/me", get(current_session))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/password", post(change_password))
        .route(
            "/api/v1/auth/password-policy/settings",
            put(update_password_policy),
        )
        .route(
            "/api/v1/auth/email/request-verification",
            post(request_verification),
        )
        .route("/api/v1/auth/mail", get(mail_panel))
        .route("/api/v1/auth/mail/templates/{kind}", put(save_template))
        .route("/api/v1/auth/mail/events/{kind}", put(save_event))
        .route("/api/v1/auth/mail/test", post(test_email))
        .route("/api/v1/auth/users", get(list_users).post(add_user))
        .route("/api/v1/auth/users/{id}/disabled", put(disable_user))
        .route("/api/v1/auth/users/{id}/role", put(change_user_role))
        .route("/api/v1/auth/audit", get(audit_events))
        .merge(totp::private_routes())
        .route(
            "/api/v1/auth/users/{id}/verify-email",
            post(send_user_verification),
        )
}

pub fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn new_token() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| ApiError::internal("random generator unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Public, read-only policy needed for bootstrap and recovery screens.
async fn get_password_policy(
    State(state): State<AppState>,
) -> Result<Json<password_policy::PasswordPolicy>, ApiError> {
    Ok(Json(password_policy::load(&state).await?))
}

/// Only the owner (or legacy operator token) can modify password rules.
async fn update_password_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(policy): Json<password_policy::PasswordPolicy>,
) -> Result<Json<password_policy::PasswordPolicy>, ApiError> {
    policy.validate_configuration()?;
    let json = serde_json::to_string(&policy)
        .map_err(|_| ApiError::internal("password policy serialization failed"))?;
    state.store.save_password_policy(&json).await?;
    let actor = user_for_bearer(&state, &headers).await.ok().map(|u| u.id);
    audit_action(&state, actor.as_deref(), None, "password-policy-updated").await;
    Ok(Json(policy))
}

/// Validate fresh local passwords and optionally check the most recent hashes.
/// Existing passwords still authenticate after a stricter policy is enabled.
async fn validate_new_password(
    state: &AppState,
    password: &str,
    username: Option<&str>,
    email: Option<&str>,
    user_id: Option<&str>,
) -> Result<bool, ApiError> {
    let policy = password_policy::load(state).await?;
    policy.validate_password(password, username, email)?;
    if let (true, Some(id)) = (policy.history_enabled, user_id) {
        let hashes = state
            .store
            .password_hash_history(id, policy.history_count)
            .await?;
        for hash in hashes {
            if verify_password(
                password.to_owned(),
                hash,
                state.auth_settings.hash_permits.clone(),
            )
            .await
            {
                return Err(ApiError::bad_request(
                    "new password must not match any of the recently used passwords",
                ));
            }
        }
    }
    Ok(policy.history_enabled)
}

fn normalize_username(name: &str) -> Result<String, ApiError> {
    let name = name.trim().to_ascii_lowercase();
    if !(3..=48).contains(&name.len())
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(ApiError::bad_request(
            "username must use 3-48 ASCII letters, digits, _ or -",
        ));
    }
    Ok(name)
}

fn normalize_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_ascii_lowercase();
    if email.len() > 320 || email.parse::<lettre::Address>().is_err() {
        return Err(ApiError::bad_request("invalid email address"));
    }
    Ok(email)
}

async fn hash_password(password: String, permits: Arc<Semaphore>) -> Result<String, ApiError> {
    let _permit = permits
        .acquire()
        .await
        .map_err(|_| ApiError::internal("hashing service unavailable"))?;
    tokio::task::spawn_blocking(move || {
        let mut salt = [0_u8; 16];
        getrandom::fill(&mut salt)
            .map_err(|_| ApiError::internal("random generator unavailable"))?;
        let mut result = [0_u8; 32];
        Argon2::default()
            .hash_password_into(password.as_bytes(), &salt, &mut result)
            .map_err(|_| ApiError::internal("password hashing failed"))?;
        Ok(format!(
            "v1.{}.{}",
            URL_SAFE_NO_PAD.encode(salt),
            URL_SAFE_NO_PAD.encode(result)
        ))
    })
    .await
    .map_err(|_| ApiError::internal("password hashing worker failed"))?
}

async fn verify_password(password: String, encoded: String, permits: Arc<Semaphore>) -> bool {
    let Ok(_permit) = permits.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || {
        use subtle::ConstantTimeEq;
        let parts = encoded.split('.').collect::<Vec<_>>();
        if parts.len() != 3 || parts[0] != "v1" {
            return false;
        }
        let Ok(salt) = URL_SAFE_NO_PAD.decode(parts[1]) else {
            return false;
        };
        let Ok(expected) = URL_SAFE_NO_PAD.decode(parts[2]) else {
            return false;
        };
        if salt.len() != 16 || expected.len() != 32 {
            return false;
        }
        let mut result = [0_u8; 32];
        if Argon2::default()
            .hash_password_into(password.as_bytes(), &salt, &mut result)
            .is_err()
        {
            return false;
        }
        bool::from(result.as_slice().ct_eq(&expected))
    })
    .await
    .unwrap_or(false)
}

async fn issue_session(
    state: &AppState,
    user: &AuthUser,
    remember: bool,
) -> Result<SessionView, ApiError> {
    let token = new_token()?;
    let lifetime = if remember {
        REMEMBER_SESSION_SECONDS
    } else {
        SESSION_SECONDS
    };
    let expires = state
        .store
        .create_session(&digest(&token), &user.id, lifetime)
        .await?;
    Ok(SessionView {
        access_token: token,
        expires_at: expires,
        user: user.view(),
    })
}

async fn audit_action(
    state: &AppState,
    actor_id: Option<&str>,
    subject_id: Option<&str>,
    action: &str,
) {
    if let Err(error) = state.store.auth_audit(actor_id, subject_id, action).await {
        tracing::warn!(%error, %action, "security audit entry was not recorded");
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AccessLevel {
    Viewer,
    Admin,
    Owner,
}

impl AccessLevel {
    fn from_user(user: &AuthUser) -> Self {
        match user.role.as_str() {
            "owner" => Self::Owner,
            "admin" => Self::Admin,
            _ => Self::Viewer,
        }
    }
}

fn required_access(method: &Method, path: &str) -> AccessLevel {
    if path.contains("/login-challenge") {
        return AccessLevel::Admin;
    }
    if path.starts_with("/api/v1/backups")
        || path.starts_with("/api/v1/diagnostics")
        || path.starts_with("/api/v1/auth/users")
        || path.starts_with("/api/v1/auth/mail")
        || path == "/api/v1/auth/audit"
        || path == "/api/v1/auth/password-policy/settings"
    {
        return AccessLevel::Owner;
    }
    if path == "/api/v1/auth/me"
        || path == "/api/v1/auth/logout"
        || path == "/api/v1/auth/password"
        || path == "/api/v1/auth/email/request-verification"
        || path.starts_with("/api/v1/auth/totp/")
        || *method == Method::GET
        || *method == Method::HEAD
        || (*method == Method::POST && matches!(path, "/api/v1/search" | "/api/v1/search/stream"))
        || path.starts_with("/api/v1/search/presets")
    {
        return AccessLevel::Viewer;
    }
    AccessLevel::Admin
}

async fn access_level(state: &AppState, headers: &HeaderMap) -> Option<AccessLevel> {
    if state.auth_settings.mode == AuthMode::Disabled {
        return Some(AccessLevel::Owner);
    }
    if state.auth_settings.mode.legacy_token()
        && state
            .admin_token
            .as_ref()
            .is_some_and(|token| token.authorizes(headers))
    {
        return Some(AccessLevel::Owner);
    }
    if state.auth_settings.mode == AuthMode::Token {
        return None;
    }
    let token = bearer(headers)?;
    if !(40..=128).contains(&token.len()) {
        return None;
    }
    let user = state
        .store
        .auth_user_for_session(&digest(token))
        .await
        .ok()
        .flatten()?;
    if state.auth_settings.require_email_verification && !user.email_verified {
        return None;
    }
    Some(AccessLevel::from_user(&user))
}

/// Authenticates the request and enforces the role required for its action.
///
/// # Errors
/// Returns 401 when no valid credential is provided and 403 on insufficient role.
pub(super) async fn authorize_action(
    state: &AppState,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
) -> Result<(), ApiError> {
    let level = access_level(state, headers)
        .await
        .ok_or_else(ApiError::unauthorized)?;
    if level < required_access(method, path) {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

async fn public_config(State(state): State<AppState>) -> Result<Json<PublicAuthConfig>, ApiError> {
    let mode = state.auth_settings.mode;
    let users = state.store.auth_user_count().await?;
    Ok(Json(PublicAuthConfig {
        mode,
        local_enabled: mode.local(),
        oidc_enabled: mode.oidc(),
        mail_enabled: state.mail_sender.is_some(),
        email_verification_required: state.auth_settings.require_email_verification,
        bootstrap_available: users == 0 && mode.local() && state.admin_token.is_some(),
        oidc_issuer: state
            .auth_settings
            .oidc
            .as_ref()
            .map(|oidc| oidc.issuer.clone()),
    }))
}

async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BootstrapRequest>,
) -> Result<Json<SessionView>, ApiError> {
    if !state.auth_settings.mode.local()
        || !state
            .admin_token
            .as_ref()
            .is_some_and(|token| token.authorizes(&headers))
    {
        return Err(ApiError::unauthorized());
    }
    let username = normalize_username(&request.username)?;
    let email = normalize_email(&request.email)?;
    validate_new_password(
        &state,
        &request.password,
        Some(&username),
        Some(&email),
        None,
    )
    .await?;
    let password_hash =
        hash_password(request.password, state.auth_settings.hash_permits.clone()).await?;
    let id = Uuid::now_v7().to_string();
    // Bootstrap proof is the independent 256-bit operator secret: avoid mail misconfiguration lockouts.
    let inserted = state
        .store
        .bootstrap_admin(&id, &username, &email, &password_hash, true)
        .await?;
    if !inserted {
        return Err(ApiError::conflict(
            "administrator has already been initialized",
        ));
    }
    let user = state
        .store
        .auth_user_by_id(&id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    audit_action(&state, None, Some(&id), "bootstrap-owner").await;
    Ok(Json(issue_session(&state, &user, false).await?))
}

async fn login(
    State(state): State<AppState>,
    Json(request): Json<Credentials>,
) -> Result<Json<LoginReply>, ApiError> {
    if !state.auth_settings.mode.local() {
        return Err(ApiError::not_found("local login is disabled"));
    }
    let login = request.login.trim().to_ascii_lowercase();
    if login.len() > 320 || login.is_empty() || request.password.len() > 128 {
        return Err(ApiError::unauthorized());
    }
    let key = digest(&login);
    if state.store.is_login_locked(&key).await? {
        return Err(ApiError::too_many_requests(
            "too many login attempts; try again in 15 minutes",
        ));
    }
    let user = state.store.auth_user_by_login(&login).await?;
    let hash = user
        .as_ref()
        .and_then(|user| user.password_hash.clone())
        .unwrap_or_else(|| {
            "v1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned()
        });
    let valid = verify_password(
        request.password,
        hash,
        state.auth_settings.hash_permits.clone(),
    )
    .await;
    let user = match user {
        Some(user)
            if valid
                && !user.disabled
                && (!state.auth_settings.require_email_verification || user.email_verified) =>
        {
            user
        }
        _ => {
            state.store.record_login_failure(&key).await?;
            return Err(ApiError::unauthorized());
        }
    };
    state.store.clear_login_failures(&key).await?;
    if state.store.totp_enabled(&user.id).await? {
        let challenge = totp::start_login_challenge(&state, &user, request.remember_me).await?;
        audit_action(
            &state,
            Some(&user.id),
            Some(&user.id),
            "login-totp-challenge",
        )
        .await;
        return Ok(Json(LoginReply::Challenge(challenge)));
    }
    let session = issue_session(&state, &user, request.remember_me).await?;
    audit_action(&state, Some(&user.id), Some(&user.id), "login-local").await;
    send_login_notification(&state, &user);
    Ok(Json(LoginReply::Session(session)))
}

fn send_login_notification(state: &AppState, user: &AuthUser) {
    let Some(sender) = state.mail_sender.clone() else {
        return;
    };
    let store = state.store.clone();
    let username = user.username.clone();
    let email = user.email.clone();
    tokio::spawn(async move {
        if store
            .is_mail_event_enabled("login-alert")
            .await
            .unwrap_or(false)
        {
            let vars = mail::MailVariables {
                username: &username,
                email: &email,
                action_url: "",
                event: "login",
            };
            if let Err(error) = sender.send(&store, "login-alert", &email, vars).await {
                tracing::warn!(%error, "login notification failed");
            }
        }
    });
}

async fn current_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CurrentSession>, ApiError> {
    if state.auth_settings.mode == AuthMode::Disabled {
        return Ok(Json(CurrentSession {
            user: None,
            method: "disabled",
        }));
    }
    if state.auth_settings.mode.legacy_token()
        && state
            .admin_token
            .as_ref()
            .is_some_and(|token| token.authorizes(&headers))
    {
        return Ok(Json(CurrentSession {
            user: None,
            method: "legacy-token",
        }));
    }
    let user = user_for_bearer(&state, &headers).await?;
    Ok(Json(CurrentSession {
        user: Some(user.view()),
        method: "session",
    }))
}

async fn user_for_bearer(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, ApiError> {
    let token = bearer(headers).ok_or_else(ApiError::unauthorized)?;
    state
        .store
        .auth_user_for_session(&digest(token))
        .await?
        .ok_or_else(ApiError::unauthorized)
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let user_id = user_for_bearer(&state, &headers)
        .await
        .ok()
        .map(|user| user.id);
    if let Some(token) = bearer(&headers) {
        state.store.revoke_session(&digest(token)).await?;
    }
    audit_action(&state, user_id.as_deref(), user_id.as_deref(), "logout").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Credentials>,
) -> Result<StatusCode, ApiError> {
    let user = user_for_bearer(&state, &headers).await?;
    // The current password is required for changes; reset links use a separate endpoint.
    if request.login.is_empty() {
        return Err(ApiError::bad_request(
            "current password required in login field",
        ));
    }
    if !verify_password(
        request.login,
        user.password_hash.clone().unwrap_or_default(),
        state.auth_settings.hash_permits.clone(),
    )
    .await
    {
        return Err(ApiError::unauthorized());
    }
    let history_enabled = validate_new_password(
        &state,
        &request.password,
        Some(&user.username),
        Some(&user.email),
        Some(&user.id),
    )
    .await?;
    let hash = hash_password(request.password, state.auth_settings.hash_permits.clone()).await?;
    state
        .store
        .change_password_with_history(&user.id, &hash, history_enabled)
        .await?;
    audit_action(&state, Some(&user.id), Some(&user.id), "password-changed").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn request_reset(
    State(state): State<AppState>,
    Json(request): Json<ResetEmailRequest>,
) -> Result<StatusCode, ApiError> {
    if !state.auth_settings.mode.local() || state.mail_sender.is_none() {
        return Err(ApiError::not_found("password recovery email is disabled"));
    }
    // Throttle repeated recovery requests even for unknown accounts; never
    // reveal address existence through the response status.
    if let Ok(email) = normalize_email(&request.email) {
        let rate_key = digest(&format!("recovery:{email}"));
        if state.store.is_login_locked(&rate_key).await? {
            return Ok(StatusCode::ACCEPTED);
        }
        state.store.record_login_failure(&rate_key).await?;
    }
    if let Ok(email) = normalize_email(&request.email)
        && let Some(user) = state.store.auth_user_by_login(&email).await?
        && user.password_hash.is_some()
        && !user.disabled
        && state.store.is_mail_event_enabled("reset-password").await?
    {
        let sender = state
            .mail_sender
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        let token = new_token()?;
        state
            .store
            .create_one_time_token(&digest(&token), &user.id, "reset-password", RESET_SECONDS)
            .await?;
        let store = state.store.clone();
        tokio::spawn(async move {
            let action = sender.action_url("reset-password", &token);
            let vars = mail::MailVariables {
                username: &user.username,
                email: &user.email,
                action_url: &action,
                event: "reset-password",
            };
            if let Err(error) = sender
                .send(&store, "reset-password", &user.email, vars)
                .await
            {
                tracing::warn!(%error, "password reset mail delivery failed");
            }
        });
    }
    Ok(StatusCode::ACCEPTED)
}

async fn reset_password(
    State(state): State<AppState>,
    Json(request): Json<ResetRequest>,
) -> Result<StatusCode, ApiError> {
    if !state.auth_settings.mode.local() || state.mail_sender.is_none() {
        return Err(ApiError::not_found("email password recovery is disabled"));
    }
    if request.token.len() < 40 || request.token.len() > 128 {
        return Err(ApiError::unauthorized());
    }
    let token_digest = digest(&request.token);
    let user_id = state
        .store
        .password_reset_user(&token_digest)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let user = state
        .store
        .auth_user_by_id(&user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let history_enabled = validate_new_password(
        &state,
        &request.password,
        Some(&user.username),
        Some(&user.email),
        Some(&user_id),
    )
    .await?;
    let hash = hash_password(request.password, state.auth_settings.hash_permits.clone()).await?;
    let consumed = state
        .store
        .consume_one_time_token(&token_digest, "reset-password")
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    if consumed != user_id {
        return Err(ApiError::unauthorized());
    }
    state
        .store
        .change_password_with_history(&user_id, &hash, history_enabled)
        .await?;
    audit_action(&state, None, Some(&user_id), "password-reset").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn request_verification(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user = user_for_bearer(&state, &headers).await?;
    if user.email_verified {
        return Ok(StatusCode::NO_CONTENT);
    }
    let Some(sender) = state.mail_sender.clone() else {
        return Err(ApiError::not_found("email is disabled"));
    };
    if !state.store.is_mail_event_enabled("verify-email").await? {
        return Err(ApiError::not_found("email verification is disabled"));
    }
    let token = new_token()?;
    state
        .store
        .create_one_time_token(&digest(&token), &user.id, "verify-email", VERIFY_SECONDS)
        .await?;
    let action = sender.action_url("verify-email", &token);
    let vars = mail::MailVariables {
        username: &user.username,
        email: &user.email,
        action_url: &action,
        event: "verify-email",
    };
    sender
        .send(&state.store, "verify-email", &user.email, vars)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "verification email failed");
            ApiError::internal("email delivery failed")
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify_email(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<StatusCode, ApiError> {
    if request.token.len() < 40 || request.token.len() > 128 {
        return Err(ApiError::unauthorized());
    }
    let user_id = state
        .store
        .consume_one_time_token(&digest(&request.token), "verify-email")
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    state.store.verify_email(&user_id).await?;
    audit_action(&state, None, Some(&user_id), "email-verified").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_oidc(State(state): State<AppState>) -> Result<Json<oidc::StartResponse>, ApiError> {
    if !state.auth_settings.mode.oidc() {
        return Err(ApiError::not_found("OIDC is disabled"));
    }
    let settings = state
        .auth_settings
        .oidc
        .as_ref()
        .ok_or_else(ApiError::unauthorized)?;
    settings
        .begin(&state.store)
        .await
        .map(Json)
        .map_err(|error| {
            tracing::warn!(%error, "OIDC start failed");
            ApiError::internal("identity provider unavailable")
        })
}

async fn complete_oidc(
    State(state): State<AppState>,
    Json(request): Json<OidcCallbackRequest>,
) -> Result<Json<SessionView>, ApiError> {
    if !state.auth_settings.mode.oidc() {
        return Err(ApiError::not_found("OIDC is disabled"));
    }
    if request.code.is_empty()
        || request.code.len() > 4096
        || request.state.len() < 16
        || request.state.len() > 512
    {
        return Err(ApiError::unauthorized());
    }
    let settings = state
        .auth_settings
        .oidc
        .as_ref()
        .ok_or_else(ApiError::unauthorized)?;
    let identity = settings
        .finish(&state.store, &request.state, &request.code)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "OIDC login rejected");
            ApiError::unauthorized()
        })?;
    let user = if let Some(user) = state
        .store
        .auth_user_by_oidc(&identity.issuer, &identity.subject)
        .await?
    {
        if user.email != identity.email || user.disabled {
            return Err(ApiError::unauthorized());
        }
        user
    } else {
        // Never merge on matching email: that would allow an unintended provider
        // identity to take over an existing local administrator account.
        if state
            .store
            .auth_user_by_login(&identity.email)
            .await?
            .is_some()
        {
            return Err(ApiError::conflict("email belongs to a different identity"));
        }
        let id = Uuid::now_v7().to_string();
        let username = format!("oidc-{}", &id[..12]);
        state
            .store
            .add_oidc_user(
                &id,
                &username,
                &identity.email,
                &identity.issuer,
                &identity.subject,
                if settings.is_owner_email(&identity.email) {
                    "owner"
                } else {
                    "viewer"
                },
            )
            .await?;
        state
            .store
            .auth_user_by_id(&id)
            .await?
            .ok_or_else(ApiError::unauthorized)?
    };
    let session = issue_session(&state, &user, request.remember_me).await?;
    audit_action(&state, Some(&user.id), Some(&user.id), "login-oidc").await;
    send_login_notification(&state, &user);
    Ok(Json(session))
}

async fn mail_panel(State(state): State<AppState>) -> Result<Json<MailPanel>, ApiError> {
    let saved = state.store.mail_templates().await?;
    let templates = mail::KINDS
        .into_iter()
        .map(|kind| {
            saved
                .iter()
                .find(|value| value.kind == kind)
                .cloned()
                .unwrap_or_else(|| mail::default_template(kind))
        })
        .collect();
    Ok(Json(MailPanel {
        enabled: state.mail_sender.is_some(),
        templates,
        events: state.store.mail_event_settings().await?,
    }))
}

async fn save_template(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Json(request): Json<MailTemplateInput>,
) -> Result<Json<MailTemplate>, ApiError> {
    if !mail::KINDS.contains(&kind.as_str()) {
        return Err(ApiError::bad_request("unknown email template"));
    }
    if request.subject.len() > 250
        || request.subject.is_empty()
        || request.subject.chars().any(char::is_control)
        || request.text_body.len() > 20_000
        || request.html_body.len() > 50_000
    {
        return Err(ApiError::bad_request(
            "invalid email template length or subject",
        ));
    }
    let template = MailTemplate {
        kind,
        subject: request.subject,
        text_body: request.text_body,
        html_body: request.html_body,
    };
    state.store.save_mail_template(&template).await?;
    Ok(Json(template))
}

async fn save_event(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Json(input): Json<MailEventInput>,
) -> Result<StatusCode, ApiError> {
    if !["verify-email", "reset-password", "login-alert"].contains(&kind.as_str()) {
        return Err(ApiError::bad_request("unknown email event"));
    }
    state
        .store
        .set_mail_event_enabled(&kind, input.enabled)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_email(
    State(state): State<AppState>,
    Json(request): Json<TestMailInput>,
) -> Result<StatusCode, ApiError> {
    let sender = state
        .mail_sender
        .as_ref()
        .ok_or_else(|| ApiError::not_found("SMTP is disabled"))?;
    let email = normalize_email(&request.to)?;
    let vars = mail::MailVariables {
        username: "Administrator",
        email: &email,
        action_url: "",
        event: "test",
    };
    sender
        .send(&state.store, "test", &email, vars)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "email test failed");
            ApiError::internal("email delivery failed")
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// All accounts currently have administrator permissions; account creation is never public.
async fn list_users(State(state): State<AppState>) -> Result<Json<Vec<AuthUserView>>, ApiError> {
    Ok(Json(state.store.list_auth_users().await?))
}

async fn add_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AddUserInput>,
) -> Result<Json<AuthUserView>, ApiError> {
    if !state.auth_settings.mode.local() {
        return Err(ApiError::bad_request("local accounts are disabled"));
    }
    let username = normalize_username(&request.username)?;
    let email = normalize_email(&request.email)?;
    validate_new_password(
        &state,
        &request.password,
        Some(&username),
        Some(&email),
        None,
    )
    .await?;
    if !matches!(request.role.as_str(), "owner" | "admin" | "viewer") {
        return Err(ApiError::bad_request("role must be owner, admin or viewer"));
    }
    if state.store.auth_user_by_login(&username).await?.is_some()
        || state.store.auth_user_by_login(&email).await?.is_some()
    {
        return Err(ApiError::conflict(
            "username or email is already registered",
        ));
    }
    let hash = hash_password(request.password, state.auth_settings.hash_permits.clone()).await?;
    let id = Uuid::now_v7().to_string();
    let verified = !state.auth_settings.require_email_verification;
    state
        .store
        .add_local_user(&id, &username, &email, &hash, verified, &request.role)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "user creation rejected");
            ApiError::conflict("username or email is already registered")
        })?;
    let user = state
        .store
        .auth_user_by_id(&id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let actor = user_for_bearer(&state, &headers)
        .await
        .ok()
        .map(|user| user.id);
    audit_action(&state, actor.as_deref(), Some(&id), "account-created").await;
    if !verified {
        send_verification_for_user(&state, &user).await?;
    }
    Ok(Json(user.view()))
}

async fn disable_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DisabledInput>,
) -> Result<Json<AuthUserView>, ApiError> {
    if !state.store.set_user_disabled(&id, request.disabled).await? {
        return Err(ApiError::bad_request(
            "cannot disable last active administrator or unknown account",
        ));
    }
    let user = state
        .store
        .auth_user_by_id(&id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let actor = user_for_bearer(&state, &headers)
        .await
        .ok()
        .map(|user| user.id);
    audit_action(
        &state,
        actor.as_deref(),
        Some(&id),
        if request.disabled {
            "account-disabled"
        } else {
            "account-enabled"
        },
    )
    .await;
    Ok(Json(user.view()))
}

async fn change_user_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RoleInput>,
) -> Result<Json<AuthUserView>, ApiError> {
    if !matches!(input.role.as_str(), "owner" | "admin" | "viewer") {
        return Err(ApiError::bad_request("unknown account role"));
    }
    if !state.store.set_user_role(&id, &input.role).await? {
        return Err(ApiError::bad_request(
            "cannot change the last active owner or unknown account",
        ));
    }
    let actor = user_for_bearer(&state, &headers)
        .await
        .ok()
        .map(|user| user.id);
    audit_action(&state, actor.as_deref(), Some(&id), "role-changed").await;
    let user = state
        .store
        .auth_user_by_id(&id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    Ok(Json(user.view()))
}

async fn audit_events(
    State(state): State<AppState>,
) -> Result<Json<Vec<AuthAuditEvent>>, ApiError> {
    Ok(Json(state.store.auth_audit_events().await?))
}

async fn send_verification_for_user(state: &AppState, user: &AuthUser) -> Result<(), ApiError> {
    if user.email_verified {
        return Ok(());
    }
    let sender = state
        .mail_sender
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("SMTP is disabled"))?;
    let token = new_token()?;
    state
        .store
        .create_one_time_token(&digest(&token), &user.id, "verify-email", VERIFY_SECONDS)
        .await?;
    let link = sender.action_url("verify-email", &token);
    let vars = mail::MailVariables {
        username: &user.username,
        email: &user.email,
        action_url: &link,
        event: "verify-email",
    };
    sender
        .send(&state.store, "verify-email", &user.email, vars)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "account verification email failed");
            ApiError::internal("email delivery failed")
        })
}

async fn send_user_verification(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = state
        .store
        .auth_user_by_id(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("account does not exist"))?;
    send_verification_for_user(&state, &user).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A preset belongs to the stable user id, not a short-lived session token.
pub(super) async fn search_preset_owner(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, ApiError> {
    if state.auth_settings.mode == AuthMode::Disabled {
        return Ok("local".to_owned());
    }
    if state.auth_settings.mode.legacy_token()
        && state
            .admin_token
            .as_ref()
            .is_some_and(|token| token.authorizes(headers))
    {
        return Ok("operator".to_owned());
    }
    Ok(user_for_bearer(state, headers).await?.id)
}

/// Manual tracker login always requires an explicit bearer credential, including
/// when the rest of the deployment has authentication disabled. Cookie-only
/// requests cannot initiate tracker actions or retrieve private CAPTCHA images.
pub(super) async fn tracker_login_actor(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, ApiError> {
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v != "same-origin" && v != "none")
    {
        return Err(ApiError::forbidden());
    }
    let token = bearer(headers).ok_or_else(ApiError::unauthorized)?;
    if !(32..=128).contains(&token.len()) {
        return Err(ApiError::unauthorized());
    }
    if state.auth_settings.mode.legacy_token()
        && state
            .admin_token
            .as_ref()
            .is_some_and(|t| t.authorizes(headers))
    {
        return Ok(format!("operator:{}", digest(token)));
    }
    if state.auth_settings.mode == AuthMode::Disabled {
        if state
            .admin_token
            .as_ref()
            .is_some_and(|t| t.authorizes(headers))
        {
            return Ok(format!("operator:{}", digest(token)));
        }
        return Err(ApiError::unauthorized());
    }
    authorize_action(
        state,
        headers,
        &Method::POST,
        "/api/v1/indexers/login-challenge",
    )
    .await?;
    Ok(format!(
        "user:{}",
        user_for_bearer(state, headers).await?.id
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        AccessLevel, AuthMode, digest, hash_password, normalize_email, normalize_username,
        password_policy::PasswordPolicy, required_access, verify_password,
    };
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    #[test]
    fn normalizes_and_validates_account_identifiers() {
        assert_eq!(normalize_username(" Alice-1 ").unwrap(), "alice-1");
        assert!(normalize_username("not allowed").is_err());
        assert_eq!(
            normalize_email(" Alice@Example.org ").unwrap(),
            "alice@example.org"
        );
        assert!(
            PasswordPolicy::default()
                .validate_password("much-longer-password", None, None)
                .is_ok()
        );
        assert!(
            PasswordPolicy::default()
                .validate_password("short", None, None)
                .is_err()
        );
        assert_ne!(digest("a"), digest("b"));
        assert_eq!(AuthMode::Disabled, AuthMode::Disabled);
    }

    #[test]
    fn password_policy_changes_require_owner_role() {
        use axum::http::Method;
        assert_eq!(
            required_access(&Method::PUT, "/api/v1/auth/password-policy/settings"),
            AccessLevel::Owner
        );
        assert_eq!(
            required_access(&Method::POST, "/api/v1/auth/password"),
            AccessLevel::Viewer
        );
    }

    #[tokio::test]
    async fn argon2id_password_hashes_use_random_salts() {
        let permits = Arc::new(Semaphore::new(2));
        let hash1 = hash_password("correct-horse-battery".into(), permits.clone())
            .await
            .unwrap();
        let hash2 = hash_password("correct-horse-battery".into(), permits.clone())
            .await
            .unwrap();
        assert_ne!(hash1, hash2);
        assert!(
            verify_password(
                "correct-horse-battery".into(),
                hash1.clone(),
                permits.clone()
            )
            .await
        );
        assert!(!verify_password("incorrect-password".into(), hash1, permits).await);
    }
}
