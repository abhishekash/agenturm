"""Agent error filtering and live branch priority keep selection meaningful."""
import time
import unittest
from textual.widgets import Button, DataTable, Select
from observatory.terminal_ui import ObservatoryApp
from tests.test_terminal_search import ManualFeed
from tests.test_agent_hierarchy_ui import HierarchyFeed, agent_node
from observatory.error_reminders import error_reference


class AgentErrorsUXTests(unittest.IsolatedAsyncioTestCase):
    async def test_error_shortcut_only_shows_recent_unseen_evidence_and_returns_to_previous_filter(self):
        feed=ManualFeed()
        feed.request_refresh=lambda:None
        now=time.time_ns()
        base=feed.snapshot.local_status['sessions'][0]
        feed.snapshot.local_status['sessions']=[
            {**base,'source_id':sid,'session_id':sid,'run_state':state,
             'trace_id':'run-'+sid,'waiting_for':'','last_activity':{},'last_record_ns':stamp}
            for sid,state,stamp in [('working','running',now),('done','completed',now-200000000000),
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
            self.assertIn('No unseen recent errors',app.query_one('#agent-detail').plain_text)
            # Lifetime failures in an older completed session are not reminders,
            # even when that session is selected explicitly.
            feed.snapshot.sessions.append({'session_id':'done',
                'agent_id':app.source_map['done']['agent_id'],'errors':2})
            app.last_update=-1;app.refresh_data()
            await pilot.pause()
            self.assertEqual(app.visible_agent_rows,[])
            app.set_session_scope('done');await pilot.pause()
            self.assertEqual(app.visible_agent_rows,[])
            app.action_all()
            await pilot.pause()
            self.assertNotEqual(app.agent_filter,'errors')
            app.set_agent_filter('errors')
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            self.assertIn('done',app.visible_agent_rows)

    async def test_opening_failure_consumes_reminder_but_keeps_history_and_new_failure(self):
        feed=ManualFeed();now=time.time_ns()
        feed.request_refresh=lambda:None
        worker='ag1_'+'a'*32
        base=feed.snapshot.local_status['sessions'][0]
        feed.snapshot.local_status['sessions']=[{**base,'source_id':'one','session_id':'one',
            'agent_id':worker,'run_state':'completed','trace_id':'run-one','last_record_ns':now}]
        error=dict(id='run-one:failure',source_id='failure',trace_id='run-one',service=base['service'],
            session_id='one',agent_id=worker,time_unix_nano=now-1,domain='shell',action='run',
            subject='pytest',result='error',detail='AssertionError: recorded test failure')
        feed.snapshot.sessions=[dict(session_id='one',agent_id=worker,services=[base['service']],
            errors=50,recent_errors=[error_reference(error)])]
        feed.snapshot.activity=[error];feed.snapshot.activity_results=[error]
        feed.api.activity=[error]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause()
            self.assertIn('Errors 1',str(app.query_one('#agent-errors',Button).label))
            app.set_agent_filter('errors');await pilot.pause()
            # Merely selecting the row and showing its preview is not acknowledgement.
            self.assertEqual(app.visible_agent_rows,['one'])
            self.assertEqual(app.workflow_controller.error_seen,set())
            app.query_one('#agent-table').focus();await pilot.press('s');await pilot.pause()
            self.assertEqual(app.selected_activity,error['id'])
            self.assertIn('recorded test failure',app.query_one('#activity-detail').plain_text)
            self.assertTrue(app.workflow_controller.error_seen)
            self.assertIn(error,feed.snapshot.activity)
            app.action_page('agents');app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertEqual(app.visible_agent_rows,[])
            self.assertIn('Errors ● 0',str(app.query_one('#agent-errors',Button).label))
            # Repeated polls cannot toggle acknowledgement off. A distinct event can appear.
            app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertEqual(app.visible_agent_rows,[])
            feed.snapshot.sessions[0]['recent_errors']=[{**error,'id':'run-one:next','source_id':'next'}]
            app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['one'])
            await pilot.resize_terminal(60,18);await pilot.pause()
            self.assertTrue(app.screen.region.contains_region(app.query_one('#agent-errors',Button).region))

    async def test_metadata_only_failure_waits_for_retained_details_even_after_a_failed_fetch(self):
        feed=ManualFeed();feed.request_refresh=lambda:None
        now=time.time_ns();base=feed.snapshot.local_status['sessions'][0];worker='ag1_'+'b'*32
        feed.snapshot.local_status['sessions']=[{**base,'source_id':'one','session_id':'one',
            'agent_id':worker,'run_state':'completed','trace_id':'run-one','last_record_ns':now}]
        error=dict(id='run-one:failure',source_id='failure',trace_id='run-one',service=base['service'],
            session_id='one',agent_id=worker,time_unix_nano=now-1,domain='shell',action='run',
            subject='pytest',result='error',detail='Retained assertion failure')
        feed.snapshot.sessions=[dict(session_id='one',agent_id=worker,services=[base['service']],
            recent_errors=[error_reference(error)])]
        feed.snapshot.activity=[];feed.snapshot.activity_results=[];feed.api.activity=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.set_agent_filter('errors');await pilot.pause()
            app.action_focus_agent();await pilot.pause()
            self.assertEqual(app.selected_activity,error['id'])
            self.assertEqual(feed.activity_query,'failure')
            self.assertFalse(app.workflow_controller.error_seen)
            await pilot.press('enter');await pilot.pause()
            self.assertIn('Only a reference is loaded',app.query_one('#activity-detail').plain_text)
            await pilot.press('escape');await pilot.pause()
            feed.snapshot.read_errors['activity']='Receiver unavailable'
            app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertFalse(app.workflow_controller.error_seen)
            feed.snapshot.read_errors.clear()
            feed.snapshot.activity=[error];feed.snapshot.activity_results=[error]
            feed.snapshot.queries['activity']=feed.activity_query
            feed.snapshot.activity_errors=True
            feed.snapshot.view_scope=app.current_evidence_scope()
            app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertFalse(app.workflow_controller.error_seen)  # A hidden inspector isn't a viewed error.
            await pilot.press('enter');await pilot.pause()
            self.assertIn('Retained assertion failure',app.query_one('#activity-detail').plain_text)
            self.assertTrue(app.workflow_controller.error_seen)

    async def test_failed_outcome_without_calls_opens_visible_evidence_in_narrow_layout(self):
        feed=ManualFeed();feed.request_refresh=lambda:None
        base=feed.snapshot.local_status['sessions'][0]
        feed.snapshot.local_status['sessions']=[{**base,'source_id':'one','session_id':'one',
            'run_state':'error','trace_id':'failed-run','last_record_ns':time.time_ns(), 'last_activity':{}}]
        feed.snapshot.sessions=[];feed.snapshot.activity=[];feed.snapshot.activity_results=[];feed.api.activity=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.set_agent_filter('errors');await pilot.pause()
            self.assertEqual(app.visible_agent_rows,['one'])
            app.action_focus_agent();await pilot.pause()
            self.assertTrue(app.query_one('#activity').has_class('expanded'))
            self.assertFalse(app.workflow_controller.error_seen)
            feed.snapshot.activity_errors=True
            feed.snapshot.view_scope=app.current_evidence_scope()
            app.last_update=-1;app.refresh_data();await pilot.pause()
            self.assertIn('runtime reported this failed outcome',app.query_one('#activity-detail').plain_text)
            self.assertTrue(app.workflow_controller.error_seen)
            self.assertEqual(app.source_map['one']['state'],'Error')
            self.assertFalse(any(r.get('acknowledgement') for r in app.workflow_controller.receipts.values()))

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
            self.assertEqual(app.query_one('#agent-table').get_row('s-parent')[1].plain,'Ready for review')
            app.set_agent_filter('today');await pilot.pause()
            self.assertEqual(app.visible_agent_rows[0],'s-parent')
            self.assertEqual(app.source_map['s-parent']['state'],'Completed')
