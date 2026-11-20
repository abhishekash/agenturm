"""Installed schema-v3 count pane, exact host facts and publication fences."""
from __future__ import annotations

import asyncio
import hashlib
import json
import threading
import time
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from examples.action_observations_process.install import install as install_actions
from examples.count_observations_process.install import install as install_counts
from examples.prompt_signals_process.install import install as install_prompt
from observatory.agent_hierarchy import canonical_agent_id
from observatory.capability_activation import activate_capabilities
from observatory.capability_counts import (CountFacts, count_facts,
    model_from_count_facts, parse_count_profile)
from observatory.capability_process import CapabilityProcess, read_capability_config
from observatory.plugin_wire import ProtocolError
from observatory.ui_contributions import PaneContribution, PaneScope
from tests.test_capability_cockpit_activation import FakeApi, FakeFeed


class RevisionApi(FakeApi):
    def __init__(self, feed):
        super().__init__(feed)
        self.revision = "a" * 64
        self.rows = [
            ("message", "user", 1), ("shell", "execute", 2),
            ("check", "failed", 3)]
        self.has_revision = True

    def get(self, path):
        if path.startswith("/api/activity?"):
            self.calls.append(path)
            owner = self.feed.snapshot.local_status["sessions"][0]
            page = {"receiver_id": "receiver-one", "activity": [
                {"id": f"event-{number}", "service": owner["service"],
                 "session_id": owner["session_id"], "agent_id": owner["agent_id"],
                 "domain": domain, "action": action, "time_unix_nano": number}
                for domain, action, number in self.rows], "next_cursor": None}
            if self.has_revision:
                page["capture_revision"] = self.revision
            return page
        return super().get(path)


class CountCapabilityTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.path = install_counts(self.root / "count")
        self.config = read_capability_config(self.path)
        self.scope = PaneScope(canonical_agent_id("SDK", "svc", "owner"),
                               "svc", "session-one")
        self.feed = FakeFeed(self.scope)
        self.api = RevisionApi(self.feed)

    def wait_model(self, pane, state="ready"):
        deadline = time.monotonic() + 4
        model = None
        while time.monotonic() < deadline:
            model = pane.query(self.scope)
            if model.state == state and "Loading" not in model.summary:
                return model
            time.sleep(0.02)
        self.fail(f"pane did not reach {state}: {model}")

    def test_installed_process_gets_counts_only_and_host_recomputes_model(self):
        profile = self.config.count_profile
        facts = CountFacts("ready", 3,
            (("users", 1), ("shell", 1), ("checks", 1)), True,
            "a" * 64, "b" * 64)
        sent = []
        original = CapabilityProcess._send

        def record(process, value, *, timeout):
            sent.append(value)
            return original(process, value, timeout=timeout)

        with patch.object(CapabilityProcess, "_send", record):
            process = CapabilityProcess(self.config).start()
            try:
                model = process.query(facts)
            finally:
                process.close()
        self.assertEqual([row.cells[1] for row in model.rows], ["1", "1", "1.00"])
        wire = json.dumps(sent)
        for private in (self.scope.agent_id, self.scope.session_id, "a" * 64,
                        "b" * 64, "event-1", "PRIVATE"):
            self.assertNotIn(private, wire)
        self.assertEqual(model, model_from_count_facts(facts, profile,
                                                        include_coverage=False))

    def test_zero_denominator_and_partial_page_are_explicit(self):
        facts = CountFacts("degraded", 1,
            (("users", 0), ("shell", 1), ("checks", 0)), False,
            "a" * 64, "b" * 64)
        process = CapabilityProcess(self.config).start()
        self.addCleanup(process.close)
        model = process.query(facts)
        self.assertEqual(model.rows[-1].cells[1], "unavailable")
        self.assertIn("recent page partial", model.summary)
        self.assertEqual(model.state, "degraded")

    def test_config_refuses_unknown_or_unbounded_selectors(self):
        original = json.loads(self.path.read_text())
        mutations = []
        raw = json.loads(json.dumps(original))
        raw["facts"]["counts"][0]["domain"] = "message; DROP TABLE"
        mutations.append(raw)
        raw = json.loads(json.dumps(original))
        raw["facts"]["counts"].append(dict(raw["facts"]["counts"][0]))
        mutations.append(raw)
        raw = json.loads(json.dumps(original))
        raw["model"]["rows"][0]["value"] = {"count": "prompt_text"}
        mutations.append(raw)
        raw = json.loads(json.dumps(original))
        raw["facts"]["counts"] *= 3
        mutations.append(raw)
        raw = json.loads(json.dumps(original))
        raw["pane"]["columns"][0][1] = 8
        mutations.append(raw)
        raw = json.loads(json.dumps(original))
        raw["facts"]["counts"][0]["label"] = "Wide 測試"
        mutations.append(raw)
        for data in mutations:
            with self.subTest(data=data), self.assertRaises(ValueError):
                self.path.write_text(json.dumps(data))
                read_capability_config(self.path)
        self.path.write_text(json.dumps(original))

    def test_missing_policy_revision_refuses_child_and_pane(self):
        self.api.has_revision = False
        activations, panes = activate_capabilities((self.path,), self.api, self.feed)
        self.addCleanup(lambda: [(host.close(), gate.close()) for host, gate in activations])
        model = self.wait_model(panes[0], "unavailable")
        self.assertIn("capture", model.summary.lower())
        self.assertIsNone(activations[0][0].process)

    def test_revoke_reenable_aba_and_page_change_discard_child_result(self):
        for change in ("policy", "page"):
            with self.subTest(change=change):
                self.api.revision = "a" * 64
                self.api.rows = [("message", "user", 1), ("shell", "execute", 2)]
                entered = threading.Event()
                release = threading.Event()
                original = CapabilityProcess.query

                def held(process, facts):
                    result = original(process, facts)
                    entered.set()
                    release.wait(2)
                    return result

                with patch.object(CapabilityProcess, "query", held):
                    activations, panes = activate_capabilities((self.path,), self.api, self.feed)
                    try:
                        deadline = time.monotonic() + 3
                        while not entered.is_set() and time.monotonic() < deadline:
                            panes[0].query(self.scope)
                            entered.wait(0.02)
                        self.assertTrue(entered.is_set())
                        if change == "policy":
                            # The current grant is true again, but the durable
                            # policy generation advanced while the child worked.
                            self.api.allowed = False
                            self.api.revision = "c" * 64
                            self.api.allowed = True
                        else:
                            self.api.rows.append(("check", "failed", 3))
                        release.set()
                        self.assertIn("changed", self.wait_model(panes[0], "unavailable").summary)
                    finally:
                        release.set()
                        for host, gate in activations:
                            host.close()
                            gate.close()

    def test_three_installed_panes_render_without_new_ui_branch_at_60(self):
        prompt = install_prompt(self.root / "prompt")
        actions = install_actions(self.root / "action")
        activations, panes = activate_capabilities((prompt, actions, self.path),
                                                   self.api, self.feed)
        self.addCleanup(lambda: [(host.close(), gate.close()) for host, gate in activations])
        models = [self.wait_model(pane) for pane in panes]
        self.assertEqual([pane.id for pane in panes],
                         ["prompt-signals", "action-observations", "count-observations"])
        self.assertEqual(models[2].rows[-1].cells[1], "1.00")
        frozen = tuple(PaneContribution(pane.id, pane.title, pane.columns,
            lambda _scope, model=model: model) for pane, model in zip(panes, models))

        async def compact():
            from observatory.terminal_ui import ObservatoryApp
            from tests.test_terminal_search import ManualFeed
            from textual.widgets import TabbedContent
            for size in ((60, 18), (80, 24)):
                app = ObservatoryApp(ManualFeed(), persist_preferences=False, ui_panes=frozen)
                async with app.run_test(size=size) as pilot:
                    app.action_page("contrib-count-observations")
                    await pilot.pause()
                    self.assertEqual(app.query_one("#pages", TabbedContent).active,
                                     "contrib-count-observations")
                    table = app.query_one("#contrib-count-observations-table")
                    self.assertEqual(table.row_count, 3)
        asyncio.run(compact())

    def test_forged_child_label_is_rejected(self):
        artifact = self.root / "count" / "count-observations"
        raw = artifact.read_text().replace("User messages", "Prompt quality")
        artifact.write_text(raw)
        artifact.chmod(0o700)
        manifest = self.root / "count" / "count-observations.toml"
        manifest.write_text(manifest.read_text().replace(
            self.config.manifest.artifact_sha256,
            hashlib.sha256(raw.encode()).hexdigest()))
        config = read_capability_config(self.path)
        process = CapabilityProcess(config).start()
        self.addCleanup(process.close)
        facts = CountFacts("ready", 1, (("users", 1), ("shell", 0), ("checks", 0)),
                           True, "a" * 64, "b" * 64)
        with self.assertRaises(ProtocolError):
            process.query(facts)


if __name__ == "__main__":
    unittest.main()
