"""Policy-coherent lifecycle projection from admitted plugin metadata."""
import tempfile
import unittest
import json
import sqlite3
import threading
import time
from urllib.request import urlopen
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from observatory.core import Store
from observatory.agent_hierarchy import canonical_agent_id
from observatory.plugin_admission import activate_source, commit_batch, initialize, set_source_capture
from observatory.plugin_projection import project_source, sessions
from observatory.plugin_wire import PollBatch, ProtocolError


GENERATION = "a" * 64


def lifecycle(event, state, stamp="100", revision=0, session="native-1", reason=None):
    payload = {"state": state}
    if reason:
        payload["reason_code"] = reason
    return {"event_id": event, "revision": revision, "kind": "lifecycle",
            "time_ns": stamp, "payload": payload,
            "identity": {"session_id": session}}


def usage(event, payload, revision=0, session="native-1"):
    return {"event_id": event, "revision": revision, "kind": "usage",
            "time_ns": "150", "payload": payload,
            "identity": {"session_id": session}}


def relationship(event, child, parent, stamp="150", revision=0, kind="delegated"):
    return {"event_id": event, "revision": revision, "kind": "relationship",
            "time_ns": stamp, "payload": {"parent_session_id": parent, "kind": kind},
            "identity": {"session_id": child}}


