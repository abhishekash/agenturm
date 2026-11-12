"""Agent error filtering and live branch priority keep selection meaningful."""
import time
import unittest
from textual.widgets import Button, DataTable, Select
from observatory.terminal_ui import ObservatoryApp
from tests.test_terminal_search import ManualFeed
from tests.test_agent_hierarchy_ui import HierarchyFeed, agent_node


class AgentErrorsUXTests(unittest.IsolatedAsyncioTestCase):
    async def test_error_shortcut_finds_old_failures_and_returns_to_previous_filter(self):
        feed=ManualFeed()
        now=time.time_ns()
        base=feed.snapshot.local_status['sessions'][0]
        feed.snapshot.local_status['sessions']=[
            {**base,'source_id':sid,'session_id':sid,'run_state':state,
             'waiting_for':'','last_activity':{},'last_record_ns':stamp}
            for sid,state,stamp in [('working','running',now),('done','completed',now),
                                    ('failed','error',now-100000000000)]
        ]
        feed.snapshot.sessions=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('active')
            await pilot.pause()
            button=app.query_one('#agent-errors',Button)
            self.assertIn('Errors 1',str(button.label))
            self.assertTrue(app.screen.region.contains_region(button.region))
            await pilot.click('#agent-errors')
            await pilot.pause()
            self.assertEqual(app.agent_filter,'errors')
            self.assertEqual(app.visible_agent_rows,['failed'])
            self.assertTrue(button.has_class('-on'))
            self.assertEqual(app.query_one('#agent-filter',Select).value,'errors')
            app.last_update=-1;app.refresh_data()
            await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['failed'])
            await pilot.click('#agent-errors')
            await pilot.pause()
            self.assertEqual(app.agent_filter,'active')
            self.assertEqual(app.visible_agent_rows,['working'])
            await pilot.resize_terminal(80,30)
            await pilot.pause()
            self.assertTrue(app.screen.region.contains_region(button.region))
            await pilot.resize_terminal(120,40)
            await pilot.pause()
            # Error filtering has an explicit empty state after the failure is resolved.
            feed.snapshot.local_status['sessions'][2]['run_state']='completed'
            app.set_agent_filter('errors')
            await pilot.pause()
            self.assertEqual(app.visible_agent_rows,[])
            self.assertIn('No session errors',app.query_one('#agent-detail').plain_text)
            # A completed session with older recorded failures is still useful
            # in Errors, even if its latest activity succeeded.
            feed.snapshot.sessions.append({'session_id':'done',
                'agent_id':app.source_map['done']['agent_id'],'errors':2})
            app.last_update=-1;app.refresh_data()
            await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['done'])
            app.action_all()
            await pilot.pause()
            self.assertNotEqual(app.agent_filter,'errors')
            app.set_agent_filter('errors')
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            self.assertEqual(app.selected_source,'done')

    async def test_tree_prioritizes_live_descendants_and_updates_sibling_order(self):
        feed=HierarchyFeed()
        now=time.time_ns()
        feed.snapshot.local_status['sessions']=[
            agent_node('done',None,'Finished','completed',stamp=now+20),
            agent_node('parent',None,'Parent finished','completed',stamp=now),
            agent_node('a','parent','Worker A','running',active=True,stamp=now+2),
            agent_node('b','parent','Worker B','running',active=True,stamp=now+1),
        ]
        feed.snapshot.sessions=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['s-parent','s-a','s-b','s-done'])
            app.select_agent('s-a')
            feed.snapshot.local_status['sessions'][3]['last_record_ns']=now+30
            app.last_update=-1;app.refresh_data()
            await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['s-parent','s-b','s-a','s-done'])
            self.assertEqual(app.selected_source,'s-a')
            self.assertEqual(app.query_one('#agent-table',DataTable).cursor_row,2)
            self.assertEqual(app.source_map['s-parent']['state'],'Completed')
