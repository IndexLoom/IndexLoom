-- Authentication is opt-in by policy; existing token-only deployments are unchanged.
-- Store only hashes of bearer, reset, verification and OIDC state secrets.
CREATE TABLE auth_users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
    email TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT,
    oidc_issuer TEXT,
    oidc_subject TEXT,
    email_verified INTEGER NOT NULL DEFAULT 0 CHECK (email_verified IN (0, 1)),
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (password_hash IS NOT NULL OR (oidc_issuer IS NOT NULL AND oidc_subject IS NOT NULL)),
    CHECK ((oidc_issuer IS NULL) = (oidc_subject IS NULL))
);
CREATE UNIQUE INDEX auth_users_oidc_idx
    ON auth_users(oidc_issuer, oidc_subject)
    WHERE oidc_issuer IS NOT NULL;

CREATE TABLE auth_sessions (
    token_sha256 TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX auth_sessions_expiration_idx ON auth_sessions(expires_at);
CREATE INDEX auth_sessions_user_idx ON auth_sessions(user_id);

CREATE TABLE auth_one_time_tokens (
    token_sha256 TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose IN ('verify-email', 'reset-password')),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX auth_one_time_expiration_idx ON auth_one_time_tokens(expires_at);

CREATE TABLE auth_oidc_flows (
    state_sha256 TEXT PRIMARY KEY NOT NULL,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX auth_oidc_expiration_idx ON auth_oidc_flows(expires_at);

CREATE TABLE auth_rate_limits (
    identity_sha256 TEXT PRIMARY KEY NOT NULL,
    failures INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE TABLE mail_templates (
    kind TEXT PRIMARY KEY NOT NULL
        CHECK (kind IN ('verify-email', 'reset-password', 'login-alert', 'test')),
    subject TEXT NOT NULL,
    text_body TEXT NOT NULL,
    html_body TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);

CREATE TABLE mail_event_settings (
    kind TEXT PRIMARY KEY NOT NULL
        CHECK (kind IN ('verify-email', 'reset-password', 'login-alert')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);
INSERT INTO mail_event_settings(kind, enabled)
VALUES ('verify-email', 1), ('reset-password', 1), ('login-alert', 0);