class PluginProjectionTests(unittest.TestCase):
    def test_existing_projection_table_migrates_without_losing_rows(self):
        with sqlite3.connect(":memory:") as db:
            db.execute("CREATE TABLE plugin_session_projection_v1 ("
                       "source_id TEXT NOT NULL,native_session_id TEXT NOT NULL,"
                       "session_key TEXT NOT NULL,state TEXT NOT NULL,reason_code TEXT NOT NULL,"
                       "state_time_ns INTEGER NOT NULL,last_record_ns INTEGER NOT NULL,"
                       "policy_epoch INTEGER NOT NULL,source_dirty_seq INTEGER NOT NULL,"
                       "PRIMARY KEY(source_id,native_session_id))")
            db.execute("INSERT INTO plugin_session_projection_v1 VALUES(?,?,?,?,?,?,?,?,?)",
                       ("source-a", "native-1", "key", "completed", "", 100, 100, 1, 1))
            initialize(db)
            self.assertEqual(db.execute("SELECT usage_json FROM plugin_session_projection_v1").fetchone()[0],
                             "{}")
            self.assertEqual(tuple(db.execute("SELECT parent_native_session_id,relationship_kind,"
                                              "relationship_state FROM plugin_session_projection_v1").fetchone()),
                             ("", "unknown", "unresolved"))

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(str(Path(self.temp.name) / "agenturm.db"))
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="source-a", generation=GENERATION)

    def commit(self, expected, cursor, records):
        return commit_batch(self.store, self.lease, expected,
                            PollBatch("source-a", cursor, tuple(records)))

    def test_explicit_lifecycle_only_and_correction_preserves_occurrence_order(self):
        self.commit("", "one", [lifecycle("a", "started", "100"),
                                lifecycle("b", "completed", "200", reason="normal")])
        self.assertEqual(sessions(self.store)["pending_sources"], 1)
        self.assertEqual(sessions(self.store)["sessions"], [])
        self.assertEqual(project_source(self.store, "source-a"), 1)
        result = sessions(self.store)
        self.assertEqual(result["pending_sources"], 0)
        self.assertEqual(result["sessions"][0]["run_state"], "completed")
        self.assertEqual(result["sessions"][0]["reason_code"], "normal")
        self.assertEqual(result["sessions"][0]["capture_content"], False)
        self.commit("one", "two", [lifecycle("a", "failed", "100", revision=1)])
        self.assertEqual(sessions(self.store)["sessions"], [])
        project_source(self.store, "source-a")
        self.assertEqual(sessions(self.store)["sessions"][0]["run_state"], "completed")

    def test_duplicate_event_at_new_cursor_does_not_dirty_projection(self):
        row = lifecycle("a", "completed")
        self.commit("", "one", [row])
        project_source(self.store, "source-a")
        self.commit("one", "two", [row])
        result = sessions(self.store)
        self.assertEqual(result["pending_sources"], 0)
        self.assertEqual(result["sessions"][0]["run_state"], "completed")

    def test_usage_counts_distinct_events_and_replaces_corrected_event(self):
        self.commit("", "one", [usage("u1", {"input_tokens": 10, "output_tokens": 0}),
                                usage("u2", {"input_tokens": 3, "cache_read_tokens": 2})])
        project_source(self.store, "source-a")
        row = sessions(self.store)["sessions"][0]
        self.assertEqual(row["metadata_usage"], {"input_tokens": 13,
                                                   "output_tokens": 0,
                                                   "cache_read_tokens": 2})
        self.assertNotIn("total_tokens", row["metadata_usage"])
        self.commit("one", "duplicate", [usage("u1", {"input_tokens": 10,
                                                     "output_tokens": 0})])
        self.assertEqual(sessions(self.store)["pending_sources"], 0)
        self.assertEqual(sessions(self.store)["sessions"][0]["metadata_usage"],
                         row["metadata_usage"])
        self.commit("duplicate", "two", [usage("u1", {"input_tokens": 12,
                                                 "output_tokens": 1}, revision=1)])
        self.assertEqual(sessions(self.store)["sessions"], [])
        project_source(self.store, "source-a")
        self.assertEqual(sessions(self.store)["sessions"][0]["metadata_usage"],
                         {"input_tokens": 15, "output_tokens": 1,
                          "cache_read_tokens": 2})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 2)

    def test_usage_large_valid_counters_remain_exact(self):
        self.commit("", "one", [usage("u1", {"input_tokens": 2**63 - 1}),
                                usage("u2", {"input_tokens": 2**63 - 1})])
        project_source(self.store, "source-a")
        self.assertEqual(sessions(self.store)["sessions"][0]["metadata_usage"]["input_tokens"],
                         str(2 * (2**63 - 1)))

    def test_usage_without_identity_does_not_create_session(self):
        record = usage("u1", {"input_tokens": 5})
        record.pop("identity")
        self.commit("", "one", [record])
        self.assertEqual(project_source(self.store, "source-a"), 0)
        self.assertEqual(sessions(self.store)["sessions"], [])

    def test_usage_is_erased_on_capture_revocation(self):
        self.commit("", "one", [usage("u1", {"input_tokens": 5})])
        project_source(self.store, "source-a")
        self.assertEqual(sessions(self.store)["sessions"][0]["metadata_usage"],
                         {"input_tokens": 5})
        set_source_capture(self.store, source_id="source-a", enabled=False)
        self.assertEqual(sessions(self.store)["sessions"], [])

    def test_projected_status_uses_existing_session_health_semantics(self):
        from observatory.source_projection import prepare_sources
        from observatory.session_status import session_health
        self.commit("", "one", [lifecycle("a", "completed")])
        project_source(self.store, "source-a")
        snapshot = SimpleNamespace(agents=[], activity=[], sessions=[],
                                   local_status={"sessions": sessions(self.store)["sessions"]})
        prepared, _, _ = prepare_sources(snapshot)
        self.assertEqual(len(prepared), 1)
        self.assertEqual(session_health(prepared[0], time.time_ns())["state"], "Completed")

    def test_ambiguous_same_time_state_is_unknown(self):
        self.commit("", "one", [lifecycle("a", "completed"), lifecycle("b", "failed")])
        project_source(self.store, "source-a")
        row = sessions(self.store)["sessions"][0]
        self.assertEqual((row["run_state"], row["reason_code"]),
                         ("unknown", "ambiguous_lifecycle"))

    def test_two_sessions_from_one_source_have_distinct_ui_identity(self):
        self.commit("", "one", [lifecycle("a", "completed", session="first"),
                                lifecycle("b", "running", session="second")])
        project_source(self.store, "source-a")
        projected = sessions(self.store)["sessions"]
        self.assertEqual(len(projected), 2)
        self.assertEqual(len({row["source_id"] for row in projected}), 2)
        self.assertEqual({row["plugin_source_id"] for row in projected}, {"source-a"})

    def test_relationship_requires_observed_parent_and_resolves_after_arrival(self):
        from observatory.source_projection import prepare_sources
        self.commit("", "one", [lifecycle("child", "running", session="child"),
                                relationship("relation", "child", "parent")])
        project_source(self.store, "source-a")
        child = sessions(self.store)["sessions"][0]
        self.assertEqual((child["relationship_kind"], child["relationship_state"]),
                         ("delegated", "unresolved"))
        self.commit("one", "two", [lifecycle("parent", "running", session="parent")])
        self.assertEqual(sessions(self.store)["sessions"], [])
        project_source(self.store, "source-a")
        projected = {row["native_session_id"]: row for row in sessions(self.store)["sessions"]}
        self.assertEqual(projected["child"]["relationship_state"], "confirmed")
        self.assertEqual(projected["child"]["parent_agent_id"],
                         canonical_agent_id("fixture.plugin", "plugin:source-a", "parent"))
        snapshot = SimpleNamespace(agents=[], activity=[], sessions=[],
                                   local_status={"sessions": list(projected.values())})
        prepared, _, _ = prepare_sources(snapshot)
        linked = {row["native_session_id"]: row for row in prepared}
        self.assertEqual(linked["child"]["parent_agent_id"], linked["parent"]["agent_id"])

    def test_corrected_relationship_and_cycle_are_policy_coherent(self):
        self.commit("", "one", [lifecycle("a", "running", session="a"),
                                lifecycle("b", "running", session="b"),
                                relationship("a-parent", "a", "b")])
        project_source(self.store, "source-a")
        self.assertEqual({r["native_session_id"]: r for r in sessions(self.store)["sessions"]}
                         ["a"]["relationship_state"], "confirmed")
        self.commit("one", "two", [relationship("b-parent", "b", "a")])
        project_source(self.store, "source-a")
        self.assertEqual({r["relationship_state"] for r in sessions(self.store)["sessions"]},
                         {"invalid"})
        self.commit("two", "three", [relationship("b-parent", "b", "other", revision=1)])
        project_source(self.store, "source-a")
        projected = {r["native_session_id"]: r for r in sessions(self.store)["sessions"]}
        self.assertEqual(projected["a"]["relationship_state"], "confirmed")
        self.assertEqual(projected["b"]["relationship_state"], "unresolved")
        set_source_capture(self.store, source_id="source-a", enabled=False)
        self.assertEqual(sessions(self.store)["sessions"], [])

    def test_same_time_relationship_conflict_never_chooses_parent(self):
        self.commit("", "one", [relationship("one", "child", "a"),
                                relationship("two", "child", "b")])
        project_source(self.store, "source-a")
        row = sessions(self.store)["sessions"][0]
        self.assertEqual((row["relationship_kind"], row["relationship_state"],
                          row["parent_agent_id"]), ("unknown", "conflicted", None))

    def test_health_exposes_projected_sessions_and_pending_coverage(self):
        from observatory.server import BoundedHTTPServer, handler_for
        handler = handler_for(self.store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.commit("", "one", [lifecycle("a", "completed"),
                                usage("u1", {"input_tokens": 7})])

        def health():
            with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/health", timeout=3) as response:
                return json.load(response)["local_agents"]

        self.assertEqual(health()["plugin_projection"]["pending_sources"], 1)
        self.assertEqual(health().get("sessions", []), [])
        project_source(self.store, "source-a")
        self.assertEqual(health()["sessions"][0]["run_state"], "completed")
        self.assertEqual(health()["sessions"][0]["metadata_usage"],
                         {"input_tokens": 7})

    def test_missing_identity_does_not_create_a_session(self):
        row = lifecycle("a", "completed")
        row.pop("identity")
        self.commit("", "one", [row])
        self.assertEqual(project_source(self.store, "source-a"), 0)
        self.assertEqual(sessions(self.store), {"sessions": [], "pending_sources": 0,
                                               "omitted_sessions": False,
                                               "retention_gaps": []})

    def test_revocation_erases_projection_and_reenabled_source_is_empty(self):
        self.commit("", "one", [lifecycle("a", "completed")])
        project_source(self.store, "source-a")
        self.assertEqual(len(sessions(self.store)["sessions"]), 1)
        set_source_capture(self.store, source_id="source-a", enabled=False)
        self.assertEqual(sessions(self.store)["sessions"], [])
        set_source_capture(self.store, source_id="source-a", enabled=True)
        self.assertEqual(sessions(self.store)["pending_sources"], 0)
        self.assertEqual(sessions(self.store)["sessions"], [])

    def test_stale_projection_cannot_publish_over_newer_batch(self):
        self.commit("", "one", [lifecycle("a", "started")])
        import observatory.plugin_projection as projection
        original = projection._reduce

        def concurrent_write(*args):
            self.commit("one", "two", [lifecycle("b", "completed", "200")])
            return original(*args)

        with patch.object(projection, "_reduce", side_effect=concurrent_write):
            with self.assertRaisesRegex(ProtocolError, "source changed"):
                project_source(self.store, "source-a")
        self.assertEqual(sessions(self.store)["sessions"], [])
        project_source(self.store, "source-a")
        self.assertEqual(sessions(self.store)["sessions"][0]["run_state"], "completed")


if __name__ == "__main__":
    unittest.main()
