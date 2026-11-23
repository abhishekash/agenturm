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
    spec = importlib.util.spec_from_file_location(f"observatory.integrations._isolated_{path.stem}", path)
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
                    self.assertIn("safe identifier", str(client.last_error))
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

    def test_progress_is_sent_immediately_with_only_exact_scope_and_public_fields(self):
        environment = {
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ENDPOINT,
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS": "authorization=Bearer%20" + "e1" * 32,
        }
        with patch.dict(os.environ, environment, clear=True):
            client = Client(transport="otlp", capture_content=True)
            with client.trace(session_id="session-1", harness_id="harness.pi",
                              turn_id="turn-2", worker_id="worker/one") as run:
                self.assertTrue(run.progress("planning", MARKER, 0))
                self.assertTrue(run.progress("waiting", MARKER))
                self.assertTrue(run.progress("processing", MARKER, 12.34))
                self.assertFalse(run.progress("step-ghp_fixture", MARKER))
                self.assertFalse(run.progress("processing", MARKER, 0.001))
                self.assertIn("two decimal places", str(client.last_error))
                self.assertEqual(len(self.requests), 3)
                self.assertEqual({row[3] for row in self.requests}, {"/native/v1/progress"})
        event_ids = set()
        for request, stage, basis_points in zip(
            self.requests[:3], ("planning", "waiting", "processing"), (0, None, 1234)
        ):
            host, port, method, path, body, headers = request
            self.assertEqual((host, port, method, path),
                             ("127.0.0.1", 14318, "POST", "/native/v1/progress"))
            self.assertEqual(headers["Authorization"], "Bearer " + "e1" * 32)
            self.assertNotIn(MARKER.encode(), body)
            record = json.loads(body)
            event_id = record.pop("event_id")
            self.assertRegex(event_id, "^[a-f0-9]{32}$")
            event_ids.add(event_id)
            self.assertGreater(int(record.pop("observed_at_unix_ns")), 0)
            self.assertEqual(record.pop("percent_basis_points", None), basis_points)
            self.assertEqual(record, {
                "trace_id": run.trace_id, "stage": stage, "session_id": "session-1",
                "target_source_id": "harness.pi", "turn_id": "turn-2", "worker_id": "worker/one",
            })
        self.assertEqual(len(event_ids), 3)
        self.assertEqual(self.requests[3][3], "/v1/traces")

    def test_captured_text_requires_explicit_mode_bearer_session_and_safe_text(self):
        environment = {
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ENDPOINT,
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS": "authorization=Bearer%20" + "e1" * 32,
        }
        fields = {"agenturm.payload.input": MARKER, "input.value": "provider arguments"}
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(ValueError, "authenticated trace receiver"):
                Client(transport="otlp", strict=True, capture_content=True)
        with patch.dict(os.environ, environment, clear=True):
            default = Client(transport="otlp", strict=True)
            with default.trace(session_id="explicit", attributes=fields):
                pass
            self.assertNotIn(MARKER.encode(), self.requests[-1][4])
            explicit = Client(transport="otlp", strict=True, capture_content=True)
            with self.assertRaisesRegex(ValueError, "explicit session"):
                with explicit.trace(attributes=fields):
                    pass
            with explicit.trace(session_id="explicit", attributes=fields):
                pass
            self.assertIn(MARKER.encode(), self.requests[-1][4])
            self.assertNotIn(b"provider arguments", self.requests[-1][4])
            self.assertEqual(len(self.requests), 2)
            with self.assertRaisesRegex(ValueError, "text was withheld"):
                with explicit.trace(session_id="explicit", attributes={
                    "agenturm.payload.output": "Authorization: Bearer synthetic credential",
                }):
                    pass
            self.assertEqual(len(self.requests), 2)

    def test_structured_sdk_projection_only_reads_message_text(self):
        from observatory.integrations.otel_payload import agents_message_text, structured_message_text

        user = {"role": "user", "parts": [
            {"type": "text", "content": MARKER},
            {"type": "tool_call", "content": "tool private"},
            {"type": "blob", "content": "image private"},
        ]}
        attrs = {
            "gen_ai.input.messages": json.dumps([user, {"role": "system", "parts": user["parts"]}]),
            "input.value": "provider private",
            "gen_ai.output.messages": json.dumps([{"role": "assistant", "parts": [
                {"type": "thinking", "content": "reasoning private"},
                {"type": "text", "content": "assistant text"},
            ]}]),
        }
        self.assertEqual(structured_message_text(attrs, "pydantic-ai"), {
            "agenturm.payload.input": MARKER, "agenturm.payload.output": "assistant text",
        })
        adk = {
            "gcp.vertex.agent.llm_request": json.dumps({"contents": [{"role": "user", "parts": [
                {"text": MARKER}, {"text": "reasoning private", "thought": True},
                {"function_call": {"args": "tool private"}},
            ]}], "config": {"headers": "provider private"}}),
            "gcp.vertex.agent.llm_response": json.dumps({"content": {
                "role": "model", "parts": [{"text": "assistant text"}],
            }}),
        }
        self.assertEqual(structured_message_text(adk, "gcp.vertex.agent"), {
            "agenturm.payload.input": MARKER, "agenturm.payload.output": "assistant text",
        })
        self.assertEqual(structured_message_text(adk, "other"), {})
        self.assertEqual(agents_message_text(None, "generation", {
            "input": [{"role": "user", "content": MARKER}],
            "output": [{"role": "assistant", "content": [{"type": "text", "text": "assistant text"}]}],
            "model_config": {"headers": "provider private"},
        }), {"agenturm.payload.input": MARKER, "agenturm.payload.output": "assistant text"})
        self.assertEqual(agents_message_text(None, "function", {
            "input": [{"role": "user", "content": MARKER}],
        }), {})

    def test_broker_bearer_is_sent_only_to_its_configured_trace_endpoint(self):
        bearer = "Bearer " + "e1" * 32
        launch_env = {
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ENDPOINT,
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS": "irrelevant=ignored,Authorization=Bearer%20" + "e1" * 32,
            "OTEL_EXPORTER_OTLP_HEADERS": "authorization=Bearer%20" + "d2" * 32,
            "OBSERVATORY_TOKEN": MARKER,
        }
        with patch.dict(os.environ, launch_env, clear=True):
            for endpoint in (None, "http://127.0.0.1:14318", "http://127.0.0.1:14319"):
                client = Client(endpoint, transport="otlp", strict=True)
                with client.trace(session_id="native-session"):
                    pass
                *_, body, headers = self.requests[-1]
                if endpoint == "http://127.0.0.1:14319":
                    self.assertNotIn("Authorization", headers)
                else:
                    self.assertEqual(headers["Authorization"], bearer)
                self.assertNotIn(bearer.encode(), body)
                self.assertNotIn("irrelevant", headers)
                self.assertNotIn(MARKER, str(headers))

        with patch.dict(os.environ, {
            "OTEL_EXPORTER_OTLP_HEADERS": launch_env["OTEL_EXPORTER_OTLP_TRACES_HEADERS"]
        }, clear=True):
            with Client(ENDPOINT, transport="otlp", strict=True).trace(session_id="native-session"):
                pass
            self.assertNotIn("Authorization", self.requests[-1][-1])

    def test_broker_bearer_discards_ambiguous_or_malformed_headers(self):
        valid = "authorization=Bearer%20" + "e1" * 32
        for header in (
            valid + ",Authorization=Bearer%20" + "e2" * 32,
            valid + ",authorization",
            valid + "%0A",
            valid + "%ZZ",
            valid.replace("%20", "+"),
            valid.replace("e1", "E1"),
            valid + "," + "x" * 4096,
        ):
            with patch.dict(os.environ, {
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ENDPOINT,
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS": header,
            }, clear=True):
                self.response = Response(status=401, body=header.encode())
                client = Client(transport="otlp")
                with client.trace(session_id="native-session"):
                    pass
                self.assertNotIn("Authorization", self.requests[-1][-1])
                self.assertEqual(str(client.last_error), "OTLP receiver returned HTTP 401")

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

    def test_automatic_sdk_copy_requires_vetted_message_shape_and_preserves_cost(self):
        sdk = integration("otlp.py", {
            "opentelemetry.sdk.trace.export": {
                "SpanExporter": type("SpanExporter", (), {}),
                "SpanExportResult": SimpleNamespace(SUCCESS=0, FAILURE=1),
            },
        })
        span = SimpleNamespace(
            get_span_context=lambda: SimpleNamespace(trace_id=1, span_id=2), parent=None,
            start_time=1, end_time=2, status=SimpleNamespace(status_code=SimpleNamespace(name="OK")),
            instrumentation_scope=SimpleNamespace(name="unrecognized.instrumentation"),
            attributes={"agenturm.payload.input": MARKER, "input.value": MARKER, "llm.cost_usd": 0},
        )
        for capture in (False, True):
            attributes = sdk._span_metadata(span, capture)["attributes"]
            self.assertEqual(attributes, {"agenturm.reported_cost.amount": "0", "agenturm.reported_cost.currency": "USD"})
            self.assertNotIn(MARKER, str(attributes))

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

    def test_usage_whitelist_and_sdk_callback_preserve_missing_fields(self):
        client = Client(ENDPOINT, transport="otlp", session_id="native-session", strict=True)
        with client.trace(attributes={
            "gen_ai.usage.input_tokens": 0,
            "llm.token_count.prompt": "0",
            "gen_ai.usage.output_tokens": 5,
            "llm.token_count.completion": 6,
            "gen_ai.usage.cached_input_tokens": "9223372036854775807",
            "gen_ai.usage.reasoning_tokens": True,
            "gen_ai.input.messages": MARKER,
        }):
            pass
        attributes = {field["key"]: field["value"] for field in self.sent_spans()[0]["attributes"]}
        self.assertEqual(attributes, {
            "session.id": {"stringValue": "native-session"},
            "gen_ai.usage.input_tokens": {"intValue": "0"},
            "gen_ai.usage.cache_read.input_tokens": {"intValue": "9223372036854775807"},
        })

        langchain = integration("langchain.py", {
            "langchain_core.callbacks": {"BaseCallbackHandler": type("BaseCallbackHandler", (), {})}
        })
        result = SimpleNamespace(generations=[[SimpleNamespace(message=SimpleNamespace(
            usage_metadata={
                "output_tokens": 0, "input_token_details": {"cache_read": 4},
                "output_token_details": {"reasoning": 1.5},
            },
        ))]])
        self.assertEqual(langchain.LangChainCallbackHandler._reported_token_attributes(result), {
            "gen_ai.usage.output_tokens": "0", "gen_ai.usage.cache_read.input_tokens": "4",
        })

        agents = integration("openai_agents.py", {
            "agents.tracing.processor_interface": {"TracingExporter": type("TracingExporter", (), {})}
        })
        reported = agents._safe_attributes(
            None, "generation", {
                "usage": {"input_tokens": 1.5, "output_tokens": 0, "cached_input_tokens": 9},
                "input": {"input_tokens": 99, "reasoning_tokens": 100},
            }, exact_usage=True,
        )
        self.assertEqual(reported, {
            "gen_ai.usage.output_tokens": 0, "gen_ai.usage.cache_read.input_tokens": 9,
        })

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
