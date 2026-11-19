"""Child-side protocol conformance and a separately executed Python adapter."""
import hashlib
import io
import sys
import tempfile
import time
import unittest
from pathlib import Path

from observatory.core import Store
from observatory.plugin_frames import read_frame, write_frame
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_runtime import PluginSession
from observatory.plugin_sdk import AdapterServer, PollRequest
from observatory.plugin_wire import PollBatch, ProtocolError, decode_message, encode_message


ROOT = Path(__file__).resolve().parents[1]


class Adapter:
    def __init__(self):
        self.acks = []

    def poll(self, request: PollRequest) -> PollBatch:
        records = ({"event_id": "one", "revision": 0, "kind": "lifecycle",
                    "time_ns": "9007199254740993", "payload": {"state": "completed"},
                    "identity": {"session_id": "native"}},) if not request.cursor else ()
        return PollBatch(request.source_id, "next" if records else request.cursor, records)

    def ack(self, cursor: str) -> None:
        self.acks.append(cursor)


class SdkTests(unittest.TestCase):
    def hello(self):
        return {"type": "hello", "protocol": 1, "plugin_id": "fixture.python",
                "source_id": "source-one", "max_frame_bytes": 4 * 1024 * 1024}

    def poll(self, cursor="", max_records=256, max_bytes=4 * 1024 * 1024):
        return {"type": "poll", "protocol": 1, "source_id": "source-one",
                "cursor": cursor, "max_records": max_records, "max_bytes": max_bytes}

    def test_state_machine_requires_ack_and_enforces_budgets(self):
        adapter = Adapter()
        server = AdapterServer("fixture.python", adapter)
        hello, _ = server.handle(self.hello())
        self.assertEqual(hello["features"], ["source-adapter/1"])
        first, _ = server.handle(self.poll())
        self.assertEqual(first["cursor"], "next")
        with self.assertRaisesRegex(ProtocolError, "unacknowledged"):
            server.handle(self.poll())
        with self.assertRaisesRegex(ProtocolError, "acknowledgment mismatch"):
            server.handle({"type": "ack", "protocol": 1, "source_id": "source-one",
                           "cursor": "wrong"})
        self.assertEqual(adapter.acks, [])
        server.handle({"type": "ack", "protocol": 1, "source_id": "source-one",
                       "cursor": "next"})
        self.assertEqual(adapter.acks, ["next"])
        idle, _ = server.handle(self.poll("next"))
        self.assertEqual(idle["records"], [])

    def test_invalid_control_and_oversized_output_fail_closed(self):
        cases = [{**self.hello(), "plugin_id": "wrong"},
                 {**self.hello(), "max_frame_bytes": True},
                 {**self.hello(), "unexpected": 1}]
        for message in cases:
            with self.subTest(message=message), self.assertRaises(ProtocolError):
                AdapterServer("fixture.python", Adapter()).handle(message)
        for message in ({**self.poll(), "max_records": 0},
                        {**self.poll(), "max_bytes": 1},
                        {**self.poll(), "source_id": "foreign"},
                        {**self.poll(), "cursor": 1}):
            server = AdapterServer("fixture.python", Adapter())
            server.handle(self.hello())
            with self.subTest(message=message), self.assertRaises(ProtocolError):
                server.handle(message)

    def test_binary_stream_loop_has_no_unframed_output(self):
        inbound = io.BytesIO()
        for message in (self.hello(), self.poll(),
                        {"type": "ack", "protocol": 1, "source_id": "source-one", "cursor": "next"},
                        self.poll("next"), {"type": "stop", "protocol": 1}):
            write_frame(inbound, encode_message(message), max_bytes=64 * 1024)
        inbound.seek(0)
        outbound = io.BytesIO()
        adapter = Adapter()
        AdapterServer("fixture.python", adapter).serve(inbound, outbound)
        outbound.seek(0)
        responses = []
        while (raw := read_frame(outbound)) is not None:
            responses.append(decode_message(raw))
        self.assertEqual([row["type"] for row in responses], ["hello_ack", "batch", "batch"])
        self.assertEqual(adapter.acks, ["next"])

    def test_external_python_adapter_reaches_live_projection(self):
        with tempfile.TemporaryDirectory() as root:
            base = Path(root)
            executable = base / "external-plugin"
            executable.write_text(
                f"#!{sys.executable}\n"
                "import sys\n"
                f"sys.path.insert(0, {str(ROOT)!r})\n"
                "from observatory.plugin_sdk import run_adapter\n"
                "from observatory.plugin_wire import PollBatch\n"
                "class Adapter:\n"
                " def poll(self, request):\n"
                "  records = ({'event_id':'one','revision':0,'kind':'lifecycle',"
                "'time_ns':'9007199254740993','payload':{'state':'completed'},"
                "'identity':{'session_id':'native'}},) if not request.cursor else ()\n"
                "  return PollBatch(request.source_id,'next' if records else request.cursor,records)\n"
                " def ack(self, cursor): pass\n"
                "run_adapter('fixture.python',Adapter())\n")
            executable.chmod(0o700)
            manifest_path = base / "external-plugin.toml"
            manifest_path.write_text(
                'id = "fixture.python"\nversion = "0.0.1"\nprotocol = "1"\n'
                f'entrypoint = ["{executable}"]\n'
                f'artifact_sha256 = "{hashlib.sha256(executable.read_bytes()).hexdigest()}"\n'
                'provides = ["source-adapter/1"]\nrequires = ["evidence-ingest/1"]\n'
                'depends_on = []\ngrants = ["configured-source-read"]\n')
            store = Store(str(base / "agenturm.db"))
            self.addCleanup(store.close)
            with PluginSession(store, PluginManifest.read(manifest_path),
                               source_id="source-one",
                               allowed_grants=("configured-source-read",), timeout=2.0) as session:
                self.assertEqual(session.poll_once().cursor, "next")
                deadline = time.monotonic() + 5
                while store.plugin_sessions()["pending_sources"] and time.monotonic() < deadline:
                    time.sleep(0.02)
                projected = store.plugin_sessions()
                self.assertEqual(projected["pending_sources"], 0)
                self.assertEqual(projected["sessions"][0]["run_state"], "completed")
                self.assertEqual(session.poll_once().records, ())


if __name__ == "__main__":
    unittest.main()
