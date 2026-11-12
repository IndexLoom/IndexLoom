"""Regression tests for SQLite snapshot, restore and safety boundaries.

Run: python3 -m unittest discover -s tests -p 'test_backup.py'
"""
import sqlite3
import tempfile
import unittest
from pathlib import Path

from tools.backup_indexloom import check_integrity, restore, snapshot


class BackupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.db = self.root / "indexloom.db"
        self.out = self.root / "snapshot.db"

    def make_database(self):
        connection = sqlite3.connect(self.db)
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("CREATE TABLE search_history (query TEXT)")
        connection.execute("INSERT INTO search_history VALUES (?)", ("千と千尋 & <anime>",))
        connection.commit()
        return connection

    def test_snapshot_reads_committed_wal_data_without_overwriting(self):
        conn = self.make_database()
        self.addCleanup(conn.close)
        snapshot(self.db, self.out)
        check_integrity(self.out)
        with sqlite3.connect(self.out) as backup:
            rows = backup.execute("SELECT query FROM search_history").fetchall()
        self.assertEqual(rows, [("千と千尋 & <anime>",)])
        self.assertEqual(self.out.stat().st_mode & 0o777, 0o600)
        with self.assertRaises(FileExistsError):
            snapshot(self.db, self.out)

    def test_restore_requires_stopped_server_and_preserves_prior_database(self):
        conn = self.make_database()
        snapshot(self.db, self.out)
        conn.close()
        active = sqlite3.connect(self.db)
        active.execute("DELETE FROM search_history")
        active.commit()
        active.close()
        with self.assertRaisesRegex(ValueError, "Stop IndexLoom"):
            restore(self.out, self.db, False)
        restore(self.out, self.db, True)
        with sqlite3.connect(self.db) as new:
            self.assertEqual(new.execute("SELECT COUNT(*) FROM search_history").fetchone()[0], 1)
        previous = self.root / "indexloom.db.before-restore"
        self.assertTrue(previous.exists())
        with sqlite3.connect(previous) as old:
            self.assertEqual(old.execute("SELECT COUNT(*) FROM search_history").fetchone()[0], 0)
        with self.assertRaises(FileExistsError):
            restore(self.out, self.db, True)

    def test_restore_refuses_pending_wal_sidecars(self):
        conn = self.make_database()
        self.addCleanup(conn.close)
        snapshot(self.db, self.out)
        wal = Path(str(self.db) + "-wal")
        self.assertTrue(wal.exists())
        with self.assertRaisesRegex(ValueError, "WAL/SHM"):
            restore(self.out, self.db, True)

    def test_corrupted_backup_is_rejected_before_replacing_original(self):
        conn = self.make_database()
        conn.close()
        self.out.write_bytes(b"not a SQLite file")
        with self.assertRaises(sqlite3.DatabaseError):
            restore(self.out, self.db, True)
        with sqlite3.connect(self.db) as current:
            self.assertEqual(current.execute("SELECT COUNT(*) FROM search_history").fetchone()[0], 1)
        self.assertFalse((self.root / "indexloom.db.before-restore").exists())

    def test_symlinks_are_rejected_for_database_source(self):
        conn = self.make_database()
        conn.close()
        alias = self.root / "alias.db"
        alias.symlink_to(self.db)
        with self.assertRaises(ValueError):
            snapshot(alias, self.out)


if __name__ == "__main__":
    unittest.main()
