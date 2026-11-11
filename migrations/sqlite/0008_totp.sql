-- RFC 6238 TOTP factor for local logins. Secrets encrypted with master key +
-- account-scoped associated data; only their SHA256 digests are kept for recovery.
CREATE TABLE auth_totp_pending (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE TABLE auth_totp_credentials (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    last_used_step INTEGER NOT NULL DEFAULT -1,
    created_at INTEGER NOT NULL
);

CREATE TABLE auth_totp_recovery (
    code_sha256 TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
CREATE INDEX auth_totp_recovery_user_idx ON auth_totp_recovery(user_id);

CREATE TABLE auth_totp_challenges (
    token_sha256 TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    remember_me INTEGER NOT NULL DEFAULT 0 CHECK (remember_me IN (0,1)),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX auth_totp_challenges_expire_idx ON auth_totp_challenges(expires_at);
