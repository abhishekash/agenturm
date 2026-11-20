"""Installed exact Rust query versus a persisted, policy-current Python Store."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_projection import project_source
from observatory.plugin_wire import PollBatch
from observatory.rust_shadow_snapshot import capture, current
from tests.test_rust_domain_reducer_v2 import python_projection


GOLDEN = Path(__file__).parent / "fixtures" / "rust-domain-query-v1.json"


class RustDomainQueryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        executable = os.environ.get("AGENTURM_RUST_QUERY_BIN")
        if not executable:
            raise unittest.SkipTest("Rust query binary is built in the Rust CI job")
        cls.binary = str(Path(executable).resolve())
        cls.cases = json.loads(GOLDEN.read_text())["cases"]

    def query(self, request):
        return subprocess.run([self.binary], input=request if isinstance(request, str)
                              else json.dumps(request), capture_output=True,
                              text=True, timeout=5)

    def test_versioned_exact_fixture_matches_persisted_store(self):
        for case in self.cases:
            with self.subTest(case=case["name"]):
                result = self.query(case.get("raw_input", case.get("input")))
                if "expected" not in case:
                    self.assertEqual((result.returncode, result.stdout), (2, ""))
                    continue
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), case["expected"])
                snapshot = case["input"]["snapshot"]
                persisted = python_projection(snapshot)
                native_id = case["input"]["query"]["native_session_id"]
                row = next((item for item in persisted["sessions"]
                            if item["native_session_id"] == native_id), None)
                self.assertEqual(case["expected"]["session"], row)
                self.assertEqual(case["expected"]["coverage"], "partial")

    def test_live_shadow_query_is_fenced_after_write_and_revocation(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Store(str(Path(directory) / "store.db"))
            try:
                lease = activate_source(store, plugin_id="fixture.plugin",
                                        source_id="query-source", generation="a" * 64)
                record = lambda event, state: {
                    "event_id": event, "revision": 0, "kind": "lifecycle",
                    "time_ns": "100", "identity": {"session_id": "native-one"},
                    "payload": {"state": state}}
                commit_batch(store, lease, "", PollBatch("query-source", "one",
                                                     (record("start", "started"),)))
                project_source(store, "query-source")
                snapshot = capture(store, "query-source")
                self.assertTrue(current(store, snapshot))
                request = {"schema_version": 1, "snapshot": snapshot.fixture,
                           "query": {"kind": "session", "source_id": "query-source",
                                     "native_session_id": "native-one"}}
                result = self.query(request)
                self.assertEqual(result.returncode, 0, result.stderr)
                reply = json.loads(result.stdout)
                self.assertTrue(current(store, snapshot))
                self.assertEqual(reply["session"], snapshot.expected["sessions"][0])
                self.assertEqual(reply["receiver_id"], store.receiver_id)
                self.assertEqual(reply["source_id"], "query-source")
                self.assertEqual(reply["policy_epoch"], str(snapshot.revision[2]))
                self.assertEqual(reply["dirty_seq"], str(snapshot.revision[3]))
                commit_batch(store, lease, "one", PollBatch("query-source", "two",
                                                       (record("finish", "completed"),)))
                self.assertFalse(current(store, snapshot))
                project_source(store, "query-source")
                newer = capture(store, "query-source")
                self.assertTrue(current(store, newer))
                set_source_capture(store, source_id="query-source", enabled=False)
                self.assertFalse(current(store, newer))
                self.assertIsNone(capture(store, "query-source"))
            finally:
                store.close()


if __name__ == "__main__":
    unittest.main()
