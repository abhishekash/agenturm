"""Installed Rust v3 source-gap fixture parity with persisted Python projection."""
import json
import os
import subprocess
import unittest
from pathlib import Path

from tests.test_rust_domain_reducer_v2 import python_projection


GOLDEN = Path(__file__).parent / "fixtures" / "rust-domain-reducer-v3.json"


class RustDomainReducerV3Tests(unittest.TestCase):
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

    def test_source_gap_capacity_rejects_2001_rows(self):
        fixture = dict(self.cases[0]["input"], rows=[], invalidated=[],
                       source_gaps=[{"native_session_id": f"session-{index:04}",
                                     "last_record_ns": "1"} for index in range(2001)])
        result = self.run_reducer(fixture)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
