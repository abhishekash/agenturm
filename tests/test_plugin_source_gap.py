"""A process source gap must not leave prior terminal evidence authoritative."""
import json
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.request import urlopen

from observatory.core import Store
from observatory.plugin_admission import (activate_source, commit_batch, initialize,
                                          set_source_capture)
from observatory.plugin_projection import project_source, sessions
from observatory.plugin_wire import PollBatch, ProtocolError


def fact(event, session, kind, payload, stamp="100"):
    return {"event_id": event, "revision": 0, "kind": kind,
            "time_ns": stamp, "identity": {"session_id": session},
            "payload": payload}


def gap(event="gap"):
    return {"event_id": event, "revision": 0, "kind": "diagnostic",
            "time_ns": "0", "payload": {"code": "possible_source_gap",
                                           "severity": "warning"}}


class SourceGapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "receiver.db"
        self.store = Store(self.path)
        self.addCleanup(self.store.close)
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="fixture-source", generation="a" * 64)

    def commit(self, expected, cursor, records):
        return commit_batch(self.store, self.lease, expected,
                            PollBatch("fixture-source", cursor, tuple(records)))

    def by_native(self):
        project_source(self.store, "fixture-source")
        return {row["native_session_id"]: row for row in sessions(self.store)["sessions"]}

    def test_gap_invalidates_prior_completed_usage_relation_and_activity_but_not_new_session(self):
        self.commit("", "one", [
            fact("parent", "parent", "lifecycle", {"state": "completed"}),
            fact("old", "old", "lifecycle", {"state": "completed"}),
            fact("usage", "old", "usage", {"input_tokens": 10}),
            fact("relation", "old", "relationship",
                 {"parent_session_id": "parent", "kind": "delegated"}),
            fact("message", "old", "message", {"role": "assistant"})])
        initial = self.by_native()
        self.assertEqual(initial["old"]["run_state"], "completed")
        self.assertEqual(initial["old"]["relationship_state"], "confirmed")
        self.assertEqual(initial["old"]["metadata_usage"], {"input_tokens": 10})
        self.assertEqual(len(self.store.activity_page()["activity"]), 1)
        replacement = [gap(), fact("new", "new", "lifecycle", {"state": "completed"}),
                       fact("new-message", "new", "message", {"role": "assistant"})]
        self.commit("one", "two", replacement)
        # Policy-current publication hides the old projection while it catches up.
        self.assertEqual(sessions(self.store)["sessions"], [])
        after = self.by_native()
        self.assertEqual((after["old"]["run_state"], after["old"]["reason_code"],
                          after["old"]["metadata_usage"], after["old"]["relationship_state"],
                          after["old"]["parent_agent_id"]),
                         ("unknown", "source_gap", {}, "unresolved", None))
        self.assertEqual((after["parent"]["run_state"], after["parent"]["reason_code"]),
                         ("unknown", "source_gap"))
        self.assertEqual(after["new"]["run_state"], "completed")
        self.assertEqual([item["session_id"] for item in self.store.activity_page()["activity"]],
                         [after["new"]["session_id"]])
        with self.store._connect() as db:
            before = db.execute("SELECT COUNT(*) FROM plugin_source_gap_invalidated_v1").fetchone()[0]
        self.assertEqual(self.commit("one", "two", replacement), "two")
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_source_gap_invalidated_v1")
                             .fetchone()[0], before)
        self.commit("two", "three", [fact("old-late", "old", "lifecycle",
                                          {"state": "completed"}, "300")])
        self.assertEqual(self.by_native()["old"]["reason_code"], "source_gap")

    def test_gap_after_preceding_same_batch_evidence_and_second_gap_are_durable(self):
        self.commit("", "one", [fact("old", "old", "lifecycle", {"state": "completed"}),
                                 gap(), fact("new", "new", "lifecycle", {"state": "completed"})])
        rows = self.by_native()
        self.assertEqual(rows["old"]["reason_code"], "source_gap")
        self.assertEqual(rows["new"]["run_state"], "completed")
        self.commit("one", "two", [gap("second-gap")])
        rows = self.by_native()
        self.assertEqual(rows["new"]["reason_code"], "source_gap")
        reopened = Store(self.path)
        self.addCleanup(reopened.close)
        self.assertEqual({row["native_session_id"]: row["reason_code"]
                          for row in sessions(reopened)["sessions"]},
                         {"old": "source_gap", "new": "source_gap"})

    def test_gap_capacity_failure_rolls_back_cursor_and_revocation_erases_invalidation(self):
        self.commit("", "one", [fact("a", "a", "lifecycle", {"state": "completed"}),
                                 fact("b", "b", "lifecycle", {"state": "completed"})])
        with patch("observatory.plugin_source_gap.MAX_INVALIDATED_SESSIONS", 1):
            with self.assertRaisesRegex(ProtocolError, "gap invalidation capacity"):
                self.commit("one", "bad", [gap()])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "one")
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_source_gap_invalidated_v1")
                             .fetchone()[0], 0)
        self.commit("one", "two", [gap()])
        self.assertEqual(self.by_native()["a"]["reason_code"], "source_gap")
        set_source_capture(self.store, source_id="fixture-source", enabled=False)
        self.assertEqual(sessions(self.store)["sessions"], [])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_source_gap_invalidated_v1")
                             .fetchone()[0], 0)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            self.commit("two", "three", [fact("new", "new", "lifecycle", {"state": "completed"})])

    def test_legacy_gap_migration_quarantines_source_without_guessing_membership(self):
        self.commit("", "one", [fact("old", "old", "lifecycle", {"state": "completed"}), gap(),
                                 fact("later", "later", "lifecycle", {"state": "completed"})])
        with self.store._connect() as db:
            db.execute("DROP TABLE plugin_source_gap_invalidated_v1")
            db.execute("DELETE FROM schema_meta WHERE key='plugin_source_gap_v1'")
            initialize(db)
            self.assertEqual(db.execute("SELECT native_session_id FROM plugin_source_gap_invalidated_v1")
                             .fetchone()[0], "*")
        after = self.by_native()
        self.assertEqual({row["reason_code"] for row in after.values()}, {"source_gap"})

    def test_gap_is_admitted_after_retention_horizon_without_reviving_old_evidence(self):
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.commit("", "one", [fact(f"e-{n}", f"session-{n}", "lifecycle",
                                          {"state": "completed"}, str(n))
                                    for n in range(1, 5)])
            self.commit("one", "two", [fact("e-5", "session-5", "lifecycle",
                                            {"state": "completed"}, "5")])
            self.assertTrue(sessions(self.store)["retention_gaps"])
            # Gap occurrence timestamps can be zero. It is a fresh source
            # diagnostic, not an old source event escaping the horizon.
            self.commit("two", "three", [gap()])
        after = self.by_native()
        self.assertEqual(after["session-5"]["reason_code"], "source_gap")
        self.assertEqual(after["session-1"]["reason_code"], "retention_gap")

    def test_session_scoped_or_revised_gap_is_rejected_before_ack(self):
        invalid = gap()
        invalid["identity"] = {"session_id": "one"}
        with self.assertRaisesRegex(ProtocolError, "source-wide"):
            self.commit("", "bad", [invalid])
        invalid = gap()
        invalid["revision"] = 1
        with self.assertRaisesRegex(ProtocolError, "source-wide"):
            self.commit("", "bad", [invalid])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1")
                             .fetchone()[0], "")

    def test_receiver_health_exposes_unknown_and_source_gap_reason(self):
        from observatory.server import BoundedHTTPServer, handler_for
        self.commit("", "one", [fact("old", "old", "lifecycle", {"state": "completed"})])
        self.by_native()
        self.commit("one", "two", [gap()])
        self.by_native()
        handler = handler_for(self.store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/health", timeout=3) as response:
            health = json.load(response)
        row = health["local_agents"]["sessions"][0]
        self.assertEqual((row["run_state"], row["reason_code"]),
                         ("unknown", "source_gap"))


if __name__ == "__main__":
    unittest.main()
