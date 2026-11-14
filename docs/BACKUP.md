# SQLite backups and restores

IndexLoom SQLite uses WAL. Never copy a live `indexloom.db` file directly and assume
it is a complete backup: use SQLite's online backup API instead. The optional
standard-library Python tool creates a consistent point-in-time database snapshot,
runs `PRAGMA integrity_check`, and refuses to overwrite an existing backup.

Run on a host with Python 3.9+ and access to the database volume:

```sh
python3 tools/backup_indexloom.py backup --database /path/to/indexloom.db --backup /secure/backups/indexloom-2026-11-12.db
```

**The encrypted tracker and proxy credentials require the original
`INDEXLOOM_MASTER_KEY` to be recovered.** A database backup alone is insufficient.
Back up your master key separately in a secure secret store. This utility never
exports the master key, OIDC secrets or SMTP environment credentials.

## Restore

1. Stop the IndexLoom server/container and ensure no process has the SQLite database open.
2. Take an additional current-state backup before attempting restore.
3. Checkpoint/remove WAL and SHM files using SQLite's documented offline procedure.
   The tool *refuses to restore* if `-wal` or `-shm` files exist, rather than silently
   discarding uncheckpointed transactions.
4. Restore with `--service-stopped` confirmation:

```sh
python3 tools/backup_indexloom.py restore --database /path/to/indexloom.db --backup /secure/backups/indexloom-2026-11-12.db --service-stopped
```

The previous database is moved to `indexloom.db.before-restore` when present, and
the source backup is left untouched. Verify the restored app, then remove the rollback
file intentionally. Don't run the restore on a live database.

## Limitations

- A backup is local data, not a signed release or multi-node database replication.
- Schema migrations may make an older snapshot incompatible with newer builds;
  restore using a compatible application release and then migrate forward.
- Backup encryption depends on filesystem security; the database contains metadata
  and search settings in addition to encrypted credentials.
- Schedule backups using your system's scheduler; this CLI does not silently perform
  unattended restores or manage retention.

## In-app management

Settings → Backups supports owner-only verified online snapshots, uploads,
downloads, deletion and explicitly queued restore on restart. See
[Stage D management](STAGE_D_MANAGEMENT.md#database-backups-and-queued-restore)
for directory configuration, limits, endpoint contract and offline restart rules.
