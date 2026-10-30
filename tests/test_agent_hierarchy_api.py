"""Additive hierarchy API and exact/subtree scope validation."""
import io
import json
import tempfile
import unittest
from pathlib import Path

from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store, normalize_native


class HierarchyApiTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.store = Store(Path(self.tmp.name) / "api.db")
        self.addCleanup(self.store.close)

    def request(self, path):
        from observatory.server import handler_for
        class Socket:
            def __init__(self, request):
                self.input = io.BytesIO(request)
                self.output = io.BytesIO()
            def makefile(self, mode, *args, **kwargs):
                return self.input if "r" in mode else self.output
            def sendall(self, data):
                self.output.write(data)
        class Server:
            server_name = "localhost"
            server_port = 4318
        raw = f"GET {path} HTTP/1.1\r\n\r\n".encode()
        sock = Socket(raw)
        handler_for(self.store, quiet=True)(sock, ("127.0.0.1", 1234), Server())
        head, payload = sock.output.getvalue().split(b"\r\n\r\n", 1)
        return int(head.split()[1]), json.loads(payload) if payload else {}

    def test_hierarchy_and_scope_endpoints_are_additive_and_validated(self):
        def span(trace, span_id, native, parent=None, session="shared"):
            attrs = {"agent.id": native, "agent.runtime": "SDK", "agent.namespace": "svc",
                     "session.id": session}
            if parent:
                attrs["agent.parent_id"] = parent
            self.store.ingest([normalize_native({
                "trace_id": trace, "span_id": span_id, "parent_span_id": None,
                "name": "agent.run", "start_unix_nano": 1, "end_unix_nano": 2,
                "status": "OK", "attributes": attrs,
                "resource_attributes": {"service.name": "svc"}, "scope_name": "fixture"}, "svc")])
        span("1" * 32, "1" * 16, "root")
        span("2" * 32, "2" * 16, "child", "root", "child-session")
        root = canonical_agent_id("SDK", "svc", "root")
        status, body = self.request("/api/hierarchy")
        self.assertEqual(status, 200)
        self.assertEqual(body["revision"], self.store.hierarchy_revision())
        self.assertIn("capabilities", body)
        self.assertEqual(len(body["nodes"]), 2)
        status, page = self.request(
            f"/api/traces?agent_id={root}&subtree=1&limit=10")
        self.assertEqual(status, 200)
        self.assertEqual({item["trace_id"] for item in page["traces"]},
                         {"1" * 32, "2" * 32})
        status, page = self.request(f"/api/traces?agent_id={root}&subtree=0&limit=10")
        self.assertEqual([item["trace_id"] for item in page["traces"]], ["1" * 32])
        self.assertEqual(self.request("/api/traces?agent_id=invalid")[0], 400)
        self.assertEqual(self.request("/api/activity?agent_id=invalid")[0], 400)
        self.assertEqual(self.request(
            f"/api/traces?agent_id={root}&scope_revision=stale")[0], 400)
        self.assertEqual(self.request(
            f"/api/activity?agent_id={root}&scope_revision=stale")[0], 400)
