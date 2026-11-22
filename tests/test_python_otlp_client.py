"""Wire and compatibility checks for the opt-in Python OTLP transport."""
from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import os
import unittest
from email.message import Message
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import patch

from observatory.client import Client
from observatory.otlp_transport import MAX_EXPORT_BYTES, MAX_EXPORT_SPANS


MARKER = "synthetic private trace content"
ENDPOINT = "http://127.0.0.1:14318/v1/traces"


class Response:
    chunked = False

    def __init__(self, body=b"{}", *, status=200, length=None):
        self.body = body
        self.status = status
        self.headers = Message()
        self.headers["Content-Length"] = str(len(body) if length is None else length)

    def read(self, limit):
        return self.body[:limit]

    def __enter__(self):
        return self

    def __exit__(self, *_):
        pass


def integration(file_name, stubs):
    # Exercise the real wrapper with synthetic SDK callbacks even in the
    # dependency-free job. The stubs provide only the optional base classes.
    modules = {}
    for dotted, types in stubs.items():
        module = ModuleType(dotted)
        module.__dict__.update(types)
        modules[dotted] = module
        names = dotted.split(".")
        for end in range(1, len(names)):
            parent = ".".join(names[:end])
            modules.setdefault(parent, ModuleType(parent))
    path = Path(__file__).resolve().parents[1] / "observatory/integrations" / file_name
    spec = importlib.util.spec_from_file_location(f"isolated_{path.stem}", path)
    module = importlib.util.module_from_spec(spec)
    with patch.dict("sys.modules", modules):
        spec.loader.exec_module(module)
    return module


