-- Stage C: integration configuration and scoped encrypted credentials.
CREATE TABLE IF NOT EXISTS integrations (
    id TEXT PRIMARY KEY NOT NULL,
    provider TEXT NOT NULL,
    name TEXT NOT NULL,
    origin TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL DEFAULT '{}',
    secret_nonce BLOB,
    secret_ciphertext BLOB,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS integration_sync_profiles (
    id TEXT NOT NULL,
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    profile_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (integration_id, id)
);
CREATE TABLE IF NOT EXISTS integration_managed_indexers (
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    local_indexer_id TEXT NOT NULL,
    remote_indexer_id INTEGER NOT NULL,
    PRIMARY KEY (integration_id, local_indexer_id),
    UNIQUE (integration_id, remote_indexer_id)
);
CREATE TABLE IF NOT EXISTS integration_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    operation TEXT NOT NULL,
    outcome TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS integration_events_integration ON integration_events(integration_id, id);

-- UTC fixed-hour/day budgets, reserved atomically before upstream requests.
CREATE TABLE indexer_search_quotas (
    indexer_id TEXT PRIMARY KEY REFERENCES indexers(id) ON DELETE CASCADE,
    hourly_limit INTEGER NOT NULL CHECK(hourly_limit >= 0),
    daily_limit INTEGER NOT NULL CHECK(daily_limit >= 0),
    hour_bucket INTEGER NOT NULL DEFAULT 0,
    day_bucket INTEGER NOT NULL DEFAULT 0,
    hour_used INTEGER NOT NULL DEFAULT 0,
    day_used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE scheduled_jobs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('app-sync','rss','maintenance','catalog')),
    target TEXT,
    interval_seconds INTEGER NOT NULL CHECK(interval_seconds BETWEEN 60 AND 2592000),
    enabled INTEGER NOT NULL DEFAULT 1,
    next_run INTEGER NOT NULL,
    lease_until INTEGER NOT NULL DEFAULT 0,
    lease_token TEXT,
    last_run INTEGER,
    last_outcome TEXT
);
CREATE TABLE rss_scan_state (
    indexer_id TEXT PRIMARY KEY REFERENCES indexers(id) ON DELETE CASCADE,
    baseline_at INTEGER NOT NULL,
    last_scan INTEGER NOT NULL
);
CREATE TABLE rss_seen (
    indexer_id TEXT NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    identity TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    PRIMARY KEY(indexer_id,identity)
);
CREATE TABLE rss_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    indexer_id TEXT NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    identity TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(indexer_id,identity)
);

-- Named-provider ownership is separate from the existing *arr numeric mapping.
CREATE TABLE integration_managed_provider_names (
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    local_indexer_id TEXT NOT NULL,
    provider_type TEXT NOT NULL CHECK(provider_type IN ('torznab','newznab')),
    remote_name TEXT NOT NULL,
    remote_id INTEGER CHECK(remote_id > 0),
    PRIMARY KEY(integration_id,local_indexer_id),
    UNIQUE(integration_id,remote_name),
    UNIQUE(integration_id,provider_type,remote_id)
);
