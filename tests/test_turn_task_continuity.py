"""UI/CLI turn capture must keep the same frozen accounting boundary."""
from copy import deepcopy
from dataclasses import replace
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest

from observatory.saved_tasks import TaskStore, TaskError, _link, render_handoff
from observatory.ui_saved_tasks import capture_selected_run
from observatory.workflow_evidence import ExecutionScope, ReviewHistory
from tests.test_terminal_search import ManualFeed


A, B, TURN = 'a'*32, 'b'*32, 'd'*32


def turn_capture():
    feed=ManualFeed()
    identity=dict(producer='opencode',agent_id='worker',session_id='session')
    usage=dict(trace_id=A,turn_id=TURN,step_trace_ids=[A,B],turn_usage_complete=True,
               scope_owner=identity,review_owner=identity,agent_count=1,agent_count_complete=True,
               input_tokens=30,output_tokens=5,input_tokens_reported=True,output_tokens_reported=True,
               usage_complete=True,cost_complete=False)
    item=dict(id='worker',service='opencode',agent_id='worker',session_id='session',
              runtime='OpenCode',trace_id=A,turn_id=TURN,run_start_ns=100,
              task='Implement the fix',state='Completed',capture_content=True)
    events=[dict(id='check-'+trace,source_id='check-'+trace,service='opencode',agent_id='worker',
                 session_id='session',trace_id=trace,turn_id=TURN,domain='shell',action='execute',
                 subject='pytest '+trace[0]+'.py',detail={'exit_code':0},result='success',time_unix_nano=stamp)
            for trace,stamp in [(A,110),(B,120)]]
    history=ReviewHistory(ExecutionScope('receiver','opencode','worker','session',A,kind='turn',turn_id=TURN))
    history.accept(dict(receiver_id='receiver',activity=events))
    projection=history.projection({**item,'_review_usage':usage})
    feed.snapshot=replace(feed.snapshot,receiver_id='receiver',activity=events,
        sessions=[dict(agent_id='worker',session_id='session',services=['opencode'],latest_run={**usage,'turn_usage':usage})])
    owner=SimpleNamespace(feed=feed,selected_source='worker',workflow_items=lambda:[item])
    return owner,item,projection


