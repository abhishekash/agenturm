"""Trusted session-inspector contributions preserve Kiro permission evidence."""
import unittest
from unittest.mock import patch

from observatory.terminal import _cockpit_app
from observatory.ui_kiro_inspector import KIRO_PERMISSIONS, permission_summary
from observatory.ui_session_inspector import (
    SessionInspectorModel, SessionInspectorProvider, SessionInspectorRegistry,
    without_label_collisions,
)
from tests.test_terminal_search import ManualFeed


PERMISSIONS = {
    "counts": {"requested": 2, "allow_once": 1, "policy_denied": 1},
    "autopilot": False,
    "recent": [{"decision": "allow_once", "capability": "shell",
                "tool": "execute_bash", "subject": ""}],
    "rules": [{"source": "Workspace", "folder": "project", "trusted": True,
               "status": "absent"}],
}
FOLDERS = ({"folder": "project", "trusted": True, "key": "project"},)
KIRO = {"runtime": "Kiro", "permissions": PERMISSIONS}


class InspectorRegistryTests(unittest.TestCase):
    def test_kiro_provider_preserves_overview_details_and_metadata_redaction(self):
        registry = SessionInspectorRegistry((KIRO_PERMISSIONS,))
        model = registry.for_session(KIRO, {"kiro_folders": FOLDERS})
        short, details = permission_summary(KIRO, FOLDERS)
        self.assertEqual(model.overview_fields, (("Permissions", short),))
        self.assertEqual(model.detail_sections, (("Permissions", details),))
        self.assertIn("trusted folder", short)
        self.assertIn("Trusted folders (1)", details)
        self.assertNotIn("/private/project/path", repr(model))
        self.assertEqual(registry.for_session({"runtime": "Kiro"}, {}),
                         SessionInspectorModel())
        self.assertEqual(registry.for_session({"runtime": "Codex", "permissions": PERMISSIONS},
                                              {"kiro_folders": FOLDERS}),
                         SessionInspectorModel())
        self.assertEqual(registry.for_session(KIRO, {}),
                         registry.for_session(KIRO, {"kiro_folders": []}))

    def test_second_provider_and_faulty_provider_are_isolated(self):
        other = SessionInspectorProvider(
            "fixture-adapter", lambda item: item.get("runtime") == "Fixture",
            lambda _item, _status: SessionInspectorModel(
                (("Adapter", "ready"),), (("Evidence", "fixture details"),)))
        broken = SessionInspectorProvider(
            "broken", lambda _item: True,
            lambda _item, _status: 1 / 0)
        registry = SessionInspectorRegistry((KIRO_PERMISSIONS, broken, other))
        self.assertEqual(registry.for_session({"runtime": "Fixture"}, {}),
                         SessionInspectorModel((("Adapter", "ready"),),
                                               (("Evidence", "fixture details"),)))
        self.assertEqual(registry.for_session(KIRO, {}).overview_fields[0][0],
                         "Permissions")
        invalid = SessionInspectorProvider(
            "invalid", lambda _item: True,
            lambda _item, _status: SessionInspectorModel(
                (("Unsafe", "\x1b[31msecret"),), ()))
        self.assertEqual(SessionInspectorRegistry((invalid,)).for_session(KIRO, {}),
                         SessionInspectorModel())
        with self.assertRaises(ValueError):
            SessionInspectorRegistry((other, other))

    def test_shell_labels_take_priority_over_provider_labels(self):
        self.assertEqual(without_label_collisions(
            ((" credits ", "shadow"), ("Adapter", "ready"),
             ("adapter", "duplicate")), ("Credits",)),
            (("Adapter", "ready"),))
        first = SessionInspectorProvider(
            "first", lambda _item: True,
            lambda _item, _status: SessionInspectorModel(
                (("Adapter", "first"),), (("Evidence", "first"),)))
        second = SessionInspectorProvider(
            "second", lambda _item: True,
            lambda _item, _status: SessionInspectorModel(
                ((" adapter ", "second"),), ((" evidence ", "second"),)))
        self.assertEqual(SessionInspectorRegistry((first, second)).for_session({}, {}),
                         SessionInspectorModel((("Adapter", "first"),),
                                               (("Evidence", "first"),)))


