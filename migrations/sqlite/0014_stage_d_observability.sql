-- Durable counters are independent of bounded audit retention.
CREATE TABLE indexer_monitoring (
 indexer_id TEXT PRIMARY KEY REFERENCES indexers(id) ON DELETE CASCADE,
 searches INTEGER NOT NULL DEFAULT 0, downloads INTEGER NOT NULL DEFAULT 0,
 failed_queries INTEGER NOT NULL DEFAULT 0, auth_errors INTEGER NOT NULL DEFAULT 0,
 rss INTEGER NOT NULL DEFAULT 0, response_count INTEGER NOT NULL DEFAULT 0,
 response_sum_ms INTEGER NOT NULL DEFAULT 0, results INTEGER NOT NULL DEFAULT 0,
 consecutive_failures INTEGER NOT NULL DEFAULT 0, open_until INTEGER NOT NULL DEFAULT 0,
 probe_until INTEGER NOT NULL DEFAULT 0, failure_class TEXT,
 last_activity INTEGER, last_success INTEGER
);
CREATE TABLE monitoring_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 timestamp INTEGER NOT NULL, category TEXT NOT NULL,
 indexer_id TEXT REFERENCES indexers(id) ON DELETE SET NULL,
 status TEXT NOT NULL, message TEXT NOT NULL, count INTEGER NOT NULL CHECK(count >= 0)
);
CREATE INDEX monitoring_events_filter ON monitoring_events(indexer_id, id DESC);
CREATE INDEX monitoring_events_time ON monitoring_events(timestamp);
-- Also enforce the bound for manual resets and future insertion paths.
CREATE TRIGGER monitoring_events_retention AFTER INSERT ON monitoring_events
BEGIN
 DELETE FROM monitoring_events WHERE timestamp < unixepoch()-2592000
 OR id <= COALESCE((SELECT id FROM monitoring_events ORDER BY id DESC LIMIT 1 OFFSET 10000),0);
END;
-- Global authentication accounting must outlive bounded event retention too.
CREATE TABLE monitoring_global (
 category TEXT PRIMARY KEY,
 operations INTEGER NOT NULL DEFAULT 0,
 failures INTEGER NOT NULL DEFAULT 0,
 response_sum_ms INTEGER NOT NULL DEFAULT 0
);
