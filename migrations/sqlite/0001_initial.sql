CREATE TABLE indexer_definitions (
    id TEXT PRIMARY KEY NOT NULL,
    source TEXT NOT NULL,
    source_revision TEXT,
    schema_version TEXT NOT NULL,
    content_yaml TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    imported_at TEXT NOT NULL
);

CREATE TABLE indexers (
    id TEXT PRIMARY KEY NOT NULL,
    definition_id TEXT NOT NULL REFERENCES indexer_definitions(id),
    display_name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    configuration_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE search_runs (
    id TEXT PRIMARY KEY NOT NULL,
    query_json TEXT NOT NULL,
    status TEXT NOT NULL,
    result_count INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    error TEXT
);

CREATE INDEX search_runs_started_at_idx ON search_runs(started_at DESC);
