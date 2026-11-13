import time
import unittest
from dataclasses import replace
from unittest.mock import patch
from observatory.ui_workflow import AttentionLedger, workflow_report, workflow_reasons, resume_command, desktop_notify, terminal_handoff, WorkflowScreen
from observatory.ui_preferences import safe_preferences
from observatory.terminal_ui import ObservatoryApp, sources
from tests.test_terminal_search import ManualFeed

class WorkflowEvidenceTests(unittest.TestCase):
    def test_alert_transitions_baseline_resolution_and_tool_wait(self):
        feed=ManualFeed();ledger=AttentionLedger()
        item={'id':'a','state':'Waiting','waiting_for':'Approval','session_id':'s'}
        self.assertEqual(ledger.update([item],feed.snapshot,{},100),[])
        self.assertEqual(ledger.update([item],feed.snapshot,{},200),[])
        self.assertEqual(ledger.rows[0][2],100)
        ledger.update([],feed.snapshot,{},300)
        self.assertEqual(ledger.update([item],feed.snapshot,{},400),[])
        self.assertEqual(ledger.rows[0][2],100)
        item['_workflow_reviewed']=True
        self.assertEqual(workflow_reasons(item,{},feed.snapshot,{}),['Approval requested'])
        item['_workflow_reviewed']=False
        item['waiting_for']='Tool response';self.assertEqual(workflow_reasons(item,{},feed.snapshot,{}),[])

    def test_thresholds_use_current_run_and_report_missing_cost(self):
        feed=ManualFeed();item={'id':'a','session_id':'s','state':'Working'}
        feed.snapshot=replace(feed.snapshot,sessions=[{'session_id':'s','input_tokens':999999,'known_cost_usd':99,'latest_run':{'trace_id':'r','input_tokens':20,'output_tokens':5,'agent_count':1,'agent_count_complete':True}}])
        ledger=AttentionLedger();ledger.update([item],feed.snapshot,{'tokens':100,'cost':1})
        self.assertEqual(ledger.rows,[])
        ledger.update([item],feed.snapshot,{'tokens':25,'cost':1})
        self.assertEqual(ledger.rows[0][1],['Task token threshold reached'])
        self.assertNotIn('workflow_limits',safe_preferences({'workflow_limits':'bad'}))
        self.assertEqual(safe_preferences({'workflow_limits':{'tokens':float('nan'),'cost':-1,'minutes':5}})['workflow_limits'],{'minutes':5})

    def test_review_does_not_claim_checks_verify_later_edits_or_other_agents(self):
        feed=ManualFeed();item={'id':'a','service':'codex','agent_id':'a','session_id':'s','state':'Completed','working_directory':'/repo'}
        events=[{'agent_id':'b','session_id':'s','domain':'shell','subject':'pytest secret','result':'success','time_unix_nano':10},
                {'agent_id':'a','domain':'shell','subject':'pytest','result':'error','time_unix_nano':20},
                {'agent_id':'a','domain':'file','action':'edit','subject':'edit a.py','time_unix_nano':30}]
        snapshot=replace(feed.snapshot,activity=events)
        for event in events:event.update(service='codex',session_id='s')
        report=workflow_report(item,[item,{'id':'b','active':True,'working_directory':'/repo','task':'other task'}],snapshot)
        self.assertIn('before latest recorded file operation',report)
        self.assertIn('other task',report)
        self.assertNotIn('pytest secret',report)
        self.assertIn('occupancy is not inferred',report)
        self.assertIn('loaded activity only',report)

    def test_resume_is_quoted_and_never_runs_a_shell(self):
        command=resume_command({'runtime':'Codex','session_id':'s; echo bad','working_directory':'/tmp/a b'})
        self.assertEqual(command,"cd -- '/tmp/a b' && codex resume 's; echo bad'")
        self.assertEqual(resume_command({'runtime':'unknown','session_id':'a'}),'')
        with patch('observatory.ui_workflow.shutil.which',return_value=None):
            self.assertIn('unavailable',terminal_handoff({}))
            self.assertIn('unavailable',desktop_notify('a','b'))

    def test_tmux_selects_only_a_verified_ancestor_and_rejects_pid_reuse(self):
        from types import SimpleNamespace
        def stat(pid,parent,started):
            fields=['S',str(parent)]+['0']*17+[str(started)]
            return str(pid)+' (agent) '+' '.join(fields)
        records={'/proc/20/stat':stat(20,10,100),'/proc/10/stat':stat(10,1,50)}
        item={'process':{'identity':[(20,'100')]}}
        def run(command,**kwargs):
            return SimpleNamespace(stdout='%3 10\n%4 99\n')
        with patch('observatory.ui_workflow.shutil.which',return_value='/bin/tmux'), \
             patch('observatory.ui_workflow.Path.read_text',lambda p:records[str(p)]), \
             patch('observatory.ui_workflow.subprocess.run',side_effect=run) as call:
            self.assertIn('Selected tmux pane %3',terminal_handoff(item))
            self.assertEqual(call.call_args.args[0],['tmux','select-pane','-t','%3'])
            call.reset_mock()
            self.assertIn('No unique',terminal_handoff({'process':{'identity':[(20,'101')]}}))
            self.assertEqual(call.call_count,1)

    def test_native_alert_is_bounded_and_elapsed_alert_needs_an_active_run(self):
        with patch('observatory.ui_workflow.sys.platform','linux'), \
             patch('observatory.ui_workflow.shutil.which',return_value='/bin/notify-send'), \
             patch('observatory.ui_workflow.subprocess.run') as call:
            self.assertIn('delivered',desktop_notify('Agenturm','Needs attention'))
            self.assertEqual(call.call_args.kwargs['timeout'],3)
            self.assertNotIn('shell',call.call_args.kwargs)
        snapshot=replace(ManualFeed().snapshot,traces=[{'trace_id':'r','start_unix_nano':1_000_000_000}])
        usage={'trace_id':'r','start_unix_nano':1_000_000_000,'input_tokens':999,'input_tokens_reported':False}
        self.assertEqual(workflow_reasons({'active':False},usage,snapshot,{'minutes':1,'tokens':1},now=100),[])
        self.assertEqual(workflow_reasons({'active':True},usage,snapshot,{'minutes':1},now=100),['Task elapsed-time threshold reached'])