class TurnTaskContinuityTests(unittest.TestCase):
    def test_ui_turn_receipt_matches_cli_import_and_survives_handoff(self):
        owner,item,projection=turn_capture()
        captured=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        self.assertEqual(captured['trace_id'],'')
        self.assertEqual(captured['turn_id'],TURN)
        self.assertEqual(captured['step_trace_ids'],[A,B])
        self.assertEqual({check['trace_id'] for check in captured['checks']},{A,B})
        ui,cli=_link(captured),_link(projection)
        for key in ('key','trace_id','turn_id','step_trace_ids','usage','review_snapshot'):
            self.assertEqual(ui[key],cli[key],key)
        with tempfile.TemporaryDirectory() as directory:
            store=TaskStore(Path(directory)/'tasks.db')
            task=store.create('Human task',notes='Keep authored intent',link=captured)
            task=TaskStore(store.path).get(task['id'])
            self.assertEqual(task['usage_summary']['input_tokens'],30)
            self.assertIn('Recorded turn: '+TURN,render_handoff(task))
            task=store.link(task['id'],dict(receiver='receiver',producer='opencode',agent_id='worker',
                session_id='session',trace_id=B,usage=dict(input_tokens=20,input_tokens_reported=True)))
            self.assertTrue(task['usage_summary']['aggregation_ambiguous'])
            self.assertIsNone(task['usage_summary']['input_tokens'])

    def test_frozen_turn_can_include_selected_later_member_without_becoming_a_step(self):
        owner,item,projection=turn_capture()
        item['trace_id']=B
        result=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        self.assertEqual(result['turn_id'],TURN)
        self.assertEqual(result['usage']['input_tokens'],30)
        item['trace_id']='c'*32
        with self.assertRaisesRegex(ValueError,'selected agent'):
            capture_selected_run(owner,item_override=item,review_snapshot=projection)

    def test_contradictory_scope_kind_or_membership_cannot_be_imported(self):
        owner,item,projection=turn_capture()
        captured=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        for change in ({'turn_id':'','trace_id':A,'step_trace_ids':[]},
                       {'step_trace_ids':[A]}, {'receiver':'another'},
                       {'turn_id':'e'*32}):
            with self.subTest(change=change),self.assertRaises(TaskError):
                _link({**deepcopy(captured),**change,'checks':[]})
        broken=deepcopy(captured);broken['review_snapshot']['scope']['kind']='run'
        with self.assertRaises(TaskError):_link(broken)

    def test_frozen_usage_is_authoritative_and_malformed_import_fails_cleanly(self):
        owner,item,projection=turn_capture()
        captured=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        captured['usage']['input_tokens']=999
        with self.assertRaisesRegex(TaskError,'usage.*disagree'):_link(captured)
        from observatory.saved_tasks import usage_summary
        self.assertIsNone(usage_summary([captured])['input_tokens'])
        for value in ('wrong',1,[],None):
            broken=deepcopy(projection);broken['usage']=value
            with self.subTest(value=value),self.assertRaises(TaskError):_link(broken)

    def test_old_frozen_turn_never_borrows_new_receiver_turn_or_usage(self):
        owner,item,projection=turn_capture()
        live=owner.feed.snapshot.sessions[0]['latest_run']['turn_usage']
        live.update(turn_id='e'*32,step_trace_ids=['c'*32],input_tokens=999)
        owner.feed.snapshot.receiver_id='new-receiver'
        owner.feed.snapshot.local_status={'capture_content':False}
        captured=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        self.assertEqual(captured['receiver'],'receiver')
        self.assertEqual(captured['turn_id'],TURN)
        self.assertEqual(captured['step_trace_ids'],[A,B])
        self.assertEqual(_link(captured)['usage']['input_tokens'],30)

    def test_same_receiver_capture_revocation_is_checked_before_capture(self):
        owner,item,projection=turn_capture()
        owner.feed.snapshot.local_status={'capture_content':False}
        captured=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        self.assertNotIn('review_snapshot',captured)
        self.assertEqual(captured['checks'],[])
        self.assertEqual(captured['task'],'Metadata-only execution')

    def test_legacy_downgraded_link_remains_readable_but_cannot_supply_totals(self):
        owner,item,projection=turn_capture()
        with tempfile.TemporaryDirectory() as directory:
            store=TaskStore(Path(directory)/'tasks.db')
            task=store.create('Human title',notes='Do not rewrite my decision',link=dict(
                receiver='receiver',producer='opencode',agent_id='worker',session_id='session',trace_id=A,
                usage=dict(input_tokens=30,input_tokens_reported=True)))
            bad=deepcopy(task['links'][0]);bad['review_snapshot']=projection
            with store._connect() as db:
                db.execute('UPDATE saved_tasks_v1 SET links_json=? WHERE id=?',(json.dumps([bad]),task['id']))
            loaded=store.get(task['id'])
            self.assertEqual(loaded['links'],[bad])
            self.assertEqual(loaded['usage_summary']['invalid_scope_links'],1)
            self.assertIsNone(loaded['usage_summary']['input_tokens'])
            self.assertIn('Scope issue:',render_handoff(loaded))
            loaded=store.update(task['id'],notes='Authored update')
            self.assertEqual(loaded['links'],[bad])
            repaired=store.link(task['id'],projection)
            self.assertEqual(len(repaired['links']),1)
            self.assertEqual(repaired['links'][0]['turn_id'],TURN)
            self.assertEqual(repaired['usage_summary']['invalid_scope_links'],0)
            self.assertEqual(repaired['notes'],'Authored update')

    def test_metadata_only_capture_does_not_reintroduce_the_original_task_text(self):
        owner,item,projection=turn_capture();item['capture_content']=False
        captured=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        self.assertNotIn('review_snapshot',captured)
        self.assertEqual(captured['checks'],[])
        self.assertNotIn('Implement the fix',json.dumps(captured))


