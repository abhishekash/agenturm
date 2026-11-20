"""A new trusted harness contributes MCP config without collector dispatch edits."""
import json
import io
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from observatory.local_agents import discover_mcp_configurations
from observatory.mcp_config_registry import (McpConfigContribution,
                                             MAX_CONFIG_BYTES,
                                             bundled_contributions)


class McpConfigRegistryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_new_trusted_harness_declaration_is_discovered_without_collector_dispatch(self):
        config = self.root / "harness.json"
        config.write_text(json.dumps({"integrations": {"mcp": {
            "remote-docs": {"token": "private-secret"}}}}))
        spec = McpConfigContribution(
            "new-harness", "New Harness", (("integrations", "mcp"),),
            lambda: (config,), lambda _root: (), priority=9)
        good = SimpleNamespace(mcp_config_contributions=lambda: (spec,))
        broken = SimpleNamespace(mcp_config_contributions=lambda: 1 / 0)
        modules = [SimpleNamespace(name="bad"), SimpleNamespace(name="new_harness")]
        imports = {"observatory.adapters.bad": broken,
                   "observatory.adapters.new_harness": good}
        errors = []
        with (patch("observatory.mcp_config_registry.pkgutil.iter_modules", return_value=modules),
              patch("observatory.mcp_config_registry.importlib.import_module",
                    side_effect=lambda name: imports[name])):
            result = discover_mcp_configurations(on_error=lambda: errors.append("bad"))
        self.assertEqual(errors, ["bad"])
        self.assertEqual(result, [{"client": "New Harness", "scope": "User",
                                   "server": "remote-docs", "config": "harness.json"}])
        self.assertNotIn("private-secret", json.dumps(result))

    def test_faulty_path_factory_does_not_hide_siblings(self):
        first = self.root / "first.json"
        first.write_text('{"mcpServers":{"alpha":{}}}')
        good = McpConfigContribution("first", "First", (("mcpServers",),),
                                     lambda: (first,), lambda _root: (), priority=1)
        broken = McpConfigContribution("broken", "Broken", (("mcpServers",),),
                                       lambda: 1 / 0, lambda _root: (), priority=3)
        errors = []
        with patch("observatory.mcp_config_registry.bundled_contributions",
                   return_value=(good, broken)):
            result = discover_mcp_configurations(on_error=lambda: errors.append("bad"))
        self.assertEqual(result, [{"client": "First", "scope": "User",
                                   "server": "alpha", "config": "first.json"}])
        self.assertEqual(errors, ["bad"])

    def test_duplicate_client_rejected_without_hiding_first_declaration(self):
        first = McpConfigContribution("first", "First", (("mcpServers",),),
                                      lambda: (), lambda _root: (), priority=1)
        duplicate = McpConfigContribution("duplicate", "First", (("mcpServers",),),
                                          lambda: (), lambda _root: (), priority=2)
        modules = [SimpleNamespace(name="second"), SimpleNamespace(name="first")]
        imports = {"observatory.adapters.first": SimpleNamespace(mcp_config_contributions=lambda: (first,)),
                   "observatory.adapters.second": SimpleNamespace(mcp_config_contributions=lambda: (duplicate,))}
        errors = []
        with (patch("observatory.mcp_config_registry.pkgutil.iter_modules", return_value=modules),
              patch("observatory.mcp_config_registry.importlib.import_module",
                    side_effect=lambda name: imports[name])):
            specs = bundled_contributions(on_error=lambda: errors.append("duplicate"))
        self.assertEqual(specs, (first,))
        self.assertEqual(errors, ["duplicate"])

    def test_config_read_is_bounded_even_when_file_grows(self):
        class TrackedFile(io.BytesIO):
            def read(self, size=-1):
                self.last_size = size
                return super().read(size)

        handle = TrackedFile(b"{" + b" " * (MAX_CONFIG_BYTES + 1))
        errors = []
        candidate = self.root / "growing.json"
        candidate.write_bytes(b"{}")
        with patch.object(Path, "open", return_value=handle):
            found = discover_mcp_configurations(
                global_paths=[(candidate, "Custom", "User")],
                on_error=lambda: errors.append("oversize"))
        self.assertEqual(handle.last_size, MAX_CONFIG_BYTES + 1)
        self.assertEqual(found, [])
        self.assertEqual(errors, ["oversize"])

    def test_bundled_declarations_are_deterministic_and_include_system_scope(self):
        specs = bundled_contributions()
        self.assertEqual(len({spec.id for spec in specs}), len(specs))
        self.assertEqual([spec.id for spec in specs],
                         [spec.id for spec in sorted(specs, key=lambda spec: (spec.priority, spec.id))])
        self.assertIn("Gemini CLI", {spec.client for spec in specs})
        system = self.root / "system.json"
        system.write_text('{"mcpServers":{"system-only":{}}}')
        with patch.dict("os.environ", {"GEMINI_CLI_SYSTEM_SETTINGS_PATH": str(system)}):
            names = discover_mcp_configurations(project_directories=[])
        self.assertIn(("Gemini CLI", "System", "system-only"),
                      {(row["client"], row["scope"], row["server"]) for row in names})


if __name__ == "__main__":
    unittest.main()
