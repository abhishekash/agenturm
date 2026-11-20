from __future__ import annotations

import hashlib
import os
import queue
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from examples.count_observations_process import install_rust
from examples.count_observations_process.install_rust import install
from examples.count_observations_process.install_rust import MAX_ARTIFACT
from observatory.capability_process import read_capability_config


@unittest.skipUnless(os.name == "posix", "installer requires POSIX no-follow flags")
class RustCountCapabilityInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / "already-built-count"
        self.binary.write_bytes(b"test executable bytes\x00\xff")
        self.binary.chmod(0o755)
        self.destination = self.root / "installed" / "count"

    def test_copies_binary_and_pins_the_exact_installed_bytes(self):
        descriptor = install(self.binary, self.destination)
        artifact = self.destination / "count-observations"
        manifest = self.destination / "count-observations.toml"
        copied = artifact.read_bytes()
        self.assertEqual(copied, self.binary.read_bytes())
        self.assertEqual(artifact.stat().st_mode & 0o777, 0o700)
        self.assertEqual(manifest.stat().st_mode & 0o777, 0o600)
        self.assertEqual(descriptor, self.destination / "capability.json")
        self.assertIn(hashlib.sha256(copied).hexdigest(), manifest.read_text())
        config = read_capability_config(descriptor)
        self.assertEqual(config.manifest.artifact_sha256, hashlib.sha256(copied).hexdigest())
        self.assertEqual(config.manifest.provides, ("capability-adapter/1",))
        self.assertEqual(config.pane_id, "count-observations")

    def test_rejects_symlinked_source_without_installing_files(self):
        symlink = self.root / "count-link"
        symlink.symlink_to(self.binary)
        with self.assertRaises(OSError):
            install(symlink, self.destination)
        self.assertFalse((self.destination / "count-observations").exists())
        self.assertFalse((self.destination / "count-observations.toml").exists())
        self.assertFalse((self.destination / "capability.json").exists())

    def test_refuses_existing_destination_entries_including_symlinks(self):
        external = self.root / "external-file"
        external.write_text("preserve")
        for existing in ("count-observations", "count-observations.toml", "capability.json"):
            with self.subTest(existing=existing):
                destination = self.root / f"installed-{existing}"
                destination.mkdir()
                entry = destination / existing
                if existing == "count-observations":
                    entry.symlink_to(external)
                else:
                    entry.write_text("preserve")
                with self.assertRaisesRegex(ValueError, "already contains"):
                    install(self.binary, destination)
                self.assertTrue(entry.is_symlink() if existing == "count-observations"
                                else entry.read_text() == "preserve")
                for other in {"count-observations", "count-observations.toml", "capability.json"} - {existing}:
                    self.assertFalse((destination / other).exists())

    def test_refuses_symlinked_destination_components(self):
        real = self.root / "real"
        real.mkdir()
        link = self.root / "linked"
        link.symlink_to(real, target_is_directory=True)
        with self.assertRaises(OSError):
            install(self.binary, link / "counts")
        self.assertFalse((real / "counts").exists())

    def test_rejects_non_executable_or_nonregular_source(self):
        self.binary.chmod(0o600)
        with self.assertRaisesRegex(ValueError, "executable regular file"):
            install(self.binary, self.destination)
        self.binary.chmod(0o755)
        directory = self.root / "binary-directory"
        directory.mkdir()
        with self.assertRaisesRegex(ValueError, "executable regular file"):
            install(directory, self.destination)

    def test_fifo_source_is_rejected_without_blocking(self):
        fifo = self.root / "count-fifo"
        os.mkfifo(fifo, 0o755)
        result = queue.Queue()

        def install_fifo():
            try:
                install(fifo, self.destination)
            except BaseException as error:
                result.put(error)

        thread = threading.Thread(target=install_fifo, daemon=True)
        thread.start()
        thread.join(timeout=1)
        self.assertFalse(thread.is_alive(), "opening a FIFO source must not block")
        self.assertIsInstance(result.get_nowait(), ValueError)
        self.assertFalse((self.destination / "count-observations").exists())

    def test_over_cap_binary_is_rejected_before_artifact_or_metadata_creation(self):
        oversized = self.root / "oversized-count"
        with oversized.open("wb") as stream:
            stream.truncate(install_rust.MAX_ARTIFACT + 1)
        oversized.chmod(0o755)
        with self.assertRaisesRegex(ValueError, "64 MiB"):
            install(oversized, self.destination)
        for name in ("count-observations", "count-observations.toml", "capability.json"):
            self.assertFalse((self.destination / name).exists())

    def test_exact_host_artifact_limit_is_installable(self):
        boundary = self.root / "boundary-count"
        with boundary.open("wb") as stream:
            stream.truncate(MAX_ARTIFACT)
        boundary.chmod(0o755)
        descriptor = install(boundary, self.destination)
        artifact = self.destination / "count-observations"
        self.assertEqual(artifact.stat().st_size, MAX_ARTIFACT)
        digest = hashlib.sha256()
        for _ in range(MAX_ARTIFACT // (1024 * 1024)):
            digest.update(bytes(1024 * 1024))
        self.assertEqual(read_capability_config(descriptor).manifest.artifact_sha256,
                         digest.hexdigest())

    def test_same_size_source_rewrite_during_copy_is_detected_and_rolled_back(self):
        self.binary.write_bytes(b"a" * (2 * 1024 * 1024))
        original_read = os.read
        changed = []

        def read_then_mutate(fd, size):
            chunk = original_read(fd, size)
            if chunk and not changed:
                rewrite_fd = os.open(self.binary, os.O_WRONLY)
                try:
                    self.assertEqual(os.pwrite(rewrite_fd, b"z" * 4096, 1024 * 1024), 4096)
                finally:
                    os.close(rewrite_fd)
                changed.append(True)
            return chunk

        with patch.object(install_rust.os, "read", read_then_mutate):
            with self.assertRaisesRegex(ValueError, "changed while it was being copied"):
                install(self.binary, self.destination)
        self.assertTrue(changed)
        self.assertEqual(self.binary.stat().st_size, 2 * 1024 * 1024)
        for name in ("count-observations", "count-observations.toml", "capability.json"):
            self.assertFalse((self.destination / name).exists())


if __name__ == "__main__":
    unittest.main()
