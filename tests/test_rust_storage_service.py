"""Installed Linux checks for the feature-off Rust storage-service skeleton."""
from __future__ import annotations

import json
import os
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


@unittest.skipUnless(sys.platform == "linux", "Linux peer credentials required")
class InstalledStorageServiceTests(unittest.TestCase):
    def setUp(self):
        executable = os.environ.get("AGENTURM_RUST_STORAGE_SERVICE_BIN")
        if not executable:
            self.skipTest("set AGENTURM_RUST_STORAGE_SERVICE_BIN to the built service binary")
        self.executable = executable
        self.temp = tempfile.TemporaryDirectory(prefix="agenturm-storage-service-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.data = self.root / "private-data"
        self.data.mkdir(mode=0o700)
        self.transport = self.root / "transport"
        self.transport.mkdir(mode=0o700)
        self.transport.chmod(0o2710)
        self.socket_path = self.transport / "agenturm-storage-v1.sock"
        self.lock_path = self.transport / ".agenturm-storage-v1.lock"
        self.children = []
        self.addCleanup(self.stop_children)

    def args(self, *, max_connections=None, client_uid=None):
        args = [
            self.executable,
            "--data-root", str(self.data),
            "--transport-dir", str(self.transport),
            "--receiver-id", "receiver:installed",
            "--client-uid", str(os.geteuid() + 1 if client_uid is None else client_uid),
            "--client-gid", str(os.getegid()),
        ]
        if max_connections is not None:
            args += ["--max-connections", str(max_connections)]
        return args

    def run_rejected(self, *, client_uid=None):
        result = subprocess.run(self.args(max_connections=1, client_uid=client_uid),
                                capture_output=True, text=True, timeout=5)
        self.assertNotEqual(result.returncode, 0, result.stderr)
        return result

    def start(self, *, max_connections=None):
        child = subprocess.Popen(self.args(max_connections=max_connections),
                                 stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        self.children.append(child)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if child.poll() is not None:
                _, error = child.communicate(timeout=1)
                self.fail(f"service exited before readiness: {error}")
            if self.socket_path.exists():
                return child
            time.sleep(0.01)
        self.fail("service socket did not appear")

    def stop_children(self):
        for child in self.children:
            if child.poll() is None:
                child.send_signal(signal.SIGTERM)
            try:
                child.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.communicate(timeout=5)

    def test_distinct_identity_socket_and_graceful_shutdown(self):
        child = self.start(max_connections=1)
        info = self.socket_path.lstat()
        self.assertTrue(stat.S_ISSOCK(info.st_mode))
        self.assertEqual(stat.S_IMODE(info.st_mode), 0o660)
        self.assertEqual(info.st_uid, os.geteuid())
        self.assertEqual(info.st_gid, os.getegid())
        self.assertEqual(stat.S_IMODE(self.lock_path.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(self.data.stat().st_mode), 0o700)
        self.assertEqual(list(self.data.iterdir()), [])
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(self.socket_path))
            self.assertEqual(client.recv(1), b"")  # Same-UID client is denied.
        child.communicate(timeout=5)
        self.assertEqual(child.returncode, 0)
        self.assertFalse(self.socket_path.exists())
        self.assertEqual(list(self.data.iterdir()), [])
        restarted = self.start(max_connections=1)
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(self.socket_path))
            self.assertEqual(client.recv(1), b"")
        restarted.communicate(timeout=5)
        self.assertEqual(restarted.returncode, 0)
        self.assertFalse(self.socket_path.exists())

    def test_second_instance_cannot_unlink_first_socket(self):
        first = self.start()
        second = subprocess.run(self.args(max_connections=1), capture_output=True,
                                text=True, timeout=5)
        self.assertNotEqual(second.returncode, 0)
        self.assertIn("holds the lock", second.stderr)
        self.assertIsNone(first.poll())
        self.assertTrue(self.socket_path.exists())
        first.send_signal(signal.SIGTERM)
        first.communicate(timeout=5)
        self.assertEqual(first.returncode, 0)
        self.assertFalse(self.socket_path.exists())

    def test_second_instance_is_locked_even_if_socket_path_removed(self):
        first = self.start()
        self.socket_path.unlink()
        second = subprocess.run(self.args(max_connections=1), capture_output=True,
                                text=True, timeout=5)
        self.assertNotEqual(second.returncode, 0)
        self.assertIsNone(first.poll())
        self.assertFalse(self.socket_path.exists())
        first.send_signal(signal.SIGTERM)
        first.communicate(timeout=5)
        self.assertEqual(first.returncode, 0)
        self.assertTrue(self.lock_path.exists())

    def test_shutdown_preserves_replaced_socket_pathname(self):
        child = self.start()
        moved = self.transport / "original-socket"
        self.socket_path.rename(moved)
        self.socket_path.write_text("operator-owned replacement", encoding="utf-8")
        child.send_signal(signal.SIGTERM)
        child.communicate(timeout=5)
        self.assertEqual(child.returncode, 0)
        self.assertEqual(self.socket_path.read_text(encoding="utf-8"),
                         "operator-owned replacement")
        self.assertTrue(stat.S_ISSOCK(moved.lstat().st_mode))

    def test_data_root_change_exits_before_peer_processing(self):
        child = self.start()
        (self.data / "unexpected.db").write_bytes(b"untouched")
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(self.socket_path))
            self.assertEqual(client.recv(1), b"")
        _, error = child.communicate(timeout=5)
        self.assertNotEqual(child.returncode, 0)
        self.assertIn("data root", error)
        self.assertEqual((self.data / "unexpected.db").read_bytes(), b"untouched")
        self.assertFalse(self.socket_path.exists())

    def test_instance_lock_replacement_exits_without_following_link(self):
        child = self.start()
        moved = self.transport / "original-lock"
        self.lock_path.rename(moved)
        target = self.root / "lock-sentinel"
        target.write_text("keep", encoding="utf-8")
        self.lock_path.symlink_to(target)
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(self.socket_path))
            self.assertEqual(client.recv(1), b"")
        _, error = child.communicate(timeout=5)
        self.assertNotEqual(child.returncode, 0)
        self.assertIn("lock", error)
        self.assertTrue(self.lock_path.is_symlink())
        self.assertEqual(target.read_text(encoding="utf-8"), "keep")
        self.assertFalse(self.socket_path.exists())

    def test_existing_symlink_is_never_removed(self):
        target = self.root / "sentinel"
        target.write_text("keep", encoding="utf-8")
        self.socket_path.symlink_to(target)
        self.run_rejected()
        self.assertTrue(self.socket_path.is_symlink())
        self.assertEqual(target.read_text(encoding="utf-8"), "keep")

    def test_existing_lock_symlink_is_never_followed(self):
        target = self.root / "lock-sentinel"
        target.write_text("keep", encoding="utf-8")
        self.lock_path.symlink_to(target)
        self.run_rejected()
        self.assertTrue(self.lock_path.is_symlink())
        self.assertEqual(target.read_text(encoding="utf-8"), "keep")

    def test_private_data_and_separate_transport_preflight(self):
        self.data.chmod(0o755)
        self.run_rejected()
        self.data.chmod(0o700)
        (self.data / "existing.db").write_bytes(b"do-not-touch")
        self.run_rejected()
        self.assertEqual((self.data / "existing.db").read_bytes(), b"do-not-touch")
        (self.data / "existing.db").unlink()
        self.transport.chmod(0o700)
        self.run_rejected()
        self.transport.chmod(0o2710)
        self.run_rejected(client_uid=os.geteuid())
        self.assertFalse(self.socket_path.exists())

    def test_standalone_collector_history_is_rejected_by_legacy_write_modes(self):
        binaries = Path(self.executable).resolve().parent
        collector = binaries / "agenturm"
        plugin_host = binaries / "agenturm-plugin-host"
        pi = binaries / "agenturm-pi-harness"
        if not all(os.access(binary, os.X_OK) for binary in (collector, plugin_host, pi)):
            self.skipTest("build the standalone app, plugin host, and Pi adapter beside the service")

        source = self.root / "session.jsonl"
        source.write_text(
            '{"type":"session","id":"service-boundary"}\n'
            '{"id":"user-1","type":"message","timestamp":"2026-11-20T12:00:00Z",'
            '"message":{"role":"user","content":"local fixture"}}\n',
            encoding="utf-8",
        )
        config = self.root / "collection.json"
        config.write_text(json.dumps({
            "version": 1,
            "plugin_host": str(plugin_host),
            "sources": [{
                "plugin_id": "example.rust-pi-harness",
                "adapter": str(pi),
                "source_root": str(source),
                "source_id": "harness.pi",
            }],
        }), encoding="utf-8")
        history = self.root / "history.sqlite"
        collected = subprocess.run(
            [collector, "collect", "--config", config, "--db", history],
            capture_output=True, text=True, check=True, timeout=10,
        )
        self.assertIn("ACK sent=true", collected.stdout)
        expected = subprocess.check_output(
            [collector, "query", "--db", history], timeout=5,
        )
        self.assertTrue(json.loads(expected)["observations"])
        for option, filename, marker_error in (
            ("--experimental-v2-scratch", "scratch.db", "scratch database marker"),
            ("--experimental-real-clone-activation", "telemetry-clone.db",
             "real-schema activation clone marker"),
        ):
            with self.subTest(option=option):
                copy = self.data / filename
                subprocess.run(
                    [collector, "backup", "--db", history, "--output", copy],
                    capture_output=True, check=True, timeout=5,
                )
                before = copy.read_bytes()
                refused = subprocess.run(
                    self.args(max_connections=1) + [option, "enabled"],
                    capture_output=True, text=True, timeout=5,
                )
                self.assertNotEqual(refused.returncode, 0)
                self.assertIn(marker_error, refused.stderr)
                self.assertFalse(self.socket_path.exists())
                self.assertEqual(copy.read_bytes(), before)
                self.assertEqual(subprocess.check_output(
                    [collector, "query", "--db", copy], timeout=5,
                ), expected)
                copy.unlink()

    def test_symlink_and_writable_parent_are_rejected(self):
        real = self.root / "real-data"
        self.data.rename(real)
        self.data.symlink_to(real, target_is_directory=True)
        self.run_rejected()
        self.data.unlink()
        real.rename(self.data)
        self.root.chmod(0o777)
        self.run_rejected()
        self.root.chmod(0o700)
        self.assertFalse(self.socket_path.exists())


if __name__ == "__main__":
    unittest.main()
