from __future__ import annotations

import hashlib
import os
import tempfile
import unittest
from pathlib import Path

from examples.count_observations_process.install_rust import install
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


if __name__ == "__main__":
    unittest.main()
