"""Exercise explicitly enabled producer-captured text through the Rust CLI."""

from __future__ import annotations

import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
APP = os.environ.get("AGENTURM_RUST_APP_BIN")
REQUIRED = os.environ.get("AGENTURM_REQUIRE_VENDOR_SDK_TESTS") == "1"
SOURCE = "otlp.private-payload"
TRACE = "1234567890abcdef1234567890abcdef"
SPAN = "1234567890abcdef"
SESSION = "payload-fixture"
INPUT = "synthetic captured request"
OUTPUT = "synthetic captured response"
UNRELATED = "synthetic excluded provider request"
PRODUCER = ROOT / "tests/fixtures/rust_vendor_trace_producer.py"


def have_openai():
    try:
        for package in ("openai", "openinference-instrumentation-openai"):
            importlib.metadata.version(package)
    except importlib.metadata.PackageNotFoundError:
        return False
    return True


@unittest.skipUnless(APP or REQUIRED, "set AGENTURM_RUST_APP_BIN to the standalone Rust binary")
class RustTelemetryPayloadTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(APP, "set AGENTURM_RUST_APP_BIN to the standalone Rust binary")
        directory = tempfile.TemporaryDirectory(prefix="agenturm-telemetry-payload-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.db = self.root / "history.sqlite"
        self.selector = self.root / "selection.json"
        self.environment = os.environ | {"PYTHONPATH": str(ROOT), "OTEL_SDK_DISABLED": "false"}

    def invoke(self, *args, code=0):
        result = subprocess.run([APP, *args], cwd=ROOT, env=self.environment,
                                text=True, capture_output=True, timeout=45)
        self.assertEqual(result.returncode, code, (result.stdout, result.stderr))
        return result

    def action(self, operation, scope):
        return self.invoke("trace-payload", operation, "--db", self.db, "--source-id", SOURCE,
                           "--payload-scope", scope)

    def snapshot(self, session):
        result = self.invoke("trace-payload", "snapshot", "--db", self.db,
                             "--source-id", SOURCE, "--session-id", session)
        return json.loads(result.stdout)["snapshot_id"]

    def preview(self, selection, *, code=0):
        self.selector.write_text(json.dumps(selection))
        result = self.invoke("trace-payload", "preview", "--db", self.db,
                             "--selection", self.selector, code=code)
        if code:
            self.assertFalse(result.stdout)
            self.assertNotIn(INPUT, result.stderr)
            self.assertNotIn(OUTPUT, result.stderr)
            return None
        self.assertEqual(result.stderr, "")
        return json.loads(result.stdout)

    def metadata(self):
        result = self.invoke("query", "--db", self.db, "--harness-id", SOURCE)
        for body in (INPUT, OUTPUT, UNRELATED, "private-vendor-fixture"):
            self.assertNotIn(body, result.stdout)
        return json.loads(result.stdout)["observations"]

    def test_standalone_payload_uses_exact_grant_and_snapshot(self):
        producer = f'''
import time
from observatory.client import Client
client = Client(transport="otlp", strict=True, session_id={SESSION!r}, capture_content=True)
start = time.time_ns()
span = {{"trace_id": {TRACE!r}, "span_id": {SPAN!r}, "status": "OK",
         "start_unix_nano": start, "end_unix_nano": start + 1,
         "attributes": {{"agenturm.payload.input": {INPUT!r}, "agenturm.payload.output": {OUTPUT!r},
                         "input.value": {UNRELATED!r}}}}}
assert client._post_spans([span])
assert client._post_spans([span])
'''
        self.invoke("run", "--db", self.db, "--source-id", SOURCE,
                    "--payload-scope", "standalone", "--", sys.executable, "-c", producer)
        self.assertEqual(len(self.metadata()), 1)
        data = self.db.read_bytes()
        self.assertNotIn(UNRELATED.encode(), data)
        self.assertEqual(self.db.stat().st_mode & 0o777, 0o600)
        selection = {
            "binding": {"source_id": SOURCE, "native_source_id": None},
            "trace_id": TRACE, "span_id": SPAN, "session_id": SESSION,
            "turn_id": None, "worker_id": None, "snapshot_id": self.snapshot(SESSION),
        }
        self.preview(selection, code=1)
        self.action("grant", "harness.codex")
        self.preview(selection, code=1)
        self.action("grant", "standalone")
        self.preview({**selection, "session_id": "another-session"}, code=1)
        self.preview({**selection, "snapshot_id": "0"}, code=1)
        self.assertEqual(self.preview(selection), {
            "origin": "producer_captured_telemetry",
            "payload": {"input": INPUT, "output": OUTPUT},
        })
        self.action("revoke", "standalone")
        self.preview(selection, code=1)

    @unittest.skipUnless(have_openai() or REQUIRED, "install OpenAI and OpenInference test packages")
    def test_openai_automatic_capture_selects_only_message_text(self):
        self.assertTrue(have_openai(), "install OpenAI and OpenInference test packages")
        source_scope = "harness.codex"
        session = "session.openai"
        result = self.invoke(
            "run", "--db", self.db, "--source-id", SOURCE, "--payload-scope", source_scope,
            "--", sys.executable, "-m", "observatory", "run", "--transport", "otlp",
            "--capture-content", "--session-id", session, "--harness-id", source_scope,
            "--turn-id", "turn.fixture", "--worker-id", "worker/fixture", PRODUCER, "openai",
            code=23,
        )
        self.assertIn("metadata and opted-in message text", result.stderr)
        self.assertNotIn("export failed", result.stderr)
        spans = self.metadata()
        self.assertEqual(len(spans), 4)
        self.action("grant", source_scope)
        complete = [row for row in spans if row["observation"]["parent_span_id"]
                    and row["observation"]["status"] == "ok"]
        self.assertEqual(len(complete), 2)
        for row in complete:
            span = row["observation"]
            selection = {
                "binding": {"source_id": SOURCE, "native_source_id": source_scope},
                "trace_id": span["trace_id"], "span_id": span["span_id"],
                "session_id": session, "turn_id": "turn.fixture", "worker_id": "worker/fixture",
                "snapshot_id": self.snapshot(session),
            }
            self.preview({**selection, "turn_id": "another-turn"}, code=1)
            receipt = self.preview(selection)
            self.assertEqual(receipt["origin"], "producer_captured_telemetry")
            self.assertEqual(receipt["payload"]["input"], "private-vendor-fixture-question")
            self.assertEqual(receipt["payload"]["output"], "private-vendor-fixture-answer")
        self.assertNotIn(b"fixture-key-never-used", self.db.read_bytes())


if __name__ == "__main__":
    unittest.main()
