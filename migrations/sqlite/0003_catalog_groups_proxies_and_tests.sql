CREATE TABLE outbound_proxies (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('http', 'socks4', 'socks5')),
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    username TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE proxy_secrets (
    proxy_id TEXT PRIMARY KEY NOT NULL REFERENCES outbound_proxies(id) ON DELETE CASCADE,
    key_version INTEGER NOT NULL DEFAULT 1 CHECK (key_version > 0),
    nonce BLOB NOT NULL CHECK (length(nonce) = 24),
    ciphertext BLOB NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE indexer_groups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    priority INTEGER NOT NULL DEFAULT 0 CHECK (priority BETWEEN -10000 AND 10000),
    proxy_id TEXT REFERENCES outbound_proxies(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

ALTER TABLE indexers ADD COLUMN proxy_id TEXT
    REFERENCES outbound_proxies(id) ON DELETE SET NULL;

CREATE TABLE indexer_group_members (
    group_id TEXT NOT NULL REFERENCES indexer_groups(id) ON DELETE CASCADE,
    indexer_id TEXT NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, indexer_id)
);

CREATE TABLE indexer_tags (
    indexer_id TEXT NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    tag TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (indexer_id, tag)
);

CREATE TABLE runtime_settings (
    singleton INTEGER PRIMARY KEY NOT NULL DEFAULT 1 CHECK (singleton = 1),
    global_proxy_id TEXT REFERENCES outbound_proxies(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO runtime_settings (singleton, global_proxy_id, updated_at)
VALUES (1, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE indexer_test_runs (
    id TEXT PRIMARY KEY NOT NULL,
    indexer_id TEXT NOT NULL REFERENCES indexers(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('success', 'failed')),
    response_time_ms INTEGER,
    message TEXT NOT NULL,
    tested_at TEXT NOT NULL
);

CREATE INDEX indexers_proxy_id_idx ON indexers(proxy_id);
CREATE INDEX indexer_groups_proxy_id_idx ON indexer_groups(proxy_id);
CREATE INDEX indexer_group_members_indexer_id_idx
    ON indexer_group_members(indexer_id, group_id);
CREATE INDEX indexer_tags_tag_idx ON indexer_tags(tag, indexer_id);
CREATE INDEX indexer_test_runs_latest_idx
    ON indexer_test_runs(indexer_id, tested_at DESC);
