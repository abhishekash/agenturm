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

    def test_non_loopback_activation_is_rejected_before_launch(self):
        with self.assertRaisesRegex(ValueError, "loopback"):
            serve(str(self.root / "external.db"), host="0.0.0.0", port=14319,
                  no_auth=True, allowed_hosts=("example.test",), local_agents=False,
                  plugin_config=self.config)
        self.assertFalse(self.fixture.marker.exists())
        self.assertFalse((self.root / "external.db").exists())

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
