"""Golden parity for the pure Rust reducer against Python's Store projection."""
import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import activate_source
from observatory.plugin_projection import MAX_JSON_SAFE_INTEGER, project_source, sessions
from observatory.plugin_wire import ProtocolError


GOLDEN = Path(__file__).parent / "fixtures" / "rust-domain-reducer-v1.json"


def session_capacity_fixture(count):
    return {"schema_version": 1, "receiver_id": "receiver-fixture",
            "source_id": "source-a", "policy_epoch": 1, "dirty_seq": 1,
            "rows": [{"event_id_hash": hashlib.sha256(f"event-{index}".encode()).hexdigest(),
                      "revision": 0, "kind": "lifecycle",
                      "time_ns": str(index + 1), "admitted_ns": str(index + 1),
                      "native_session_id": f"session-{index:04d}",
                      "payload": {"state": "completed"}}
                     for index in range(count)], "invalidated": []}


def python_projection(fixture):
    """Run the production Python reducer over a disposable admitted snapshot."""
    with tempfile.TemporaryDirectory() as directory:
        store = Store(str(Path(directory) / "projection.db"))
        try:
            with store._connect() as db:
                db.execute("UPDATE schema_meta SET value=? WHERE key='receiver_id'",
                           (fixture["receiver_id"],))
            with patch.object(store, "receiver_namespace", return_value=fixture["receiver_id"]):
                activate_source(store, plugin_id="fixture.plugin",
                                source_id=fixture["source_id"], generation="a" * 64)
                with store._connect() as db:
                    db.execute("UPDATE plugin_source_cursor_v1 SET dirty_seq=?,policy_epoch=? "
                               "WHERE source_id=?", (fixture["dirty_seq"],
                                                     fixture["policy_epoch"], fixture["source_id"]))
                    for row in fixture["rows"]:
                        db.execute(
                            "INSERT INTO plugin_evidence_v1(source_id,event_id_hash,revision,kind,"
                            "time_ns,payload_json,record_digest,native_session_id,policy_epoch,"
                            "admitted_ns) VALUES(?,?,?,?,?,?,?,?,?,?)",
                            (fixture["source_id"], row["event_id_hash"], row["revision"],
                             row["kind"], int(row["time_ns"]),
                             json.dumps(row["payload"], sort_keys=True, separators=(",", ":")),
                             "a" * 64, row.get("native_session_id") or "",
                             fixture["policy_epoch"], int(row["admitted_ns"])))
                    for item in fixture.get("invalidated", []):
                        db.execute(
                            "INSERT INTO plugin_retention_invalidated_v1(source_id,"
                            "native_session_id,last_record_ns) VALUES(?,?,?)",
                            (fixture["source_id"], item["native_session_id"],
                             int(item["last_record_ns"])))
                project_source(store, fixture["source_id"])
                with store._connect() as db:
                    raw = db.execute(
                        "SELECT native_session_id,session_key,state,reason_code,"
                        "state_time_ns,last_record_ns,usage_json FROM "
                        "plugin_session_projection_v1 WHERE source_id=?",
                        (fixture["source_id"],)).fetchall()
                normalized = {row["native_session_id"]: {
                     "native_session_id": row["native_session_id"],
                     "session_key": row["session_key"],
                     "reported_state": row["state"],
                     "run_state": {"started": "running", "paused": "waiting",
                                   "cancelled": "interrupted"}.get(row["state"], row["state"]),
                     "reason_code": row["reason_code"],
                     "state_time_ns": row["state_time_ns"],
                     "last_record_ns": row["last_record_ns"],
                     "metadata_usage": {
                         key: value if value <= MAX_JSON_SAFE_INTEGER else str(value)
                         for key, value in json.loads(row["usage_json"]).items()}}
                    for row in raw}
                page = sessions(store, limit=1000)
                if len(raw) <= 1000:
                    if page["omitted_sessions"] or len(page["sessions"]) != len(raw):
                        raise AssertionError("Python display oracle was truncated")
                    for displayed in page["sessions"]:
                        projected = normalized[displayed["native_session_id"]]
                        for field, rendered in (("session_key", displayed["session_id"]),
                                                ("reported_state", displayed["reported_state"]),
                                                ("run_state", displayed["run_state"]),
                                                ("reason_code", displayed["reason_code"]),
                                                ("last_record_ns", displayed["last_record_ns"]),
                                                ("metadata_usage", displayed["metadata_usage"])):
                            if projected[field] != rendered:
                                raise AssertionError(f"Python projection/display mismatch: {field}")
                elif not page["omitted_sessions"] or len(page["sessions"]) != 1000:
                    raise AssertionError("Python display limit was not observed")
                return {"schema_version": 1, "sessions": [
                    row for _native_id, row in sorted(normalized.items())]}
        finally:
            store.close()


class RustDomainReducerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        executable = os.environ.get("AGENTURM_RUST_REDUCER_BIN")
        if not executable:
            raise unittest.SkipTest("Rust reducer binary is built in the Rust CI job")
        cls.binary = str(Path(executable).resolve())
        cls.cases = json.loads(GOLDEN.read_text())["cases"]

    def run_reducer(self, fixture):
        raw = fixture if isinstance(fixture, str) else json.dumps(fixture)
        return subprocess.run([self.binary], input=raw,
                              capture_output=True, text=True, timeout=5)

    def test_golden_cases_match_python_store_projection(self):
        for case in self.cases:
            if "expected" not in case:
                continue
            with self.subTest(case=case["name"]):
                self.assertEqual(python_projection(case["input"]), case["expected"])
                result = self.run_reducer(case["input"])
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), case["expected"])

    def test_invalid_cases_fail_closed(self):
        for case in self.cases:
            if "expected" in case:
                continue
            with self.subTest(case=case["name"]):
                result = self.run_reducer(case.get("raw_input", case.get("input")))
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                if case.get("python_error"):
                    with self.assertRaisesRegex(ProtocolError, case["python_error"]):
                        python_projection(case["input"])
                if case.get("python_empty"):
                    self.assertEqual(python_projection(case["input"]),
                                     {"schema_version": 1, "sessions": []})

    def test_invalid_utf8_is_rejected_before_reduction(self):
        result = subprocess.run([self.binary], input=b"\xff",
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, b"")

    def test_complete_python_oracle_and_session_capacity(self):
        for count in (201, 2000):
            with self.subTest(count=count):
                full = session_capacity_fixture(count)
                expected = python_projection(full)
                self.assertEqual(len(expected["sessions"]), count)
                result = self.run_reducer(full)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), expected)

        over_capacity = session_capacity_fixture(2001)
        with self.assertRaisesRegex(ProtocolError, "plugin session projection capacity reached"):
            python_projection(over_capacity)
        result = self.run_reducer(over_capacity)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
