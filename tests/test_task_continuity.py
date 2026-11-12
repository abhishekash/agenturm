"""User-owned structured continuity survives revisions and older task writers."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest

from observatory.saved_tasks import TaskConflict, TaskError, TaskStore, backup_task_database, render_handoff


class ContinuityStorageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'tasks.sqlite3'
        self.store = TaskStore(self.path)
        self.task = self.store.create('Retry expiry', notes='Freeform notes remain unchanged')

    def test_structured_revisions_supersession_and_old_writer_survive_reopen(self):
        task = self.store.update_brief(self.task['id'], {'intent': 'Preserve retries', 'next_action': 'Run expiry check'}, expected_revision=1)
        task = self.store.add_decision(task['id'], kind='rejected', text='Global timeout', rationale='Breaks long operations', expected_revision=2)
        first = task['decisions'][0]
        task = self.store.add_decision(task['id'], kind='accepted', text='Per-request deadline', supersedes_id=first['id'], expected_revision=3)
        self.assertEqual(task['decisions'][0], first)
        with self.assertRaises(TaskConflict):
            self.store.update_brief(task['id'], {'intent': 'Stale'}, expected_revision=2)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(len(db.execute('PRAGMA table_info(saved_tasks_v1)').fetchall()), 10)
            db.execute('UPDATE saved_tasks_v1 SET notes=?,revision=revision+1 WHERE id=?', ('Older writer changed notes', task['id']))
            db.execute('INSERT INTO saved_tasks_v1 VALUES(?,?,?,?,?,?,?,?,?,?)', ('older-created','Old task','','',0,1,1,1,'[]','{}'))
        reopened = TaskStore(self.path).get(task['id'])
        self.assertEqual(reopened['brief']['intent'], 'Preserve retries')
        self.assertEqual(reopened['decisions'], task['decisions'])
        self.assertEqual(reopened['revision'], 5)
        self.assertEqual(reopened['notes'], 'Older writer changed notes')
        self.assertEqual(TaskStore(self.path).get('older-created')['decisions'], [])
        handoff = render_handoff(reopened)
        self.assertIn('Per-request deadline', handoff)
        self.assertNotIn('Global timeout', handoff)
        self.assertIn('Global timeout', render_handoff(reopened, include_decision_history=True))
        self.assertEqual(json.loads(render_handoff(reopened, 'json'))['task']['decisions'][0], first)

    def test_invalid_bounds_sources_and_cross_task_references_are_atomic(self):
        first = self.store.add_decision(self.task['id'], kind='accepted', text='Keep request IDs')['decisions'][0]
        other = self.store.create('Other')
        for values in ({'kind':'unknown','text':'x'}, {'kind':'accepted','text':''},
                       {'kind':'accepted','text':'x'*4001}, {'kind':'accepted','text':'x','supersedes_id':'missing'},
                       {'kind':'accepted','text':'x','source':{'content_available':False}},
                       {'kind':'accepted','text':'x','source':{'raw_prompt':'hidden'}}):
            with self.assertRaises(TaskError): self.store.add_decision(self.task['id'], **values)
        with self.assertRaises(TaskError):
            self.store.add_decision(other['id'], kind='accepted', text='Wrong task', supersedes_id=first['id'])
        with self.assertRaises(TaskError): self.store.update_brief(self.task['id'], {'unknown':'x'})
        with self.assertRaises(TaskError): self.store.update_brief(self.task['id'], {'intent':'x'*4001})
        self.assertEqual(self.store.get(self.task['id'])['revision'], 2)
        self.assertEqual(len(self.store.get(self.task['id'])['decisions']), 1)

    def test_decision_history_limit_never_evicts_and_failed_update_rolls_back(self):
        for i in range(128): self.store.add_decision(self.task['id'], kind='accepted', text=str(i))
        before = self.store.get(self.task['id'])
        with self.assertRaises(TaskError): self.store.add_decision(self.task['id'], kind='accepted', text='Overflow')
        self.assertEqual(self.store.get(self.task['id']), before)

    def test_aggregate_budget_includes_all_structured_versions(self):
        before = self.store.get(self.task['id'])
        for _ in range(128):
            try:
                before = self.store.add_decision(self.task['id'], kind='accepted', text='x'*4000,
                    rationale='y'*4000, scope='z'*4000)
            except TaskError as exc:
                self.assertIn('512 KiB', str(exc))
                break
        else:
            self.fail('Aggregate task budget did not include structured decisions')
        self.assertEqual(self.store.get(self.task['id']), before)
        self.assertLess(len(before['decisions']), 128)

    def test_source_reference_survives_expiry_without_promising_retained_content(self):
        source = {'receiver':'receiver-a','producer':'codex','agent_id':'worker', 'trace_id':'a'*32,
                  'event_id':'removed-event','coverage':'partial','content_available':True}
        task = self.store.add_decision(self.task['id'], kind='accepted', text='<script>explicit excerpt</script>', source=source)
        self.assertIn('not been revalidated', task['decisions'][0]['source']['availability'])
        html = render_handoff(task, 'html')
        self.assertNotIn('<script>', html)
        self.assertIn('removed-event', html)
        self.assertIn('partial', html)
        self.assertEqual(task['notes'], self.task['notes'])

    def test_backup_before_migration_does_not_modify_source_and_refuses_overwrite(self):
        with sqlite3.connect(self.path) as db:
            for table in ('task_briefs_v1','task_decisions_v1','task_schema_v1'): db.execute('DROP TABLE '+table)
        before = self.path.read_bytes()
        target = self.path.with_name('backup.sqlite3')
        backup_task_database(self.path, target)
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(os.stat(target).st_mode & 0o777, 0o600)
        with self.assertRaises(FileExistsError): backup_task_database(self.path, target)
        self.assertEqual(TaskStore(target).get(self.task['id'])['notes'], self.task['notes'])
        with sqlite3.connect(self.path) as db:
            self.assertIsNone(db.execute("SELECT name FROM sqlite_master WHERE name='task_schema_v1'").fetchone())

    def test_unsupported_schema_initialization_is_transactional(self):
        with sqlite3.connect(self.path) as db:
            db.execute('UPDATE task_schema_v1 SET version=99')
        with self.assertRaisesRegex(TaskError, 'Unsupported'): TaskStore(self.path)

    def cli(self, *args):
        return subprocess.run([sys.executable,'-m','observatory','task','--tasks-db',str(self.path),*args], text=True, capture_output=True)

    def test_cli_brief_decisions_and_conflict_exit(self):
        result = self.cli('brief',self.task['id'],'--intent','Human intent','--next-action','Read failing test','--expected-revision','1','--json')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['brief']['intent'],'Human intent')
        stale = self.cli('brief',self.task['id'],'--intent','Stale','--expected-revision','1','--json')
        self.assertEqual(stale.returncode,3)
        self.assertEqual(json.loads(stale.stderr)['error']['code'],'conflict')
        decision = self.cli('decide',self.task['id'],'--kind','rejected','--text','Global timeout','--rationale','Too broad','--json')
        self.assertEqual(decision.returncode,0,decision.stderr)
        listing = self.cli('decisions',self.task['id'],'--json')
        self.assertEqual(json.loads(listing.stdout)['decisions'][0]['kind'],'rejected')
        brief = self.cli('handoff',self.task['id'])
        self.assertIn('Human intent',brief.stdout)
        self.assertIn('Too broad',brief.stdout)

    def test_file_inputs_are_bounded_and_rejected_without_revision_changes(self):
        brief = self.path.with_name('brief.json')
        brief.write_text(json.dumps({'intent': 'Use the saved brief', 'next_action': 'Check expiry'}))
        result = self.cli('brief', self.task['id'], '--file', str(brief), '--json')
        self.assertEqual(result.returncode, 0, result.stderr)
        text = self.path.with_name('decision.txt')
        text.write_text('Keep native retries')
        result = self.cli('decide', self.task['id'], '--kind', 'accepted', '--text-file', str(text), '--json')
        self.assertEqual(result.returncode, 0, result.stderr)
        before = self.store.get(self.task['id'])
        text.write_text('x'*4001)
        self.assertEqual(self.cli('decide', self.task['id'], '--kind', 'accepted', '--text-file', str(text)).returncode, 2)
        brief.write_text('x'*(128*1024+1))
        self.assertEqual(self.cli('brief', self.task['id'], '--file', str(brief)).returncode, 2)
        self.assertEqual(self.store.get(self.task['id']), before)

    def test_same_trace_on_distinct_receivers_is_not_deduplicated(self):
        evidence = {'trace_id':'a'*32,'session_id':'s','agent_id':'a','runtime':'Codex','producer':'codex',
                    'usage':{'input_tokens':10,'input_tokens_reported':True}}
        task = self.store.link(self.task['id'], {**evidence,'receiver':'receiver-one'})
        task = self.store.link(task['id'], {**evidence,'receiver':'receiver-two'})
        self.assertEqual(len(task['links']), 2)
        self.assertEqual(task['usage_summary']['linked_runs'], 2)
        self.assertEqual(task['usage_summary']['input_tokens'], 20)
        task = self.store.link(task['id'], {**evidence,'receiver':'receiver-one'})
        self.assertEqual(len(task['links']), 2)

    def test_native_lifecycle_runs_stay_distinct_with_unknown_usage(self):
        evidence = {'session_id':'same','agent_id':'worker','runtime':'Codex','producer':'codex',
                    'receiver':'receiver-one','run_start_ns':12345,'usage':{'input_tokens':9999}}
        task = self.store.link(self.task['id'], evidence)
        task = self.store.link(task['id'], {**evidence,'run_start_ns':23456})
        self.assertEqual(len(task['links']),2)
        self.assertEqual(task['usage_summary']['linked_runs'],2)
        self.assertEqual(task['usage_summary']['unscoped_links'],0)
        self.assertIsNone(task['usage_summary']['input_tokens'])
        self.assertIn('Recorded native lifecycle start',render_handoff(task))
        for invalid in (False,0,-1,'12345'):
            with self.assertRaises(TaskError):
                self.store.link(task['id'],{**evidence,'run_start_ns':invalid})
        from observatory.workflow_evidence import ExecutionScope, ReviewHistory
        scope = ExecutionScope('receiver-one','codex','worker','same',run_start_ns=12345,kind='run')
        projection = ReviewHistory(scope).projection({'task':'Native review','state':'Completed'})
        task = self.store.link(task['id'], {**evidence,'review_snapshot':projection})
        self.assertEqual(task['links'][0]['review_snapshot'],projection)


class FrozenReviewCaptureTests(unittest.TestCase):
    def test_link_keeps_the_exact_preview_and_rejects_another_receiver(self):
        from dataclasses import asdict
        from types import SimpleNamespace
        from observatory.ui_saved_tasks import capture_selected_run
        from observatory.workflow_evidence import ReviewHistory, scope_for
        from tests.test_terminal_search import ManualFeed
        feed=ManualFeed()
        item={'id':'worker','session_id':'shared','agent_id':'worker','service':'codex',
              'runtime':'Codex','latest_trace_id':'a'*32,'task':'Review retries'}
        feed.snapshot.local_status['receiver_id']='receiver-one'
        event={'id':'check','source_id':'check','service':'codex','session_id':'shared','agent_id':'worker',
               'trace_id':'a'*32,'domain':'shell','subject':'pytest tests/test_expiry.py',
               'result':'success','time_unix_nano':12345}
        feed.snapshot.activity=[event]
        scope=scope_for(item,feed.snapshot,feed.api.endpoint)
        history=ReviewHistory(scope)
        history.accept({'activity':[event],'next_cursor':None})
        projection=history.projection(item)
        owner=SimpleNamespace(feed=feed,selected_source='worker',workflow_items=lambda:[item],
                              last_review={'item_id':'worker','projection':projection})
        evidence=capture_selected_run(owner)
        self.assertEqual(evidence['receiver'],'receiver-one')
        self.assertEqual(evidence['review_snapshot'],projection)
        projection['checks'][0]['command']='mutated afterward'
        self.assertNotIn('mutated afterward',json.dumps(evidence))
        original_item = dict(item)
        item['latest_trace_id'] = 'b'*32
        feed.snapshot.local_status['receiver_id'] = 'receiver-two'
        frozen = capture_selected_run(owner, item_override=original_item, review_snapshot=evidence['review_snapshot'])
        self.assertEqual(frozen['trace_id'], 'a'*32)
        self.assertEqual(frozen['receiver'], 'receiver-one')
        self.assertEqual(frozen['review_snapshot'], evidence['review_snapshot'])
        with tempfile.TemporaryDirectory() as directory:
            store=TaskStore(Path(directory)/'tasks.sqlite3')
            task=store.create('Frozen review',link=evidence)
            self.assertIn('pytest tests/test_expiry.py',render_handoff(task))
            with self.assertRaises(TaskError):
                store.link(task['id'],{**evidence,'receiver':'receiver-two'})
        item['capture_content']=False
        metadata=capture_selected_run(owner)
        self.assertEqual(metadata['checks'],[])
        self.assertNotIn('review_snapshot',metadata)
