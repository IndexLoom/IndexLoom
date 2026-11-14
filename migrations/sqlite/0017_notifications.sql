-- Public configuration and encrypted credentials are deliberately separate.
CREATE TABLE notification_channels (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
    config TEXT NOT NULL CHECK(json_valid(config)),
    events TEXT NOT NULL CHECK(json_valid(events)),
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    last_test_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE notification_templates (
    scope TEXT NOT NULL DEFAULT '', -- empty = global; otherwise channel ID
    event TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(scope,event)
);
CREATE TABLE notification_deliveries (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL REFERENCES notification_channels(id) ON DELETE CASCADE,
    event TEXT NOT NULL,
    event_key TEXT NOT NULL,
    channel_revision INTEGER NOT NULL,
    payload TEXT NOT NULL CHECK(json_valid(payload)),
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','sending','retry','sent','failed','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt INTEGER NOT NULL DEFAULT (unixepoch()),
    lease TEXT,
    lease_until INTEGER,
    summary TEXT,
    duration_ms INTEGER,
    is_test INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(channel_id,event_key)
);
CREATE INDEX notification_due ON notification_deliveries(status,next_attempt,lease_until);
CREATE INDEX notification_log ON notification_deliveries(created_at DESC,id DESC);
CREATE INDEX notification_channel_log ON notification_deliveries(channel_id,created_at DESC);