class InspectorLayoutTests(unittest.IsolatedAsyncioTestCase):
    async def test_cockpit_kiro_at_floor_and_standard_size(self):
        for size in ((60, 18), (80, 24)):
            with self.subTest(size=size):
                feed = ManualFeed()
                session = {"source_id": "source-kiro", "session_id": "session-kiro",
                           "service": "kiro", "runtime": "Kiro", "task": "Review permissions",
                           "project": "project", "permissions": PERMISSIONS}
                feed.api.sessions = [session]
                feed.refresh()
                feed.snapshot.local_status["kiro_folders"] = list(FOLDERS)
                app = _cockpit_app(feed)
                async with app.run_test(size=size) as pilot:
                    await pilot.pause()
                    app.refresh_timer.stop()
                    app.set_agent_filter("all")
                    app.select_agent("source-kiro")
                    await pilot.pause()
                    title, fields, _sections = app.inspector_payloads["agent-detail"]
                    self.assertIn(("Permissions", permission_summary(KIRO, FOLDERS)[0]), fields)
                    app.action_expand()
                    await pilot.pause()
                    app.set_session_mode("details")
                    await pilot.pause()
                    self.assertIn("Permissions",
                                  [label for label, _ in app.inspector_payloads["agent-detail"][2]])
                    self.assertIn("Trusted folders (1)",
                                  app.query_one("#agent-detail").plain_text)
                    feed.api.sessions = []
                    feed.refresh()
                    app.refresh_data()
                    await pilot.pause()
                    self.assertNotIn("Trusted folders (1)",
                                     app.query_one("#agent-detail").plain_text)

    async def test_synthetic_provider_uses_generic_inspector_path(self):
        from observatory.terminal_ui import ObservatoryApp

        provider = SessionInspectorProvider(
            "fixture-adapter", lambda item: item.get("runtime") == "Fixture",
            lambda _item, _status: SessionInspectorModel(
                (("Adapter", "ready"), (" credits ", "shadow credits"),
                 (" agent ", "shadow identity")),
                (("Evidence", "fixture details"),
                 (" evidence coverage ", "shadow coverage"))))
        feed = ManualFeed()
        feed.api.sessions = [{"source_id": "source-fixture", "session_id": "session-fixture",
                              "service": "fixture", "runtime": "Fixture",
                              "task": "Inspect fixture", "project": "project",
                              "credits": 1.5, "credits_reported": True}]
        feed.refresh()
        app = ObservatoryApp(feed, persist_preferences=False,
                             session_inspector_providers=(provider,))
        credit_patch = patch('observatory.terminal_ui.credit_summary',
                             return_value='This run 1.5\nFixture-reported credits')
        credit_patch.start()
        self.addCleanup(credit_patch.stop)
        async with app.run_test(size=(60, 18)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            app.set_agent_filter("all")
            app.select_agent("source-fixture")
            await pilot.pause()
            fields = app.inspector_payloads["agent-detail"][1]
            self.assertIn(("Adapter", "ready"), fields)
            self.assertEqual(sum(label.strip().casefold() == "agent"
                                 for label, _ in fields), 1)
            self.assertEqual(sum(label.strip().casefold() == "credits"
                                 for label, _ in fields), 1)
            self.assertNotIn("shadow identity", repr(fields))
            self.assertNotIn("shadow credits", repr(fields))
            app.action_expand()
            await pilot.pause()
            app.set_session_mode("details")
            await pilot.pause()
            sections = app.inspector_payloads["agent-detail"][2]
            self.assertIn(("Evidence", "fixture details"), sections)
            self.assertEqual(sum(label.strip().casefold() == "evidence coverage"
                                 for label, _ in sections), 1)
            self.assertNotIn("shadow coverage", repr(sections))


if __name__ == "__main__":
    unittest.main()
