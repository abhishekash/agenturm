"""Finite plugin evidence horizon, fail-closed corrections and coverage."""
import tempfile
import json
import threading
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.request import urlopen

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source, sessions
from observatory.plugin_wire import PollBatch, ProtocolError


def event(number, *, revision=0, session=None, stamp=None, state="completed"):
    return {"event_id": f"event-{number}", "revision": revision,
            "kind": "lifecycle", "time_ns": str(stamp or number),
            "payload": {"state": state},
            "identity": {"session_id": session or f"session-{number}"}}


class RetentionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(str(Path(self.temp.name) / "store.db"))
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="fixture-source", generation="a" * 64)

    def commit(self, expected, cursor, records):
        return commit_batch(self.store, self.lease, expected,
                            PollBatch("fixture-source", cursor, tuple(records)))

    def snapshot(self):
        with self.store._connect() as db:
            source = tuple(db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1").fetchone())
            evidence = [tuple(row) for row in db.execute(
                "SELECT event_id_hash,revision FROM plugin_evidence_v1 ORDER BY event_id_hash")]
            gap = [tuple(row) for row in db.execute(
                "SELECT watermark_ns,dropped_events FROM plugin_retention_v1")]
            return source, evidence, gap

    def fill(self):
        self.commit("", "one", [event(n) for n in range(1, 5)])
        project_source(self.store, "fixture-source")

    def test_checkpoint_preserves_cursor_and_marks_evicted_sessions_unknown(self):
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.fill()
            batch = PollBatch("fixture-source", "two", (event(5),))
            self.assertEqual(commit_batch(self.store, self.lease, "one", batch), "two")
            after = self.snapshot()
            self.assertEqual(after[0], ("two", 2))
            self.assertEqual(len(after[1]), 3)
            self.assertEqual(after[2], [(2, 2)])
            self.assertEqual(commit_batch(self.store, self.lease, "one", batch), "two")
            self.assertEqual(self.snapshot(), after)
            self.assertEqual(sessions(self.store)["sessions"], [])
            project_source(self.store, "fixture-source")
            projection = sessions(self.store)
            rows = {row["native_session_id"]: row for row in projection["sessions"]}
            self.assertEqual(set(rows), {f"session-{n}" for n in range(1, 6)})
            for n in (1, 2):
                self.assertEqual((rows[f"session-{n}"]["run_state"],
                                  rows[f"session-{n}"]["reason_code"],
                                  rows[f"session-{n}"]["metadata_usage"]),
                                 ("unknown", "retention_gap", {}))
            self.assertEqual(rows["session-5"]["run_state"], "completed")
            self.assertEqual(projection["retention_gaps"], [
                {"source_id": "fixture-source", "watermark_ns": "2", "dropped_events": 2}])

    def test_evicted_correction_and_old_duplicate_fail_without_cursor_advance(self):
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.fill()
            self.commit("one", "two", [event(5)])
            before = self.snapshot()
            for stale in (event(1, revision=1, state="failed"), event(1)):
                with self.subTest(stale=stale), self.assertRaisesRegex(ProtocolError, "horizon"):
                    self.commit("two", "bad", [event(6), stale])
                self.assertEqual(self.snapshot(), before)
            self.commit("two", "three", [event(6)])

    def test_watermark_above_json_safe_integer_is_exact_decimal_text(self):
        large = 2**53 + 100
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.commit("", "one", [event(n, stamp=large + n) for n in range(1, 5)])
            self.commit("one", "two", [event(5, stamp=large + 5)])
            self.assertEqual(sessions(self.store)["retention_gaps"][0]["watermark_ns"],
                             str(large + 2))
            with self.assertRaisesRegex(ProtocolError, "horizon"):
                self.commit("two", "bad", [event(1, stamp=large + 1)])

    def test_invalidation_budget_refuses_batch_atomically(self):
        with (patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4),
              patch("observatory.plugin_retention.MAX_INVALIDATED_SESSIONS", 1)):
            self.fill()
            before = self.snapshot()
            with self.assertRaisesRegex(ProtocolError, "invalidation capacity"):
                self.commit("one", "two", [event(5)])
            self.assertEqual(self.snapshot(), before)

    def test_evicted_relationship_cannot_keep_confirmed_parent_or_terminal_state(self):
        relation = {"event_id": "relation", "revision": 0, "kind": "relationship",
                    "time_ns": "3", "payload": {"parent_session_id": "parent",
                                                 "kind": "delegated"},
                    "identity": {"session_id": "child"}}
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.commit("", "one", [event(1, session="parent"),
                                    event(2, session="child"), relation, event(4)])
            project_source(self.store, "fixture-source")
            self.assertEqual({r["native_session_id"]: r for r in sessions(self.store)["sessions"]}
                             ["child"]["relationship_state"], "confirmed")
            self.commit("one", "two", [event(5)])
            project_source(self.store, "fixture-source")
            child = {r["native_session_id"]: r for r in sessions(self.store)["sessions"]}["child"]
            self.assertEqual((child["run_state"], child["reason_code"],
                              child["relationship_state"], child["parent_agent_id"]),
                             ("unknown", "retention_gap", "unresolved", None))

    def test_revocation_erases_gap_and_fences_old_worker(self):
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.fill()
            self.commit("one", "two", [event(5)])
            project_source(self.store, "fixture-source")
            self.assertTrue(sessions(self.store)["retention_gaps"])
            set_source_capture(self.store, source_id="fixture-source", enabled=False)
            self.assertEqual(sessions(self.store)["retention_gaps"], [])
            with self.store._connect() as db:
                self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_retention_invalidated_v1")
                                 .fetchone()[0], 0)
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                self.commit("two", "three", [event(6)])

    def test_health_reports_gap_without_exposing_event_contents(self):
        from observatory.server import BoundedHTTPServer, handler_for
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.fill()
            self.commit("one", "two", [event(5)])
            project_source(self.store, "fixture-source")
        handler = handler_for(self.store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/health", timeout=3) as response:
            local = json.load(response)["local_agents"]
        self.assertEqual(local["plugin_projection"]["retention_gaps"], [
            {"source_id": "fixture-source", "watermark_ns": "2", "dropped_events": 2}])
        affected = {row["native_session_id"]: row for row in local["sessions"]}["session-1"]
        self.assertEqual((affected["run_state"], affected["reason_code"],
                          affected["metadata_usage"]), ("unknown", "retention_gap", {}))


if __name__ == "__main__":
    unittest.main()