class WorkflowJourney(unittest.IsolatedAsyncioTestCase):
    async def test_terminal_uppercase_R_reviews_only_visible_completions(self):
        from observatory.attention import AttentionLedger as PureLedger
        from tests.test_attention import item, snapshot
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.workflow_controller.close()
            ledger=PureLedger();now=time.time()
            ledger.update([item(id='done',agent_id='done',state='Completed',waiting_for=''),
                           item(id='approval',agent_id='approval')],snapshot(now),{},now)
            app.workflow_ledger.rows=ledger.rows
            app.action_workflow();await pilot.pause()
            expected=tuple(row[0]['_attention_id'] for row in app.screen.visible_rows
                           if row[0]['_attention_kind']=='completion')
            self.assertEqual(len(expected),1)
            with patch.object(app.workflow_controller,'review_completions') as review:
                await pilot.press('R');await pilot.pause()
                review.assert_called_once_with(expected)
                review.reset_mock()
                await pilot.resize_terminal(120,36);await pilot.pause()
                await pilot.press('R');await pilot.pause()
                review.assert_called_once_with(expected)

    async def test_queue_floor_preserves_incident_cursor_scroll_and_keyboard_drilldown(self):
        from observatory.attention import AttentionLedger as PureLedger
        from tests.test_attention import item,snapshot
        from textual.widgets import DataTable,RichLog
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(60,20)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.workflow_controller.close()
            ledger=PureLedger();now=time.time()
            ledger.update([item(id=str(i),agent_id=str(i),task='Task '+str(i)) for i in range(25)],snapshot(now),{},now)
            app.workflow_ledger.rows=ledger.rows
            app.action_workflow();await pilot.pause()
            screen=app.screen;table=screen.query_one(DataTable)
            self.assertGreaterEqual(table.size.height,14)
            self.assertFalse(screen.query_one(RichLog).display)
            self.assertIs(app.focused,table)
            await pilot.press(*(['down']*20));await pilot.pause()
            key=table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
            position=table.scroll_y
            app.workflow_ledger.rows=[(dict(it,task=it['task']+' refreshed'),reason,since) for it,reason,since in ledger.rows]
            screen.render_report();await pilot.pause()
            self.assertEqual(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value,key)
            self.assertEqual(table.scroll_y,position)
            await pilot.press('enter');await pilot.pause()
            self.assertEqual(screen.selected_incident,key)
            self.assertTrue(screen.query_one(RichLog).display)
            self.assertFalse(table.display)
            await pilot.press('escape');await pilot.pause()
            self.assertIs(app.screen,screen);self.assertTrue(table.display)
            self.assertEqual(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value,key)
            await pilot.resize_terminal(45,15);await pilot.pause()
            self.assertTrue(screen.query_one('#workflow-too-small').display)
            await pilot.resize_terminal(60,18);await pilot.pause()
            self.assertFalse(screen.query_one('#workflow-too-small').display)
            self.assertGreaterEqual(table.size.height,12)
            with patch.object(app,'action_review_work') as review:
                await pilot.click('#workflow-review');await pilot.pause()
                self.assertEqual(review.call_args.args[0]['_attention_id'],key)

    async def test_modal_settings_and_attention_selection_at_floor(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause();app.set_agent_filter('all');await pilot.press('w');await pilot.pause()
            self.assertIsInstance(app.screen,WorkflowScreen)
            from textual.widgets import Input,RichLog,Checkbox,Button
            self.assertIn('OBSERVATION HEALTH',app.screen.query_one(RichLog).lines[2].text+' '+app.screen.query_one(RichLog).lines[3].text)
            await pilot.click('#workflow-settings');await pilot.pause()
            app.screen.query_one('#workflow-tokens',Input).value='25'
            await pilot.click('#workflow-save');await pilot.pause()
            self.assertEqual(app.workflow_limits['tokens'],25)
            app.screen.query_one(Checkbox).value=True;await pilot.pause()
            self.assertTrue(app.workflow_notify)
            await pilot.click('#workflow-settings');await pilot.pause()
            for size in ((80,24),(60,20)):
                await pilot.resize_terminal(*size);await pilot.pause()
                for button in app.screen.query('#workflow-controls Button'):self.assertTrue(app.screen.region.contains_region(button.region))
                self.assertGreaterEqual(app.screen.query_one(RichLog).size.height,5)
            await pilot.press('escape');await pilot.pause()
            self.assertNotIsInstance(app.screen,WorkflowScreen)
