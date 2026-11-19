"""A separately installed fake inbound connector through the real process/Store path."""
import json
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit
from urllib.request import urlopen
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_config import read_plugin_config
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError
from observatory.server import BoundedHTTPServer, serve


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "fake_inbound_connector"


def lifecycle(event, session, stamp, state, revision=0):
    return {"id": event, "session": session, "at_ns": str(stamp),
            "kind": "lifecycle", "state": state, "revision": revision}


class FixtureState:
    def __init__(self):
        self.lock = threading.Lock()
        self.revision = "r1"
        self.pages = []
        self.throttle = 0
        self.redirect = False
        self.requests = []


class FixtureHTTP(BaseHTTPRequestHandler):
    state: FixtureState

    def do_GET(self):
        parsed = urlsplit(self.path)
        if parsed.path != "/events" or set(parse_qs(parsed.query, keep_blank_values=True)) != {"after"}:
            self.send_error(404)
            return
        token = parse_qs(parsed.query, keep_blank_values=True)["after"][0]
        with self.state.lock:
            self.state.requests.append(token)
            if self.state.throttle:
                self.state.throttle -= 1
                self.send_response(429)
                self.send_header("Retry-After", "1")
                self.end_headers()
                return
            if self.state.redirect:
                self.send_response(302)
                self.send_header("Location", "http://example.invalid/events")
                self.end_headers()
                return
            revision = self.state.revision
            pages = list(self.state.pages)
        if token and not token.startswith(revision + ":"):
            self.send_response(410)
            self.end_headers()
            return
        try:
            index = int(token.split(":", 1)[1]) if token else 0
        except ValueError:
            self.send_error(400)
            return
        if index < 0 or index > len(pages):
            self.send_error(400)
            return
        records = pages[index] if index < len(pages) else []
        next_token = f"{revision}:{index + 1}" if records else token
        payload = json.dumps({"records": records, "next": next_token},
                             separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, _format, *_args):
        pass


class FakeInboundConnectorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.state = FixtureState()
        handler = type("BoundFixtureHTTP", (FixtureHTTP,), {"state": self.state})
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.thread.join, 2)
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.endpoint = f"http://127.0.0.1:{self.server.server_port}/events"
        installed = self.root / "installed"
        result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                 "--destination", str(installed), "--endpoint", self.endpoint],
                                capture_output=True, text=True, check=True)
        self.config = Path(result.stdout.strip())
        self.store = Store(str(self.root / "store.db"))
        self.addCleanup(self.store.close)

    def session(self):
        source = read_plugin_config(self.config)[0]
        return PluginSession(self.store, source.manifest, source_id=source.source_id,
                             allowed_grants=source.allowed_grants,
                             source_root=source.source_root, timeout=source.timeout)

    def projected(self):
        project_source(self.store, "fake-inbound-source")
        return self.store.plugin_sessions()["sessions"]

    def test_pages_rate_limit_restart_and_correction(self):
        with self.state.lock:
            self.state.pages = [
                [lifecycle("start", "session-1", 100, "started")],
                [{"id": "usage", "session": "session-1", "at_ns": "150",
                  "kind": "usage", "usage": {"input_tokens": 4}}],
                [lifecycle("finish", "session-1", 200, "completed")],
            ]
        with self.session() as connector:
            first = connector.poll_once()
            self.assertEqual([r["kind"] for r in first.records], ["lifecycle"])
            with self.state.lock:
                self.state.throttle = 1
            limited = connector.poll_once()
            self.assertEqual((limited.cursor, limited.records), (first.cursor, ()))
            with self.state.lock:
                count = len(self.state.requests)
            self.assertEqual(connector.poll_once().records, ())
            with self.state.lock:
                self.assertEqual(len(self.state.requests), count)
            time.sleep(1.05)
            usage = connector.poll_once()
            self.assertEqual(usage.records[0]["payload"], {"input_tokens": 4})
        with self.session() as resumed:
            self.assertEqual(resumed.cursor, usage.cursor)
            completed = resumed.poll_once()
            self.assertEqual(completed.records[0]["payload"], {"state": "completed"})
            self.assertEqual(resumed.poll_once().records, ())
            with self.state.lock:
                self.state.pages.append([
                    {"id": "usage", "session": "session-1", "at_ns": "150",
                     "kind": "usage", "revision": 1, "usage": {"input_tokens": 6}}])
            corrected = resumed.poll_once()
            self.assertEqual(corrected.records[0]["revision"], 1)
        row = self.projected()[0]
        self.assertEqual((row["run_state"], row["metadata_usage"]),
                         ("completed", {"input_tokens": 6}))
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 3)

    def test_expired_cursor_emits_durable_gap_then_resumes_new_revision(self):
        with self.state.lock:
            self.state.pages = [[lifecycle("before", "old", 100, "running")]]
        with self.session() as connector:
            before = connector.poll_once()
        with self.state.lock:
            self.state.revision = "r2"
            self.state.pages = [[lifecycle("after", "new", 300, "completed")]]
        with self.session() as resumed:
            self.assertEqual(resumed.cursor, before.cursor)
            gap = resumed.poll_once()
            self.assertEqual(gap.records[0]["payload"],
                             {"code": "cursor_expired", "severity": "warning"})
            self.assertTrue(gap.cursor.startswith("gap:"))
            self.assertEqual(resumed.last_diagnostic["code"], "cursor_expired")
        with self.session() as next_process:
            self.assertEqual(next_process.cursor, gap.cursor)
            self.assertEqual(next_process.last_diagnostic["code"], "cursor_expired")
            recovered = next_process.poll_once()
            self.assertEqual(recovered.records[0]["identity"]["session_id"], "new")
        self.assertEqual({row["native_session_id"] for row in self.projected()}, {"old", "new"})

    def test_invalid_remote_content_rolls_back_page_and_cursor(self):
        with self.state.lock:
            self.state.pages = [[{**lifecycle("bad", "session", 100, "running"),
                                 "prompt": "private content"}]]
        with self.session() as connector:
            with self.assertRaises(ProtocolError):
                connector.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)

    def test_redirect_is_rejected_without_cursor_advance(self):
        with self.state.lock:
            self.state.redirect = True
        with self.session() as connector:
            with self.assertRaises(ProtocolError):
                connector.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT cursor FROM plugin_source_cursor_v1").fetchone()[0], "")

    def test_installed_connector_reaches_receiver_without_kernel_registration(self):
        with self.state.lock:
            self.state.pages = [[lifecycle("served", "external", 100, "completed")]]
        import socket
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        servers = []
        failures = []

        def make_server(address, handler):
            server = BoundedHTTPServer(address, handler)
            servers.append(server)
            return server

        def run_server():
            try:
                serve(str(self.root / "receiver.db"), no_auth=True, port=port,
                      local_agents=False, plugin_config=self.config)
            except Exception as exc:
                failures.append(exc)

        with patch("observatory.server.ThreadingHTTPServer", side_effect=make_server):
            thread = threading.Thread(target=run_server, daemon=True)
            thread.start()
            try:
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline:
                    try:
                        with urlopen(f"http://127.0.0.1:{port}/api/health", timeout=1) as response:
                            local = json.load(response)["local_agents"]
                        if local.get("sessions") and local.get("plugin_workers"):
                            break
                    except OSError:
                        pass
                    time.sleep(0.05)
                else:
                    self.fail(f"connector did not reach receiver health: {failures}")
                self.assertEqual(local["sessions"][0]["native_session_id"], "external")
                self.assertEqual(local["sessions"][0]["run_state"], "completed")
                self.assertEqual(local["plugin_workers"][0]["state"], "running")
            finally:
                if servers:
                    servers[0].shutdown()
                thread.join(timeout=8)
        self.assertFalse(thread.is_alive())
        self.assertEqual(failures, [])

    def test_installer_rejects_non_loopback_endpoint(self):
        target = self.root / "rejected"
        for url in ("https://example.com/events", "http://127.0.0.1:80/other",
                    "http://localhost:80/events", "http://127.0.0.1:80@evil.com/events"):
            result = subprocess.run([sys.executable, str(PACKAGE / "install.py"),
                                     "--destination", str(target), "--endpoint", url],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, url)
            self.assertFalse(target.exists())


if __name__ == "__main__":
    unittest.main()