class TurnTaskUIJourney(unittest.IsolatedAsyncioTestCase):
    async def test_queued_decision_save_rechecks_policy_and_keeps_editor_usable(self):
        from unittest.mock import patch
        from textual.widgets import TextArea, Static
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_saved_tasks import TaskDecisionEditor
        owner,item,projection=turn_capture()
        source=dict(receiver='receiver',producer='opencode',agent_id='worker',session_id='session',trace_id=B,event_id='check-'+B)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'tasks.db';task=TaskStore(path).create('Human task')
            app=ObservatoryApp(owner.feed,persist_preferences=False,task_store_path=path)
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();app.refresh_timer.stop()
                editor=TaskDecisionEditor(app,task,source=source,excerpt='Captured excerpt')
                app.push_screen(editor);await pilot.pause()
                original=editor.job
                with patch.object(editor,'job') as queued:
                    await pilot.press('ctrl+s')
                owner.feed.snapshot.local_status={'capture_content':False}
                args=queued.call_args
                original(*args.args,**args.kwargs)
                await app.workers.wait_for_complete();await pilot.pause()
                self.assertEqual(TaskStore(path).get(task['id'])['decisions'],[])
                self.assertIn('revoked',editor.query_one('#task-status',Static).render().plain)
                self.assertFalse(editor.query_one(TextArea).disabled)
                editor.query_one(TextArea).load_text('My independent replacement decision')
                await pilot.press('ctrl+s')
                await app.workers.wait_for_complete();await pilot.pause()
                decision=TaskStore(path).get(task['id'])['decisions'][0]
                self.assertEqual(decision['text'],'My independent replacement decision')
                self.assertIsNone(decision['source'])

    async def test_non_anchor_excerpt_keeps_its_actual_source_in_decision_and_handoff(self):
        from textual.widgets import DataTable
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewExcerptScreen
        from observatory.ui_saved_tasks import SavedTasksScreen, TaskDecisionEditor
        owner,item,projection=turn_capture()
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'tasks.db'
            task=TaskStore(path).create('Human task')
            app=ObservatoryApp(owner.feed,persist_preferences=False,task_store_path=path)
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();app.refresh_timer.stop()
                app.push_screen(ReviewExcerptScreen(app,projection,item))
                await pilot.pause()
                app.screen.query_one(DataTable).move_cursor(row=1)
                await pilot.press('enter')
                await app.workers.wait_for_complete();await pilot.pause()
                self.assertIsInstance(app.screen,SavedTasksScreen)
                await pilot.press('enter')
                await app.workers.wait_for_complete();await pilot.pause()
                self.assertIsInstance(app.screen,TaskDecisionEditor)
                self.assertEqual(app.screen.source['trace_id'],B)
                await pilot.press('ctrl+s')
                await app.workers.wait_for_complete();await pilot.pause()
                saved=TaskStore(path).get(task['id'])
                self.assertEqual(saved['decisions'][0]['source']['trace_id'],B)
                self.assertIn(B,render_handoff(saved))

    async def test_revocation_in_open_editor_keeps_authored_notes_but_drops_source_payload(self):
        from textual.widgets import Input, TextArea
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_saved_tasks import TaskEditor
        owner,item,projection=turn_capture()
        context=capture_selected_run(owner,item_override=item,review_snapshot=projection)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'tasks.db'
            app=ObservatoryApp(owner.feed,persist_preferences=False,task_store_path=path)
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();app.refresh_timer.stop()
                app.push_screen(TaskEditor(app,context=context))
                await pilot.pause()
                app.screen.query_one('#task-name',Input).value='My approved title'
                app.screen.query_one(TextArea).load_text('My independent decision')
                owner.feed.snapshot.local_status={'capture_content':False}
                await pilot.press('ctrl+s')
                await app.workers.wait_for_complete();await pilot.pause()
                store=TaskStore(path);saved=store.get(store.list()[0]['id'])
                self.assertEqual(saved['title'],'My approved title')
                self.assertEqual(saved['notes'],'My independent decision')
                self.assertNotIn('review_snapshot',saved['links'][0])
                self.assertEqual(saved['links'][0]['checks'],[])
                self.assertEqual(saved['links'][0]['task'],'Metadata-only execution')

    async def test_review_save_and_keyboard_handoff_keep_frozen_turn(self):
        from textual.widgets import Input, TextArea
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        from observatory.ui_saved_tasks import SavedTasksScreen, TaskEditor, TaskDetailScreen, HandoffScreen

        owner,item,projection=turn_capture()
        item['_review_usage']=deepcopy(owner.feed.snapshot.sessions[0]['latest_run']['turn_usage'])
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'tasks.db'
            app=ObservatoryApp(owner.feed,persist_preferences=False,task_store_path=path)
            async with app.run_test(size=(80,24)) as pilot:
                async def ready(predicate):
                    deadline=time.monotonic()+6
                    while not predicate() and time.monotonic()<deadline:
                        await pilot.pause(.05)
                    self.assertTrue(predicate())
                await pilot.pause();app.refresh_timer.stop()
                # Seed the same cache used by a completed source-evidence read.
                cached=app.review_cache.get(ExecutionScope(**projection['scope']),None,True)
                cached.accept(dict(receiver_id='receiver',activity=owner.feed.snapshot.activity))
                app.push_screen(ReviewScreen(app,item))
                await pilot.pause()
                self.assertEqual(app.screen.projection['usage']['step_trace_ids'],[A,B])
                self.assertFalse(path.exists())
                # Save is reachable by Tab/Enter, including from the review log.
                app.screen.query_one('#review-save').focus()
                await pilot.press('enter')
                await ready(lambda:isinstance(app.screen,TaskEditor))
                self.assertFalse(path.exists() and TaskStore(path).count(), 'Opening a draft must not save it')
                app.screen.query_one('#task-name',Input).value='Human approved task'
                app.screen.query_one(TextArea).load_text('Decision: preserve both recorded steps.')
                await pilot.press('ctrl+s')
                await ready(lambda:isinstance(app.screen,SavedTasksScreen) and not app.screen.busy)
                await pilot.press('enter')
                await ready(lambda:isinstance(app.screen,TaskDetailScreen))
                saved=TaskStore(path).get(app.screen.record['id'])
                self.assertEqual(saved['links'][0]['turn_id'],TURN)
                self.assertEqual(saved['links'][0]['step_trace_ids'],[A,B])
                self.assertEqual(saved['usage_summary']['input_tokens'],30)
                self.assertEqual({row['trace_id'] for row in saved['links'][0]['checks']},{A,B})
                await pilot.press('h')
                await ready(lambda:isinstance(app.screen,HandoffScreen) and bool(app.screen.markdown))
                self.assertIn('Recorded turn: '+TURN,app.screen.markdown)
                self.assertIn('Decision: preserve both recorded steps.',app.screen.markdown)
