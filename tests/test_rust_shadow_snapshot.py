"""A live Store read snapshot can be compared with the pure Rust reducer."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch, ProtocolError
from observatory.rust_shadow_snapshot import capture, current


def fact(event, kind, payload, session="session-one", **identity):
    return {"event_id": event, "revision": 0, "kind": kind, "time_ns": "100",
            "payload": payload,
            "identity": {"session_id": session, **identity}}


class ShadowSnapshotTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.store = Store(str(Path(temp.name) / "receiver.db"))
        self.addCleanup(self.store.close)
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="shadow-source", generation="a" * 64)

    def populate(self):
        records = [
            fact("parent", "lifecycle", {"state": "completed"}, session="parent"),
            fact("child", "lifecycle", {"state": "started"}),
            fact("relation", "relationship",
                 {"parent_session_id": "parent", "kind": "delegated"}),
            fact("usage", "usage", {"input_tokens": 12}),
            fact("message", "message", {"role": "user"}, turn_id="turn-one"),
            fact("tool", "tool", {"category": "shell", "result": "succeeded"},
                 turn_id="turn-one", operation_id="op-one"),
            fact("check", "check", {"category": "test", "result": "passed"},
                 turn_id="turn-one"),
        ]
        commit_batch(self.store, self.lease, "",
                     PollBatch("shadow-source", "one", tuple(records)))
        project_source(self.store, "shadow-source")

    def test_snapshot_is_content_free_and_fenced_after_commit_or_revocation(self):
        self.populate()
        snapshot = capture(self.store, "shadow-source")
        self.assertIsNotNone(snapshot)
        self.assertTrue(current(self.store, snapshot))
        self.assertEqual(len(snapshot.expected["sessions"]), 2)
        self.assertEqual(len(snapshot.expected["activities"]), 3)
        self.assertNotIn("private prompt", json.dumps(snapshot.fixture))
        child = next(row for row in snapshot.expected["sessions"]
                     if row["native_session_id"] == "session-one")
        self.assertEqual(child["relationship_state"], "confirmed")
        commit_batch(self.store, self.lease, "one", PollBatch("shadow-source", "two", (
            fact("next", "lifecycle", {"state": "completed"}),)))
        self.assertFalse(current(self.store, snapshot))
        self.assertIsNone(capture(self.store, "shadow-source"))
        project_source(self.store, "shadow-source")
        newer = capture(self.store, "shadow-source")
        self.assertTrue(current(self.store, newer))
        set_source_capture(self.store, source_id="shadow-source", enabled=False)
        self.assertFalse(current(self.store, newer))
        self.assertIsNone(capture(self.store, "shadow-source"))

    def test_corrupt_stored_metadata_fails_before_export(self):
        self.populate()
        with self.store._connect() as db:
            db.execute("UPDATE plugin_evidence_v1 SET payload_json=? WHERE kind='message'",
                       ('{"role":"user","prompt":"private prompt"}',))
        with self.assertRaisesRegex(ProtocolError, "invalid content-free"):
            capture(self.store, "shadow-source")

    def test_corrupt_published_activity_cannot_become_oracle(self):
        self.populate()
        with self.store._connect() as db:
            row = db.execute("SELECT event_key,payload FROM activity_index_v1 "
                             "WHERE kind='plugin' LIMIT 1").fetchone()
            payload = json.loads(row["payload"])
            payload["subject"] = "private prompt"
            db.execute("UPDATE activity_index_v1 SET payload=? WHERE event_key=?",
                       (json.dumps(payload), row["event_key"]))
        with self.assertRaisesRegex(ProtocolError, "differs from admitted metadata"):
            capture(self.store, "shadow-source")

    def test_unknown_source_refuses_snapshot(self):
        with self.assertRaisesRegex(ProtocolError, "unknown plugin source"):
            capture(self.store, "missing")

    def test_source_gap_is_not_misreported_as_rust_reducer_parity(self):
        self.populate()
        commit_batch(self.store, self.lease, "one", PollBatch("shadow-source", "two", (
            {"event_id": "gap", "revision": 0, "kind": "diagnostic",
             "time_ns": "0", "payload": {"code": "possible_source_gap",
                                           "severity": "warning"}},)))
        project_source(self.store, "shadow-source")
        self.assertIsNone(capture(self.store, "shadow-source"))

    def test_installed_rust_reducer_matches_policy_current_store(self):
        binary = os.environ.get("AGENTURM_RUST_REDUCER_BIN")
        if not binary:
            self.skipTest("Rust reducer built in Rust CI job")
        self.populate()
        snapshot = capture(self.store, "shadow-source")
        self.assertTrue(current(self.store, snapshot))
        result = subprocess.run([str(Path(binary).resolve())],
                                input=json.dumps(snapshot.fixture), text=True,
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(current(self.store, snapshot))
        self.assertEqual(json.loads(result.stdout), snapshot.expected)


if __name__ == "__main__":
    unittest.main()
