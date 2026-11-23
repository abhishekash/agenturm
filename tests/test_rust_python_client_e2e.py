"""Exercise the Python producer through the Rust launcher and receiver."""
from __future__ import annotations

import importlib.util
import json
import os
import select
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


RUST_APP = os.environ.get("AGENTURM_RUST_APP_BIN")
REPO = Path(__file__).resolve().parents[1]
SDK_AVAILABLE = bool(importlib.util.find_spec("opentelemetry"))
if SDK_AVAILABLE:
    SDK_AVAILABLE = bool(importlib.util.find_spec("opentelemetry.sdk"))


@unittest.skipUnless(RUST_APP, "set AGENTURM_RUST_APP_BIN to the standalone Rust binary")
class RustPythonProducerTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.database = self.root / "history.sqlite"
        self.producer = self.root / "producer.py"
        self.environment = os.environ | {
            "PYTHONPATH": str(REPO),
            "OBSERVATORY_TOKEN": "legacy-private-receiver-token",
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS": "authorization=legacy-private-receiver-token",
            "HTTP_PROXY": "http://invalid.example",
            "http_proxy": "http://invalid.example",
            "NO_PROXY": "",
            "no_proxy": "",
        }

    def observations(self, source):
        query = subprocess.run(
            [RUST_APP, "query", "--db", self.database, "--harness-id", source, "--limit", "4096"],
            capture_output=True, text=True, check=True, timeout=10,
        )
        self.assertNotIn("synthetic private", query.stdout)
        return json.loads(query.stdout)["observations"]

    def launch(self, source, status):
        result = subprocess.run(
            [RUST_APP, "run", "--db", self.database, "--source-id", source,
             "--", sys.executable, self.producer],
            env=self.environment, capture_output=True, text=True, timeout=25,
        )
        self.assertEqual(result.returncode, status, result.stderr)
        return json.loads(result.stdout)

    def test_launcher_reopens_exact_python_scope_ancestry_and_exit(self):
        self.producer.write_text("""
import asyncio
import json
from observatory.client import Client

async def produce():
    client = Client(transport="otlp", strict=True, capture_exception_message=True)
    async with client.trace("synthetic private name", task="synthetic private task",
                            session_id="python-session", harness_id="harness.pi",
                            turn_id="python-turn", worker_id="python-worker") as run:
        async with run.span("synthetic private parent") as parent:
            parent.set_attribute("llm.cost_usd", 10)
            parent.set_attribute("llm.cost_known", False)
            parent.set_attribute("gen_ai.usage.input_tokens", 10)
            parent.set_attribute("llm.token_count.prompt", 20)
            async with run.span("synthetic private child") as child:
                child.set_attribute("session.id", "synthetic private alternate session")
                child.set_attribute("agenturm.reported_cost.amount", 0)
                child.set_attribute("agenturm.reported_cost.currency", "EUR")
                child.set_attribute("llm.token_count.prompt", 0)
                child.set_attribute("gen_ai.usage.cached_input_tokens", 8)
                child.set_attribute("llm.token_count.completion", "11")
                child.add_event("synthetic private event")
                child.set_status("ERROR")
    print(json.dumps({"root": run.root.span_id, "parent": parent.span_id,
                      "child": child.span_id, "trace": run.trace_id,
                      "started": child.start_ns, "ended": child.end_ns}))

asyncio.run(produce())
raise SystemExit(23)
""")
        ids = self.launch("otlp.python", 23)
        rows = self.observations("otlp.python")
        costs = [row for row in rows if row["observation"]["kind"] == "reported_cost"]
        usage, = [row for row in rows if row["observation"]["kind"] == "span_token_usage"]
        self.assertEqual(usage["observation"], {
            "kind": "span_token_usage", "target_source_id": "harness.pi",
            "span": {"trace_id": ids["trace"], "span_id": ids["child"]},
            "input_tokens": 0, "cache_read_input_tokens": 8, "output_tokens": 11,
        })
        self.assertEqual(usage["identity"]["turn_id"], "python-turn")
        self.assertEqual(usage["identity"]["worker_id"], "python-worker")
        rows = [row for row in rows if row["observation"]["kind"] == "trace_span"]
        self.assertEqual(len(costs), 1)
        self.assertEqual(costs[0]["observation"], {
            "kind": "reported_cost", "currency": "EUR", "nanounits": "0",
            "span": {"trace_id": ids["trace"], "span_id": ids["child"]},
        })
        self.assertEqual(costs[0]["identity"]["turn_id"], "python-turn")
        self.assertEqual(costs[0]["identity"]["worker_id"], "python-worker")
        self.assertEqual(len(rows), 3)
        for row in rows:
            identity = row["identity"]
            self.assertEqual(identity["session_id"], "python-session")
            self.assertEqual(identity["turn_id"], "python-turn")
            self.assertEqual(identity["worker_id"], "python-worker")
            self.assertEqual(row["observation"]["target_source_id"], "harness.pi")
            self.assertEqual(row["observation"]["trace_id"], ids["trace"])
        by_span = {row["observation"]["span_id"]: row for row in rows}
        self.assertIsNone(by_span[ids["root"]]["observation"].get("parent_span_id"))
        self.assertEqual(by_span[ids["parent"]]["observation"]["parent_span_id"], ids["root"])
        child = by_span[ids["child"]]
        self.assertEqual(child["observation"]["parent_span_id"], ids["parent"])
        self.assertEqual(child["observation"]["status"], "error")
        self.assertEqual(int(child["observation"]["started_at_unix_ns"]), ids["started"])
        self.assertEqual(int(child["observed_at_unix_ns"]), ids["ended"])

    def test_standalone_receiver_reports_partial_failure_and_commits_batches(self):
        self.producer.write_text("""
from observatory.client import Client

client = Client(transport="otlp")
with client.trace() as run:
    assert not run.progress("synthetic private progress")
    assert "does not export progress" in str(client.last_error)
assert "rejected one or more spans" in str(client.last_error)
with client.trace(session_id="python-large") as run:
    for index in range(1024):
        run.span(f"synthetic private span {index}").finish()
assert client.last_error is None, client.last_error
""")
        receiver = subprocess.Popen(
            [RUST_APP, "serve-traces", "--listen", "127.0.0.1:0", "--db", self.database,
             "--source-id", "otlp.python-standalone"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            self.assertTrue(select.select([receiver.stdout], [], [], 8)[0])
            announcement = receiver.stdout.readline()
            self.assertTrue(announcement.startswith("Listening for OTLP traces at "), announcement)
            endpoint = announcement.rsplit(" ", 1)[-1].strip()
            result = subprocess.run(
                [sys.executable, self.producer],
                env=self.environment | {"OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": endpoint},
                capture_output=True, text=True, timeout=20,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
        finally:
            if receiver.poll() is None:
                receiver.terminate()
            try:
                receiver.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                receiver.kill()
                receiver.communicate()
        self.assertEqual(receiver.returncode, 0)
        rows = self.observations("otlp.python-standalone")
        self.assertEqual(len(rows), 1025)
        self.assertEqual({row["identity"]["session_id"] for row in rows}, {"python-large"})
        spans = [row["observation"] for row in rows]
        root = next(span for span in spans if span.get("parent_span_id") is None)
        self.assertEqual({span.get("parent_span_id") for span in spans}, {None, root["span_id"]})

    def test_slow_reply_cannot_extend_the_request_deadline(self):
        from observatory.client import Client

        stopping = threading.Event()
        replied = threading.Event()

        class SlowReply(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                body = b'{"partialSuccess":{"rejectedSpans":"0"}}'
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                replied.set()
                try:
                    for byte in body:
                        if stopping.wait(0.04):
                            break
                        self.wfile.write(bytes([byte]))
                        self.wfile.flush()
                except OSError:
                    pass

            def log_message(self, *_):
                pass

        with HTTPServer(("127.0.0.1", 0), SlowReply) as receiver:
            thread = threading.Thread(target=receiver.handle_request, daemon=True)
            thread.start()
            try:
                client = Client(f"http://127.0.0.1:{receiver.server_port}", transport="otlp",
                                session_id="deadline", timeout=0.35)
                with client.trace():
                    pass
                self.assertTrue(replied.is_set())
                self.assertIsInstance(client.last_error, TimeoutError)
            finally:
                stopping.set()
                thread.join(timeout=2)
            self.assertFalse(thread.is_alive())

    @unittest.skipUnless(SDK_AVAILABLE, "install the optional OpenTelemetry SDK")
    def test_automatic_mode_uses_real_sdk_with_offline_instrumentor_fixture(self):
        target = self.root / "target.py"
        target.write_text("""
import json
from opentelemetry import trace

tracer = trace.get_tracer("fixture")
with tracer.start_as_current_span("synthetic private root") as root:
    root.set_attribute("session.id", "synthetic private session")
    with tracer.start_as_current_span("synthetic private tool") as child:
        child.add_event("synthetic private event", {"payload": "synthetic private output"})
        child.set_attribute("llm.cost_usd", 0.000000123)
        child.set_attribute("llm.token_count.prompt", 0)
        child.set_attribute("llm.token_count.total", 4)
        child.set_status(trace.StatusCode.OK)
print(json.dumps({"trace": f"{root.get_span_context().trace_id:032x}",
                  "parent": f"{root.get_span_context().span_id:016x}",
                  "child": f"{child.get_span_context().span_id:016x}"}))
raise SystemExit(24)
""")
        self.producer.write_text(f"""
from importlib.machinery import ModuleSpec
from types import ModuleType
import sys
from unittest.mock import patch

def fixture(name, **attrs):
    module = ModuleType(name)
    module.__spec__ = ModuleSpec(name, loader=None)
    module.__path__ = []
    module.__dict__.update(attrs)
    return module

class Instrumentor:
    is_instrumented_by_opentelemetry = True
    def instrumentation_dependencies(self):
        return ()
    def instrument(self, *, tracer_provider, config):
        assert config.hide_inputs and config.hide_outputs and config.hide_llm_invocation_parameters
    def uninstrument(self):
        pass

def trace_config(**options):
    return fixture("trace_config", **options)

packages = {{
    "openai": fixture("openai"),
    "openinference": fixture("openinference"),
    "openinference.instrumentation": fixture("openinference.instrumentation", TraceConfig=trace_config),
    "openinference.instrumentation.openai": fixture("openinference.instrumentation.openai",
                                                      OpenAIInstrumentor=Instrumentor),
    "packaging": fixture("packaging"),
    "packaging.requirements": fixture("packaging.requirements", Requirement=object),
}}
from observatory.__main__ import main
sys.argv = ["agenturm", "run", "--transport", "otlp",
            "--session-id", "auto-session", "--harness-id", "harness.pi",
            {str(target)!r}]
with patch.dict(sys.modules, packages), patch(
    "observatory.auto._check_receiver", side_effect=AssertionError("legacy health call")
), patch("observatory.auto._Connection", side_effect=AssertionError("legacy connection")), patch(
    "observatory.receiver_state.endpoint_token", side_effect=AssertionError("legacy token read")
):
    main()
""")
        ids = self.launch("otlp.python-auto", 24)
        rows = self.observations("otlp.python-auto")
        usage, = [row["observation"] for row in rows if row["observation"]["kind"] == "span_token_usage"]
        self.assertEqual(usage, {
            "kind": "span_token_usage", "target_source_id": "harness.pi",
            "span": {"trace_id": ids["trace"], "span_id": ids["child"]},
            "input_tokens": 0, "total_tokens": 4,
        })
        cost = [row["observation"] for row in rows if row["observation"]["kind"] == "reported_cost"]
        self.assertEqual(cost, [{
            "kind": "reported_cost", "currency": "USD", "nanounits": "123",
            "span": {"trace_id": ids["trace"], "span_id": ids["child"]},
        }])
        rows = [row for row in rows if row["observation"]["kind"] == "trace_span"]
        self.assertEqual(len(rows), 2)
        self.assertEqual({row["identity"]["session_id"] for row in rows}, {"auto-session"})
        self.assertEqual({row["observation"]["target_source_id"] for row in rows}, {"harness.pi"})
        by_span = {row["observation"]["span_id"]: row["observation"] for row in rows}
        self.assertEqual({span["trace_id"] for span in by_span.values()}, {ids["trace"]})
        self.assertEqual(by_span[ids["child"]]["parent_span_id"], ids["parent"])
        self.assertEqual(by_span[ids["child"]]["status"], "ok")


if __name__ == "__main__":
    unittest.main()
