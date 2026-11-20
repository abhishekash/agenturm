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

    def test_exact_activity_matches_pi_correlation_correction_and_gap(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Store(str(Path(directory) / "store.db"))
            try:
                lease = activate_source(store, plugin_id="example.rust-pi-harness",
                                        source_id="pi-query", generation="a" * 64)
                def fact(event, kind, payload, stamp, *, turn=None, operation=None,
                         revision=0):
                    identity = {"session_id": "pi-native"}
                    if turn is not None:
                        identity["turn_id"] = turn
                    if operation is not None:
                        identity["operation_id"] = operation
                    return {"event_id": event, "revision": revision, "kind": kind,
                            "time_ns": str(stamp), "identity": identity, "payload": payload}
                commit_batch(store, lease, "", PollBatch("pi-query", "one", (
                    fact("user-one", "message", {"role": "user"}, 10, turn="first"),
                    fact("call", "tool", {"category": "file", "result": "started"},
                         11, turn="first", operation="shared"))))
                commit_batch(store, lease, "one", PollBatch("pi-query", "two", (
                    fact("user-two", "message", {"role": "user"}, 20, turn="second"),
                    fact("result", "tool", {"category": "other", "result": "failed"},
                         21, operation="shared"))))
                project_source(store, "pi-query")
                before = capture(store, "pi-query")
                self.assertTrue(current(store, before))
                expected = next(row for row in before.expected["activities"]
                                if row["result"] == "failed")
                self.assertEqual(expected["metadata"]["tool_turn_relation"], "matched_call")
                request = {"schema_version": 1, "snapshot": before.fixture,
                           "query": {"kind": "activity", "source_id": "pi-query",
                                     "event_id": expected["id"]}}
                queried = self.query(request)
                self.assertEqual(queried.returncode, 0, queried.stderr)
                self.assertEqual(json.loads(queried.stdout)["activity"], expected)
                self.assertEqual(json.loads(queried.stdout)["coverage"], "partial")
                commit_batch(store, lease, "two", PollBatch("pi-query", "three", (
                    fact("result", "tool", {"category": "other", "result": "succeeded"},
                         21, operation="shared", revision=1),)))
                self.assertFalse(current(store, before))
                project_source(store, "pi-query")
                corrected = capture(store, "pi-query")
                request["snapshot"] = corrected.fixture
                after = self.query(request)
                self.assertEqual(after.returncode, 0, after.stderr)
                self.assertEqual(json.loads(after.stdout)["activity"]["result"], "succeeded")
                gap = {"event_id": "gap", "revision": 0, "kind": "diagnostic",
                       "time_ns": "30", "payload": {"code": "possible_source_gap",
                                                   "severity": "warning"}}
                commit_batch(store, lease, "three", PollBatch("pi-query", "four", (gap,)))
                project_source(store, "pi-query")
                gapped = capture(store, "pi-query")
                request["snapshot"] = gapped.fixture
                missing = self.query(request)
                self.assertEqual(missing.returncode, 0, missing.stderr)
                self.assertIsNone(json.loads(missing.stdout)["activity"])
                self.assertEqual(json.loads(missing.stdout)["coverage"], "partial")
            finally:
                store.close()

    def test_exact_activity_preserves_uncorrelated_non_pi_turn(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Store(str(Path(directory) / "store.db"))
            try:
                lease = activate_source(store, plugin_id="example.rust-kiro-harness",
                                        source_id="kiro-query", generation="a" * 64)
                records = tuple({
                    "event_id": event, "revision": 0, "kind": "tool",
                    "time_ns": str(stamp), "identity": {
                        "session_id": "kiro-native", "turn_id": "original",
                        "operation_id": "denied"},
                    "payload": {"category": "shell", "result": "denied"}}
                    for event, stamp in (("call", 10), ("result", 20)))
                commit_batch(store, lease, "", PollBatch("kiro-query", "one", records))
                project_source(store, "kiro-query")
                snapshot = capture(store, "kiro-query")
                self.assertTrue(current(store, snapshot))
                for expected in snapshot.expected["activities"]:
                    self.assertEqual(expected["metadata"]["tool_turn_relation"],
                                     "unverified_native_turn")
                    request = {"schema_version": 1, "snapshot": snapshot.fixture,
                               "query": {"kind": "activity", "source_id": "kiro-query",
                                         "event_id": expected["id"]}}
                    result = self.query(request)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(json.loads(result.stdout)["activity"], expected)
            finally:
                store.close()

    def test_exact_activity_quarantines_legacy_pi_result_without_call(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Store(str(Path(directory) / "store.db"))
            try:
                lease = activate_source(store, plugin_id="example.rust-pi-harness",
                                        source_id="old-pi-query", generation="a" * 64)
                old_result = {"event_id": "old-result", "revision": 0, "kind": "tool",
                              "time_ns": "20", "identity": {
                                  "session_id": "pi-native", "turn_id": "later-user",
                                  "operation_id": "call"},
                              "payload": {"category": "file", "result": "failed"}}
                commit_batch(store, lease, "", PollBatch("old-pi-query", "one", (old_result,)))
                project_source(store, "old-pi-query")
                snapshot = capture(store, "old-pi-query")
                expected = snapshot.expected["activities"][0]
                self.assertEqual(expected["turn_id"], "")
                self.assertEqual(expected["metadata"]["tool_turn_relation"], "missing_call")
                request = {"schema_version": 1, "snapshot": snapshot.fixture,
                           "query": {"kind": "activity", "source_id": "old-pi-query",
                                     "event_id": expected["id"]}}
                result = self.query(request)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["activity"], expected)
            finally:
                store.close()


if __name__ == "__main__":
    unittest.main()
