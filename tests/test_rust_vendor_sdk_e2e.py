"""Qualify actual provider SDKs and framework instrumentation through Rust."""

from __future__ import annotations

import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


RUST_APP = os.environ.get("AGENTURM_RUST_APP_BIN")
BROKER_CONFIG = os.environ.get("AGENTURM_RUST_BROKER_CONFIG")
REQUIRED = os.environ.get("AGENTURM_REQUIRE_VENDOR_SDK_TESTS") == "1"
REPO = Path(__file__).resolve().parents[1]
PRODUCER = REPO / "tests/fixtures/rust_vendor_trace_producer.py"


def installed(*packages):
    try:
        for package in packages:
            importlib.metadata.version(package)
    except importlib.metadata.PackageNotFoundError:
        return False
    return True


OPENAI_AUTO = installed("openai", "openinference-instrumentation-openai", "opentelemetry-sdk")
ANTHROPIC_AUTO = installed("anthropic", "openinference-instrumentation-anthropic", "opentelemetry-sdk")
LANGCHAIN_AUTO = OPENAI_AUTO and installed("langchain-openai", "openinference-instrumentation-langchain")
LANGGRAPH_CALLBACKS = installed("langgraph", "langchain-openai", "openai")


def requires_sdk(available, description):
    return unittest.skipUnless(available or REQUIRED, description)


