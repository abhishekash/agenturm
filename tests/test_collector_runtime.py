"""The receiver and terminal share an explicit trusted collector registry."""
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.collector_runtime import CollectorRegistry, start_local_collector
from observatory.native_readers import NativeReaderSpec
from observatory.native_sources import NativeSourceSpec
from tests.test_native_adapter import FixtureAdapter


class CollectorCompositionTests(unittest.TestCase):
    def test_bundled_registry_can_disable_without_constructing_disabled_reader(self):
        registry = CollectorRegistry.bundled()
        self.assertEqual({spec.id for spec in registry.sources},
                         {"codex", "claude", "pi", "kiro"})
        self.assertEqual({spec.id for spec in registry.readers}, {"opencode"})
        disabled = registry.without("pi", "opencode")
        self.assertEqual({spec.id for spec in disabled.sources},
                         {"codex", "claude", "kiro"})
        self.assertEqual(disabled.readers, ())
        self.assertEqual(len(registry.sources), 4)
        with self.assertRaisesRegex(ValueError, "unknown collector plugin"):
            registry.without("missing")

    def test_injected_sixth_adapter_is_selected_by_composition_root(self):
        fixture = NativeSourceSpec("fixture", "Fixture", lambda: Path("/absent"),
                                   adapter_factory=FixtureAdapter)
        registry = CollectorRegistry((fixture,), ())
        with patch("observatory.local_agents.LocalAgents.start", autospec=True,
                   side_effect=lambda monitor: monitor):
            monitor = start_local_collector(None, capture_details=False, registry=registry)
        self.assertIs(monitor.source_specs[0], fixture)
        self.assertEqual(monitor.reader_specs, ())
        self.assertFalse(monitor.capture_details)

    def test_conflicting_registration_fails_before_start(self):
        source = NativeSourceSpec("same", "Fixture", lambda: Path("/absent"),
                                  adapter_factory=FixtureAdapter)
        reader = NativeReaderSpec("same", lambda _store, _capture: object())
        with self.assertRaisesRegex(ValueError, "must be unique"):
            CollectorRegistry((source,), (reader,))
        with self.assertRaisesRegex(ValueError, "duplicate collector plugin"):
            CollectorRegistry((source, source), ())


if __name__ == "__main__":
    unittest.main()
