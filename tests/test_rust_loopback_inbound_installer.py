from __future__ import annotations

import hashlib
import os
import queue
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from examples.rust_loopback_inbound_connector import install as installer
from examples.rust_loopback_inbound_connector.install import MAX_ARTIFACT, install
from observatory.plugin_config import read_plugin_config


@unittest.skipUnless(os.name == "posix", "Rust connector installer requires POSIX")
class RustLoopbackInboundInstallerTests(unittest.TestCase):
    def setUp(self):
        binary = os.environ.get("AGENTURM_RUST_LOOPBACK_INBOUND_BIN")
        if not binary:
            self.skipTest("Rust loopback connector binary is not configured")
        self.binary = Path(binary)
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.destination = self.root / "installed" / "connector"
        self.endpoint = "http://127.0.0.1:32123/events"

    def test_copies_binary_and_pins_exact_connector_contract(self):
        config_path = install(self.binary, self.destination, self.endpoint, "source-one")
        source = self.destination / "rust-loopback-inbound"
        manifest = self.destination / "rust-loopback-inbound.toml"
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        self.assertEqual(manifest.stat().st_mode & 0o777, 0o600)
        self.assertIn(digest, manifest.read_text())
        self.assertEqual((self.destination / "source.json").read_text(),
                         '{"endpoint":"http://127.0.0.1:32123/events"}')
        configured, = read_plugin_config(config_path)
        self.assertEqual(configured.source_id, "source-one")
        self.assertEqual(configured.required_features, ("connector-adapter/1",))
        self.assertEqual(set(configured.allowed_grants),
                         {"configured-source-read", "loopback-http-read"})
        self.assertTrue({"source-adapter/1", "configured-root/1", "connector.inbound/1",
                         "connector-adapter/1"} <= set(configured.manifest.provides))

    def test_endpoint_policy_rejects_network_credentials_queries_and_redirect_targets(self):
        for endpoint in (
            "https://127.0.0.1:32123/events",
            "http://localhost:32123/events",
            "http://127.0.0.2:32123/events",
            "http://user@127.0.0.1:32123/events",
            "http://127.0.0.1:32123/events?after=x",
            "http://127.0.0.1:32123/events?",
            "http://127.0.0.1:32123/events#fragment",
            "http://127.0.0.1:32123/events#",
            "http://127.0.0.1:32123/events\n",
            "http://127.0.0.1:32123/other",
        ):
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                install(self.binary, self.destination, endpoint, "source-one")
            self.assertFalse(self.destination.exists())

    def test_refuses_existing_files_symlinks_and_linked_destination_components(self):
        self.destination.mkdir(parents=True)
        external = self.root / "external"
        external.write_text("do not replace")
        (self.destination / "source.json").symlink_to(external)
        with self.assertRaisesRegex(ValueError, "already contains"):
            install(self.binary, self.destination, self.endpoint, "source-one")
        self.assertTrue((self.destination / "source.json").is_symlink())
        self.assertEqual(external.read_text(), "do not replace")

        target = self.root / "real"
        target.mkdir()
        linked = self.root / "linked"
        linked.symlink_to(target, target_is_directory=True)
        with self.assertRaises(OSError):
            install(self.binary, linked / "new", self.endpoint, "source-one")
        self.assertFalse((target / "new").exists())

    def test_source_symlink_and_fifo_are_rejected_without_blocking(self):
        linked_binary = self.root / "binary-link"
        linked_binary.symlink_to(self.binary)
        with self.assertRaises(OSError):
            install(linked_binary, self.destination, self.endpoint, "source-one")

        fifo = self.root / "binary-fifo"
        os.mkfifo(fifo, 0o755)
        result = queue.Queue()

        def install_fifo():
            try:
                install(fifo, self.destination, self.endpoint, "source-one")
            except BaseException as error:
                result.put(error)

        thread = threading.Thread(target=install_fifo, daemon=True)
        thread.start()
        thread.join(timeout=1)
        self.assertFalse(thread.is_alive(), "FIFO source open must not block")
        self.assertIsInstance(result.get_nowait(), ValueError)
        self.assertFalse((self.destination / "rust-loopback-inbound").exists())

    def test_artifact_limit_is_checked_before_creating_installed_files(self):
        oversized = self.root / "oversized-binary"
        with oversized.open("wb") as stream:
            stream.truncate(MAX_ARTIFACT + 1)
        oversized.chmod(0o755)
        with self.assertRaisesRegex(ValueError, "64 MiB"):
            install(oversized, self.destination, self.endpoint, "source-one")
        self.assertEqual(list(self.destination.iterdir()), [])

    def test_same_size_binary_rewrite_during_copy_rolls_back_install(self):
        binary = self.root / "mutable-binary"
        binary.write_bytes(b"a" * (2 * 1024 * 1024))
        binary.chmod(0o755)
        original_read = os.read
        changed = []

        def read_then_mutate(fd, size):
            chunk = original_read(fd, size)
            if chunk and not changed:
                rewrite_fd = os.open(binary, os.O_WRONLY)
                try:
                    self.assertEqual(os.pwrite(rewrite_fd, b"z" * 4096, 1024 * 1024), 4096)
                finally:
                    os.close(rewrite_fd)
                changed.append(True)
            return chunk

        with patch.object(installer.os, "read", read_then_mutate):
            with self.assertRaisesRegex(ValueError, "changed while being copied"):
                install(binary, self.destination, self.endpoint, "source-one")
        self.assertTrue(changed)
        self.assertEqual(binary.stat().st_size, 2 * 1024 * 1024)
        self.assertEqual(list(self.destination.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
