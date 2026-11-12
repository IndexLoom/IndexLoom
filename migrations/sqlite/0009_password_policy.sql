-- Password policy applies to NEW local passwords, never to existing logins,
-- OIDC identities or operator bearer tokens. A missing row means defaults.
CREATE TABLE auth_password_policy (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    policy_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Only Argon2id password hashes are stored; plaintext is never persisted.
-- Old hashes support optional reuse prevention. Retain the most recent ten.
CREATE TABLE auth_password_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    password_hash TEXT NOT NULL,
    changed_at INTEGER NOT NULL
);
CREATE INDEX auth_password_history_user_idx ON auth_password_history(user_id, id DESC);
