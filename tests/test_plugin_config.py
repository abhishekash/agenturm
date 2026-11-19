"""Explicit configuration never discovers or executes project-local code."""
import json
import os
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.request import urlopen

from observatory.plugin_config import read_plugin_config
from observatory.plugin_process import PluginProcess
from observatory.server import BoundedHTTPServer, serve
from tests import test_plugin_process


class ConfigTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fixture = test_plugin_process.ProcessHostTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        manifest = self.fixture.manifest("ok")
        self.config = self.root / "plugins.toml"
        self.config.write_text(
            'schema = 1\n[[sources]]\n'
            f'manifest = "{manifest.origin}"\n'
            'source_id = "fixture-source"\n'
            'grants = ["configured-source-read"]\n'
            'capture = "metadata-only"\n'
            'poll_interval = 0.1\n'
            'timeout = 0.5\n')

    def test_explicit_config_is_inert_and_bounded(self):
        sources = read_plugin_config(self.config)
        self.assertEqual(len(sources), 1)
        self.assertEqual(sources[0].source_id, "fixture-source")
        self.assertFalse(self.fixture.marker.exists())
        relative = self.config.read_text().replace(
            str(sources[0].manifest.origin),
            os.path.relpath(sources[0].manifest.origin, self.root))
        self.config.write_text(relative)
        self.assertEqual(read_plugin_config(self.config)[0].manifest.origin,
                         sources[0].manifest.origin)
        for invalid in (self.config.read_text().replace('schema = 1', 'schema = true'),
                        self.config.read_text().replace('metadata-only', 'content'),
                        self.config.read_text().replace('timeout = 0.5', 'timeout = -1'),
                        self.config.read_text() + 'unknown = "field"\n'):
            self.config.write_text(invalid)
            with self.assertRaises(ValueError):
                read_plugin_config(self.config)

    def test_required_features_are_claimed_and_checked_against_child_handshake(self):
        original = self.config.read_text()
        self.config.write_text(original + 'requires_features = ["connector-adapter/1"]\n')
        with self.assertRaisesRegex(ValueError, "unclaimed required"):
            read_plugin_config(self.config)
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "connector-adapter/1"]'))
        source = read_plugin_config(self.config)[0]
        self.assertEqual(source.required_features, ("connector-adapter/1",))
        self.assertEqual(source.runtime_features, ("connector-adapter/1",))
        host = PluginProcess(source.manifest, source_id=source.source_id,
                             allowed_grants=source.allowed_grants,
                             required_features=source.required_features)
        with self.assertRaisesRegex(ValueError, "handshake features"):
            host.start()
        self.assertIsNone(host.process)
        for features in ('["connector-adapter/1", "connector-adapter/1"]',
                         '["bad feature"]', '"connector-adapter/1"'):
            with self.subTest(features=features):
                self.config.write_text(original + f'requires_features = {features}\n')
                with self.assertRaises(ValueError):
                    read_plugin_config(self.config)

    def test_serve_preflight_receives_required_features_before_store_creation(self):
        original = self.config.read_text()
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "connector-adapter/1"]'))
        self.config.write_text(original + 'requires_features = ["connector-adapter/1"]\n')
        target = self.root / "feature-preflight.db"
        observed = []

        def inspect(_manifest, **kwargs):
            observed.append(kwargs["required_features"])
            raise RuntimeError("stopped after inert preflight")

        with patch("observatory.plugin_process.PluginProcess", side_effect=inspect):
            with self.assertRaisesRegex(RuntimeError, "stopped after inert preflight"):
                serve(str(target), no_auth=True, port=14331,
                      local_agents=False, plugin_config=self.config)
        self.assertEqual(observed, [("connector-adapter/1",)])
        self.assertFalse(target.exists())

    def test_non_loopback_activation_is_rejected_before_launch(self):
        with self.assertRaisesRegex(ValueError, "loopback"):
            serve(str(self.root / "external.db"), host="0.0.0.0", port=14319,
                  no_auth=True, allowed_hosts=("example.test",), local_agents=False,
                  plugin_config=self.config)
        self.assertFalse(self.fixture.marker.exists())
        self.assertFalse((self.root / "external.db").exists())

    def test_replacement_requires_manifest_claim_and_filters_trusted_reader(self):
        text = self.config.read_text() + 'replaces = ["pi"]\n'
        self.config.write_text(text)
        with self.assertRaisesRegex(ValueError, "unclaimed"):
            read_plugin_config(self.config)
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "harness.pi/1"]'))
        self.assertEqual(read_plugin_config(self.config)[0].replaces, ("pi",))
        self.config.write_text(text.replace('replaces = ["pi"]',
                                            'replaces = ["pi", "pi"]'))
        with self.assertRaisesRegex(ValueError, "unclaimed"):
            read_plugin_config(self.config)
        self.config.write_text(text)
        with patch("observatory.server.ThreadingHTTPServer") as server, \
             patch("observatory.collector_runtime.start_local_collector") as collector, \
             patch("observatory.plugin_supervisor.PluginSupervisor") as supervisor:
            supervisor.return_value.start.return_value = supervisor.return_value
            serve(str(self.root / "replacement.db"), no_auth=True, port=14321,
                  plugin_config=self.config)
        selected = collector.call_args.kwargs["registry"]
        self.assertNotIn("pi", {spec.id for spec in selected.sources})
        self.assertIn("codex", {spec.id for spec in selected.sources})
        self.assertTrue(server.return_value.serve_forever.called)
        self.assertFalse(self.fixture.marker.exists())

    def test_replacement_can_claim_registered_non_jsonl_reader(self):
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "harness.opencode/1"]'))
        self.config.write_text(self.config.read_text() + 'replaces = ["opencode"]\n')
        source = read_plugin_config(self.config)[0]
        self.assertEqual(source.replaces, ("opencode",))
        with patch("observatory.server.ThreadingHTTPServer") as server, \
             patch("observatory.collector_runtime.start_local_collector") as collector, \
             patch("observatory.plugin_supervisor.PluginSupervisor") as supervisor:
            supervisor.return_value.start.return_value = supervisor.return_value
            serve(str(self.root / "reader-replacement.db"), no_auth=True, port=14330,
                  plugin_config=self.config)
        selected = collector.call_args.kwargs["registry"]
        self.assertNotIn("opencode", {spec.id for spec in selected.readers})
        self.assertIn("pi", {spec.id for spec in selected.sources})
        self.assertTrue(server.return_value.serve_forever.called)
        self.assertFalse(self.fixture.marker.exists())

    def test_replacement_rejects_unknown_source_before_store_creation(self):
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "harness.unknown/1"]'))
        self.config.write_text(self.config.read_text() + 'replaces = ["unknown"]\n')
        target = self.root / "unknown.db"
        with self.assertRaisesRegex(ValueError, "unavailable"):
            serve(str(target), no_auth=True, port=14322, plugin_config=self.config)
        self.assertFalse(target.exists())

    def test_invalid_plugin_grant_fails_before_store_creation(self):
        self.config.write_text(self.config.read_text().replace(
            'grants = ["configured-source-read"]', 'grants = []'))
        target = self.root / "bad-grant.db"
        with self.assertRaises(ValueError):
            serve(str(target), no_auth=True, port=14323, plugin_config=self.config)
        self.assertFalse(target.exists())

    def test_replacement_artifact_mismatch_fails_before_disabling_reader(self):
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "harness.pi/1"]'))
        self.config.write_text(self.config.read_text() + 'replaces = ["pi"]\n')
        (self.fixture.root / "plugin.py").write_text("#!/bin/sh\nexit 0\n")
        target = self.root / "bad-artifact.db"
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            serve(str(target), no_auth=True, port=14324, plugin_config=self.config)
        self.assertFalse(target.exists())

    def test_replacement_requires_local_collector_before_store_creation(self):
        manifest = self.fixture.root / "plugin.toml"
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "harness.pi/1"]'))
        self.config.write_text(self.config.read_text() + 'replaces = ["pi"]\n')
        target = self.root / "no-local.db"
        with self.assertRaisesRegex(ValueError, "local collection"):
            serve(str(target), no_auth=True, port=14325,
                  local_agents=False, plugin_config=self.config)
        self.assertFalse(target.exists())

    def test_configured_root_reaches_child_only_with_declared_feature(self):
        self.fixture.manifest("root")
        manifest = self.fixture.root / "plugin.toml"
        self.config.write_text(self.config.read_text() + f'root = "{self.fixture.root}"\n')
        with self.assertRaisesRegex(ValueError, "declared feature"):
            serve(str(self.root / "unclaimed-root.db"), no_auth=True, port=14326,
                  local_agents=False, plugin_config=self.config)
        self.assertFalse((self.root / "unclaimed-root.db").exists())
        manifest.write_text(manifest.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "configured-root/1"]'))
        source = read_plugin_config(self.config)[0]
        self.assertEqual(source.source_root, str(self.fixture.root))
        with PluginProcess(source.manifest, source_id=source.source_id,
                           allowed_grants=source.allowed_grants,
                           source_root=source.source_root) as child:
            self.assertEqual(child.poll("").records[0]["event_id"], "fixture-1")

    def test_configured_root_must_be_absolute_and_bounded(self):
        base = self.config.read_text()
        for root in ('relative/path', 'x' * 1025, '/tmp\\nother'):
            with self.subTest(root=root[:30]):
                self.config.write_text(base + f'root = "{root}"\n')
                with self.assertRaises(ValueError):
                    read_plugin_config(self.config)

    def test_serve_activates_only_the_named_config_and_exposes_session(self):
        running = []

        def make_server(address, handler):
            server = BoundedHTTPServer(address, handler)
            running.append(server)
            return server

        failures = []

        def run():
            try:
                serve(str(self.root / "agenturm.db"), host="127.0.0.1", port=port,
                      no_auth=True, local_agents=False, plugin_config=self.config)
            except Exception as exc:
                failures.append(exc)

        import socket
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        with patch("observatory.server.ThreadingHTTPServer", side_effect=make_server):
            thread = threading.Thread(target=run, daemon=True)
            thread.start()
            try:
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline:
                    if running:
                        try:
                            with urlopen(f"http://127.0.0.1:{port}/api/health", timeout=1) as response:
                                health = json.load(response)
                            local = health["local_agents"]
                            if local.get("sessions") and local.get("plugin_workers"):
                                break
                        except OSError:
                            pass
                    time.sleep(0.05)
                else:
                    self.fail(f"plugin did not reach health: {failures}")
                self.assertEqual(local["sessions"][0]["run_state"], "completed")
                self.assertEqual(local["plugin_workers"][0]["state"], "running")
                self.assertEqual(local["plugin_projection"]["pending_sources"], 0)
            finally:
                if running:
                    running[0].shutdown()
                thread.join(timeout=8)
        self.assertFalse(thread.is_alive())
        self.assertEqual(failures, [])


if __name__ == "__main__":
    unittest.main()
