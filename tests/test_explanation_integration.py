"""Actual existing views disclose evidence denominators and obey live Feed policy."""
from copy import deepcopy
from dataclasses import replace
import unittest
from unittest.mock import patch

from observatory.terminal_ui import ObservatoryApp
from observatory.ui_monitoring import UsageScreen
from tests.test_usage_render_key import usage_feed


class ExplanationIntegration(unittest.IsolatedAsyncioTestCase):
    async def mount(self,app,pilot):
        await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all');app.select_agent('source-1')
        await app.workers.wait_for_complete();await pilot.pause()

    async def test_usage_modal_live_feed_denial_and_receiver_replacement_are_sticky(self):
        for replacement in ('capture','receiver'):
            feed=usage_feed();app=ObservatoryApp(feed,persist_preferences=False)
            async with app.run_test(size=(60,18)) as pilot:
                await self.mount(app,pilot);app.action_usage();await pilot.pause();screen=app.screen
                self.assertIsInstance(screen,UsageScreen);self.assertIn('Private first task',screen.details)
                if replacement=='capture':
                    feed.snapshot=replace(feed.snapshot,local_status={'capture_content':False})
                else:feed.snapshot=replace(feed.snapshot,receiver_id='replacement')
                app.refresh_data();screen.check_policy()
                self.assertTrue(screen.invalidated);self.assertNotIn('Private first task',screen.details)
                feed.snapshot=app.data;screen.check_policy()
                self.assertTrue(screen.invalidated);self.assertNotIn('Private first task',screen.details)
                depth=len(app.screen_stack)
                with patch.object(app,'copy_to_clipboard') as copied:
                    await pilot.press('c','x');await pilot.pause()
                    copied.assert_not_called();self.assertEqual(len(app.screen_stack),depth)
                await pilot.press('ctrl+c');await pilot.pause();self.assertIsNot(app.screen,screen)

    async def test_configured_estimate_cache_options_and_unknown_occupancy(self):
        feed=usage_feed();app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await self.mount(app,pilot)
            usage=app.data.sessions[0]['latest_run'];selected=app.source_map['source-1']
            owner=dict(producer=selected['service'],agent_id=selected['agent_id'],session_id=selected['session_id'])
            usage.update(review_owner=owner,usage_complete=True,models_complete=True)
            app.cost_estimate_rates={'recorded-model':dict(unit='USD_per_million_tokens',input_accounting='flat_input',input=1,output=2)}
            app.refresh_task_usage();before=app._task_usage_inputs
            details=app.query_one('#usage-summary').tooltip
            self.assertIn('Configured estimate $',details);self.assertIn('cumulative usage is not occupancy',details)
            app.cost_estimate_rates['recorded-model']['input']=3;app.refresh_task_usage()
            self.assertNotEqual(before,app._task_usage_inputs)
            usage['models_complete']=False;app.refresh_task_usage()
            self.assertIn('Configured estimate unavailable',app.query_one('#usage-summary').tooltip)

    async def test_mcp_inspector_uses_loaded_tool_outcome_and_duration_denominators(self):
        feed=usage_feed();base=feed.snapshot.mcp[0]
        feed.snapshot=replace(feed.snapshot,mcp_results=[
            {**base,'id':'one','status':'OK','duration_ms':10},
            {**base,'id':'two','status':'ERROR','duration_ms':30},
            {**base,'id':'three','status':'unknown','duration_ms':None,'output':None}],
            receiver_id='fixture-receiver')
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await self.mount(app,pilot);app.action_page('mcp');await pilot.pause()
            # The manual Feed refreshes on page changes; publish the exact
            # generated page after that refresh, as a real Feed would do.
            app.data=replace(app.data,receiver_id='fixture-receiver',mcp_results=[
                {**base,'id':'one','status':'OK','duration_ms':10},
                {**base,'id':'two','status':'ERROR','duration_ms':30},
                {**base,'id':'three','status':'unknown','duration_ms':None,'output':None}])
            app.render_mcp();await pilot.pause()
            text=app.query_one('#mcp-detail').plain_text
            self.assertIn('1 failed / 2 known outcomes',text)
            self.assertIn('1 unknown',text);self.assertIn('2 known durations',text)
            self.assertIn('Small sample',text);self.assertIn('Loaded records only',text)


if __name__=='__main__':unittest.main()
