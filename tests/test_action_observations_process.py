"""Installed external capability model over exact host-owned aggregates."""
from __future__ import annotations

import asyncio
import hashlib
import json
import time
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from examples.action_observations_process.install import install as install_actions
from examples.prompt_signals_process.install import install as install_prompt
from observatory.agent_hierarchy import canonical_agent_id
from observatory.capability_activation import ReceiverActivityReader, activate_capabilities
from observatory.capability_facts import PromptSignalFacts
from observatory.capability_process import CapabilityProcess, read_capability_config
from observatory.plugin_wire import ProtocolError
from observatory.ui_contributions import PaneContribution, PaneScope
from tests.test_capability_cockpit_activation import FakeApi, FakeFeed


class ActionObservationsProcessTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.prompt_path = install_prompt(self.root / "prompt")
        self.action_path = install_actions(self.root / "actions")
        self.prompt = read_capability_config(self.prompt_path)
        self.action = read_capability_config(self.action_path)
        self.scope = PaneScope(canonical_agent_id("SDK", "svc", "owner"),
                               "svc", "session-one")

    def test_v1_and_v2_installed_processes_are_exact_and_content_free(self):
        facts = PromptSignalFacts("ready", observed_events=3, user_messages=2,
                                  shell_actions=1, first_shell_after_user_ms=250,
                                  window_complete=True)
        sent = []
        actual_send = CapabilityProcess._send

        def record(process, value, *, timeout):
            sent.append(value)
            return actual_send(process, value, timeout=timeout)

        with patch.object(CapabilityProcess, "_send", record):
            prompt = CapabilityProcess(self.prompt).start()
            actions = CapabilityProcess(self.action).start()
            try:
                legacy = prompt.query(facts)
                model = actions.query(facts)
                self.assertEqual(legacy.rows[0].cells[1], "2")
                self.assertEqual(model.rows[-1].cells,
                                 ("shell actions / user messages", "0.50"))
                self.assertEqual(model.state, "ready")
            finally:
                actions.close()
                prompt.close()
        self.assertNotIn("session-one", json.dumps(sent))
        self.assertNotIn(self.scope.agent_id, json.dumps(sent))
        self.assertEqual(len([item for item in sent if item["type"] == "facts"]), 2)

    def test_zero_denominator_and_partial_window_are_explicit(self):
        facts = PromptSignalFacts("degraded", observed_events=1, user_messages=0,
                                  shell_actions=1, window_complete=False)
        process = CapabilityProcess(self.action).start()
        self.addCleanup(process.close)
        model = process.query(facts)
        self.assertEqual(model.rows[-1].cells[1], "unavailable")
        self.assertIn("partial recent window", model.summary)
        self.assertEqual(model.state, "degraded")

    def test_receiver_cannot_override_bounded_typed_activity_page(self):
        class Gate:
            def confirm_live(self, _scope):
                return "receiver-one"
            def _deny(self, _scope):
                pass
            def _invalidate(self, _scope):
                pass

        class Api:
            rows = []
            def get(self, _path):
                return {"receiver_id": "receiver-one", "activity": self.rows}

        api = Api()
        reader = ReceiverActivityReader(api, Gate())
        for rows in ([{}] * 3, ["not-an-event"]):
            with self.subTest(rows=rows):
                api.rows = rows
                with self.assertRaises(ValueError):
                    reader.activity_page(limit=2, service=self.scope.service,
                        session_id=self.scope.session_id, agent_id=self.scope.agent_id,
                        subtree=False, include_reviews=False)

    def test_v2_formula_bounds_and_forged_process_model_fail_closed(self):
        original = json.loads(self.action_path.read_text())
        bad_models = [
            {"rows": [{"key": "bad", "value": {"fact": "prompt_text"}}]},
            {"rows": [{"key": "a", "value": {"fact": "user_messages"}},
                      {"key": "a", "value": {"fact": "shell_actions"}}]},
            {"rows": [{"key": "bad", "value":
                      {"ratio": ["shell_actions", "first_shell_after_user_ms"]}}]},
            {"rows": [{"key": f"row-{i}", "value": {"fact": "user_messages"}}
                      for i in range(7)]},
        ]
        for model in bad_models:
            with self.subTest(model=model):
                self.action_path.write_text(json.dumps({**original, "model": model}))
                with self.assertRaises(ValueError):
                    read_capability_config(self.action_path)
        oversized = json.loads(json.dumps(original))
        oversized["pane"]["columns"][0][1] = 80
        self.action_path.write_text(json.dumps(oversized))
        with self.assertRaises(ValueError):
            read_capability_config(self.action_path)
        self.action_path.write_text(json.dumps(original))

        facts = PromptSignalFacts("ready", observed_events=2, user_messages=1,
                                  shell_actions=1, window_complete=True)
        process = CapabilityProcess(self.action)
        valid = {"type": "model", "protocol": 1, "seq": 1, "state": "ready",
                 "summary": "2 scoped events · complete recent window · observational signals only",
                 "rows": [
                     {"key": "users", "cells": ["Recorded user messages", "1"], "detail": ""},
                     {"key": "shell", "cells": ["Recorded shell actions", "1"], "detail": ""},
                     {"key": "actions-per-user",
                      "cells": ["shell actions / user messages", "1.00"], "detail": ""}]}
        for forged in (
            {**valid, "summary": "100% prompt efficacy"},
            {**valid, "rows": [*valid["rows"][:2],
                               {**valid["rows"][2], "detail": "private command"}]},
            {**valid, "rows": list(reversed(valid["rows"]))},
        ):
            with self.subTest(forged=str(forged)[:70]), self.assertRaises(ProtocolError):
                process._validated_model(forged, 1, facts)

        artifact = self.root / "actions" / "action-observations"
        raw = artifact.read_text().replace("shell actions / user messages",
                                           "prompt efficacy score")
        artifact.write_text(raw)
        artifact.chmod(0o700)
        manifest = self.root / "actions" / "action-observations.toml"
        manifest.write_text(manifest.read_text().replace(
            self.action.manifest.artifact_sha256, hashlib.sha256(raw.encode()).hexdigest()))
        changed = read_capability_config(self.action_path)
        process = CapabilityProcess(changed).start()
        self.addCleanup(process.close)
        with self.assertRaises(ProtocolError):
            process.query(PromptSignalFacts("ready", observed_events=1,
                user_messages=1, shell_actions=1, window_complete=True))

    def test_two_explicit_panes_share_receiver_policy_and_render_at_60_columns(self):
        feed = FakeFeed(self.scope)
        api = FakeApi(feed)
        activations, panes = activate_capabilities((self.prompt_path, self.action_path), api, feed)
        self.addCleanup(lambda: [(host.close(), gate.close()) for host, gate in activations])
        self.assertEqual(panes[1].query(PaneScope("", "svc", "session-one")).state,
                         "unavailable")
        deadline = time.monotonic() + 4
        models = []
        while time.monotonic() < deadline:
            models = [pane.query(self.scope) for pane in panes]
            if all(model.state == "ready" for model in models):
                break
            time.sleep(0.02)
        self.assertEqual([model.state for model in models], ["ready", "ready"])
        self.assertEqual([pane.id for pane in panes], ["prompt-signals", "action-observations"])
        self.assertEqual(models[1].rows[-1].cells[1], "0.00")
        self.assertTrue(any(path.startswith("/api/activity?") for path in api.calls))

        # Freeze the installed, host-validated models for the UI shell test;
        # the live policy gate is tested above and at the revocation below.
        frozen = tuple(PaneContribution(pane.id, pane.title, pane.columns,
            lambda _scope, model=model: model) for pane, model in zip(panes, models))

        async def compact_layout():
            from observatory.terminal_ui import ObservatoryApp
            from tests.test_terminal_search import ManualFeed
            from textual.widgets import TabbedContent
            app = ObservatoryApp(ManualFeed(), persist_preferences=False, ui_panes=frozen)
            async with app.run_test(size=(60, 18)) as pilot:
                for pane in frozen:
                    app.action_page("contrib-" + pane.id)
                    await pilot.pause()
                    self.assertEqual(app.query_one("#pages", TabbedContent).active,
                                     "contrib-" + pane.id)
                    table = app.query_one("#contrib-" + pane.id + "-table")
                    self.assertEqual(table.row_count, 3)
                table = app.query_one("#contrib-action-observations-table")
                self.assertEqual(str(table.get_cell_at((2, 0))),
                                 "shell actions / user messages")
        asyncio.run(compact_layout())

        # A runtime failure is local to its pane; the other admitted process
        # and its scoped model remain available.
        action_host = activations[1][0]
        self.assertIsNotNone(action_host.process)
        action_host.process.process.kill()
        action_host.process.process.wait(timeout=2)
        with action_host.lock:
            action_host.model_at = 0
        panes[1].query(self.scope)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and panes[1].query(self.scope).state != "unavailable":
            time.sleep(0.02)
        self.assertEqual(panes[1].query(self.scope).state, "unavailable")
        self.assertEqual(panes[0].query(self.scope).state, "ready")

        api.allowed = False
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and any(
                pane.query(self.scope).state != "unavailable" for pane in panes):
            time.sleep(0.02)
        self.assertEqual([pane.query(self.scope).state for pane in panes],
                         ["unavailable", "unavailable"])


if __name__ == "__main__":
    unittest.main()
