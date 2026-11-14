# Stage D management (35–37)

All endpoints use the existing management authentication and no-store middleware.
Bulk writes require admin/owner; diagnostics and **every** backup endpoint require
owner (including GET/download). Legacy operator tokens retain owner access.
Deployments explicitly configured with disabled authentication retain their
existing unrestricted behavior; use local/hybrid/token auth for protected access.

## Bulk indexers

`POST /api/v1/indexers/bulk` accepts a strict camelCase object. Example:

```json
{"ids":["example-main","example-other"],"enabled":false,"tags":{"mode":"append","values":["private"]},"groupIds":["movies"]}
```

One to 500 unique lowercase kebab-case IDs are required. Unknown IDs fail the
entire operation. Changes are a single SQLite transaction, including relation
changes. Tags support `append`, `remove`, `replace`, with at most 50 unique tags
(case insensitive), 1–50 bytes each, no control characters or surrounding spaces.
At most 50 unique group IDs can replace membership. Unknown groups/proxies/
FlareSolverr instances fail via foreign keys and roll back all writes.
`proxyId` and `flaresolverrInstanceId` preserve on omission, clear on `""`, or assign
an existing identifier. `enabled` preserves on omission. `{"ids":["example-main"],
"delete":true}` deletes instances and their dependent records; delete cannot be
combined with edits. No secret decryption or configuration JSON rewrite occurs.
Success returns `{"count":2}` and clears the executor cache.

The Indexers → Bulk management panel provides filtered selection, tags, group
replacement, enable/disable, refresh, and confirmed deletion. The server enforces
limits independently of the UI. Catalog synchronization may recreate catalog
instances after deletion; disable them to retain a lasting preference.

## Runtime diagnostics

`GET /api/v1/diagnostics/logs?severity=error&component=scheduler&limit=100&before=123`
returns newest-first structured records with monotonic integer ID, UTC timestamp,
severity, component and message. `before` is an exclusive ID cursor. Limit is
1–200, default 100. Severity is `info`, `warn`, or `error`; component is an exact
match. SQLite retains at most 2,000 records via a trigger.

This is an explicit internal diagnostic sink, **not** a tracing archive and not
the activity/security journal. Only reviewed enum events are accepted: storage
startup, catalog synchronization failure, failed scheduled runtime job,
and backup snapshot failure. Raw exceptions, request bodies, URLs, usernames,
tokens and secrets never enter the sink. Ordinary tracing output remains on
stdout and may contain more detail; not every runtime failure is persisted.
Settings → Logs / Diagnostics filters, refreshes and loads older pages. Download
exports only the currently displayed records as JSON, using a fixed filename.

## Database backups and queued restore

Configure `INDEXLOOM_BACKUP_DIR` to a dedicated absolute persistent directory,
owned by the application account. It must not be a symlink or the database
directory itself. The app sets the
directory to 0700 and files to 0600 on Unix. Run one autonomous server instance
per database/backup directory. Do not share a backup directory across databases.
No filesystem paths are accepted from API clients or returned by the API.

- `GET /api/v1/backups`: opaque IDs, byte counts, capacity and queued status.
- `POST /api/v1/backups`: consistent SQLite `VACUUM INTO` snapshot, followed by
  read-only full `integrity_check`, foreign-key checks and known migration checksum
  validation before publish. Snapshots with unknown/newer migrations are refused.
- `POST /api/v1/backups/upload`: raw SQLite bytes (`application/octet-stream`),
  same integrity/schema validation. Upload does not queue a restore.
- `GET /api/v1/backups/{id}`: fixed safe attachment filename and SQLite MIME type.
- `DELETE /api/v1/backups/{id}`: deletes only that opaque UUID snapshot.
- `POST /api/v1/backups/{id}/restore` with
  `{"confirmation":"RESTORE ON RESTART"}`: verifies the source, snapshots the
  current database, atomically stages a private validated copy, then publishes a
  synced marker bound to the absolute database path. Returns `status: "queued"`,
  `restartRequired: true`, and the rollback snapshot ID. It never reports restore
  completed. One pending restore is allowed.

Retention is explicitly managed: at most 10 snapshots, each at most 256 MiB.
Capacity exhaustion refuses creation/upload/queueing instead of deleting existing
backups. Restore queueing needs a free slot for its mandatory current-state
snapshot. Files that are symlinks, nonregular or oversized are rejected. Only
canonical UUID IDs resolve; no path separators or alternate UUID forms work.

At the next autonomous server start, **before Store::connect and migrations**, the
server validates the marker and stage again. It refuses restoration if WAL or SHM
files remain, if the DB path differs, or if the stage is invalid. Stop all database
clients and checkpoint the database offline before retrying startup; never delete
an uncheckpointed WAL. The existing `tools/backup_indexloom.py` provides equivalent
manual offline restore mechanics. Startup retains a separate uniquely named
`*.before-restore-*.db` copy beside the database (including post-queue writes),
copies the validated stage to the database filesystem, syncs it, and atomically
renames it over the database. It then consumes the marker and opens the database.
Interrupted pre-rename restores leave the old database intact; a leftover
`*.restore-tmp` is deliberately not overwritten and requires operator inspection.
If a crash occurs between replacement and marker removal, restart may reapply the
same snapshot before opening the database. Keep the rollback copies until verified.

The Settings Backups panel exposes create/upload/download/delete and confirmed
queueing. Restart is an operator action, not an HTTP process-control operation.
Queued restoration replaces later writes; the UI says so explicitly. No secret
key is exported: encrypted credentials require the original master key. Metadata
and non-secret configuration in SQLite are not encrypted. Store snapshots and
master keys separately. Uploads are integrity checked, not authenticated: only
restore trusted backups from a compatible IndexLoom version.

Validation: targeted storage tests cover transaction rollback, unchanged encrypted
ciphertext/configuration, ID validation, bounded diagnostics and file snapshots.
Server tests cover role separation, traversal/symlinks and offline restore integrity,
WAL refusal, and preservation of current data. Browser keyboard/screen-reader
behavior requires manual verification; native labeled controls are used throughout.
