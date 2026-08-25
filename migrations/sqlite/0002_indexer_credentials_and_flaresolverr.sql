CREATE TABLE flaresolverr_instances (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    endpoint TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    timeout_seconds INTEGER NOT NULL DEFAULT 60 CHECK (timeout_seconds BETWEEN 5 AND 300),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

ALTER TABLE indexers ADD COLUMN user_agent TEXT;
ALTER TABLE indexers ADD COLUMN flaresolverr_instance_id TEXT
    REFERENCES flaresolverr_instances(id) ON DELETE SET NULL;

CREATE TABLE indexer_secrets (
    indexer_id TEXT PRIMARY KEY NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    key_version INTEGER NOT NULL DEFAULT 1 CHECK (key_version > 0),
    nonce BLOB NOT NULL CHECK (length(nonce) = 24),
    ciphertext BLOB NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX indexers_flaresolverr_instance_id_idx
    ON indexers(flaresolverr_instance_id);
