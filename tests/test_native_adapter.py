"""An injected JSONL format contributes evidence without a collector edit."""
import json
import os
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from observatory.core import Store
from observatory.local_activity import emit
from observatory.local_agents import LocalAgents, Session
from observatory.native_sources import NativeSourceSpec, builtin_sources


class FixtureAdapter:
    def initial_task(self, row):
        return row.get("prompt")

    def parse(self, session, row, stamp, prior_models):
        kind = row.get("type")
        if kind == "turn":
            session.turn = row["id"]
            session.started[session.turn] = stamp
            session.tasks[session.turn] = row["prompt"]
            return [session.root(stamp, "running")]
        if kind == "check":
            return [session.span(("check", row["id"]), "Fixture check", stamp,
                    {"tool.name": "fixture-check", "gen_ai.operation.name": "execute_tool"})]
        if kind == "complete":
            return [session.model_span(("model", row["id"]), row["usage"], stamp),
                    session.root(stamp, "completed")]
        return []

    def activity(self, session, row, stamp):
        if row.get("type") != "check":
            return []
        return [emit(session, row, stamp, "tool", "result", "Fixture check",
                     "passed", "success", {"tool.name": "fixture-check"})]

    def observe_state(self, session, row, stamp):
        return ("completed" if row.get("type") == "complete" else "running", None)


