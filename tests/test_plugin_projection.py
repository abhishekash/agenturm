"""Policy-coherent lifecycle projection from admitted plugin metadata."""
import tempfile
import unittest
import json
import threading
import time
from urllib.request import urlopen
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
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


class PluginProjectionTests(unittest.TestCase):
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

    def test_health_exposes_projected_sessions_and_pending_coverage(self):
        from observatory.server import BoundedHTTPServer, handler_for
        handler = handler_for(self.store, quiet=True)
        self.addCleanup(handler.response_cache.close)
        server = BoundedHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.commit("", "one", [lifecycle("a", "completed")])

        def health():
            with urlopen(f"http://127.0.0.1:{server.server_address[1]}/api/health", timeout=3) as response:
                return json.load(response)["local_agents"]

        self.assertEqual(health()["plugin_projection"]["pending_sources"], 1)
        self.assertEqual(health().get("sessions", []), [])
        project_source(self.store, "source-a")
        self.assertEqual(health()["sessions"][0]["run_state"], "completed")

    def test_missing_identity_does_not_create_a_session(self):
        row = lifecycle("a", "completed")
        row.pop("identity")
        self.commit("", "one", [row])
        self.assertEqual(project_source(self.store, "source-a"), 0)
        self.assertEqual(sessions(self.store), {"sessions": [], "pending_sources": 0,
                                               "omitted_sessions": False})

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
