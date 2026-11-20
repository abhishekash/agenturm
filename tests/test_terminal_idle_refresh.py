import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock

from observatory.terminal_ui import ObservatoryApp


class IdleRefreshTest(unittest.TestCase):
    def test_unchanged_live_snapshot_skips_idle_redraw_but_preserves_updates(self):
        snapshot = SimpleNamespace(updated=1, error='')
        feed = SimpleNamespace(
            snapshot=snapshot,
            connection=SimpleNamespace(phase='live'),
            retry_requested=False,
        )
        screen=SimpleNamespace(selections=[])
        app = SimpleNamespace(
            feed=feed,
            screen_stack=[screen],
            last_update=1,
            _last_tick_snapshot=snapshot,
            _last_tick_at=time.monotonic(),
            refresh_data=Mock(),
        )
        ObservatoryApp.refresh_tick(app)
        app.refresh_data.assert_not_called()

        feed.snapshot = SimpleNamespace(updated=2, error='')
        ObservatoryApp.refresh_tick(app)
        app.refresh_data.assert_called_once_with()

        app.refresh_data.reset_mock()
        app._last_tick_at -= 3
        ObservatoryApp.refresh_tick(app)
        app.refresh_data.assert_called_once_with()

    def test_reconnecting_and_pending_interaction_keep_refreshing(self):
        snapshot = SimpleNamespace(updated=1, error='')
        feed = SimpleNamespace(
            snapshot=snapshot,
            connection=SimpleNamespace(phase='reconnecting'),
            retry_requested=False,
        )
        screen=SimpleNamespace(selections=[])
        app = SimpleNamespace(
            feed=feed,
            screen_stack=[screen],
            last_update=1,
            _last_tick_snapshot=snapshot,
            _last_tick_at=time.monotonic(),
            refresh_data=Mock(),
        )
        ObservatoryApp.refresh_tick(app)
        app.refresh_data.assert_called_once_with()

        app.refresh_data.reset_mock()
        feed.connection.phase = 'live'
        feed.retry_requested = True
        ObservatoryApp.refresh_tick(app)
        app.refresh_data.assert_called_once_with()

        app.refresh_data.reset_mock()
        feed.retry_requested = False
        screen.selections = [object()]
        ObservatoryApp.refresh_tick(app)
        app.refresh_data.assert_called_once_with()


class ShutdownRefreshTest(unittest.IsolatedAsyncioTestCase):
    async def test_pending_refresh_tick_after_screen_teardown_is_ignored(self):
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed

        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
        self.assertEqual(app.screen_stack,[])
        refresh=Mock()
        app.refresh_data=refresh
        # A timer message queued before teardown can still invoke its callback.
        ObservatoryApp.refresh_tick(app)
        refresh.assert_not_called()
