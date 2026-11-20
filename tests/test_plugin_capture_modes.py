"""Only receiver-owned capture declarations can select a process mode."""
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.plugin_capture_modes import (
    CaptureMode, DEFAULT_CAPTURE_MODES,
)
from observatory.plugin_config import read_plugin_config
from observatory.plugin_runtime import PluginSession
from observatory.plugin_supervisor import PluginSupervisor
from observatory.plugin_wire import ProtocolError
from observatory.server import serve
from tests import test_plugin_process


class CaptureModeRegistryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        fixture = test_plugin_process.ProcessHostTests("runTest")
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        manifest = fixture.manifest("ok")
        manifest_path = Path(manifest.origin)
        self.manifest_path = manifest_path
        manifest_path.write_text(manifest_path.read_text().replace(
            'provides = ["source-adapter/1"]',
            'provides = ["source-adapter/1", "audit-extension/1"]'))
        self.config = self.root / "plugins.toml"
        self.config.write_text(
            'schema = 1\n[[sources]]\n'
            f'manifest = "{manifest_path}"\n'
            'source_id = "audit-source"\n'
            'grants = ["configured-source-read"]\n'
            'capture = "audit-only-v1"\n'
            'poll_interval = 0.1\n'
            'timeout = 0.5\n'
            'requires_features = ["audit-extension/1"]\n')
        self.registry = DEFAULT_CAPTURE_MODES.with_mode(CaptureMode(
            "audit-only-v1", frozenset({"audit-extension/1"})))

    def test_trusted_second_mode_selects_config_and_session_without_private_refs(self):
        with self.assertRaisesRegex(ValueError, "unsupported plugin capture mode"):
            read_plugin_config(self.config)
        source = read_plugin_config(self.config, capture_modes=self.registry)[0]
        self.assertEqual(source.capture_mode, "audit-only-v1")
        store = Store(self.root / "agenturm.db")
        self.addCleanup(store.close)
        session = PluginSession(store, source.manifest, source_id=source.source_id,
                                allowed_grants=source.allowed_grants,
                                required_features=source.runtime_features,
                                capture_source=source, capture_modes=self.registry)
        self.assertIs(session.capture_mode, self.registry.get("audit-only-v1"))
        self.assertIsNone(session.capture_mode.reference_feature)
        self.assertIsNone(session.capture_grant)
        self.assertFalse(session.process.content_refs_enabled)
        self.assertIsNone(session.process._batch_feature)
        supervisor = PluginSupervisor(store, (source,), capture_modes=self.registry)
        self.assertEqual(supervisor.sources, (source,))
        with self.assertRaisesRegex(ValueError, "unsupported plugin capture mode"):
            PluginSupervisor(store, (source,))
        forged = replace(source, required_features=())
        forged_session = PluginSession(
            store, forged.manifest, source_id=forged.source_id,
            allowed_grants=forged.allowed_grants,
            required_features=forged.runtime_features,
            capture_source=forged, capture_modes=self.registry)
        with self.assertRaisesRegex(ProtocolError, "trusted mode policy"):
            forged_session.start()
        self.assertIsNone(forged_session.lease)

    def test_receiver_composition_passes_only_host_selected_registry(self):
        with (patch("observatory.server.ThreadingHTTPServer") as server,
              patch("observatory.plugin_supervisor.PluginSupervisor") as supervisor):
            supervisor.return_value.start.return_value = supervisor.return_value
            serve(str(self.root / "receiver.db"), no_auth=True, port=14341,
                  local_agents=False, plugin_config=self.config,
                  capture_modes=self.registry)
        self.assertIs(supervisor.call_args.kwargs["capture_modes"], self.registry)
        self.assertTrue(server.return_value.serve_forever.called)

    def test_config_cannot_install_validator_or_borrow_pi_reference_feature(self):
        self.config.write_text(self.config.read_text() + 'capture_registry = "file.py"\n')
        with self.assertRaisesRegex(ValueError, "invalid plugin source configuration"):
            read_plugin_config(self.config, capture_modes=self.registry)
        self.config.write_text(self.config.read_text().replace(
            'capture_registry = "file.py"\n', '').replace(
            '"audit-extension/1"]', '"audit-extension/1", "pi-content-ref/1"]'))
        self.manifest_path.write_text(self.manifest_path.read_text().replace(
            '"audit-extension/1"]', '"audit-extension/1", "pi-content-ref/1"]'))
        with self.assertRaisesRegex(ValueError, "matching explicit capture mode"):
            read_plugin_config(self.config, capture_modes=self.registry)

    def test_registry_is_bounded_and_immutable(self):
        with self.assertRaises(ValueError):
            self.registry.with_mode(CaptureMode("audit-only-v1"))
        with self.assertRaises(ValueError):
            CaptureMode("bad", required_features={"audit-extension/1"})
        with self.assertRaises(TypeError):
            self.registry._modes["untrusted"] = CaptureMode("untrusted")


if __name__ == "__main__":
    unittest.main()
