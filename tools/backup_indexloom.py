#!/usr/bin/env python3
"""Consistent, verified, offline-manageable SQLite backup/restore for IndexLoom.

Run on a host with Python 3.9+, not necessarily inside the minimal Rust container.
Never includes INDEXLOOM_MASTER_KEY; store the key separately and securely.
"""
import argparse
from contextlib import closing
import os
from pathlib import Path
import shutil
import sqlite3
import sys
import tempfile


def check_integrity(path: Path) -> None:
    # Use immutable mode only for complete offline snapshot files.
    with closing(sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)) as conn:
        result = conn.execute("PRAGMA integrity_check").fetchone()
        if result is None or result[0] != "ok":
            raise RuntimeError(f"SQLite integrity check failed: {result}")


def snapshot(database: Path, destination: Path) -> None:
    if not database.is_file() or database.is_symlink():
        raise ValueError("Source must be an existing regular SQLite database")
    if destination.exists():
        raise FileExistsError(f"Refusing to overwrite backup: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(prefix=".indexloom-backup-", suffix=".db", dir=destination.parent, delete=False) as tmp:
        stage = Path(tmp.name)
    try:
        stage.chmod(0o600)
        with closing(sqlite3.connect(f"{database.resolve().as_uri()}?mode=ro", uri=True)) as src, closing(sqlite3.connect(stage)) as dst:
            src.backup(dst, pages=256, sleep=0.1)
        check_integrity(stage)
        if destination.exists():
            raise FileExistsError("Backup destination appeared during write")
        os.replace(stage, destination)
    finally:
        stage.unlink(missing_ok=True)


def restore(snapshot_file: Path, database: Path, confirmed_stopped: bool) -> None:
    if not confirmed_stopped:
        raise ValueError("Stop IndexLoom first, then pass --service-stopped to confirm")
    if not snapshot_file.is_file() or snapshot_file.is_symlink():
        raise ValueError("Backup must be a regular SQLite file")
    if database.is_symlink():
        raise ValueError("Refusing to restore through a symlink")
    if any(Path(str(database) + suffix).exists() for suffix in ("-wal", "-shm")):
        raise ValueError("SQLite WAL/SHM files exist. Stop service, checkpoint and remove them safely before restoring.")
    check_integrity(snapshot_file)
    database.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(prefix=".indexloom-restore-", suffix=".db", dir=database.parent, delete=False) as tmp:
        stage = Path(tmp.name)
    try:
        shutil.copyfile(snapshot_file, stage)
        stage.chmod(0o600)
        check_integrity(stage)
        if database.exists():
            preserved = database.with_name(database.name + ".before-restore")
            if preserved.exists():
                raise FileExistsError(f"Move existing rollback copy first: {preserved}")
            os.replace(database, preserved)
        os.replace(stage, database)
    finally:
        stage.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    for action in ("backup", "restore"):
        command = commands.add_parser(action)
        command.add_argument("--database", required=True, type=Path, help="indexloom.db path")
        command.add_argument("--backup", required=True, type=Path, help="backup .db path")
        if action == "restore":
            command.add_argument("--service-stopped", action="store_true", help="Confirm the server is stopped")
    args = parser.parse_args()
    try:
        if args.action == "backup":
            snapshot(args.database, args.backup)
            print(f"Verified backup saved: {args.backup}")
        else:
            restore(args.backup, args.database, args.service_stopped)
            print(f"Verified restored database: {args.database}")
            print("Original database retained at: " + str(args.database) + ".before-restore")
    except (OSError, ValueError, sqlite3.Error, RuntimeError) as exc:
        print(f"Backup/restore failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
