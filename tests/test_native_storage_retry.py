"""Storage backpressure must not consume native evidence without persisting it."""
import json
from copy import deepcopy
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from unittest.mock import patch

from observatory.core import IndexNotReady, Store, normalize_native
from observatory.local_agents import Session, LocalAgents


class NativeStorageRetryTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);self.store=Store(self.root/'events.db');self.addCleanup(self.store.close)
        self.path=self.root/'session.jsonl'
        self.path.write_text('\n'.join(json.dumps(row) for row in [
            dict(type='event_msg',timestamp='2026-11-13T12:00:00Z',payload=dict(type='task_started',turn_id='one')),
            dict(type='event_msg',timestamp='2026-11-13T12:00:01Z',
                payload=dict(type='item_completed',item=dict(type='UserMessage',content='Original request to preserve')))])+'\n')
        self.session=Session(self.store,self.path,'Codex')

    def test_each_partial_storage_failure_retries_exact_batch_without_reparsing(self):
        for stage in ('ingest','ingest_logs'):
            with self.subTest(stage=stage):
                session=Session(self.store,self.path,'Codex')
                with patch.object(self.store,stage,side_effect=sqlite3.OperationalError('database is locked')):
                    with self.assertRaises(sqlite3.OperationalError):session.poll()
                self.assertIsNotNone(session._pending_ingest)
                self.assertEqual(session.hierarchy()['coverage'],'partial')
                records=session.records_total
                session.poll()
                self.assertIsNone(session._pending_ingest)
                self.assertEqual(session.records_total,records)
                with self.store._connect() as db:
                    counts=tuple(db.execute('SELECT (SELECT COUNT(*) FROM spans),(SELECT COUNT(*) FROM logs)').fetchone())
                self.assertGreater(counts[0],0);self.assertGreater(counts[1],0)
                session.poll()
                with self.store._connect() as db:
                    self.assertEqual(tuple(db.execute('SELECT (SELECT COUNT(*) FROM spans),(SELECT COUNT(*) FROM logs)').fetchone()),counts)
                self.assertIn('Original request to preserve',json.dumps(self.store.activity_page()))

    def test_discovery_keeps_pending_source_until_storage_drains(self):
        with patch.object(self.store,'ingest_logs',side_effect=sqlite3.OperationalError('database is locked')):
            with self.assertRaises(sqlite3.OperationalError):self.session.poll()
        agents=LocalAgents(self.store);agents.sessions[self.path]=self.session
        self.path.unlink()
        with patch.object(Path,'home',return_value=self.root/'empty-home'),patch.object(Path,'is_dir',return_value=False):
            agents.discover()
        self.assertIn(self.path,agents.sessions)
        # The pending batch persists even though its file disappeared meanwhile.
        with self.assertRaises(FileNotFoundError):self.session.poll()
        self.assertIsNone(self.session._pending_ingest)
        self.assertIn('Original request to preserve',json.dumps(self.store.activity_page()))

    def test_internal_detection_scrubs_old_and_same_batch_content_and_retries(self):
        self.session.poll()
        with self.path.open('a') as stream:
            stream.write(json.dumps(dict(type='turn_context',timestamp='2026-11-13T12:00:01Z',
                payload=dict(turn_id='review',model='codex-auto-review')))+'\n')
        with patch.object(self.store,'redact_internal_source',side_effect=sqlite3.OperationalError('database is locked')):
            with self.assertRaises(sqlite3.OperationalError):self.session.poll()
        self.assertTrue(self.session._redaction_pending)
        self.assertNotIn('Original request to preserve',json.dumps(self.session.last_activity))
        self.session.poll()
        self.assertFalse(self.session._redaction_pending)
        with self.store._connect() as db:
            captured=' '.join(str(tuple(row)) for row in db.execute('SELECT body,attributes FROM logs'))
            captured+=' '.join(str(tuple(row)) for row in db.execute('SELECT attributes,events,links FROM spans'))
        self.assertNotIn('Original request to preserve',captured)
        # Also classify a single batch after its first records were generated.
        other=self.root/'one-batch.jsonl';other.write_text(self.path.read_text())
        Session(self.store,other,'Codex').poll()
        with self.store._connect() as db:
            self.assertNotIn('Original request to preserve',str([tuple(row) for row in db.execute('SELECT body,attributes FROM logs')]))

    def test_internal_redaction_uses_exact_source_identity_and_retains_usage(self):
        source='f'*32
        records=[]
        for i,resource in enumerate(({'observatory.source_id':source},{'unrelated':source}),1):
            records.append(normalize_native(dict(trace_id=f'{i:032x}',span_id=f'{i:016x}',name='model',
                start_unix_nano=1,end_unix_nano=2,resource_attributes=resource,
                attributes={'input.value':'PRIVATE_INPUT','output.value':'PRIVATE_OUTPUT','gen_ai.usage.input_tokens':20},
                events=[dict(name='agent.activity',time_unix_nano=2,attributes={'agent.activity.detail':'PRIVATE_DETAIL'})]),'codex'))
        self.store.ingest(records);self.store.redact_internal_source(source)
        with self.store._connect() as db:
            rows=db.execute('SELECT trace_id,attributes,events FROM spans ORDER BY trace_id').fetchall()
        self.assertNotIn('PRIVATE_',str(tuple(rows[0])))
        self.assertEqual(json.loads(rows[0]['attributes'])['gen_ai.usage.input_tokens'],20)
        self.assertIn('PRIVATE_INPUT',rows[1]['attributes'])

    def append(self,row):
        with self.path.open('a') as stream:stream.write(json.dumps(row)+'\n')

    def captured(self, *, include_sessions=True):
        # Inspect retained text without repairing or bypassing public readiness.
        # This is needed while redaction deliberately leaves derivation pending.
        with self.store._read() as db:
            db.execute('BEGIN')
            captured=''.join(str([tuple(row) for row in db.execute(sql)]) for sql in (
                'SELECT attributes,events,links FROM spans',
                'SELECT body,attributes FROM logs',
                'SELECT summary_json,activity_json,error_refs_json FROM trace_summaries',
                'SELECT finding_json FROM finding_cache'))
        return captured+(json.dumps(self.store.sessions()) if include_sessions else '')

    def test_pending_old_collector_cannot_overwrite_newer_completion(self):
        for stage in ('ingest','ingest_logs'):
            with self.subTest(stage=stage):
                old=Session(self.store,self.path,'Codex')
                with patch.object(self.store,stage,side_effect=sqlite3.OperationalError('database is locked')):
                    with self.assertRaises(sqlite3.OperationalError):old.poll()
                self.append(dict(type='event_msg',timestamp='2026-11-13T12:01:00Z',payload=dict(type='task_complete',turn_id='one')))
                newer=Session(self.store,self.path,'Codex');newer.poll()
                # Flush while the source is gone, matching rotation/removal.
                text=self.path.read_text();self.path.unlink()
                with self.assertRaises(FileNotFoundError):old.poll()
                with self.store._connect() as db:
                    state=db.execute("SELECT json_extract(attributes,'$.\"agent.run.state\"') FROM spans WHERE name='agent.run'").fetchone()[0]
                self.assertEqual(state,'completed')
                self.path.write_text(text)

    def test_other_collector_policy_prevents_private_retry_and_status_restore(self):
        old=Session(self.store,self.path,'Codex')
        with patch.object(self.store,'ingest_logs',side_effect=sqlite3.OperationalError('database is locked')):
            with self.assertRaises(sqlite3.OperationalError):old.poll()
        self.append(dict(type='turn_context',timestamp='2026-11-13T12:01:00Z',payload=dict(turn_id='review',model='codex-auto-review')))
        newer=Session(self.store,self.path,'Codex');newer.poll()
        self.assertNotIn('Original request to preserve',self.captured())
        monitor=LocalAgents(self.store);monitor.sessions[self.path]=old;monitor.runtime_updated=time.monotonic()
        self.assertNotIn('Original request to preserve',json.dumps(monitor.status()))
        self.path.unlink()
        with self.assertRaises(FileNotFoundError):old.poll()
        self.assertNotIn('Original request to preserve',self.captured())
        self.assertNotIn('Original request to preserve',json.dumps(old.last_activity))

    def test_redaction_invalidates_derived_text_before_async_refresh(self):
        self.session.poll()
        self.assertIn('Original request to preserve',json.dumps(self.store.sessions()))
        with patch.object(self.store,'_refresh_after_write') as scheduled:
            self.store.redact_internal_source(self.session.source_id)
        scheduled.assert_called_once()
        self.assertNotIn('Original request to preserve',self.captured(include_sessions=False))
        with self.assertRaises(IndexNotReady):self.store.sessions()
        with self.store._connect() as db:
            trace_ids=[row[0] for row in db.execute('SELECT trace_id FROM trace_activity')]
            self.assertTrue(trace_ids)
            self.assertEqual(db.execute('SELECT COUNT(*) FROM trace_summaries').fetchone()[0],0)
            self.assertEqual(db.execute('SELECT COUNT(*) FROM finding_cache').fetchone()[0],0)
        self.store._refresh_summaries(trace_ids)
        self.assertNotIn('Original request to preserve',self.captured(include_sessions=False))
        # Raw redaction also invalidates Activity. Repair through the existing
        # path only after proving the pre-refresh privacy contract above.
        self.store._drain_external_activity()
        self.assertTrue(self.store.index_status()['ready'])
        self.assertNotIn('Original request to preserve',self.captured())

    def test_write_policy_survives_reopen_and_rejects_legacy_unsafe_writer(self):
        self.session.poll();self.store.redact_internal_source(self.session.source_id)
        with self.assertRaisesRegex(sqlite3.IntegrityError,'metadata-only'):
            with self.store._connect() as db:
                db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"input.value\"','PRIVATE_RESTORED'),service='codex'")
        reopened=Store(self.store.path);self.addCleanup(reopened.close)
        self.assertIn(self.session.source_id,reopened.internal_source_ids([self.session.source_id]))
        # Old unclassified Session instances remain safe against the durable policy.
        stale=Session(reopened,self.path,'Codex');stale.poll()
        self.assertNotIn('Original request to preserve',self.captured())

    def test_equal_timestamp_streaming_updates_use_recorded_file_position(self):
        source='e'*32
        def version(offset,tokens):
            return normalize_native(dict(trace_id='d'*32,span_id='1'*16,name='model',start_unix_nano=1,end_unix_nano=2,
                scope_name='observatory.local_agents',resource_attributes={'observatory.source_id':source},
                attributes={'gen_ai.usage.input_tokens':tokens,'observatory.native_file_id':'1:2',
                    'observatory.native_file_offset':offset,'observatory.native_mtime_ns':offset}), 'codex')
        self.store.ingest([version(10,1)])
        self.store.ingest([version(20,9)],native_retry=True)
        self.store.ingest([version(10,1)],native_retry=True)
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT json_extract(attributes,'$.\"gen_ai.usage.input_tokens\"') FROM spans").fetchone()[0],9)

    def test_legacy_internal_capture_is_scrubbed_during_upgrade(self):
        self.session.poll()
        with self.store._connect() as db:
            db.execute("UPDATE spans SET service='codex-review'")
            db.execute("UPDATE logs SET service='codex-review'")
            db.execute("DELETE FROM schema_meta WHERE key='native_internal_policy_v1'")
        self.assertIn('Original request to preserve',self.captured(include_sessions=False))
        with self.assertRaises(IndexNotReady):self.store.sessions()
        upgraded=Store(self.store.path);self.addCleanup(upgraded.close)
        self.assertTrue(upgraded.index_status()['ready'])
        self.assertIn(self.session.source_id,upgraded.internal_source_ids([self.session.source_id]))
        self.assertNotIn('Original request to preserve',self.captured())

    def test_first_attempt_cannot_replace_new_generation_completion(self):
        original_ingest=self.store.ingest
        def interleaved(spans,**kwargs):
            replacement=self.root/'replacement.jsonl'
            replacement.write_text(self.path.read_text()+json.dumps(dict(type='event_msg',
                timestamp='2026-11-13T12:01:00Z',payload=dict(type='task_complete',turn_id='one')))+'\n')
            replacement.replace(self.path)
            with patch.object(self.store,'ingest',original_ingest):
                Session(self.store,self.path,'Codex').poll()
            return original_ingest(spans,**kwargs)
        with patch.object(self.store,'ingest',side_effect=interleaved):self.session.poll()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT json_extract(attributes,'$.\"agent.run.state\"') FROM spans WHERE name='agent.run'").fetchone()[0],'completed')
        # A real new generation remains writable, including equal-time corrections.
        replacement=self.root/'next.jsonl';replacement.write_text(self.path.read_text())
        replacement.replace(self.path)
        newer=Session(self.store,self.path,'Codex');newer.poll()
        info=self.path.stat()
        with self.store._connect() as db:
            self.assertEqual(db.execute("SELECT json_extract(attributes,'$.\"observatory.native_file_id\"') FROM spans WHERE name='agent.run'").fetchone()[0],f'{info.st_dev}:{info.st_ino}')

    def test_missing_pending_slots_are_counted_separately_from_discovery(self):
        agents=LocalAgents(self.store)
        missing=self.root/'missing.jsonl';pending=Session(self.store,missing,'Codex')
        pending._pending_ingest=([],[]);agents.sessions[missing]=pending
        home=self.root/'home';sessions=home/'.codex'/'sessions';sessions.mkdir(parents=True)
        for name in ('a.jsonl','b.jsonl'):(sessions/name).write_text('{}\n')
        with patch.object(Path,'home',return_value=home),patch.dict('os.environ',{},clear=True),patch('observatory.local_agents.MAX_SOURCES',2):
            agents.discover()
        self.assertEqual(len(agents.sessions),2)
        self.assertEqual(agents.discovered_sources,2)
        self.assertEqual(agents.omitted_sources,1)
        present=next(path for path in agents.sessions if path!=missing)
        agents.sessions.pop(missing)
        agents.sessions[present]._pending_ingest=([],[])
        with patch.object(Path,'home',return_value=home),patch.dict('os.environ',{},clear=True),patch('observatory.local_agents.MAX_SOURCES',2):
            agents.discover()
        self.assertEqual(len(agents.sessions),2)
        self.assertEqual(agents.omitted_sources,0)

    def test_rejected_future_stamped_generation_cannot_publish_activity(self):
        with patch.object(self.store,'ingest',side_effect=sqlite3.OperationalError('locked')):
            with self.assertRaises(sqlite3.OperationalError):self.session.poll()
        spans,logs=deepcopy(self.session._pending_ingest)
        replacement=self.root/'new.jsonl'
        replacement.write_text(self.path.read_text()+json.dumps(dict(type='event_msg',
            timestamp='2026-11-13T12:01:00Z',payload=dict(type='task_complete',turn_id='one')))+'\n')
        replacement.replace(self.path);Session(self.store,self.path,'Codex').poll()
        linked=deepcopy(logs[-1]);unlinked=deepcopy(logs[-1])
        info=self.path.stat();current_file=f'{info.st_dev}:{info.st_ino}'
        for index,log in enumerate((linked,unlinked),1):
            log['attributes'].update({'observatory.record_id':str(index)*32,
                'observatory.native_file_id':current_file,'mcp.server.name':'test-server',
                'mcp.tool.name':'read','agent.activity.action':'read'})
            log['span_id']='c'*16 if index==1 else None
        self.store.ingest_logs([linked,unlinked],native_source_path=self.path)
        stale=deepcopy(linked)
        stale['attributes']['observatory.native_file_id']=logs[-1]['attributes']['observatory.native_file_id']
        with self.store._connect() as db:before_logs=[tuple(row) for row in db.execute('SELECT * FROM logs ORDER BY digest')]
        self.assertEqual(self.store.ingest_logs([stale],native_source_path=self.path),0)
        with self.store._connect() as db:self.assertEqual([tuple(row) for row in db.execute('SELECT * FROM logs ORDER BY digest')],before_logs)
        before_sessions=self.store.sessions();before_endpoints=self.store.endpoints()
        with self.store._connect() as db:before_activity=[tuple(row) for row in db.execute('SELECT * FROM trace_activity')]
        future=time.time_ns()+10**15
        for span in spans:span['end_unix_nano']=future;span['resource_attributes']['host.name']='stale-future-host'
        for log in logs:log['time_unix_nano']=future;log['resource_attributes']['host.name']='stale-future-host'
        self.assertEqual(self.store.ingest(spans,native_source_path=self.path),0)
        self.assertEqual(self.store.ingest_logs(logs,native_source_path=self.path),0)
        with self.store._connect() as db:self.assertEqual([tuple(row) for row in db.execute('SELECT * FROM trace_activity')],before_activity)
        self.assertEqual(self.store.sessions(),before_sessions)
        self.assertEqual(self.store.endpoints(),before_endpoints)
