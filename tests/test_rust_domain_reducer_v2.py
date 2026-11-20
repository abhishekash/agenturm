"""Installed Rust v2 domain fixture parity with Python's persisted projection."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_admission import _metadata, activate_source
from observatory.plugin_projection import MAX_JSON_SAFE_INTEGER, project_source
from observatory.plugin_wire import validate_batch


GOLDEN = Path(__file__).parent / "fixtures" / "rust-domain-reducer-v2.json"


def python_projection(fixture):
    with tempfile.TemporaryDirectory() as directory:
        store = Store(str(Path(directory) / "projection.db"))
        try:
            with store._connect() as db:
                db.execute("UPDATE schema_meta SET value=? WHERE key='receiver_id'",
                           (fixture["receiver_id"],))
            with patch.object(store, "receiver_namespace", return_value=fixture["receiver_id"]):
                activate_source(store, plugin_id=fixture["plugin_id"],
                                source_id=fixture["source_id"], generation="a" * 64)
                with store._connect() as db:
                    db.execute("UPDATE plugin_source_cursor_v1 SET dirty_seq=?,policy_epoch=? "
                               "WHERE source_id=?", (fixture["dirty_seq"],
                                                     fixture["policy_epoch"], fixture["source_id"]))
                    for row in fixture["rows"]:
                        identity = {key.removeprefix("native_"): value
                                    for key, value in row.items()
                                    if key in {"native_session_id", "native_worker_id",
                                               "native_turn_id", "native_operation_id"}
                                    and value}
                        record = {"event_id": row["event_id_hash"],
                                  "revision": row["revision"], "kind": row["kind"],
                                  "time_ns": row["time_ns"], "payload": row["payload"],
                                  **({"identity": identity} if identity else {})}
                        validate_batch({"type": "batch", "protocol": 1,
                                        "source_id": fixture["source_id"], "cursor": "fixture",
                                        "records": [record]}, source_id=fixture["source_id"])
                        _metadata(record)
                        db.execute(
                            "INSERT INTO plugin_evidence_v1(source_id,event_id_hash,revision,kind,"
                            "time_ns,payload_json,record_digest,native_session_id,native_worker_id,"
                            "native_turn_id,native_operation_id,policy_epoch,admitted_ns) "
                            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                            (fixture["source_id"], row["event_id_hash"], row["revision"],
                             row["kind"], int(row["time_ns"]),
                             json.dumps(row["payload"], sort_keys=True, separators=(",", ":")),
                             "a" * 64, row.get("native_session_id") or "",
                             row.get("native_worker_id") or "", row.get("native_turn_id") or "",
                             row.get("native_operation_id") or "", fixture["policy_epoch"],
                             int(row["admitted_ns"])))
                    for item in fixture.get("invalidated", []):
                        db.execute("INSERT INTO plugin_retention_invalidated_v1(source_id,"
                                   "native_session_id,last_record_ns) VALUES(?,?,?)",
                                   (fixture["source_id"], item["native_session_id"],
                                    int(item["last_record_ns"])))
                    for item in fixture.get("source_gaps", []):
                        db.execute("INSERT INTO plugin_source_gap_invalidated_v1(source_id,"
                                   "native_session_id,last_record_ns) VALUES(?,?,?)",
                                   (fixture["source_id"], item["native_session_id"],
                                    int(item["last_record_ns"])))
                project_source(store, fixture["source_id"])
                with store._connect() as db:
                    sessions = db.execute(
                        "SELECT native_session_id,session_key,state,reason_code,state_time_ns,"
                        "last_record_ns,usage_json,parent_native_session_id,relationship_kind,"
                        "relationship_state FROM plugin_session_projection_v1 WHERE source_id=?",
                        (fixture["source_id"],)).fetchall()
                    activities = db.execute(
                        "SELECT payload FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
                        (fixture["source_id"],)).fetchall()
                result = []
                for row in sessions:
                    state = row["state"]
                    result.append({
                        "native_session_id": row["native_session_id"],
                        "session_key": row["session_key"],
                        "reported_state": state,
                        "run_state": {"started": "running", "paused": "waiting",
                                      "cancelled": "interrupted"}.get(state, state),
                        "reason_code": row["reason_code"],
                        "state_time_ns": row["state_time_ns"],
                        "last_record_ns": row["last_record_ns"],
                        "metadata_usage": {
                            key: value if value <= MAX_JSON_SAFE_INTEGER else str(value)
                            for key, value in json.loads(row["usage_json"]).items()},
                        "parent_native_session_id": row["parent_native_session_id"],
                        "relationship_kind": row["relationship_kind"],
                        "relationship_state": row["relationship_state"],
                    })
                return {"schema_version": fixture["schema_version"],
                        "sessions": sorted(result, key=lambda row: row["native_session_id"]),
                        "activities": sorted((json.loads(row["payload"]) for row in activities),
                                             key=lambda fact: fact["id"])}
        finally:
            store.close()


class RustDomainReducerV2Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        executable = os.environ.get("AGENTURM_RUST_REDUCER_BIN")
        if not executable:
            raise unittest.SkipTest("Rust reducer binary is built in the Rust CI job")
        cls.binary = str(Path(executable).resolve())
        cls.cases = json.loads(GOLDEN.read_text())["cases"]

    def run_reducer(self, fixture):
        return subprocess.run([self.binary], input=json.dumps(fixture), capture_output=True,
                              text=True, timeout=5)

    def test_valid_golden_cases_match_store(self):
        for case in self.cases:
            if "expected" not in case:
                continue
            with self.subTest(case=case["name"]):
                self.assertEqual(python_projection(case["input"]), case["expected"])
                result = self.run_reducer(case["input"])
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), case["expected"])

    def test_invalid_cases_reject(self):
        for case in self.cases:
            if "expected" in case:
                continue
            with self.subTest(case=case["name"]):
                result = self.run_reducer(case["input"])
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
