"""Content-free process-plugin facts in the policy-current Activity index."""
import json
import tempfile
import threading
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from urllib.request import urlopen

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source, sessions
from observatory.plugin_wire import PollBatch, ProtocolError


def fact(event, kind, payload, *, session="native-1", revision=0, stamp="100",
         worker="worker-1", turn="turn-1", operation=None):
    identity = {"session_id": session, "worker_id": worker, "turn_id": turn}
    if operation is not None:
        identity["operation_id"] = operation
    return {"event_id": event, "revision": revision, "kind": kind,
            "time_ns": stamp, "payload": payload,
            "identity": identity}


class PluginActivityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = str(Path(self.temp.name) / "store.db")
        self.store = Store(self.path)
        self.lease = activate_source(self.store, plugin_id="fixture.plugin",
                                     source_id="fixture-source", generation="a" * 64)

    def commit(self, expected, cursor, records):
        return commit_batch(self.store, self.lease, expected,
                            PollBatch("fixture-source", cursor, tuple(records)))

    def activity(self, **filters):
        return self.store.activity_page(**filters)["activity"]

    def test_metadata_facts_are_indexed_without_content_or_false_completion(self):
        self.commit("", "one", [
            fact("start", "lifecycle", {"state": "started"}, stamp="10"),
            fact("message", "message", {"role": "user"}, stamp="20"),
            fact("tool", "tool", {"category": "shell", "result": "failed"}, stamp="30"),
            fact("check", "check", {"category": "test", "result": "failed"}, stamp="40")])
        self.assertEqual(self.activity(), [])  # Dirty source hides old revision.
        project_source(self.store, "fixture-source")
        rows = self.activity()
        self.assertEqual([(r["domain"], r["action"], r["result"]) for r in rows],
                         [("check", "test", "failed"), ("tool", "shell", "failed"),
                          ("message", "user", "observed")])
        self.assertTrue(all(r["detail"] == "" and r["source"] == "plugin" for r in rows))
        self.assertTrue(all(r["session_id"] == sessions(self.store)["sessions"][0]["session_id"]
                            for r in rows))
        self.assertTrue(all(r["agent_id"] and len(r["turn_id"]) == 32
                            for r in rows))
        self.assertEqual(sessions(self.store)["sessions"][0]["run_state"], "running")
        from observatory.source_projection import prepare_sources
        snapshot = SimpleNamespace(agents=[], activity=[], sessions=[],
                                   local_status={"sessions": sessions(self.store)["sessions"]})
        prepared, _, _ = prepare_sources(snapshot)
        self.assertEqual(rows[0]["agent_id"], prepared[0]["agent_id"])
        exact = self.store.activity_page(agent_id=prepared[0]["agent_id"])
        self.assertEqual(len(exact["activity"]), 3)
        self.assertEqual(exact["agent_scope"]["coverage"], "partial")
        self.assertEqual(len(self.store.activity_page(agent_id=prepared[0]["agent_id"],
                                                      subtree=True)["activity"]), 3)
        self.assertEqual(len(self.store.activity_page(turn_id=rows[0]["turn_id"])["activity"]), 3)
        self.assertEqual(len(self.activity(domain="check", errors_only=True)), 1)
        self.assertEqual({r["domain"] for r in self.activity(review=True)},
                         {"message", "tool", "check"})
        raw = json.dumps(rows)
        self.assertNotIn("worker-1", raw)
        self.assertNotIn("turn-1", raw)

    def test_correction_hides_stale_activity_then_replaces_result(self):
        initial = fact("check", "check", {"category": "test", "result": "failed"})
        self.commit("", "one", [initial])
        project_source(self.store, "fixture-source")
        before = self.activity()[0]
        self.commit("one", "two", [fact("check", "check", {"category": "test", "result": "passed"},
                                        revision=1)])
        self.assertEqual(self.activity(), [])
        project_source(self.store, "fixture-source")
        after = self.activity()[0]
        self.assertEqual((before["id"], before["result"]), (after["id"], "failed"))
        self.assertEqual(after["result"], "passed")
        self.assertEqual(self.activity(errors_only=True), [])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM activity_index_v1 "
                                         "WHERE kind='plugin'").fetchone()[0], 1)

    def test_pi_tool_phases_share_opaque_operation_without_trace_claim(self):
        self.commit("", "one", [
            fact("pi:call", "tool", {"category": "file", "result": "started"},
                 operation="native-call-1", stamp="10"),
            fact("pi:result", "tool", {"category": "file", "result": "failed"},
                 operation="native-call-1", stamp="20")])
        project_source(self.store, "fixture-source")
        rows = self.activity()
        self.assertEqual([row["result"] for row in rows], ["failed", "started"])
        operation = rows[0]["operation_id"]
        self.assertEqual(rows[1]["operation_id"], operation)
        self.assertTrue(operation.startswith("plugin-operation:"))
        self.assertTrue(all(row["trace_id"] is None and row["span_id"] is None
                            and row["metadata"]["coverage"] == "partial" for row in rows))
        self.assertNotIn("native-call-1", json.dumps(rows))
        from observatory.terminal_ui import copy_event_text
        exported = copy_event_text(rows[0])
        self.assertIn("Operation ID: " + operation, exported)
        self.assertIn("Coverage: partial", exported)
        self.assertNotIn("native-call-1", exported)
        self.assertEqual([row["id"] for row in self.activity(review=True)],
                         [rows[0]["id"]])  # Failed phase is reviewable; start is not.
        self.commit("one", "two", [
            fact("pi:result", "tool", {"category": "file", "result": "succeeded"},
                 operation="native-call-1", stamp="20", revision=1)])
        self.assertEqual(self.activity(), [])
        project_source(self.store, "fixture-source")
        corrected = self.activity()
        self.assertEqual({row["operation_id"] for row in corrected}, {operation})
        self.assertEqual(self.activity(review=True), [])
        with self.assertRaisesRegex(ProtocolError, "identity or kind changed"):
            self.commit("two", "three", [
                fact("pi:result", "tool", {"category": "file", "result": "failed"},
                     operation="other-call", stamp="20", revision=2)])
        set_source_capture(self.store, source_id="fixture-source", enabled=False)
        self.assertEqual(self.activity(), [])

    def test_revocation_removes_activity_and_fences_old_worker(self):
        self.commit("", "one", [fact("m", "message", {"role": "assistant"})])
        project_source(self.store, "fixture-source")
        self.assertEqual(len(self.activity()), 1)
        set_source_capture(self.store, source_id="fixture-source", enabled=False)
        self.assertEqual(self.activity(), [])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM activity_index_v1 "
                                         "WHERE kind='plugin'").fetchone()[0], 0)
        with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
            self.commit("one", "two", [fact("later", "message", {"role": "user"})])

    def test_existing_projection_requires_republish_on_activity_upgrade(self):
        self.commit("", "one", [fact("m", "message", {"role": "user"})])
        project_source(self.store, "fixture-source")
        self.assertEqual(len(self.activity()), 1)
        with self.store._connect() as db:
            db.execute("DELETE FROM schema_meta WHERE key='plugin_activity_projection_v1'")
        reopened = Store(self.path)
        self.addCleanup(reopened.close)
        self.assertEqual(reopened.activity_page()["activity"], [])
        self.assertEqual(sessions(reopened)["pending_sources"], 1)
        project_source(reopened, "fixture-source")
        self.assertEqual(len(reopened.activity_page()["activity"]), 1)

    def test_operation_upgrade_invalidates_prior_activity_projection(self):
        self.commit("", "one", [fact("m", "message", {"role": "user"})])
        project_source(self.store, "fixture-source")
        self.assertEqual(len(self.activity()), 1)
        with self.store._connect() as db:
            row = db.execute("SELECT event_key,payload FROM activity_index_v1 "
                             "WHERE kind='plugin'").fetchone()
            old = json.loads(row["payload"])
            old["turn_id"] = "plugin-turn:legacy"
            db.execute("UPDATE activity_index_v1 SET turn_id=?,payload=? WHERE event_key=?",
                       (old["turn_id"], json.dumps(old), row["event_key"]))
            db.execute("DELETE FROM schema_meta WHERE key='plugin_activity_projection_v2'")
        reopened = Store(self.path)
        self.addCleanup(reopened.close)
        self.assertEqual(reopened.activity_page()["activity"], [])
        self.assertEqual(sessions(reopened)["pending_sources"], 1)
        project_source(reopened, "fixture-source")
        activity = reopened.activity_page()["activity"]
        self.assertEqual(len(activity), 1)
        self.assertEqual(len(activity[0]["turn_id"]), 32)
        self.assertEqual(len(reopened.activity_page(turn_id=activity[0]["turn_id"])["activity"]), 1)

    def test_http_activity_cache_cannot_replay_stale_policy_or_revision(self):
        from observatory.server import BoundedHTTPServer, handler_for
        handler = handler_for(self.store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        def page(turn_id=""):
            suffix = "?turn_id=" + turn_id if turn_id else ""
            with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/activity{suffix}",
                         timeout=3) as response:
                return json.load(response)["activity"]

        self.commit("", "one", [fact("c", "check", {"category": "test", "result": "failed"})])
        project_source(self.store, "fixture-source")
        self.assertEqual(page()[0]["result"], "failed")
        self.assertEqual(page()[0]["result"], "failed")  # warm HTTP cache
        turn_id = page()[0]["turn_id"]
        self.assertEqual(len(page(turn_id)), 1)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM investigation_dirty_v1 "
                                        "WHERE kind='plugin'").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM investigation_terms_v1 "
                                        "WHERE event_key IN (SELECT event_key FROM activity_index_v1 "
                                        "WHERE kind='plugin')").fetchone()[0], 0)
        self.commit("one", "two", [fact("c", "check", {"category": "test", "result": "passed"},
                                        revision=1)])
        self.assertEqual(page(), [])
        self.assertEqual(page(turn_id), [])
        project_source(self.store, "fixture-source")
        self.assertEqual(page()[0]["result"], "passed")
        self.assertEqual(page(turn_id)[0]["result"], "passed")
        set_source_capture(self.store, source_id="fixture-source", enabled=False)
        self.assertEqual(page(), [])
        self.assertEqual(page(turn_id), [])

    def test_retention_invalidation_removes_affected_activity(self):
        with patch("observatory.plugin_admission.MAX_SOURCE_RECORDS", 4):
            self.commit("", "one", [
                fact("m1", "tool", {"category": "file", "result": "failed"},
                     session="session-1", stamp="1", operation="call-1"),
                *(fact(f"m{n}", "message", {"role": "user"},
                       session=f"session-{n}", stamp=str(n)) for n in range(2, 5))])
            project_source(self.store, "fixture-source")
            self.assertEqual(len(self.activity()), 4)
            self.commit("one", "two", [fact("m5", "message", {"role": "user"},
                                          session="session-5", stamp="5")])
            self.assertEqual(self.activity(), [])
            project_source(self.store, "fixture-source")
            self.assertEqual(len(self.activity()), 3)
            invalidated = {r["native_session_id"] for r in sessions(self.store)["sessions"]
                           if r["reason_code"] == "retention_gap"}
            self.assertEqual(invalidated, {"session-1", "session-2"})
            active_session_keys = {r["session_id"] for r in self.activity()}
            self.assertFalse(active_session_keys & {
                r["session_id"] for r in sessions(self.store)["sessions"]
                if r["native_session_id"] in invalidated})

    def test_same_native_id_in_two_plugin_sources_remains_separate(self):
        self.commit("", "one", [fact("m", "message", {"role": "user"})])
        project_source(self.store, "fixture-source")
        other = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id="other-source", generation="a" * 64)
        commit_batch(self.store, other, "", PollBatch("other-source", "one", (
            fact("m", "message", {"role": "user"}),)))
        project_source(self.store, "other-source")
        rows = self.activity()
        self.assertEqual(len(rows), 2)
        self.assertEqual(len({row["session_id"] for row in rows}), 2)
        self.assertEqual(len({row["agent_id"] for row in rows}), 2)
        for row in rows:
            self.assertEqual(len(self.store.activity_page(agent_id=row["agent_id"])["activity"]), 1)

    def test_operation_identity_is_scoped_to_session_and_source(self):
        self.commit("", "one", [
            fact("a", "tool", {"category": "other", "result": "started"},
                 session="first", operation="shared"),
            fact("b", "tool", {"category": "other", "result": "started"},
                 session="second", operation="shared")])
        project_source(self.store, "fixture-source")
        other = activate_source(self.store, plugin_id="fixture.plugin",
                                source_id="other-source", generation="a" * 64)
        commit_batch(self.store, other, "", PollBatch("other-source", "one", (
            fact("a", "tool", {"category": "other", "result": "started"},
                 session="first", operation="shared"),)))
        project_source(self.store, "other-source")
        operations = {row["operation_id"] for row in self.activity()}
        self.assertEqual(len(operations), 3)
        turns = {row["turn_id"] for row in self.activity()}
        self.assertEqual(len(turns), 3)
        for turn_id in turns:
            self.assertEqual(len(self.store.activity_page(turn_id=turn_id)["activity"]), 1)

    def test_rejects_raw_content_and_wrong_payload_before_any_write(self):
        bad = [
            fact("a", "message", {"role": "user", "text": "private prompt"}),
            fact("a", "message", {"role": ["user"]}),
            fact("a", "tool", {"category": "shell", "result": "succeeded",
                               "command": "cat private"}),
            fact("a", "tool", {"category": "shell", "result": "success"}),
            fact("a", "check", {"category": "test", "result": "failed",
                                "output": "secret"}),
            fact("a", "check", {"category": "unknown", "result": "failed"}),
            fact("a", "check", {"category": "test"}),
            fact("a", "message", {"role": "user"}, operation="call-1"),
            fact("a", "lifecycle", {"state": "running"}, operation="call-1"),
            {k: v for k, v in fact("a", "message", {"role": "user"}).items()
             if k != "identity"},
        ]
        for item in bad:
            with self.subTest(item=item), self.assertRaises(ProtocolError):
                self.commit("", "bad", [fact("good", "message", {"role": "assistant"}), item])
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")


if __name__ == "__main__":
    unittest.main()
