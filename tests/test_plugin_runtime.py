"""Opt-in process and Store composition across idle polls, replay and fencing."""
import tempfile
import time
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch
from observatory.plugin_admission_port import PythonStoreAdmissionPort
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import PollBatch, ProtocolError
from tests import test_plugin_process


class RecordingAdmissionPort:
    def __init__(self, store):
        self.delegate = PythonStoreAdmissionPort(store)
        self.calls = []

    def activate_source(self, **kwargs):
        self.calls.append("activate_source")
        return self.delegate.activate_source(**kwargs)

    def install_reference_schema(self, *args):
        self.calls.append("install_reference_schema")
        return self.delegate.install_reference_schema(*args)

    def member_cursors(self, *args):
        self.calls.append("member_cursors")
        return self.delegate.member_cursors(*args)

    def last_source_diagnostic(self, *args):
        self.calls.append("last_source_diagnostic")
        return self.delegate.last_source_diagnostic(*args)

    def commit_batch(self, *args, **kwargs):
        self.calls.append("commit_batch")
        return self.delegate.commit_batch(*args, **kwargs)

    def capture_revoked(self, *args):
        self.calls.append("capture_revoked")
        return self.delegate.capture_revoked(*args)

    def classify_lease_failure(self, *args):
        self.calls.append("classify_lease_failure")
        return self.delegate.classify_lease_failure(*args)

    def close(self):
        self.calls.append("close")
        return self.delegate.close()


class PluginRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(str(Path(self.temp.name) / "agenturm.db"))
        self.fixture = test_plugin_process.ProcessHostTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def session(self, mode="ok", *, admission_port=None):
        return PluginSession(self.store, self.fixture.manifest(mode),
                             source_id="fixture-source",
                             allowed_grants=("configured-source-read",), timeout=1.0,
                             admission_port=admission_port)

    def state(self):
        with self.store._connect() as db:
            cursor = db.execute("SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 "
                                "WHERE source_id='fixture-source'").fetchone()
            evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0]
            return tuple(cursor), evidence

    def test_initial_idle_poll_keeps_worker_and_cursor_without_database_writes(self):
        with self.session("idle") as session:
            self.assertEqual(session.cursor, "")
            for _ in range(3):
                self.assertEqual(session.poll_once(), PollBatch("fixture-source", "", ()))
                self.assertEqual(session.cursor, "")
                self.assertIsNotNone(session.process.process)
            self.assertEqual(self.state(), (("", 0), 0))
        self.assertEqual(self.fixture.marker.read_text().splitlines(), ["", "", ""])

    def test_idle_after_durable_progress_uses_resume_cursor(self):
        manifest = self.fixture.manifest("idle")
        lease = activate_source(self.store, plugin_id=manifest.id,
                                source_id="fixture-source", generation=manifest.artifact_sha256)
        record = {"event_id": "one", "revision": 0, "kind": "lifecycle",
                  "time_ns": "9007199254740993", "payload": {"state": "completed"}}
        commit_batch(self.store, lease, "", PollBatch("fixture-source", "next", (record,)))
        before = self.state()
        with PluginSession(self.store, manifest, source_id="fixture-source",
                           allowed_grants=("configured-source-read",), timeout=1.0) as session:
            self.assertEqual(session.cursor, "next")
            self.assertEqual(session.poll_once(), PollBatch("fixture-source", "next", ()))
            self.assertEqual(self.state(), before)

    def test_live_worker_replays_exact_batch_without_duplicate_evidence(self):
        with self.session() as session:
            first = session.poll_once()
            self.assertEqual(first.cursor, "next")
            self.assertEqual(self.state(), (("next", 1), 1))
            deadline = time.monotonic() + 3
            while self.store.plugin_sessions()["pending_sources"] and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertEqual(self.store.plugin_sessions()["pending_sources"], 0)
            self.assertIsNone(session.projection_error)
            self.assertEqual(session.poll_once(), first)
            self.assertEqual(self.state(), (("next", 1), 1))
        self.assertEqual(self.fixture.marker.read_text().splitlines(), ["next", "next"])

    def test_injected_port_owns_activation_diagnostic_and_commit(self):
        port = RecordingAdmissionPort(self.store)
        with self.session(admission_port=port) as session:
            batch = session.poll_once()
            self.assertEqual(batch.cursor, "next")
            self.assertEqual(session.cursor, "next")
        self.assertEqual(port.calls, [
            "install_reference_schema", "activate_source",
            "last_source_diagnostic", "commit_batch", "close",
        ])
        self.assertEqual(self.state(), (("next", 1), 1))

    def test_session_rejects_an_untrusted_port_before_launch(self):
        with self.assertRaisesRegex(TypeError, "trusted contract"):
            self.session(admission_port=object())
        self.assertFalse(self.fixture.marker.exists())

    def test_startup_activation_failure_closes_injected_port(self):
        class ActivationFailurePort(RecordingAdmissionPort):
            def activate_source(self, **kwargs):
                self.calls.append("activate_source")
                raise RuntimeError("backend activation failed")

        port = ActivationFailurePort(self.store)
        session = self.session(admission_port=port)
        with self.assertRaisesRegex(RuntimeError, "backend activation failed"):
            session.start()
        self.assertEqual(port.calls, [
            "install_reference_schema", "activate_source", "close",
        ])
        self.assertFalse(self.fixture.marker.exists())

    def test_revoked_lease_closes_worker_without_ack(self):
        session = self.session().start()
        try:
            manifest = session.process.manifest
            activate_source(self.store, plugin_id=manifest.id,
                            source_id="fixture-source", generation=manifest.artifact_sha256)
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                session.poll_once()
            self.assertIsNone(session.process.process)
            self.assertFalse(self.fixture.marker.exists())
            self.assertEqual(self.state(), (("", 0), 0))
        finally:
            session.close()


if __name__ == "__main__":
    unittest.main()
