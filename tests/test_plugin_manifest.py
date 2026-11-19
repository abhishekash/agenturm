"""Manifest discovery is inert and activation is deterministic and fail-closed."""
import json
import tempfile
import unittest
from pathlib import Path

from observatory.plugin_manifest import (ManifestError, PluginManifest,
                                         discover_manifests, plan_activation)


def write_manifest(root, plugin_id, *, provides=(), requires=(), depends_on=(),
                   grants=(), protocol="1", entrypoint=("fixture-plugin",)):
    path = root / (plugin_id.replace(".", "-") + ".toml")
    fields = {
        "id": plugin_id, "version": "0.1.0", "protocol": protocol,
        "entrypoint": list(entrypoint), "artifact_sha256": "a" * 64,
        "provides": list(provides), "requires": list(requires),
        "depends_on": list(depends_on), "grants": list(grants),
    }
    path.write_text("".join(f"{key} = {json.dumps(value)}\n" for key, value in fields.items()))
    return path


class ManifestTests(unittest.TestCase):
    def test_discovery_does_not_execute_manifest_entrypoint(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "executed"
            executable = root / "plugin"
            executable.write_text(f"#!/bin/sh\ntouch {marker}\n")
            executable.chmod(0o755)
            path = write_manifest(root, "fixture.reader", entrypoint=(str(executable),))
            manifests = discover_manifests([path])
            self.assertEqual(manifests["fixture.reader"].entrypoint, (str(executable),))
            self.assertFalse(marker.exists())
            self.assertEqual(plan_activation(manifests, (), host_features=(), allowed_grants=()), ())
            self.assertFalse(marker.exists())

    def test_activation_orders_dependencies_and_resolves_one_provider(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = [
                write_manifest(root, "fixture.view", requires=("metrics/1",),
                               grants=("scoped-read",)),
                write_manifest(root, "fixture.metrics", provides=("metrics/1",),
                               requires=("evidence-ingest/1",)),
                write_manifest(root, "fixture.source", provides=("source-adapter/1",),
                               grants=("configured-source-read",)),
            ]
            manifests = discover_manifests(reversed(files))
            order = plan_activation(manifests, manifests,
                host_features=("evidence-ingest/1",),
                allowed_grants=("scoped-read", "configured-source-read"))
            self.assertEqual([item.id for item in order],
                             ["fixture.metrics", "fixture.source", "fixture.view"])
            with self.assertRaisesRegex(ManifestError, "disabled dependency|missing required feature"):
                plan_activation(manifests, ("fixture.view",),
                    host_features=("evidence-ingest/1",), allowed_grants=("scoped-read",))
            with self.assertRaisesRegex(ManifestError, "ungranted access"):
                plan_activation(manifests, ("fixture.source",),
                    host_features=(), allowed_grants=())

    def test_conflicts_cycles_and_protocol_mismatch_fail_before_start(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = [
                write_manifest(root, "fixture.one", provides=("metrics/1",),
                               depends_on=("fixture.two",)),
                write_manifest(root, "fixture.two", provides=("metrics/1",),
                               depends_on=("fixture.one",)),
                write_manifest(root, "fixture.view", requires=("metrics/1",)),
            ]
            manifests = discover_manifests(files)
            with self.assertRaisesRegex(ManifestError, "ambiguous provider"):
                plan_activation(manifests, manifests, host_features=(), allowed_grants=())
            with self.assertRaisesRegex(ManifestError, "dependency cycle"):
                plan_activation(manifests, ("fixture.one", "fixture.two"),
                    host_features=(), allowed_grants=())
            with self.assertRaisesRegex(ManifestError, "duplicate plugin ID"):
                discover_manifests((files[0], files[0]))
            incompatible = PluginManifest.read(write_manifest(root, "fixture.old", protocol="2..3"))
            with self.assertRaisesRegex(ManifestError, "incompatible protocol"):
                plan_activation({incompatible.id: incompatible}, (incompatible.id,),
                    host_features=(), allowed_grants=(), protocol_major=1)

    def test_multiple_providers_require_an_explicit_choice(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [
                write_manifest(root, "fixture.alpha", provides=("metrics/1",)),
                write_manifest(root, "fixture.beta", provides=("metrics/1",)),
                write_manifest(root, "fixture.view", requires=("metrics/1",),
                               depends_on=("fixture.beta",)),
            ]
            manifests = discover_manifests(paths)
            order = plan_activation(manifests, manifests, host_features=(), allowed_grants=())
            self.assertLess([item.id for item in order].index("fixture.beta"),
                            [item.id for item in order].index("fixture.view"))

    def test_manifest_rejects_unknown_fields_oversize_and_relative_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = write_manifest(root, "fixture.reader")
            path.write_text(path.read_text() + "surprise = true\n")
            with self.assertRaisesRegex(ManifestError, "missing or unknown fields"):
                PluginManifest.read(path)
            path.write_bytes(b"x" * (64 * 1024 + 1))
            with self.assertRaisesRegex(ManifestError, "exceeds byte limit"):
                PluginManifest.read(path)
            path = write_manifest(root, "fixture.reader", entrypoint=("./plugin",))
            with self.assertRaisesRegex(ManifestError, "command name or absolute path"):
                PluginManifest.read(path)


if __name__ == "__main__":
    unittest.main()
