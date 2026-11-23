"""Exercise explicitly enabled producer-captured text through the Rust CLI."""

from __future__ import annotations

import importlib.metadata
import json
import os
import shutil
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


def installed(*packages):
    try:
        for package in packages:
            importlib.metadata.version(package)
    except importlib.metadata.PackageNotFoundError:
        return False
    return True


OPENAI = installed("openai", "openinference-instrumentation-openai")
ANTHROPIC = installed("anthropic", "openinference-instrumentation-anthropic")
LANGCHAIN = OPENAI and installed("langchain-openai", "openinference-instrumentation-langchain")
LANGGRAPH = installed("langgraph", "langchain-openai", "openai")
AGENTS = installed("openai-agents")
PYDANTIC = installed("pydantic-ai")
ADK = installed("google-adk")
NODE = shutil.which("node")


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
        return json.loads(result.stdout)

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
        return [
            row for row in json.loads(result.stdout)["observations"]
            if row["observation"]["kind"] == "trace_span"
        ]

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
            "turn_id": None, "worker_id": None, **self.snapshot(SESSION),
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

    @unittest.skipUnless(OPENAI or REQUIRED, "install OpenAI and OpenInference test packages")
    def test_openai_automatic_capture_selects_only_message_text(self):
        self.assertTrue(OPENAI, "install OpenAI and OpenInference test packages")
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
                **self.snapshot(session),
            }
            self.preview({**selection, "turn_id": "another-turn"}, code=1)
            receipt = self.preview(selection)
            self.assertEqual(receipt["origin"], "producer_captured_telemetry")
            self.assertEqual(receipt["payload"]["input"], "private-vendor-fixture-question")
            self.assertEqual(receipt["payload"]["output"], "private-vendor-fixture-answer")
        self.assertNotIn(b"fixture-key-never-used", self.db.read_bytes())

    def exercise_vendor(self, vendor, *, available, automatic=False):
        self.assertTrue(available, f"SDK test dependency for {vendor} is required")
        session = f"session.{vendor}"
        harness = "harness.codex"
        arguments = [sys.executable]
        if automatic:
            arguments.extend([
                "-m", "observatory", "run", "--transport", "otlp", "--capture-content",
                "--session-id", session, "--harness-id", harness, "--turn-id", "turn.fixture",
                "--worker-id", "worker/fixture",
            ])
        self.environment["AGENTURM_FIXTURE_CAPTURE_TEXT"] = "1"
        result = self.invoke(
            "run", "--db", self.db, "--source-id", SOURCE, "--payload-scope", harness,
            "--", *arguments, PRODUCER, vendor, code=23,
        )
        self.assertNotIn("export failed", result.stderr)
        snapshot = self.snapshot(session)
        self.action("grant", harness)
        outputs = []
        for row in self.metadata():
            span = row["observation"]
            self.selector.write_text(json.dumps({
                "binding": {"source_id": SOURCE, "native_source_id": harness},
                "trace_id": span["trace_id"], "span_id": span["span_id"],
                "session_id": session, "turn_id": "turn.fixture", "worker_id": "worker/fixture",
                **snapshot,
            }))
            read = subprocess.run([APP, "trace-payload", "preview", "--db", self.db,
                                   "--selection", self.selector], text=True, capture_output=True,
                                  timeout=10)
            self.assertIn(read.returncode, (0, 1), read.stderr)
            if read.returncode == 0:
                value = json.loads(read.stdout)
                self.assertEqual(value["origin"], "producer_captured_telemetry")
                outputs.append(value["payload"])
            else:
                self.assertFalse(read.stdout)
        self.assertTrue(any("private-vendor-fixture-question" in entry.get("input", "")
                            for entry in outputs), f"{vendor} input coverage")
        self.assertTrue(any("private-vendor-fixture-answer" in entry.get("output", "")
                            for entry in outputs), f"{vendor} output coverage")
        self.assertNotIn("lookup_fixture", json.dumps(outputs))
        self.assertNotIn(b"fixture-key-never-used", self.db.read_bytes())

    @unittest.skipUnless(ANTHROPIC or REQUIRED, "install Anthropic and OpenInference")
    def test_anthropic_message_text(self):
        self.exercise_vendor("anthropic", available=ANTHROPIC, automatic=True)

    @unittest.skipUnless(LANGCHAIN or REQUIRED, "install LangChain and OpenInference")
    def test_langchain_model_message_text(self):
        self.exercise_vendor("langchain", available=LANGCHAIN, automatic=True)

    @unittest.skipUnless(LANGGRAPH or REQUIRED, "install LangGraph and LangChain OpenAI")
    def test_langgraph_callback_model_text(self):
        self.exercise_vendor("langgraph", available=LANGGRAPH)

    @unittest.skipUnless(AGENTS or REQUIRED, "install OpenAI Agents")
    def test_openai_agents_model_text(self):
        self.exercise_vendor("openai-agents", available=AGENTS)

    @unittest.skipUnless(PYDANTIC or REQUIRED, "install Pydantic AI")
    def test_pydantic_ai_model_text(self):
        self.exercise_vendor("pydantic-ai", available=PYDANTIC)

    @unittest.skipUnless(ADK or REQUIRED, "install Google ADK")
    def test_google_adk_model_text(self):
        self.exercise_vendor("google-adk", available=ADK)

    @unittest.skipUnless(NODE, "install Node.js")
    def test_node_explicit_payload(self):
        module = (ROOT / "clients/javascript/index.mjs").as_uri()
        producer = f"""
import {{ Client }} from {json.dumps(module)};
const client = new Client({{transport:'otlp', strict:true, captureContent:true}});
await client.trace({{sessionId:{SESSION!r}, attributes: {{
  'agenturm.payload.input': {INPUT!r}, 'agenturm.payload.output': {OUTPUT!r},
  'input.value': {UNRELATED!r},
}}}}, () => {{}});
"""
        self.invoke("run", "--db", self.db, "--source-id", SOURCE, "--payload-scope", "standalone",
                    "--", NODE, "--input-type=module", "--eval", producer)
        row, = self.metadata()
        span = row["observation"]
        self.action("grant", "standalone")
        self.assertEqual(self.preview({
            "binding": {"source_id": SOURCE, "native_source_id": None},
            "trace_id": span["trace_id"], "span_id": span["span_id"], "session_id": SESSION,
            "turn_id": None, "worker_id": None, **self.snapshot(SESSION),
        })["payload"], {"input": INPUT, "output": OUTPUT})
        self.assertNotIn(UNRELATED.encode(), self.db.read_bytes())


if __name__ == "__main__":
    unittest.main()
