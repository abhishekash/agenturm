"""Real indexed Activity feeds a scoped, metadata-only contributed pane."""
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from examples.prompt_signals_capability.provider import pane
from observatory.agent_hierarchy import canonical_agent_id
from observatory.capability_facts import prompt_signal_facts
from observatory.core import Store, normalize_native
from observatory.ui_contributions import PaneScope, validate_model


class PromptSignalCapabilityTests(unittest.TestCase):
    def setUp(self):
        temp = TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.store = Store(Path(temp.name) / "signals.db")
        self.addCleanup(self.store.close)
        self.owner = canonical_agent_id("SDK", "svc", "owner")
        self.sibling = canonical_agent_id("SDK", "svc", "sibling")

    def install(self):
        def span(letter, actor, session, events):
            attrs = {"agent.id": actor, "agent.runtime": "SDK",
                     "agent.namespace": "svc", "session.id": session}
            return normalize_native(dict(trace_id=letter * 32, span_id=letter * 16,
                name="agent.run", start_unix_nano=1_000_000_000,
                end_unix_nano=4_000_000_000, status="OK", attributes=attrs,
                events=[dict(name="agent.activity", time_unix_nano=stamp,
                    attributes={"agent.activity.domain": domain,
                                "agent.activity.action": action,
                                "agent.activity.subject": subject,
                                "agent.activity.detail": "PRIVATE_DETAIL"})
                    for stamp, domain, action, subject in events]), "svc")
        self.store.ingest([
            span("a", "owner", "session-1", [
                (1_000_000_000, "message", "user", "PRIVATE_PROMPT"),
                (1_250_000_000, "shell", "execute", "PRIVATE_COMMAND")]),
            span("b", "sibling", "session-1", [
                (1_100_000_000, "message", "user", "SIBLING_PROMPT")]),
            span("c", "owner", "session-2", [
                (1_200_000_000, "shell", "execute", "OTHER_SESSION_COMMAND")]),
        ])

    def test_real_store_scope_and_pane_never_expose_content(self):
        self.install()
        scope = PaneScope(agent_id=self.owner, service="svc", session_id="session-1")
        facts = prompt_signal_facts(self.store, scope, capture_allowed=True)
        self.assertEqual((facts.state, facts.user_messages, facts.shell_actions,
                          facts.first_shell_after_user_ms), ("ready", 1, 1, 250))
        contribution = pane(lambda selected: prompt_signal_facts(
            self.store, selected, capture_allowed=True))
        model = validate_model(contribution, contribution.query(scope))
        self.assertEqual(model.state, "ready")
        self.assertIn("2 scoped events", model.summary)
        self.assertNotIn("PRIVATE", repr(model))
        self.assertNotIn("SIBLING", repr(model))
        self.assertNotIn("OTHER_SESSION", repr(model))

    def test_capture_denial_or_inexact_scope_returns_no_data(self):
        self.install()
        scope = PaneScope(agent_id=self.owner, service="svc", session_id="session-1")
        with patch.object(self.store, "activity_page", side_effect=AssertionError("must not query")):
            denied = prompt_signal_facts(self.store, scope, capture_allowed=False)
        self.assertEqual(denied.state, "unavailable")
        for inexact in (PaneScope(service="svc", session_id="session-1"),
                        PaneScope(agent_id=self.owner, service="svc"),
                        PaneScope(agent_id=self.owner, session_id="session-1")):
            self.assertEqual(prompt_signal_facts(self.store, inexact, capture_allowed=True).state,
                             "unavailable")

    def test_bounded_page_reports_partial_coverage(self):
        self.install()
        scope = PaneScope(agent_id=self.owner, service="svc", session_id="session-1")
        from observatory import capability_facts
        with patch.object(capability_facts, "MAX_SIGNAL_EVENTS", 1):
            facts = prompt_signal_facts(self.store, scope, capture_allowed=True)
        self.assertEqual(facts.state, "degraded")
        self.assertFalse(facts.window_complete)
        self.assertIn("partial", pane(lambda _: facts).query(scope).summary)


class PromptSignalShellTests(unittest.IsolatedAsyncioTestCase):
    async def test_contributed_pane_uses_generic_small_screen_shell(self):
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed
        from textual.widgets import TabbedContent
        from observatory.capability_facts import PromptSignalFacts

        contribution = pane(lambda _scope: PromptSignalFacts(
            "ready", observed_events=2, user_messages=1, shell_actions=1,
            first_shell_after_user_ms=250, window_complete=True))
        app = ObservatoryApp(ManualFeed(), persist_preferences=False,
                             ui_panes=(contribution,))
        async with app.run_test(size=(60, 18)) as pilot:
            app.action_page("contrib-prompt-signals")
            await pilot.pause()
            self.assertEqual(app.query_one("#pages", TabbedContent).active,
                             "contrib-prompt-signals")
            table = app.query_one("#contrib-prompt-signals-table")
            self.assertEqual(table.row_count, 3)
            self.assertEqual(str(table.get_cell_at((0, 0))), "Recorded user messages")


if __name__ == "__main__":
    unittest.main()
