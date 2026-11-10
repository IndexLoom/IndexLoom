-- Retain the last verified release digest, so periodic polls do not re-import an
-- unchanged release. This never stores secrets or upstream credentials.
CREATE TABLE catalog_release_state (
    source_url TEXT PRIMARY KEY NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
