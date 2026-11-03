import unittest
from unittest.mock import Mock

from rich.text import Text

from observatory.ui_render import update_status_text


class StatusRenderingBudgetTests(unittest.TestCase):
    def test_unchanged_status_does_not_request_a_repaint(self):
        class Widget:
            update = Mock()

        widget = Widget()
        self.assertTrue(update_status_text(widget, Text('Connected', style='green')))
        self.assertFalse(update_status_text(widget, Text('Connected', style='green')))
        widget.update.assert_called_once()
        self.assertTrue(update_status_text(widget, Text('Connected', style='blue')))
        self.assertTrue(update_status_text(widget, Text('Disconnected', style='blue')))
        self.assertEqual(widget.update.call_count, 3)


class NavigationBudgetTests(unittest.IsolatedAsyncioTestCase):
    async def test_programmatic_navigation_does_not_generate_more_navigation(self):
        from textual.widgets import Select, TabbedContent
        from unittest.mock import patch
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(110, 40)) as pilot:
            await pilot.pause()
            with patch.object(app, 'action_page', wraps=app.action_page) as navigate:
                for page in ('host', 'metrics', 'findings', 'agents'):
                    app.action_page(page)
                    await pilot.pause()
                self.assertEqual(navigate.call_count, 4)
                self.assertEqual(app.query_one('#pages', TabbedContent).active, 'agents')
                picker = app.query_one('#more-views', Select)
                picker.value = 'host'
                await pilot.pause()
                self.assertEqual(app.query_one('#pages', TabbedContent).active, 'host')
                self.assertEqual(navigate.call_count, 5)
