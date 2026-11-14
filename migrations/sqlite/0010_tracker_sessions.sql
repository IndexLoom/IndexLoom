-- Revisions fence off writes from requests using replaced credentials.
ALTER TABLE indexers ADD COLUMN session_revision INTEGER NOT NULL DEFAULT 0;

CREATE TABLE indexer_sessions (
    indexer_id TEXT PRIMARY KEY NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    key_version INTEGER NOT NULL CHECK (key_version = 1),
    expires_at INTEGER NOT NULL,
    nonce BLOB NOT NULL CHECK (length(nonce) = 24),
    ciphertext BLOB NOT NULL
);
CREATE INDEX indexer_sessions_expiry_idx ON indexer_sessions(expires_at);