@unittest.skipUnless(RUST_APP or REQUIRED, "set AGENTURM_RUST_APP_BIN to the standalone Rust binary")
class RustVendorSdkTests(unittest.TestCase):
    def run_producer(self, vendor, automatic=False):
        self.assertIsNotNone(RUST_APP, "AGENTURM_RUST_APP_BIN is required")
        with tempfile.TemporaryDirectory() as directory:
            db = Path(directory) / "history.sqlite"
            receiver = f"otlp.{vendor}"
            command = [RUST_APP]
            storage = ["--db", db]
            if BROKER_CONFIG:
                command.append("broker")
                storage = ["--broker-config", BROKER_CONFIG]
            args = [sys.executable]
            if automatic:
                args.extend([
                    "-m", "observatory", "run", "--transport", "otlp",
                    "--session-id", f"session.{vendor}", "--harness-id", "harness.codex",
                    "--turn-id", "turn.fixture", "--worker-id", "worker/fixture",
                ])
            args.extend([PRODUCER, vendor])
            environment = os.environ | {
                "PYTHONPATH": os.pathsep.join(filter(None, (str(REPO), os.environ.get("PYTHONPATH")))),
                "LANGCHAIN_TRACING": "false", "LANGCHAIN_TRACING_V2": "false",
                "LANGSMITH_TRACING": "false",
                "OTEL_SDK_DISABLED": "false", "OBSERVATORY_TOKEN": "unused-legacy-token",
            }
            result = subprocess.run(
                [*command, "run", *storage, "--source-id", receiver, "--", *args],
                cwd=REPO, env=environment, capture_output=True, text=True, timeout=75,
            )
            self.assertEqual(result.returncode, 23, result.stderr)
            if automatic:
                self.assertIn("Agenturm is exporting metadata", result.stderr)
            self.assertNotIn("export failed", result.stderr)
            expected = json.loads(result.stdout)

            result = subprocess.run(
                [*command, "query", *storage, "--harness-id", receiver],
                capture_output=True, text=True, check=True, timeout=10,
            )
            persisted = db.read_bytes() if not BROKER_CONFIG else b""
            for private in ("private-vendor-fixture", "fixture-key-never-used", "unused-legacy-token"):
                self.assertNotIn(private, result.stdout)
                self.assertNotIn(private.encode(), persisted)
            rows = json.loads(result.stdout)["observations"]
            self.assertEqual(len(rows), len(expected))
            observed = {}
            for row in rows:
                span = row["observation"]
                self.assertEqual(span["kind"], "trace_span")
                self.assertEqual(span["target_source_id"], "harness.codex")
                self.assertEqual(row["identity"]["session_id"], f"session.{vendor}")
                self.assertEqual(row["identity"]["turn_id"], "turn.fixture")
                self.assertEqual(row["identity"]["worker_id"], "worker/fixture")
                observed[span["span_id"]] = {
                    "trace": span["trace_id"], "span": span["span_id"],
                    "parent": span["parent_span_id"], "start": int(span["started_at_unix_ns"]),
                    "end": int(row["observed_at_unix_ns"]), "status": span["status"],
                }
            self.assertEqual(len(observed), len(expected))
            for span in expected:
                saved = observed[span["span"]]
                self.assertEqual(saved, {key: span[key] for key in saved})
                self.assertGreater(saved["start"], 0)
                self.assertGreaterEqual(saved["end"], saved["start"])
                if saved["parent"] is not None:
                    self.assertEqual(observed[saved["parent"]]["trace"], saved["trace"])
            return expected

    @requires_sdk(OPENAI_AUTO, "install cached OpenAI and its OpenInference instrumentation")
    def test_openai_sync_stream_and_error_flush_through_automatic_launcher(self):
        spans = self.run_producer("openai", True)
        fixture, = [span for span in spans if span["instrumentor"] == "agenturm.fixture"]
        captured = [span for span in spans
                    if span["instrumentor"] == "openinference.instrumentation.openai"]
        self.assertEqual(len(captured), 3)
        self.assertEqual({span["parent"] for span in captured}, {fixture["span"]})
        self.assertEqual(sorted(span["status"] for span in captured), ["error", "ok", "ok"])

    @requires_sdk(LANGCHAIN_AUTO, "install cached LangChain OpenAI and instrumentation")
    def test_langchain_automatic_instrumentation_preserves_its_callback_hierarchy(self):
        spans = self.run_producer("langchain", True)
        captured = [span for span in spans
                    if span["instrumentor"] == "openinference.instrumentation.langchain"]
        self.assertGreaterEqual(len(captured), 3)
        self.assertEqual(len({span["trace"] for span in captured}), 1)
        self.assertEqual(sum(span["parent"] is None for span in captured), 1)
        self.assertTrue(any(span["instrumentor"] == "openinference.instrumentation.openai"
                            for span in spans))

    @requires_sdk(LANGGRAPH_CALLBACKS, "install cached LangGraph and LangChain OpenAI")
    def test_langgraph_real_callbacks_retain_graph_node_and_model_spans(self):
        spans = self.run_producer("langgraph", False)
        self.assertGreaterEqual(len(spans), 4)
        self.assertEqual(len({span["trace"] for span in spans}), 1)
        self.assertEqual(sum(span["parent"] is None for span in spans), 1)
        by_id = {span["span"]: span for span in spans}
        self.assertTrue(any(span["parent"] and by_id[span["parent"]]["parent"] for span in spans))

    @requires_sdk(ANTHROPIC_AUTO, "install Anthropic and its OpenInference instrumentation")
    def test_anthropic_messages_flush_through_automatic_launcher(self):
        spans = self.run_producer("anthropic", automatic=True)
        parent, = [span for span in spans if span["instrumentor"] == "agenturm.fixture"]
        model, = [span for span in spans
                  if span["instrumentor"] == "openinference.instrumentation.anthropic"]
        self.assertEqual(model["parent"], parent["span"])
        self.assertEqual(model["status"], "ok")

    @requires_sdk(installed("openai-agents"), "install OpenAI Agents SDK")
    def test_openai_agents_real_tool_workflow_preserves_normalized_sdk_identity(self):
        spans = self.run_producer("openai-agents")
        operations = {span["operation"] for span in spans}
        self.assertTrue({"agent", "function"} <= operations, operations)
        self.assertTrue({"generation", "response"} & operations, operations)
        self.assertEqual(len({span["trace"] for span in spans}), 1)
        self.assertTrue(any(span["parent"] for span in spans))

    @requires_sdk(installed("pydantic-ai", "opentelemetry-sdk"), "install Pydantic AI and OpenTelemetry")
    def test_pydantic_ai_native_instrumentation_flushes_model_and_tool_spans(self):
        spans = self.run_producer("pydantic-ai")
        operations = [span["operation"] for span in spans]
        self.assertIn("invoke_agent", operations)
        self.assertGreaterEqual(operations.count("chat"), 2)
        self.assertEqual(operations.count("execute_tool"), 1)
        self.assertTrue(any(span["parent"] for span in spans))

    @requires_sdk(installed("google-adk", "opentelemetry-sdk"), "install Google ADK and OpenTelemetry")
    def test_google_adk_native_instrumentation_flushes_agent_and_tool_spans(self):
        spans = self.run_producer("google-adk")
        operations = [span["operation"] for span in spans]
        self.assertIn("invoke_agent", operations)
        self.assertGreaterEqual(operations.count("generate_content"), 2)
        self.assertEqual(operations.count("execute_tool"), 1)
        self.assertTrue(any(span["parent"] for span in spans))
