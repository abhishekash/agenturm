"""Bounded pure keys and actual cockpit usage-formatting invalidation."""
from copy import deepcopy
from dataclasses import replace
import unittest
from unittest.mock import patch

from textual.widgets import Static
from observatory.terminal_ui import ObservatoryApp
from observatory.usage_render_key import usage_render_key,MAX_NODES,MAX_TEXT_BYTES,MAX_DEPTH
from tests.test_terminal_search import ManualFeed


class UsageRenderKeyTests(unittest.TestCase):
    def test_detaches_nested_inputs_and_ignores_object_key_order(self):
        source={'models':['recorded-model'],'scope_owner':{'agent_id':'owner'},'input_tokens':10}
        before=usage_render_key(source)
        self.assertEqual(before,usage_render_key(deepcopy(source)))
        self.assertEqual(before,usage_render_key(dict(reversed(list(source.items())))))
        source['models'].append('second-model')
        self.assertNotEqual(before,usage_render_key(source))
        source['scope_owner']['agent_id']='another-owner'
        self.assertNotEqual(before,usage_render_key(source))

    def test_scalar_types_and_signed_zero_remain_distinct(self):
        for left,right in ((True,1),(False,0),(1,1.0),(0.0,-0.0),([1],(1,))):
            self.assertNotEqual(usage_render_key(left),usage_render_key(right))

    def test_unsupported_cycles_and_budgets_bypass(self):
        cycle=[];cycle.append(cycle)
        deep=0
        for _ in range(MAX_DEPTH+1):deep=[deep]
        for value in (object(),{1:'non-string key'},set(),cycle,deep,[0]*MAX_NODES,
                      'x'*(MAX_TEXT_BYTES+1),'界'*(MAX_TEXT_BYTES//2),float('inf'),float('nan'),1<<129):
            with self.subTest(kind=type(value)):
                self.assertIsNone(usage_render_key(value))
        # One bad input does not poison a later admitted value.
        self.assertIsNotNone(usage_render_key({'input_tokens':1}))


def usage_feed():
    feed=ManualFeed()
    feed.snapshot=replace(feed.snapshot,receiver_id='fixture-receiver',sessions=[
        dict(session_id='session-1',services=['codex'],latest_run=dict(trace_id='run-1',task='Private first task',
            input_tokens=100,output_tokens=20,known_cost_usd=.1,cost_complete=True,agent_count=1,agent_count_complete=True,
            models=['recorded-model'],scope_owner={'producer':'codex','session_id':'session-1','agent_id':''})),
        dict(session_id='session-2',services=['claude'],latest_run=dict(trace_id='run-2',task='Second task',
            input_tokens=200,output_tokens=40,agent_count=1,agent_count_complete=True))])
    return feed


class UsageRenderJourney(unittest.IsolatedAsyncioTestCase):
    async def mounted(self,app,pilot):
        await pilot.pause()
        app.refresh_timer.stop()
        app.set_agent_filter('all');app.select_agent('source-1')
        await app.workers.wait_for_complete();await pilot.pause()

    async def test_unchanged_calls_skip_formatter_and_nested_mutation_invalidates(self):
        app=ObservatoryApp(usage_feed(),persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await self.mounted(app,pilot)
            from observatory.terminal_ui import task_usage_summary
            with patch('observatory.terminal_ui.task_usage_summary',wraps=task_usage_summary) as formatter:
                app._task_usage_inputs=None
                for _ in range(25):app.refresh_task_usage()
                self.assertEqual(formatter.call_count,1)
                usage=app.data.sessions[0]['latest_run']
                usage['input_tokens']=350
                app.refresh_task_usage();self.assertEqual(formatter.call_count,2)
                self.assertIn('Input: 350',app.query_one('#usage-summary',Static).tooltip)
                usage['models'].append('second-model')
                app.refresh_task_usage();self.assertEqual(formatter.call_count,3)
                usage['scope_owner']['agent_id']='changed-owner'
                app.refresh_task_usage();self.assertEqual(formatter.call_count,4)
                usage['task']='Updated task title'
                app.refresh_task_usage();self.assertEqual(formatter.call_count,5)
                self.assertIn('Updated task title',app.query_one('#usage-summary',Static).tooltip)
                usage['unsupported']=object()
                app.refresh_task_usage();app.refresh_task_usage()
                self.assertEqual(formatter.call_count,7)
                self.assertIsNone(app._task_usage_inputs)

    async def test_selection_width_theme_and_namespace_invalidate(self):
        app=ObservatoryApp(usage_feed(),persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await self.mounted(app,pilot)
            from observatory.terminal_ui import task_usage_summary
            with patch('observatory.terminal_ui.task_usage_summary',wraps=task_usage_summary) as formatter:
                app._task_usage_inputs=None;app.refresh_task_usage()
                old_key=app._task_usage_inputs
                app.select_agent('source-2');app.refresh_task_usage()
                self.assertNotEqual(old_key,app._task_usage_inputs)
                self.assertIn('Second task',app.query_one('#usage-summary',Static).tooltip)
                count=formatter.call_count
                app.data=replace(app.data,receiver_id='replacement-receiver');app.feed.snapshot=app.data
                app.refresh_task_usage();self.assertEqual(formatter.call_count,count+1)
                count=formatter.call_count
                app.theme='Bright';app.refresh_task_usage()
                self.assertGreater(formatter.call_count,count)
                old_key=app._task_usage_inputs
                await pilot.resize_terminal(80,24);await pilot.pause();app.refresh_task_usage()
                self.assertNotEqual(old_key,app._task_usage_inputs)
                old_key=app._task_usage_inputs
                await pilot.resize_terminal(60,18);await pilot.pause();app.refresh_task_usage()
                self.assertNotEqual(old_key,app._task_usage_inputs)
                old_key=app._task_usage_inputs
                app.cost_estimate_rates={'recorded-model':{'unit':'USD_per_million_tokens','input':1}}
                app.refresh_task_usage();self.assertNotEqual(old_key,app._task_usage_inputs)

    async def test_in_place_capture_revocation_clears_retained_key_and_private_title(self):
        app=ObservatoryApp(usage_feed(),persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await self.mounted(app,pilot)
            app.refresh_task_usage()
            self.assertIn('Private first task',app.query_one('#usage-summary',Static).tooltip)
            selected=app.data.local_status['sessions'][0]
            selected.update(capture_content=False,capture_revision=2)
            app.refresh_task_usage()
            tooltip=app.query_one('#usage-summary',Static).tooltip
            self.assertNotIn('Private first task',tooltip)
            self.assertIn('metadata-only',tooltip)
            self.assertIsNone(app._task_usage_inputs)
            app.refresh_task_usage();self.assertIsNone(app._task_usage_inputs)


if __name__=='__main__':unittest.main()