class PythonOtlpClientTests(unittest.TestCase):
    def setUp(self):
        self.requests = []
        self.response = Response()
        self.responses = []

        def connect(host, port, timeout):
            def send(method, path, *, body, headers):
                self.requests.append((host, port, method, path, body, headers))
            return SimpleNamespace(
                request=send,
                getresponse=lambda: self.responses.pop(0) if self.responses else self.response,
                close=lambda: None,
            )

        network = patch("observatory.otlp_transport._DeadlineConnection", side_effect=connect)
        self.addCleanup(network.stop)
        network.start()

    def sent_spans(self):
        return [
            span
            for *_, body, headers in self.requests
            for resource in json.loads(body)["resourceSpans"]
            for scope in resource["scopeSpans"]
            for span in scope["spans"]
        ]

    def test_async_context_preserves_ancestry_and_only_explicit_scope_on_wire(self):
        async def produce():
            with patch.dict(os.environ, {
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ENDPOINT,
                "OBSERVATORY_TOKEN": MARKER,
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS": "authorization=" + MARKER,
                "HTTP_PROXY": "http://invalid.example",
            }), patch("observatory.receiver_state.endpoint_token", side_effect=AssertionError("legacy token")):
                client = Client(transport="otlp", resource_attributes={"private": MARKER},
                                capture_exception_message=True)
                async with client.trace(
                    MARKER, task=MARKER, session_id="session-1", harness_id="harness.pi",
                    turn_id="turn-2", worker_id="worker/one", attributes={"session.id": MARKER},
                ) as run:
                    self.assertFalse(await run.aprogress(MARKER))
                    self.assertIn("does not export progress", str(client.last_error))
                    async with run.span(MARKER) as parent:
                        async with run.span(MARKER) as child:
                            child.set_attribute("agenturm.source.id", MARKER)
                            child.add_event(MARKER, {"message": MARKER})
                            child.set_status("ERROR")
                self.assertIsNone(client.last_error)
                return run, parent, child

        run, parent, child = asyncio.run(produce())
        self.assertEqual(len(self.requests), 1)
        host, port, method, path, body, headers = self.requests[0]
        self.assertEqual((host, port, method, path), ("127.0.0.1", 14318, "POST", "/v1/traces"))
        self.assertNotIn("Authorization", headers)
        self.assertNotIn(MARKER.encode(), body)
        root, wire_parent, wire_child = self.sent_spans()
        self.assertNotIn("parentSpanId", root)
        self.assertEqual(wire_parent["parentSpanId"], run.root.span_id)
        self.assertEqual(wire_child["parentSpanId"], parent.span_id)
        self.assertEqual(wire_child["spanId"], child.span_id)
        self.assertEqual(wire_child["status"], {"code": 2})
        self.assertEqual({span["traceId"] for span in self.sent_spans()}, {run.trace_id})
        self.assertEqual(wire_child["startTimeUnixNano"], str(child.start_ns))
        self.assertEqual(wire_child["endTimeUnixNano"], str(child.end_ns))
        self.assertEqual(
            {item["key"]: item["value"]["stringValue"] for item in wire_child["attributes"]},
            {"session.id": "session-1", "agenturm.source.id": "harness.pi",
             "agenturm.turn.id": "turn-2", "agenturm.worker.id": "worker/one"},
        )

    def test_partial_failure_is_reported_and_strict_keeps_application_error(self):
        self.response = Response(json.dumps({"partialSuccess": {
            "rejectedSpans": "1", "errorMessage": MARKER,
        }}).encode())
        best_effort = Client(ENDPOINT, transport="otlp")
        with best_effort.trace("unscoped"):
            pass
        self.assertIn("rejected one or more spans", str(best_effort.last_error))
        self.assertNotIn(MARKER, str(best_effort.last_error))

        strict = Client(ENDPOINT, transport="otlp", strict=True)
        with self.assertRaisesRegex(OSError, "rejected one or more spans"):
            with strict.trace():
                pass
        with self.assertRaisesRegex(RuntimeError, "original callback failure"):
            with strict.trace():
                raise RuntimeError("original callback failure")
        self.assertIsInstance(strict.last_error, OSError)

    def test_invalid_destinations_bad_reply_and_no_legacy_posts(self):
        for endpoint in (
            "https://127.0.0.1:14318", "http://localhost:14318", "http://127.1:",
            "http://192.0.2.1:14318", "http://token@127.0.0.1",
            "http://127.0.0.1/api/spans", "http://127.0.0.1:0",
            "http://127.0.0.1/v1/traces?", "http://[::1]suffix",
        ):
            with self.subTest(endpoint=endpoint):
                with self.assertRaisesRegex(ValueError, "literal loopback"):
                    Client(endpoint, transport="otlp", strict=True)
        self.assertEqual(self.requests, [])
        self.assertEqual(
            Client("http://[::1]:14318", transport="otlp", strict=True).endpoint,
            "http://[::1]:14318/v1/traces",
        )
        client = Client(ENDPOINT, transport="otlp")
        self.assertFalse(client._post("/api/progress", {"value": MARKER}))
        self.assertEqual(self.requests, [])
        for reply, error in (
            (Response(status=302), "HTTP 302"),
            (Response(length=4097), "unbounded reply"),
            (Response(b"{}{}", length=4), "invalid reply"),
            (Response(b'{"partialSuccess":{"rejectedSpans":false}}'), "invalid reply"),
        ):
            self.response = reply
            with client.trace(session_id="scope"):
                pass
            self.assertIn(error, str(client.last_error))

    def test_limits_and_exact_scope_validation(self):
        client = Client(ENDPOINT, transport="otlp", strict=True, session_id="session")
        with client.trace() as run:
            for _ in range(MAX_EXPORT_SPANS):
                run.span("child").finish()
        bodies = [request[4] for request in self.requests]
        self.assertEqual(len(bodies), 2)
        self.assertTrue(all(len(body) <= MAX_EXPORT_BYTES for body in bodies))
        self.assertEqual(len(self.sent_spans()), MAX_EXPORT_SPANS + 1)
        self.requests.clear()
        with patch("observatory.otlp_transport.MAX_EXPORT_BYTES", 500):
            with client.trace() as run:
                for _ in range(3):
                    run.span("child").finish()
            self.assertGreater(len(self.requests), 1)
            self.assertTrue(all(len(row[4]) <= 500 for row in self.requests))
            self.assertEqual(len(self.sent_spans()), 4)
        for arguments in (
            {"session_id": "trimmed "}, {"session_id": "path/session"},
            {"session_id": "session", "harness_id": "otlp.synthetic"},
            {"harness_id": "harness.pi"}, {"worker_id": "worker"},
            {"session_id": "session", "turn_id": "turn"},
        ):
            with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                client.trace(**arguments)
        with self.assertRaisesRegex(ValueError, "identity"):
            client._post_spans([{
                "trace_id": "unsupported", "span_id": "01" * 8, "status": "OK",
                "start_unix_nano": 1, "end_unix_nano": 2,
            }])

    def test_best_effort_retains_valid_spans_and_attempts_later_batches(self):
        client = Client(ENDPOINT, transport="otlp", session_id="session")
        parent = {
            "trace_id": "01" * 16, "span_id": "01" * 8, "status": "OK",
            "start_unix_nano": 1, "end_unix_nano": 2,
        }
        child = {**parent, "span_id": "02" * 8, "parent_span_id": parent["span_id"]}
        bad = {**parent, "span_id": "invalid"}
        self.assertFalse(client._post_spans([parent, bad, child]))
        self.assertIn("identity", str(client.last_error))
        self.assertEqual([row["spanId"] for row in self.sent_spans()], [parent["span_id"], child["span_id"]])

        self.requests.clear()
        self.responses = [Response(status=503), Response()]
        with patch("observatory.otlp_transport.MAX_EXPORT_SPANS", 1):
            self.assertFalse(client._post_spans([parent, child]))
        self.assertEqual([row["spanId"] for row in self.sent_spans()], [parent["span_id"], child["span_id"]])
        self.assertEqual(len(self.requests), 2)
        self.assertIn("HTTP 503", str(client.last_error))

    def test_vendor_callback_ids_preserve_existing_ancestry(self):
        client = Client(
            ENDPOINT, transport="otlp", session_id="native-session", harness_id="harness.pi",
            strict=True,
        )
        agents = integration("openai_agents.py", {
            "agents.tracing.processor_interface": {"TracingExporter": type("TracingExporter", (), {})}
        })
        exported = SimpleNamespace(type="function", export=lambda: {"input": MARKER, "output": MARKER})
        stamp = "2026-11-20T00:00:00Z"
        parent = SimpleNamespace(
            trace_id="trace_vendor", span_id="span_parent", parent_id=None, error=None,
            started_at=stamp, ended_at=stamp, span_data=exported,
        )
        child = SimpleNamespace(**{**vars(parent), "span_id": "span_child", "parent_id": "span_parent"})
        agents.OpenAIAgentsExporter(client, capture_content=True).export([
            SimpleNamespace(trace_id="trace_vendor", name=MARKER, group_id="unbound-group"),
            parent, child,
        ])
        wire_parent, wire_child = self.sent_spans()
        self.assertEqual(wire_child["traceId"], hashlib.sha256(b"trace_vendor").hexdigest()[:32])
        self.assertEqual(wire_parent["spanId"], hashlib.sha256(b"span_parent").hexdigest()[:16])
        self.assertEqual(wire_child["parentSpanId"], wire_parent["spanId"])
        self.assertNotIn(MARKER.encode(), self.requests[0][4])
        self.assertNotIn(b"unbound-group", self.requests[0][4])

        self.requests.clear()
        langchain = integration("langchain.py", {
            "langchain_core.callbacks": {"BaseCallbackHandler": type("BaseCallbackHandler", (), {})}
        })
        with client.trace() as run:
            handler = langchain.LangChainCallbackHandler(capture_content=True).attach(run)
            handler.on_chain_start({"name": MARKER}, MARKER, run_id="uuid-style-parent")
            handler.on_tool_start({"name": MARKER}, MARKER, run_id="uuid-style-child",
                                  parent_run_id="uuid-style-parent")
            handler.on_tool_end(MARKER, run_id="uuid-style-child")
            handler.on_chain_end(MARKER, run_id="uuid-style-parent")
        lc_root, lc_parent, lc_child = self.sent_spans()
        self.assertEqual(lc_parent["parentSpanId"], lc_root["spanId"])
        self.assertEqual(lc_child["parentSpanId"], lc_parent["spanId"])
        self.assertNotIn(MARKER.encode(), self.requests[0][4])

    def test_legacy_transport_preserves_span_and_progress_payloads(self):
        with patch("observatory.receiver_state.endpoint_token", return_value="legacy-token"), patch(
            "observatory.client.receiver_urlopen",
        ) as network:
            network.return_value.__enter__.return_value.status = 200
            client = Client()
            with client.trace("legacy-trace", session_id="legacy-session", task="legacy-task") as run:
                self.assertTrue(run.progress("planning", "legacy-progress", 20))
                with run.span("legacy-child", attributes={"private": "legacy-value"}):
                    pass
        progress, batch = [entry.args[0] for entry in network.call_args_list]
        self.assertEqual(progress.full_url, "http://127.0.0.1:4318/api/progress")
        self.assertEqual(batch.full_url, "http://127.0.0.1:4318/api/spans")
        self.assertEqual(batch.get_header("Authorization"), "Bearer legacy-token")
        self.assertEqual(json.loads(progress.data)["message"], "legacy-progress")
        self.assertEqual(json.loads(batch.data)["spans"][1]["attributes"], {"private": "legacy-value"})
        self.assertEqual(self.requests, [])
