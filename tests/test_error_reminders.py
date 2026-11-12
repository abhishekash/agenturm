"""Error reminders are recent, exactly scoped and consumed once, not deleted."""
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from observatory.core import Store, normalize_native
from observatory.error_reminders import event_receipt, outcome_event, pending_failures, recent_sources, reminders
from observatory.ui_workflow_controller import WorkflowController
from observatory.workflow_store import WorkflowStore, WorkflowStoreError
from tests.test_workflow_controller import Owner


def agent(identity='one', stamp=10, **values):
    return dict(dict(id=identity, agent_id='agent-'+identity, session_id=identity,
                     service='codex', runtime='Codex', project='repo', stamp=stamp,
                     state='Completed', trace_id='run-'+identity), **values)


def event(identity='failure', **values):
    return dict(dict(id=identity, source_id=identity, service='codex', agent_id='agent-one',
                     session_id='one', trace_id='run-one', domain='shell', action='run',
                     subject='pytest', result='error', time_unix_nano=10), **values)


def snapshot(events=(), **values):
    return SimpleNamespace(**dict(dict(receiver_id='receiver', local_status={},
        sessions=[dict(agent_id='agent-one',session_id='one',services=['codex'],recent_errors=list(events))]), **values))


class ErrorReminderTests(unittest.TestCase):
    def test_recent_two_are_per_project_producer_and_workers_share_root_slot(self):
        items=[agent('old',1),agent('previous',2),agent('current',3),
               agent('worker',4,parent_agent_id='agent-previous',relationship_state='confirmed',relationship_kind='delegated'),
               agent('other',1,project='other'),agent('claude',1,service='claude')]
        self.assertEqual(recent_sources(items),{'previous','current','worker','other','claude'})
        # A cycle cannot hang the rendering thread.
        items[0].update(parent_agent_id='agent-old',relationship_state='confirmed',relationship_kind='delegated')
        self.assertNotIn('old',recent_sources(items))

    def test_lifetime_totals_do_not_create_reminders_and_old_scope_cannot_revive_them(self):
        items=[agent('one',1,errors=50),agent('two',2),agent('three',3)]
        data=snapshot([event()],sessions=[dict(agent_id='agent-one',session_id='one',services=['codex'],errors=50,recent_errors=[event()])])
        self.assertEqual(reminders(items,data),{})
        self.assertEqual(reminders([agent(errors=50)],snapshot([],sessions=[])),{})

    def test_stored_project_without_directory_shares_its_unambiguous_local_project_window(self):
        items=[agent('old',1),agent('previous',2),agent('current',3,working_directory='/work/repo')]
        self.assertEqual(recent_sources(items),{'previous','current'})
        # A shared basename cannot conflate separate checkouts.
        items.append(agent('other',1,working_directory='/elsewhere/repo'))
        self.assertIn('other',recent_sources(items))

    def test_viewed_error_suppressed_but_new_error_and_receiver_stay_independent(self):
        data=snapshot([event()]);items=[agent()]
        key=event_receipt(event(),data)
        self.assertIn('one',reminders(items,data))
        self.assertEqual(reminders(items,data,{key}),{})
        self.assertIn('one',reminders(items,snapshot([event('new')]),{key}))
        self.assertIn('one',reminders(items,snapshot([event()],receiver_id='other'),{key}))
        for field in ('service','agent_id','session_id'):
            self.assertEqual(reminders(items,snapshot([event(**{field:'different'})])),{})

    def test_matching_success_only_suppresses_prior_identical_operation_in_exact_run(self):
        failed=event();success=event('success',result='success',time_unix_nano=11)
        self.assertEqual(pending_failures([failed,success]),[])
        for field in ('service','agent_id','session_id','trace_id','subject'):
            self.assertEqual(pending_failures([failed,{**success,field:'other'}]),[failed])
        self.assertEqual(pending_failures([failed,{**success,'time_unix_nano':10}]),[failed])
        self.assertEqual(pending_failures([failed,success,event('again',time_unix_nano=12)])[0]['id'],'again')

    def test_native_failure_uses_stable_run_not_display_timestamp_and_unanchored_is_not_invented(self):
        item=agent(state='Failed');first=outcome_event(item)
        self.assertEqual(event_receipt(first,snapshot()),event_receipt(outcome_event({**item,'stamp':999}),snapshot()))
        self.assertNotEqual(event_receipt(first,snapshot()),event_receipt(outcome_event({**item,'trace_id':'new'}),snapshot()))
        self.assertIsNone(outcome_event(agent(state='Failed',trace_id='')))
        self.assertIsNone(outcome_event(agent(state='Waiting')))

    def test_durable_seen_is_idempotent_private_and_bounded(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/'workflow.db';store=WorkflowStore(path)
            key=event_receipt(event(subject='private command'),snapshot())
            store.mark_error_seen([key],now=1);store.mark_error_seen([key],now=999)
            self.assertEqual(set(WorkflowStore(path).load_error_seen()),{key})
            with store._transaction(write=False) as db:
                self.assertEqual(db.execute('SELECT seen_at FROM workflow_error_seen_v1').fetchone()[0],1)
            self.assertNotIn(b'private command',path.read_bytes())
            with self.assertRaises(WorkflowStoreError):store.mark_error_seen(['raw prompt'])
            with patch('observatory.workflow_store.MAX_INCIDENTS',2):
                store.mark_error_seen(['a'*64],now=2)
                store.mark_error_seen(['b'*64],now=3)
                self.assertEqual(set(store.load_error_seen()),{'a'*64,'b'*64})

    def test_controller_persists_off_thread_and_survives_restart_without_reviewing_task(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/'workflow.db';owner=Owner();owner.workflow_notify=False
            controller=WorkflowController(owner,store_path=path)
            controller.update([],owner.feed.snapshot,{});owner.wait()
            key=event_receipt(event(),owner.feed.snapshot,owner.feed.api.endpoint)
            controller.error_displayed([event()]);owner.wait()
            controller.error_displayed([event()]);owner.wait()
            self.assertIn(key,controller.error_seen);self.assertEqual(controller.receipts,{})
            shown=snapshot(receiver_id='paused-receiver')
            controller.error_displayed([event()],snapshot=shown);owner.wait()
            self.assertIn(event_receipt(event(),shown),controller.error_seen)
            controller.close()
            restarted=WorkflowController(owner,store_path=path)
            restarted.update([],owner.feed.snapshot,{});owner.wait()
            self.assertIn(key,restarted.error_seen);restarted.close()

    def test_unavailable_storage_reports_memory_only_seen_and_does_not_stop_monitoring(self):
        owner=Owner();owner.workflow_notify=False
        controller=WorkflowController(owner)
        with patch('observatory.ui_workflow_controller.WorkflowStore',side_effect=WorkflowStoreError('read only')):
            controller.update([],owner.feed.snapshot,{});owner.wait()
        controller.error_displayed([event()]);owner.wait()
        self.assertTrue(controller.error_seen)
        self.assertIn('durable storage is unavailable',owner.messages[-1])
        controller.error_displayed([event()]);owner.wait()
        self.assertEqual(len(owner.messages),1)
        controller.close()

    def test_first_visible_error_can_persist_before_first_workflow_poll(self):
        with TemporaryDirectory() as tmp:
            owner=Owner();path=Path(tmp)/'workflow.db'
            controller=WorkflowController(owner,store_path=path)
            controller.error_displayed([event()]);owner.wait()
            key=event_receipt(event(),owner.feed.snapshot,owner.feed.api.endpoint)
            self.assertIn(key,WorkflowStore(path).load_error_seen())
            self.assertEqual(owner.messages,[])
            controller.close()

    def test_receiver_projects_exact_recent_error_references_without_changing_totals(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'events.db');self.addCleanup(store.close)
            def span(trace, stamp, result):
                return normalize_native(dict(trace_id=trace*32,span_id='1'*16,name='tool',
                    start_unix_nano=stamp,end_unix_nano=stamp+1,status='OK',
                    attributes={'session.id':'one','agent.id':'worker'},events=[{
                        'name':'agent.activity','time_unix_nano':stamp,'attributes':{
                            'agent.activity.domain':'shell','agent.activity.action':'run',
                            'agent.activity.subject':'pytest','agent.activity.result':result}}]),'codex')
            store.ingest([span('a',1,'error'),span('b',10,'success'),span('c',20,'error')])
            summary=store.sessions()[0]
            self.assertEqual(len(summary['recent_errors']),1)
            failed=summary['recent_errors'][0]
            self.assertEqual(failed['trace_id'],'c'*32)
            original=store.activity_page(errors_only=True)['activity']
            self.assertIn(failed['id'],{row['id'] for row in original})
            self.assertEqual(len(original),2)
            self.assertNotIn('detail',failed)
            # Sessions polling must not deserialize captured trace payloads.
            # The compact error index is independent of that large body cache.
            with store._connect() as db:
                db.execute('UPDATE trace_summaries SET activity_json=?',('not-json '+('private body '*100000),))
            self.assertEqual(store.sessions()[0]['recent_errors'],summary['recent_errors'])

    def test_log_only_error_projection_uses_real_activity_identity_and_later_success(self):
        with TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'events.db');self.addCleanup(store.close)
            def log(stamp,result):
                return dict(trace_id=None,span_id=None,time_unix_nano=stamp,service='codex',
                    severity_number=9,severity_text='INFO',event_name='agent.activity',body='private stderr',
                    scope_name='',resource_attributes={},attributes={'session.id':'one','agent.id':'worker',
                        'agent.activity.domain':'shell','agent.activity.action':'run',
                        'agent.activity.subject':'pytest','agent.activity.result':result})
            store.ingest_logs([log(1,'error')])
            failed=store.sessions()[0]['recent_errors'][0]
            self.assertEqual(failed['id'],store.activity_page()['activity'][0]['id'])
            self.assertNotIn('private stderr',str(failed))
            store.ingest_logs([log(2,'success')])
            self.assertEqual(store.sessions()[0]['recent_errors'],[])
            self.assertEqual(store.sessions()[0]['errors'],1)