class AdapterTests(unittest.TestCase):
    def test_bundled_factories_discover_trusted_sources_in_stable_order(self):
        specs = builtin_sources()
        self.assertEqual([spec.id for spec in specs], ["codex", "claude", "pi", "kiro"])
        self.assertEqual([type(spec.build_adapter()).__name__ for spec in specs],
                         ["CodexAdapter", "ClaudeAdapter", "PiAdapter", "KiroAdapter"])

    def test_new_bundled_module_needs_no_dispatch_edit(self):
        import observatory.native_sources as sources
        module = SimpleNamespace(__name__="observatory.adapters.fixture", SOURCE_PRIORITY=9,
                                 source_spec=lambda: NativeSourceSpec(
            "fixture", "Fixture", lambda: Path("/absent"), adapter_factory=FixtureAdapter))
        with (patch.object(sources.pkgutil, "iter_modules", return_value=[SimpleNamespace(name="fixture")]),
              patch.object(sources.importlib, "import_module", return_value=module)):
            self.assertEqual([spec.id for spec in builtin_sources()], ["fixture"])
            session = Session(None, Path("/absent/fixture.jsonl"), "Fixture")
            self.assertIsInstance(session.adapter, FixtureAdapter)

    def test_bundled_factory_rejects_duplicate_ids(self):
        import observatory.native_sources as sources
        module = SimpleNamespace(__name__="observatory.adapters.fixture", SOURCE_PRIORITY=9,
                                 source_spec=lambda: NativeSourceSpec(
            "fixture", "Fixture", lambda: Path("/absent"), adapter_factory=FixtureAdapter))
        with (patch.object(sources.pkgutil, "iter_modules", return_value=[
                    SimpleNamespace(name="one"), SimpleNamespace(name="two")]),
              patch.object(sources.importlib, "import_module", return_value=module)):
            with self.assertRaisesRegex(ValueError, "duplicate bundled source ID"):
                builtin_sources()

    def test_injected_adapter_ingests_turn_check_usage_and_completion(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / "fixture.jsonl"
            now = time.time()
            rows = [
                {"type": "turn", "id": "turn-1", "timestamp": now, "prompt": "Run a fixture check"},
                {"type": "check", "id": "check-1", "timestamp": now + 1},
                {"type": "complete", "id": "done-1", "timestamp": now + 2,
                 "usage": {"input_tokens": 10, "output_tokens": 4}},
            ]
            source.write_text("".join(json.dumps(row) + "\n" for row in rows))
            store = Store(root / "agenturm.db")
            try:
                spec = NativeSourceSpec("fixture", "Fixture", lambda: root,
                    accepts_file=lambda path: Path(path) == source, adapter_factory=FixtureAdapter)
                collector = LocalAgents(store, source_specs=(spec,), reader_specs=())
                collector.discover()
                self.assertEqual(collector.poll(), 4)
                status = collector.status()["sessions"]
                self.assertEqual(len(status), 1)
                self.assertEqual(status[0]["run_state"], "completed")
                with store._connect() as db:
                    spans = db.execute("SELECT name,attributes FROM spans ORDER BY name").fetchall()
                    logs = db.execute("SELECT event_name FROM logs").fetchall()
                self.assertEqual({row["name"] for row in spans},
                                 {"agent.run", "Fixture check", "Fixture · model call"})
                model = next(json.loads(row["attributes"]) for row in spans
                             if row["name"] == "Fixture · model call")
                self.assertEqual(model["gen_ai.usage.input_tokens"], 10)
                self.assertEqual(len(logs), 1)
                self.assertEqual(logs[0]["event_name"], "agent.activity")
                disabled = LocalAgents(store, source_specs=(), reader_specs=())
                disabled.discover()
                self.assertEqual(disabled.sessions, {})
            finally:
                store.close()

    def test_adapter_contract_is_validated_before_collection(self):
        with self.assertRaisesRegex(ValueError, "no native adapter"):
            Session(None, Path("/absent/unknown.jsonl"), "Unknown")
        with self.assertRaisesRegex(ValueError, "requires an adapter factory"):
            NativeSourceSpec("bad", "Bad", lambda: Path("/absent"))
        with self.assertRaisesRegex(ValueError, "requires an adapter factory"):
            NativeSourceSpec("bad", "Bad", lambda: Path("/absent"), adapter_factory=object())
        with self.assertRaisesRegex(ValueError, "has an invalid adapter"):
            NativeSourceSpec("bad", "Bad", lambda: Path("/absent"),
                             adapter_factory=lambda: object()).build_adapter()

    def test_overlapping_adapters_do_not_ingest_ambiguous_file(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / "session.jsonl").touch()
            first = NativeSourceSpec("first", "First", lambda: root, adapter_factory=FixtureAdapter)
            alias = root.parent / (root.name + "-alias")
            alias.symlink_to(root, target_is_directory=True)
            self.addCleanup(alias.unlink)
            second = NativeSourceSpec("second", "Second", lambda: alias, adapter_factory=FixtureAdapter)
            collector = LocalAgents(None, source_specs=(first, second), reader_specs=())
            collector.discover()
            self.assertEqual(collector.sessions, {})
            self.assertEqual(collector.ambiguous_sources, 1)
            self.assertEqual(collector.last_error["operation"], "Resolving native source ownership")

    def test_adapter_factory_isolates_session_state(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / "one.jsonl").touch()
            (root / "two.jsonl").touch()
            spec = NativeSourceSpec("fixture", "Fixture", lambda: root,
                                    adapter_factory=FixtureAdapter)
            collector = LocalAgents(None, source_specs=(spec,), reader_specs=())
            collector.discover()
            self.assertEqual(len(collector.sessions), 2)
            left, right = (session.adapter for session in collector.sessions.values())
            self.assertIsNot(left, right)

    def test_conflicts_cannot_starve_older_valid_sources(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            overlap = root / "overlap"
            valid = root / "valid"
            overlap.mkdir()
            valid.mkdir()
            now = time.time()
            for index in range(60):
                path = overlap / f"ambiguous-{index}.jsonl"
                path.touch()
                os.utime(path, (now, now))
            for index in range(50):
                path = valid / f"valid-{index}.jsonl"
                path.touch()
                os.utime(path, (now - 100, now - 100))
            alias = root / "alias"
            alias.symlink_to(overlap, target_is_directory=True)
            specs = (
                NativeSourceSpec("one", "One", lambda: overlap, adapter_factory=FixtureAdapter),
                NativeSourceSpec("two", "Two", lambda: alias, adapter_factory=FixtureAdapter),
                NativeSourceSpec("valid", "Valid", lambda: valid, adapter_factory=FixtureAdapter),
            )
            collector = LocalAgents(None, source_specs=specs, reader_specs=())
            collector.discover()
            self.assertEqual(collector.ambiguous_sources, 60)
            self.assertEqual(len(collector.sessions), 50)
            self.assertTrue(all(path.parent == valid for path in collector.sessions))


if __name__ == "__main__":
    unittest.main()
