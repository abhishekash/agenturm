"""Opt-in Rust process host through pinned artifacts, Store and process cleanup."""
import hashlib
import json
import os
import select
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.request import urlopen

from observatory.core import Store
from observatory.plugin_admission import activate_source, commit_batch, set_source_capture
from observatory.plugin_config import read_plugin_config
from observatory.plugin_manifest import PluginManifest
from observatory.plugin_process import PluginProcess
from observatory.plugin_projection import project_source
from observatory.plugin_runtime import PluginSession
from observatory.plugin_wire import ProtocolError
from observatory.plugin_frames import read_frame, write_frame
from observatory.server import BoundedHTTPServer, serve


PACKAGE = Path(__file__).resolve().parents[1] / "examples" / "rust_sample_harness_plugin"


def _running(pid):
    """Linux CI: a reparented zombie is no longer a runnable worker."""
    status = Path(f"/proc/{pid}/stat")
    try:
        return status.read_text().split(") ", 1)[1][0] != "Z"
    except FileNotFoundError:
        return False


class RustPluginHostTests(unittest.TestCase):
    def setUp(self):
        host = os.environ.get("AGENTURM_RUST_PLUGIN_HOST_BIN")
        adapter = os.environ.get("AGENTURM_RUST_SAMPLE_BIN")
        if not host or not adapter or not Path("/proc").exists():
            self.skipTest("Rust host and sample binaries are built in Linux Rust CI")
        self.host = str(Path(host).resolve())
        self.host_hash = hashlib.sha256(Path(self.host).read_bytes()).hexdigest()
        self.adapter = str(Path(adapter).resolve())
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source_file = self.root / "events.jsonl"
        result = subprocess.run([
            sys.executable, str(PACKAGE / "install.py"),
            "--binary", self.adapter,
            "--destination", str(self.root / "installed"),
            "--source", str(self.source_file),
        ], capture_output=True, text=True, check=True)
        self.source = read_plugin_config(Path(result.stdout.strip()))[0]
        self.config = Path(result.stdout.strip())
        self.store = Store(str(self.root / "store.db"))
        self.addCleanup(self.store.close)

    def session(self):
        return PluginSession(
            self.store, self.source.manifest, source_id=self.source.source_id,
            allowed_grants=self.source.allowed_grants, timeout=1.0,
            source_root=self.source.source_root,
            rust_host_executable=self.host, rust_host_sha256=self.host_hash)

    def append(self, *rows):
        with self.source_file.open("ab") as stream:
            for row in rows:
                stream.write((json.dumps(row, separators=(",", ":")) + "\n").encode())

    def test_installed_adapter_commit_correction_restart_through_host(self):
        self.append({"id": "one", "session": "child", "at_ns": "100",
                     "kind": "lifecycle", "state": "started"},
                    {"id": "tokens", "session": "child", "at_ns": "110",
                     "kind": "usage", "usage": {"input_tokens": 3}})
        with self.session() as session:
            first = session.poll_once()
            self.assertEqual(len(first.records), 2)
            self.assertEqual(session.poll_once().records, ())
            with self.store._connect() as db:
                self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 2)
        with self.session() as resumed:
            self.assertEqual(resumed.cursor, first.cursor)
            self.append({"id": "one", "session": "child", "at_ns": "100",
                         "kind": "lifecycle", "revision": 1, "state": "completed"},
                        {"id": "tokens", "session": "child", "at_ns": "110",
                         "kind": "usage", "revision": 1, "usage": {"input_tokens": 5}})
            self.assertEqual(len(resumed.poll_once().records), 2)
            project_source(self.store, self.source.source_id)
            projected = self.store.plugin_sessions()["sessions"][0]
            self.assertEqual(projected["run_state"], "completed")
            self.assertEqual(projected["metadata_usage"], {"input_tokens": 5})
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 2)

    def test_wrong_host_digest_fails_before_child_launch(self):
        with self.assertRaisesRegex(ProtocolError, "Rust plugin host artifact hash mismatch"):
            PluginProcess(self.source.manifest, source_id=self.source.source_id,
                          allowed_grants=self.source.allowed_grants,
                          source_root=self.source.source_root,
                          rust_host_executable=self.host, rust_host_sha256="0" * 64)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_source_cursor_v1").fetchone()[0], 0)

    def test_crash_after_commit_before_ack_restarts_from_store_cursor(self):
        self.append({"id": "one", "session": "resume", "at_ns": "100",
                     "kind": "lifecycle", "state": "completed"})
        manifest = self.source.manifest
        lease = activate_source(self.store, plugin_id=manifest.id,
                                source_id=self.source.source_id,
                                generation=manifest.artifact_sha256)

        def host():
            return PluginProcess(manifest, source_id=self.source.source_id,
                                 allowed_grants=self.source.allowed_grants,
                                 source_root=self.source.source_root,
                                 rust_host_executable=self.host,
                                 rust_host_sha256=self.host_hash)

        with host() as first:
            batch = first.poll("")
            committed = commit_batch(self.store, lease, "", batch)
            self.assertEqual(committed, batch.cursor)
            # Kill the host and child without sending ack, as after a crash.
            first.close(force=True)
        restarted = activate_source(self.store, plugin_id=manifest.id,
                                    source_id=self.source.source_id,
                                    generation=manifest.artifact_sha256)
        self.assertEqual(restarted.cursor, committed)
        with host() as resumed:
            idle = resumed.poll(restarted.cursor)
            self.assertEqual(idle.records, ())
            self.assertEqual(commit_batch(self.store, restarted, restarted.cursor, idle),
                             restarted.cursor)
            resumed.ack(restarted.cursor)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 1)
            self.assertEqual(db.execute("SELECT dirty_seq FROM plugin_source_cursor_v1").fetchone()[0], 1)

    def test_policy_revocation_fences_rust_hosted_worker(self):
        self.append({"id": "one", "session": "revoked", "at_ns": "100",
                     "kind": "lifecycle", "state": "started"})
        with self.session() as worker:
            first = worker.poll_once()
            self.assertTrue(first.records)
            set_source_capture(self.store, source_id=self.source.source_id, enabled=False)
            self.append({"id": "later", "session": "revoked", "at_ns": "200",
                         "kind": "lifecycle", "state": "completed"})
            with self.assertRaisesRegex(ProtocolError, "stale plugin source lease"):
                worker.poll_once()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM plugin_evidence_v1").fetchone()[0], 0)

    def test_receiver_health_and_pre_store_host_preflight(self):
        self.append({"id": "one", "session": "health", "at_ns": "100",
                     "kind": "lifecycle", "state": "completed"})
        denied_db = self.root / "denied.db"
        with self.assertRaisesRegex(ProtocolError, "Rust plugin host artifact hash mismatch"):
            serve(str(denied_db), no_auth=True, port=14342,
                  local_agents=False, plugin_config=self.config,
                  rust_plugin_host=self.host,
                  rust_plugin_host_sha256="0" * 64)
        self.assertFalse(denied_db.exists())
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
                      local_agents=False, plugin_config=self.config,
                      rust_plugin_host=self.host,
                      rust_plugin_host_sha256=self.host_hash)
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
                    self.fail(f"Rust-hosted adapter did not reach receiver health: {failures}")
                self.assertEqual(local["sessions"][0]["native_session_id"], "health")
                self.assertEqual(local["sessions"][0]["run_state"], "completed")
                self.assertEqual(local["plugin_workers"][0]["state"], "running")
            finally:
                if servers:
                    servers[0].shutdown()
                thread.join(timeout=8)
        self.assertFalse(thread.is_alive())
        self.assertEqual(failures, [])

    def test_timeout_kills_host_and_child_process_group(self):
        marker = self.root / "child.pid"
        slow = self.root / "slow.py"
        slow.write_text(
            "#!/usr/bin/python3\n"
            "import json,os,struct,sys,time\n"
            "open(os.environ['AGENTURM_PLUGIN_SOURCE_ROOT'],'w').write(str(os.getpid()))\n"
            "def read():\n"
            " n=struct.unpack('>I',sys.stdin.buffer.read(4))[0]\n"
            " return json.loads(sys.stdin.buffer.read(n))\n"
            "def send(v):\n"
            " b=json.dumps(v).encode();sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()\n"
            "while True:\n"
            " m=read()\n"
            " if m['type']=='hello':send({'type':'hello_ack','protocol':1,'plugin_id':'fixture.slow','features':['source-adapter/1','configured-root/1']})\n"
            " elif m['type']=='poll':time.sleep(30)\n"
            " elif m['type']=='stop':break\n")
        slow.chmod(0o700)
        digest = hashlib.sha256(slow.read_bytes()).hexdigest()
        manifest_path = self.root / "slow.toml"
        manifest_path.write_text(
            'id = "fixture.slow"\nversion = "0.1.0"\nprotocol = "1"\n'
            f'entrypoint = ["{slow}"]\nartifact_sha256 = "{digest}"\n'
            'provides = ["source-adapter/1", "configured-root/1"]\n'
            'requires = ["evidence-ingest/1"]\ndepends_on = []\n'
            'grants = ["configured-source-read"]\n')
        host = PluginProcess(
            PluginManifest.read(manifest_path), source_id="slow-source",
            allowed_grants=("configured-source-read",), timeout=0.25,
            source_root=str(marker),
            rust_host_executable=self.host, rust_host_sha256=self.host_hash)
        with host:
            child_pid = int(marker.read_text())
            group_id = host.process.pid
            self.assertEqual(os.getpgid(child_pid), group_id)
            with self.assertRaisesRegex(ProtocolError, "timed out"):
                host.poll("")
        deadline = time.monotonic() + 2
        while _running(child_pid) and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertFalse(_running(child_pid), "Rust host left a runnable child behind")

    def test_host_crash_cleanup_kills_surviving_child(self):
        host = PluginProcess(
            self.source.manifest, source_id=self.source.source_id,
            allowed_grants=self.source.allowed_grants,
            source_root=self.source.source_root,
            rust_host_executable=self.host, rust_host_sha256=self.host_hash)
        with host:
            group_id = host.process.pid
            children = Path(f"/proc/{group_id}/task/{group_id}/children").read_text().split()
            self.assertEqual(len(children), 1)
            child_pid = int(children[0])
            self.assertTrue(_running(child_pid))
            host.process.kill()
            host.process.wait(timeout=2)
            host.close(force=True)
        deadline = time.monotonic() + 2
        while _running(child_pid) and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertFalse(_running(child_pid), "crashed Rust host left a runnable child behind")

    def test_parent_pipe_closure_ends_blocked_child_without_python_cleanup(self):
        source = self.root / "blocked.fifo"
        os.mkfifo(source)
        host = PluginProcess(
            self.source.manifest, source_id=self.source.source_id,
            allowed_grants=self.source.allowed_grants,
            source_root=str(source),
            rust_host_executable=self.host, rust_host_sha256=self.host_hash)
        try:
            host.start()
            group_id = host.process.pid
            children = Path(f"/proc/{group_id}/task/{group_id}/children").read_text().split()
            self.assertEqual(len(children), 1)
            child_pid = int(children[0])
            host._send({"type": "poll", "protocol": 1,
                        "source_id": self.source.source_id, "cursor": "",
                        "max_records": 256, "max_bytes": 4 * 1024 * 1024})
            # The child blocks opening the FIFO. Closing the parent control
            # pipe must make Rust stop it before Python calls close().
            host.process.stdin.close()
            self.assertEqual(host.process.wait(timeout=2), 2)
            self.assertFalse(_running(child_pid), "Rust host did not reap blocked child")
        finally:
            host.close(force=True)

    def test_saturated_control_queue_reaps_blocked_child_without_python_cleanup(self):
        source = self.root / "saturated.fifo"
        os.mkfifo(source)
        host = PluginProcess(
            self.source.manifest, source_id=self.source.source_id,
            allowed_grants=self.source.allowed_grants,
            source_root=str(source),
            rust_host_executable=self.host, rust_host_sha256=self.host_hash)
        try:
            host.start()
            group_id = host.process.pid
            children = Path(f"/proc/{group_id}/task/{group_id}/children").read_text().split()
            self.assertEqual(len(children), 1)
            child_pid = int(children[0])
            host._send({"type": "poll", "protocol": 1,
                        "source_id": self.source.source_id, "cursor": "",
                        "max_records": 256, "max_bytes": 4 * 1024 * 1024})
            # The child blocks opening the FIFO. A burst larger than the
            # control queue must fail closed while Python still holds stdin.
            time.sleep(0.05)
            frame = json.dumps({"type": "ack", "protocol": 1,
                                "cursor": "unused"}).encode()
            payload = (struct.pack(">I", len(frame)) + frame) * 3
            self.assertEqual(os.write(host.process.stdin.fileno(), payload), len(payload))
            self.assertEqual(host.process.wait(timeout=2), 2)
            self.assertFalse(host.process.stdin.closed)
            self.assertFalse(_running(child_pid), "Rust host did not reap blocked child")
        finally:
            host.close(force=True)

    def _raw_host(self, body):
        """Exercise transport supervision without Python's timeout/reader help."""
        child = self.root / "blocked-child.py"
        marker = self.root / "blocked-child.pid"
        marker.unlink(missing_ok=True)
        child.write_text(
            "#!/usr/bin/python3\nimport fcntl,json,os,struct,sys,time\n"
            "fcntl.fcntl(sys.stdin.fileno(),fcntl.F_SETPIPE_SZ,4096)\n"
            f"open({str(marker)!r},'w').write(str(os.getpid()))\n"
            "def read():\n"
            " n=struct.unpack('>I',sys.stdin.buffer.read(4))[0]\n"
            " return json.loads(sys.stdin.buffer.read(n))\n"
            "def send(v):\n"
            " b=json.dumps(v).encode();sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()\n"
            + body)
        child.chmod(0o700)
        process = subprocess.Popen([self.host, "--child", str(child)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)

        def cleanup():
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=2)
            for stream in (process.stdin, process.stdout, process.stderr):
                stream.close()

        self.addCleanup(cleanup)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            if marker.exists() and marker.read_text():
                return process, int(marker.read_text())
            time.sleep(0.01)
        self.fail("transport fixture did not start")

    def test_blocked_child_write_observes_parent_eof_and_write_deadline(self):
        for close_parent in (True, False):
            with self.subTest(close_parent=close_parent):
                process, child_pid = self._raw_host("time.sleep(30)\n")
                request = {"type": "hello", "protocol": 1, "padding": ""}
                # A transport-valid maximum control frame plus its prefix
                # exceeds the child's unread pipe, without exceeding our cap.
                request["padding"] = "x" * (65536 - len(json.dumps(request)))
                write_frame(process.stdin, json.dumps(request), max_bytes=65536)
                time.sleep(0.1)
                if close_parent:
                    process.stdin.close()
                self.assertEqual(process.wait(timeout=2 if close_parent else 7), 2)
                self.assertFalse(_running(child_pid), "blocked writer left child running")

    def test_blocked_parent_write_observes_parent_eof_and_write_deadline(self):
        for close_parent in (True, False):
            with self.subTest(close_parent=close_parent):
                process, child_pid = self._raw_host(
                    "read();send({'type':'hello_ack'})\n"
                    "read();send({'type':'batch','cursor':'next','padding':'x'*4000000})\n"
                    "time.sleep(30)\n")
                write_frame(process.stdin, json.dumps({"type": "hello", "protocol": 1}))
                self.assertTrue(select.select([process.stdout], [], [], 2)[0], "hello timed out")
                self.assertEqual(json.loads(read_frame(process.stdout))["type"], "hello_ack")
                write_frame(process.stdin, json.dumps({"type": "poll", "protocol": 1}))
                # Deliberately never drain the large response; only Rust can
                # observe control EOF or its write deadline and reap the child.
                time.sleep(0.2)
                if close_parent:
                    process.stdin.close()
                self.assertEqual(process.wait(timeout=2 if close_parent else 7), 2)
                self.assertFalse(_running(child_pid), "blocked writer left child running")

    def test_successful_close_reaps_group_helpers_before_leader(self):
        helper_marker = self.root / "helper.pid"
        child = self.root / "helper-parent.py"
        child.write_text(
            "#!/usr/bin/python3\nimport json,struct,sys,subprocess\n"
            "def read():\n"
            " n=struct.unpack('>I',sys.stdin.buffer.read(4))[0]\n"
            " return json.loads(sys.stdin.buffer.read(n))\n"
            "def send(v):\n"
            " b=json.dumps(v).encode();sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()\n"
            "read()\nhelper=subprocess.Popen(['/usr/bin/sleep','30'])\n"
            f"open({str(helper_marker)!r},'w').write(str(helper.pid))\n"
            "send({'type':'hello_ack','protocol':1,'plugin_id':'fixture.helper','features':['source-adapter/1']})\n"
            "read()\n")
        child.chmod(0o700)
        manifest_path = self.root / "helper.toml"
        manifest_path.write_text(
            'id="fixture.helper"\nversion="0.1.0"\nprotocol="1"\n'
            f'entrypoint=["{child}"]\nartifact_sha256="{hashlib.sha256(child.read_bytes()).hexdigest()}"\n'
            'provides=["source-adapter/1"]\nrequires=["evidence-ingest/1"]\n'
            'depends_on=[]\ngrants=[]\n')
        host = PluginProcess(PluginManifest.read(manifest_path), source_id="helper-source",
                             rust_host_executable=self.host, rust_host_sha256=self.host_hash)
        self.addCleanup(host.close, force=True)
        host.start()
        process = host.process
        helper_pid = int(helper_marker.read_text())
        self.addCleanup(lambda: os.kill(helper_pid, signal.SIGKILL) if _running(helper_pid) else None)
        self.assertEqual(os.getpgid(helper_pid), process.pid)
        original_signal = host._signal_group
        reserved_leader = []

        def signal_group(sig):
            if sig == signal.SIGKILL:
                reserved_leader.append(process.returncode is None and Path(f"/proc/{process.pid}").exists())
            original_signal(sig)

        with patch.object(host, "_signal_group", side_effect=signal_group):
            host.close()
        self.assertEqual(process.returncode, 0)
        self.assertEqual(reserved_leader, [True])
        deadline = time.monotonic() + 2
        while _running(helper_pid) and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertFalse(_running(helper_pid), "successful close left group helper running")


if __name__ == "__main__":
    unittest.main()
