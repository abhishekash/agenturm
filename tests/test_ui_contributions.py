"""A trusted capability adds a pane without changing central view dispatch."""
import unittest

from observatory.ui_contributions import (PaneContribution, PaneModel, PaneRegistry,
                                          PaneRow, PaneScope, validate_model)


def fixture_pane(query):
    return PaneContribution("prompt-efficacy", "Prompt signals",
                            (("Signal", 24, 2), ("Observed", 12, 0)), query)


class RegistryTests(unittest.TestCase):
    def test_registry_rejects_collisions_and_unbounded_models(self):
        pane = fixture_pane(lambda _scope: PaneModel("ready", "1 observed", (
            PaneRow("clarification", ("Clarifications", "1")),)))
        self.assertEqual(PaneRegistry((pane,)).get("prompt-efficacy"), pane)
        for bad in (PaneContribution("metrics", "Override", pane.columns, pane.query),
                    PaneContribution("A BAD ID", "Invalid", pane.columns, pane.query)):
            with self.assertRaises(ValueError):
                PaneRegistry((bad,))
        with self.assertRaises(ValueError):
            PaneRegistry((pane, pane))
        with self.assertRaises(ValueError):
            validate_model(pane, PaneModel("ready", "unsafe", (
                PaneRow("x", ("prompt text\nleak", "1")),)))
        with self.assertRaises(ValueError):
            validate_model(pane, PaneModel("ready", "many", tuple(
                PaneRow(str(index), ("signal", "1")) for index in range(501))))


class ShellTests(unittest.IsolatedAsyncioTestCase):
    async def test_contributed_pane_navigation_scope_failure_and_small_floor(self):
        from textual.widgets import Select, TabbedContent
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed

        calls = []
        def query(scope: PaneScope):
            calls.append(scope)
            return PaneModel("ready", "1 of 3 recorded turns · limited coverage", (
                PaneRow("clarification", ("Clarifications", "1"), "1 observed signal"),))

        app = ObservatoryApp(ManualFeed(), persist_preferences=False,
                             ui_panes=(fixture_pane(query),))
        async with app.run_test(size=(60, 18)) as pilot:
            await pilot.pause()
            picker = app.query_one("#more-views", Select)
            self.assertIn("contrib-prompt-efficacy", [value for _, value in picker._options])
            app.action_page("contrib-prompt-efficacy")
            await pilot.pause()
            self.assertEqual(app.query_one("#pages", TabbedContent).active,
                             "contrib-prompt-efficacy")
            self.assertTrue(calls)
            table = app.query_one("#contrib-prompt-efficacy-table")
            self.assertEqual(table.row_count, 1)
            self.assertEqual(str(table.get_cell_at((0, 0))), "Clarifications")
            self.assertIn("limited coverage", str(app.query_one(
                "#contrib-prompt-efficacy-status").render()))

    async def test_failed_provider_keeps_existing_views_usable(self):
        from textual.widgets import TabbedContent
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed

        def fail(_scope):
            raise RuntimeError("private provider detail")

        app = ObservatoryApp(ManualFeed(), persist_preferences=False,
                             ui_panes=(fixture_pane(fail),))
        async with app.run_test(size=(100, 30)) as pilot:
            app.action_page("contrib-prompt-efficacy")
            await pilot.pause()
            self.assertEqual(app.query_one("#contrib-prompt-efficacy-table").row_count, 0)
            self.assertNotIn("private provider detail", str(app.query_one(
                "#contrib-prompt-efficacy-status").render()))
            app.action_page("agents")
            await pilot.pause()
            self.assertEqual(app.query_one("#pages", TabbedContent).active, "agents")


if __name__ == "__main__":
    unittest.main()
