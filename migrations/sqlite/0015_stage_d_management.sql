CREATE TABLE diagnostic_logs (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 timestamp TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 severity TEXT NOT NULL CHECK(severity IN ('info','warn','error')),
 component TEXT NOT NULL,
 message TEXT NOT NULL
);
CREATE INDEX diagnostic_logs_filter ON diagnostic_logs(severity, component, id DESC);
CREATE TRIGGER diagnostic_logs_capacity AFTER INSERT ON diagnostic_logs BEGIN
 DELETE FROM diagnostic_logs WHERE id <= NEW.id - 2000;
END;
