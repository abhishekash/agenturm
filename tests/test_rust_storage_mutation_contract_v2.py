"""Cross-language fixture for the feature-off lifecycle mutation contract."""
import hashlib
import json
import unittest
from pathlib import Path

from observatory.plugin_admission import _validated
from observatory.plugin_wire import PollBatch


FIXTURE = Path(__file__).parent / "fixtures" / "rust_storage_mutation_v2.json"


class RustStorageMutationContractTests(unittest.TestCase):
    def test_python_admission_derives_the_same_prepared_evidence_and_digest(self):
        fixture = json.loads(FIXTURE.read_text())
        request = fixture["request"]
        operation = request["operation"]
        source_id = operation["lease"]["source_id"]
        record = fixture["source_record"]
        digest, rows, _ = _validated(
            PollBatch(source_id, operation["cursor"], (record,)), source_id
        )
        self.assertEqual(operation["batch_digest"], digest)
        row = rows[0]
        fields = (
            "source_id", "event_id_hash", "revision", "kind", "time_ns",
            "payload_json", "record_digest", "native_session_id",
            "native_worker_id", "native_turn_id", "native_operation_id", "member_id",
        )
        prepared = dict(zip(fields, row, strict=True))
        self.assertEqual(prepared.pop("source_id"), source_id)
        self.assertEqual(prepared.pop("member_id"), "")
        self.assertEqual(operation["records"], [prepared])
        canonical = json.dumps(
            request, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode()
        self.assertEqual(hashlib.sha256(canonical).hexdigest(), fixture["request_digest"])


if __name__ == "__main__":
    unittest.main()
