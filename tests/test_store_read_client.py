"""A query-only client can read one current plugin view without Store startup."""
import hashlib
import sqlite3
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch, ProtocolError
from observatory.store_read_client import StoreReadClient


SOURCE = "read-client-source"


def event(number, *, state="completed"):
    return {"event_id": f"event-{number}", "revision": 0, "kind": "lifecycle",
            "time_ns": str(number), "payload": {"state": state},
            "identity": {"session_id": f"session-{number}"}}


class StoreReadClientTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def source(self):
        store = Store(self.root / "store.db")
        self.addCleanup(store.close)
        lease = activate_source(store, plugin_id="fixture.plugin", source_id=SOURCE,
                                generation="a" * 64)
        return store, lease

    def read(self, path):
        client = StoreReadClient(path)
        self.addCleanup(client.close)
        return client

    def assert_parity(self, store, client):
        for limit in (1, 2, 200):
            self.assertEqual(client.plugin_sessions(limit), store.plugin_sessions(limit))

    def test_missing_path_and_unsupported_schema_do_not_initialize_store(self):
        absent = self.root / "missing" / "store.db"
        with StoreReadClient(absent) as client:
            with self.assertRaises(ProtocolError):
                client.plugin_sessions()
        self.assertFalse(absent.parent.exists())
        invalid = self.root / "unsupported.db"
        with closing(sqlite3.connect(invalid)) as db:
            db.execute("CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            db.execute("INSERT INTO schema_meta VALUES('receiver_id',?)", ("a" * 32,))
            db.execute("INSERT INTO schema_meta VALUES('plugin_sessions_read_schema','1')")
            db.commit()
        before = invalid.read_bytes()
        with StoreReadClient(invalid) as client:
            with self.assertRaisesRegex(ProtocolError, "unsupported Store read schema"):
                client.plugin_sessions()
        self.assertEqual(invalid.read_bytes(), before)

    def test_pending_projection_usage_gap_retention_and_revocation_match_store(self):
        store, lease = self.source()
        client = self.read(store.path)
        self.assert_parity(store, client)
        huge_usage = {"event_id": "usage-one", "revision": 0, "kind": "usage",
                      "time_ns": "3", "payload": {"input_tokens": 2**53 + 1},
                      "identity": {"session_id": "session-1"}}
        commit_batch(store, lease, "", PollBatch(SOURCE, "one", (event(1), event(2), huge_usage)))
        self.assert_parity(store, client)
        project_source(store, SOURCE)
        self.assert_parity(store, client)
        rows = {row["native_session_id"]: row for row in client.plugin_sessions(2)["sessions"]}
        self.assertEqual(rows["session-1"]["metadata_usage"],
                         {"input_tokens": str(2**53 + 1)})
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            commit_batch(store, lease, "one", PollBatch(SOURCE, "two", (event(3), event(4))))
            project_source(store, SOURCE)
            commit_batch(store, lease, "two", PollBatch(SOURCE, "three", (event(5),)))
        self.assert_parity(store, client)
        project_source(store, SOURCE)
        self.assert_parity(store, client)
        self.assertTrue(client.plugin_sessions()["retention_gaps"])
        commit_batch(store, lease, "three", PollBatch(SOURCE, "four", ({
            "event_id": "gap", "revision": 0, "kind": "diagnostic", "time_ns": "0",
            "payload": {"code": "possible_source_gap", "severity": "warning"}},)))
        project_source(store, SOURCE)
        self.assert_parity(store, client)
        self.assertTrue(any(row["reason_code"] == "source_gap"
                            for row in client.plugin_sessions()["sessions"]))
        set_source_capture(store, source_id=SOURCE, enabled=False)
        self.assert_parity(store, client)
        self.assertEqual(client.plugin_sessions()["sessions"], [])

    def test_read_does_not_change_main_database_or_expose_writer_api(self):
        store, lease = self.source()
        commit_batch(store, lease, "", PollBatch(SOURCE, "one", (event(1),)))
        project_source(store, SOURCE)
        path = Path(store.path)
        expected = store.plugin_sessions()
        before = (hashlib.sha256(path.read_bytes()).digest(), path.stat().st_mtime_ns,
                  {item.name for item in self.root.iterdir()})
        with patch.object(Store, "__init__", side_effect=AssertionError("writer opened")):
            with StoreReadClient(path) as client:
                self.assertFalse(hasattr(client, "commit_batch"))
                self.assertEqual(client.plugin_sessions(), expected)
        after = (hashlib.sha256(path.read_bytes()).digest(), path.stat().st_mtime_ns,
                 {item.name for item in self.root.iterdir()})
        self.assertEqual(after, before)

    def test_schema_version_and_symlink_retarget_fail_closed(self):
        store, lease = self.source()
        commit_batch(store, lease, "", PollBatch(SOURCE, "one", (event(1),)))
        project_source(store, SOURCE)
        alias = self.root / "alias.db"
        alias.symlink_to(Path(store.path).name)
        client = self.read(alias)
        self.assert_parity(store, client)
        replacement = self.root / "replacement.db"
        with store._connect() as source, closing(sqlite3.connect(replacement)) as target:
            source.backup(target)
        alias.unlink()
        alias.symlink_to(replacement.name)
        with self.assertRaisesRegex(ProtocolError, "target changed"):
            client.plugin_sessions()
        with store._connect() as db:
            db.execute("UPDATE schema_meta SET value='2' "
                       "WHERE key='plugin_sessions_read_schema'")
        with StoreReadClient(store.path) as newer:
            with self.assertRaisesRegex(ProtocolError, "schema version"):
                newer.plugin_sessions()

    def test_malformed_published_usage_fails_closed(self):
        store, lease = self.source()
        commit_batch(store, lease, "", PollBatch(SOURCE, "one", (event(1),)))
        project_source(store, SOURCE)
        with store._connect() as db:
            db.execute("UPDATE plugin_session_projection_v1 SET usage_json='{broken' "
                       "WHERE source_id=?", (SOURCE,))
        with StoreReadClient(store.path) as client:
            with self.assertRaisesRegex(ProtocolError, "unavailable"):
                client.plugin_sessions()


if __name__ == "__main__":
    unittest.main()
