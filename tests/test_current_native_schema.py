"""Synthetic current-version source shapes; no agent/provider invocation or capture."""
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
import json
import os
import sqlite3
import threading
from copy import deepcopy
from unittest.mock import patch

from observatory.core import Store
from observatory.local_agents import Session, identity
from observatory.opencode import OpenCodeReader
from observatory.workflow_evidence import ReviewHistory, scope_for
from observatory.work_receipts import freeze_receipt
from observatory.saved_tasks import TaskStore
from observatory.review_cli import collect_review
from observatory.server import BoundedHTTPServer, handler_for
from observatory.terminal import Api
from observatory.opencode_turns import PREFIX, epoch, trace_id, usage_attributes
from tests.native_schema_fixture import ROOT, load_current_opencode


class CurrentNativeSchemaTests(unittest.TestCase):
    def test_codex_current_wire_fields_reach_review_and_export(self):
        with TemporaryDirectory() as directory:
            store = Store(Path(directory) / 'receiver.db')
            self.addCleanup(store.close)
            path = Path(directory) / 'rollout.jsonl'
            path.write_bytes((ROOT / 'codex-0.167.1.synthetic.jsonl').read_bytes())
            reader = Session(store, path, 'Codex')
            reader.poll()
            item = dict(service='codex', agent_id=reader.agent_id, session_id=reader.session_id,
                        trace_id=identity(reader.source_id, reader.turn), task=reader.task_description,
                        state='Completed')
            scope = scope_for(item, SimpleNamespace(receiver_id=store.receiver_id))
            history = ReviewHistory(scope)
            history.accept(store.activity_page(trace_id=scope.run_id, review=True, limit=200))
            projection = history.projection(item)
            self.assertEqual(projection['request']['text'], 'Repair current schema fidelity')
            self.assertEqual(projection['agent_update']['text'], 'Current schema fixture preserves checks and edits.')
            self.assertEqual({row['path'] for row in projection['edits']}, {'src/evidence.py', 'tests/test_evidence.py'})
            self.assertEqual(len(projection['checks']), 2)
            self.assertTrue(all(row['result'] == 'passed' for row in projection['checks']))
            events = store.activity_page(trace_id=scope.run_id)['activity']
            update = next(row for row in events if row.get('subject') == 'src/evidence.py')
            self.assertIn('unified_diff', update['detail'])
            self.assertNotIn('"diff":', update['detail'])
            for rendered in freeze_receipt(projection).values():
                self.assertIn('src/evidence.py', rendered)

    def test_current_opencode_split_parts_and_idle_turns_are_available(self):
        with TemporaryDirectory() as directory:
            store = Store(Path(directory) / 'receiver.db')
            self.addCleanup(store.close)
            path = Path(directory) / 'source.db'
            fixture = load_current_opencode(path)
            with patch('observatory.opencode.time.time_ns', return_value=fixture['clock_ns']):
                reader = OpenCodeReader(store, path=path)
                reader.poll()
            events = store.activity_page(session_id='ses_schema_fixture')['activity']
            subjects = {row['subject'] for row in events}
            self.assertIn('First assistant step', subjects)
            self.assertIn('Current turn implementation complete', subjects)
            self.assertTrue(any('pytest tests -q' in row.get('detail', '') for row in events))
            from observatory.opencode_turns import epoch, trace_id
            first = trace_id(reader.source_id, epoch(reader.source_id, reader.fingerprint),
                             'ses_schema_fixture', 'msg_step_one')
            turn = store.review_turn(first)
            self.assertIsNotNone(turn)
            self.assertEqual(len(turn['step_trace_ids']), 2)
            self.assertTrue(turn['turn_usage_complete'])
            self.assertEqual((turn['input_tokens'], turn['output_tokens']), (17, 10))
            self.assertTrue(turn['turn_closed'])
            self.assertEqual(turn['turn_coverage_reasons'], [])
            rows = store.activity_page(turn_id=turn['turn_id'])['activity']
            self.assertIn('Also preserve the pending request boundary', {row['subject'] for row in rows})
            self.assertNotIn('An unrelated following request', {row['subject'] for row in rows})


class CurrentOpenCodeBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.directory = TemporaryDirectory(); self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.path = self.root / 'source.db'
        self.fixture = load_current_opencode(self.path)
        self.store = Store(self.root / 'receiver.db'); self.addCleanup(self.store.close)
        self.reader = OpenCodeReader(self.store, path=self.path)

    def poll(self, reader=None):
        with patch('observatory.opencode.time.time_ns', return_value=self.fixture['clock_ns']):
            return (reader or self.reader).poll()

    def trace(self, message='msg_step_one', reader=None):
        reader = reader or self.reader
        return trace_id(reader.source_id, epoch(reader.source_id, reader.fingerprint), 'ses_schema_fixture', message)

    def edit(self, query, parameters=()):
        with sqlite3.connect(self.path) as db: db.execute(query, parameters)

    def test_authenticated_turn_review_freezes_exact_membership_in_saved_handoff(self):
        self.poll(); trace = self.trace(); turn = self.store.review_turn(trace)
        server = BoundedHTTPServer(('127.0.0.1', 0), handler_for(self.store, 'synthetic-token', quiet=True))
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        self.addCleanup(server.server_close); self.addCleanup(server.shutdown)
        projection = collect_review(Api(f'http://127.0.0.1:{server.server_port}', 'synthetic-token'),
            service='opencode', agent_id=turn['scope_owner']['agent_id'], session_id='ses_schema_fixture',
            trace_id=trace, pages=5, whole_turn=True)
        self.assertEqual(projection['request']['text'], 'Also preserve the pending request boundary')
        self.assertEqual(projection['agent_update']['text'], 'Current turn implementation complete')
        self.assertEqual(projection['checks'][0]['command'], 'pytest tests -q')
        self.assertEqual(projection['checks'][0]['result'], 'passed')
        self.assertEqual(projection['usage']['values']['input_tokens'], 17)
        self.assertNotIn('An unrelated following request', json.dumps(projection))
        tasks = TaskStore(self.root / 'tasks.db'); task = tasks.create('Current schema handoff')
        linked = tasks.link(task['id'], projection)['links'][0]
        self.assertEqual(linked['turn_id'], turn['turn_id'])
        self.assertEqual(set(linked['step_trace_ids']), set(turn['step_trace_ids']))
        self.assertEqual(linked['review_snapshot'], projection)
        for kind, text in freeze_receipt(projection).items():
            self.assertIn('pytest tests -q', text, kind)

    def test_disjoint_usage_components_are_inclusive_only_with_complete_source_fields(self):
        with sqlite3.connect(self.path) as db:
            row = json.loads(db.execute("SELECT data FROM session_message WHERE id='msg_step_one'").fetchone()[0])
            row['tokens'] = dict(input=12, output=7, reasoning=3, cache=dict(read=9, write=4))
            db.execute("UPDATE session_message SET data=? WHERE id='msg_step_one'", (json.dumps(row),))
        self.poll(); turn = self.store.review_turn(self.trace())
        self.assertEqual((turn['input_tokens'], turn['output_tokens']), (30, 13))
        self.assertEqual((turn['cache_read_input_tokens'], turn['cache_write_input_tokens'], turn['reasoning_output_tokens']), (9,4,3))
        for tokens in (dict(input=1,output=2), dict(input=1,output=2,reasoning=False,cache=dict(read=0,write=0)),
                       dict(input=1,output=2,reasoning=0,cache=dict(read=-1,write=0))):
            attrs = usage_attributes(tokens)
            if 'cache' not in tokens or tokens.get('cache',{}).get('read') == -1:
                self.assertNotIn('gen_ai.usage.input_tokens', attrs)
            if 'reasoning' not in tokens or tokens['reasoning'] is False:
                self.assertNotIn('gen_ai.usage.output_tokens', attrs)

    def test_unknown_prefix_never_becomes_public_turn(self):
        self.edit("DELETE FROM session_message WHERE id='msg_idle_prefix'")
        self.poll(); self.assertIsNone(self.store.review_turn(self.trace()))
        with self.store._connect() as db:
            attrs = json.loads(db.execute("SELECT attributes FROM spans WHERE trace_id=? AND name='agent.run'", (self.trace(),)).fetchone()[0])
        self.assertNotIn('agent.turn.id', attrs)
        self.assertEqual(attrs[PREFIX+'boundary_kind'], 'unknown-prefix')
        self.assertFalse(attrs[PREFIX+'source_complete'])

    def test_current_usage_rejects_unsafe_counts_and_inclusive_overflow(self):
        for invalid in (False,True,-1,0.5,float('nan'),float('inf'),10**1000,1e100,2**53):
            with self.subTest(value=str(invalid)[:30]):
                attrs=usage_attributes(dict(input=invalid,output=1,reasoning=invalid,cache=dict(read=0,write=0)))
                self.assertNotIn('gen_ai.usage.input_tokens',attrs)
                self.assertNotIn('gen_ai.usage.output_tokens',attrs)
        attrs=usage_attributes(dict(input=2**53-1,output=2**53-1,reasoning=1,cache=dict(read=1,write=0)))
        self.assertNotIn('gen_ai.usage.input_tokens',attrs)
        self.assertNotIn('gen_ai.usage.output_tokens',attrs)

    def test_open_idle_interval_never_infers_completion_from_assistant_finish(self):
        self.edit("DELETE FROM session_message WHERE seq>=50")
        self.poll(); turn = self.store.review_turn(self.trace())
        self.assertFalse(turn['turn_closed']); self.assertTrue(turn['turn_usage_complete'])
        self.assertEqual((turn['input_tokens'], turn['output_tokens']), (17,10))
        self.assertEqual(self.reader.sessions[0]['run_state'], 'running')
        with self.store._connect() as db:
            attrs = json.loads(db.execute("SELECT attributes FROM spans WHERE trace_id=? AND name='agent.run'", (self.trace(),)).fetchone()[0])
        self.assertEqual(attrs['agent.run.state'], 'running')
        self.assertNotIn(PREFIX+'closed_id', attrs)

    def test_source_epoch_survives_restart_but_changes_on_atomic_replacement(self):
        self.poll(); old_trace = self.trace(); old_agent = self.reader.sessions[0]['agent_id']
        restarted = OpenCodeReader(self.store, path=self.path); self.poll(restarted)
        self.assertEqual(self.trace(reader=restarted), old_trace)
        self.assertEqual(restarted.sessions[0]['agent_id'], old_agent)
        replacement = self.root / 'new.db'; load_current_opencode(replacement); os.replace(replacement,self.path)
        self.poll(); self.assertNotEqual(self.trace(), old_trace)
        self.assertNotEqual(self.reader.sessions[0]['agent_id'], old_agent)
        self.assertNotEqual(self.store.review_turn(self.trace())['turn_id'], self.store.review_turn(old_trace)['turn_id'])

    def test_legacy_inline_layout_retains_legacy_step_and_parent_identities(self):
        self.edit('DROP TABLE session_part')
        with sqlite3.connect(self.path) as db:
            data=json.loads(db.execute("SELECT data FROM session_message WHERE id='msg_step_one'").fetchone()[0])
            data['parentID']='msg_user_one'
            data['content']=[dict(type='text',text='Legacy inline assistant')]
            db.execute("UPDATE session_message SET data=? WHERE id='msg_step_one'",(json.dumps(data),))
        self.poll()
        trace=identity('opencode-trace','ses_schema_fixture','msg_step_one')
        turn=self.store.review_turn(trace)
        self.assertEqual(turn['turn_id'],identity('opencode-turn',self.reader.source_id,'ses_schema_fixture','msg_user_one'))
        self.assertIn('Legacy inline assistant',{row['subject']for row in self.store.activity_page(trace_id=trace)['activity']})
        with self.store._connect() as db:
            attrs=json.loads(db.execute("SELECT attributes FROM spans WHERE trace_id=? AND name='agent.run'",(trace,)).fetchone()[0])
        self.assertNotIn(PREFIX+'schema',attrs)

    def test_equal_timestamp_equal_length_split_text_update_is_observed(self):
        self.poll()
        with sqlite3.connect(self.path) as db:
            data = json.loads(db.execute("SELECT data FROM session_part WHERE message_id='msg_step_two' AND ordinal=0").fetchone()[0])
            data['text'] = 'Current turn implementation COMPLETE'
            db.execute("UPDATE session_part SET data=? WHERE message_id='msg_step_two' AND ordinal=0", (json.dumps(data),))
        self.poll()
        rows = self.store.activity_page(trace_id=self.trace('msg_step_two'))['activity']
        self.assertIn('Current turn implementation COMPLETE', {row['subject'] for row in rows})

    def test_current_inline_and_split_parts_preserve_tool_identity_timing_and_output(self):
        self.poll(); trace=self.trace()
        with self.store._connect() as db:
            before=tuple(db.execute("SELECT span_id,start_ns,end_ns,attributes FROM spans WHERE trace_id=? AND name='shell'",(trace,)).fetchone())
        with sqlite3.connect(self.path) as db:
            data=json.loads(db.execute("SELECT data FROM session_message WHERE id='msg_step_one'").fetchone()[0])
            data['content']=[json.loads(row[0])for row in db.execute("SELECT data FROM session_part WHERE message_id='msg_step_one' ORDER BY ordinal")]
            db.execute("UPDATE session_message SET data=? WHERE id='msg_step_one'",(json.dumps(data),))
            db.execute("DELETE FROM session_part WHERE message_id='msg_step_one'")
        self.poll()
        with self.store._connect() as db:
            rows=db.execute("SELECT span_id,start_ns,end_ns,attributes FROM spans WHERE trace_id=? AND name='shell'",(trace,)).fetchall()
        self.assertEqual(len(rows),1)
        self.assertEqual(tuple(rows[0])[:3],before[:3])
        self.assertEqual(json.loads(rows[0]['attributes'])['output.value'],json.loads(before[3])['output.value'])
        self.assertIn('First assistant step',{row['subject']for row in self.store.activity_page(trace_id=trace)['activity']})

    def test_deleting_final_split_text_clears_the_current_assistant_update(self):
        self.poll()
        self.edit("DELETE FROM session_part WHERE message_id='msg_step_other'")
        self.poll()
        rows=self.store.activity_page(trace_id=self.trace('msg_step_other'))['activity']
        self.assertNotIn('Following turn response',{row['subject'] for row in rows})

    def test_background_shell_launcher_is_not_a_completed_check(self):
        with sqlite3.connect(self.path) as db:
            data=json.loads(db.execute("SELECT data FROM session_part WHERE message_id='msg_step_one' AND type='tool'").fetchone()[0])
            data['state']['metadata']['status']='running'
            db.execute("UPDATE session_part SET data=? WHERE message_id='msg_step_one' AND type='tool'",(json.dumps(data),))
        self.poll(); trace=self.trace(); turn=self.store.review_turn(trace)
        item=dict(service='opencode', agent_id=turn['scope_owner']['agent_id'], session_id='ses_schema_fixture',trace_id=trace)
        history=ReviewHistory(scope_for(item, SimpleNamespace(receiver_id=self.store.receiver_id)))
        history.accept(self.store.activity_page(trace_id=trace,review=True))
        projection=history.projection(item)
        self.assertTrue(all(check['result']!='passed' for check in projection['checks']))

    def test_current_metadata_only_capture_keeps_usage_and_boundary_without_text(self):
        self.reader.capture_details=False; self.poll(); trace=self.trace()
        self.assertTrue(self.store.review_turn(trace)['turn_usage_complete'])
        events=json.dumps(self.store.activity_page(session_id='ses_schema_fixture')['activity'])
        self.assertNotIn('Current turn implementation complete',events)
        self.assertNotIn('pytest tests -q',events)
        self.assertEqual(self.reader.sessions[0]['task'],'')
        self.assertFalse(self.reader.sessions[0]['capture_content'])

    def test_live_streaming_does_not_rewind_older_message_cursor(self):
        with sqlite3.connect(self.path) as db:
            template=json.loads(db.execute("SELECT data FROM session_message WHERE id='msg_step_one'").fetchone()[0])
            db.execute('DELETE FROM session_part'); db.execute('DELETE FROM session_message')
            for n in range(90):
                db.execute('INSERT INTO session_message VALUES(?,?,?,?,?,?,?)',
                    (f'msg_bulk_{n:03}', 'ses_schema_fixture','assistant',n*10,1794650400000+n,1794650400000+n,
                     json.dumps({**template,'content':[dict(type='text',text=f'Generated step {n}')]})))
        seen=set()
        for poll in range(5):
            with sqlite3.connect(self.path) as db:
                db.execute("UPDATE session_message SET time_updated=time_updated+1 WHERE id='msg_bulk_089'")
            self.poll()
            with self.store._connect() as db:
                seen.update(row[0] for row in db.execute("SELECT trace_id FROM spans WHERE name='agent.run'"))
        self.assertEqual(len(seen),90)
        self.assertLessEqual(len(self.reader._message_revisions),30)
        self.assertLessEqual(len(self.reader._v2_windows['ses_schema_fixture']['pending']),30)

    def test_partial_part_capture_never_claims_complete_turn_usage(self):
        with patch('observatory.opencode.MAX_PARTS_PER_MESSAGE',1): self.poll()
        turn=self.store.review_turn(self.trace())
        self.assertFalse(turn['turn_usage_complete']); self.assertNotIn('input_tokens',turn)
        self.assertTrue(turn['turn_coverage_reasons'])

    def test_membership_cap_never_allocates_a_partial_total(self):
        with patch('observatory.opencode_turns.MAX_MEMBERS',1): self.poll()
        turn=self.store.review_turn(self.trace())
        self.assertFalse(turn['turn_usage_complete']); self.assertNotIn('input_tokens',turn)
        self.assertTrue(turn['turn_coverage_reasons'])

    def test_replacement_during_source_read_refuses_publication(self):
        original=self.reader._decode; changed=[]
        def decode(raw):
            value=original(raw)
            if not changed:
                replacement=self.root/'replacement.db'; load_current_opencode(replacement)
                os.replace(replacement,self.path);changed.append(True)
            return value
        with patch.object(self.reader,'_decode',side_effect=decode): self.assertEqual(self.poll(),0)
        self.assertEqual(self.reader.sessions,[])
        self.assertEqual(self.store.activity_page()['activity'],[])
        self.assertGreater(self.reader.backlog_records,0)
        self.assertGreater(self.poll(),0)

    def test_capture_revoked_during_read_refuses_text_publication(self):
        original=self.reader._decode
        def decode(raw):
            value=original(raw);self.reader.capture_details=False;return value
        with patch.object(self.reader,'_decode',side_effect=decode): self.assertEqual(self.poll(),0)
        self.assertEqual(self.store.activity_page()['activity'],[])
        self.assertEqual(self.reader.sessions,[])
        self.poll()
        self.assertNotIn('pytest tests -q',json.dumps(self.store.activity_page()['activity']))

    def test_wal_writer_cannot_mix_new_usage_into_old_boundary_snapshot(self):
        self.edit('PRAGMA journal_mode=WAL')
        original=self.reader._decode;changed=[]
        def decode(raw):
            value=original(raw)
            if not changed:
                with sqlite3.connect(self.path) as db:
                    data=json.loads(db.execute("SELECT data FROM session_message WHERE id='msg_step_two'").fetchone()[0])
                    data['tokens']['input']=500
                    db.execute("UPDATE session_message SET data=?,time_updated=time_updated+1 WHERE id='msg_step_two'",(json.dumps(data),))
                changed.append(True)
            return value
        with patch.object(self.reader,'_decode',side_effect=decode): self.poll()
        first=self.store.review_turn(self.trace())
        self.assertEqual(first['input_tokens'],17)
        self.poll();second=self.store.review_turn(self.trace())
        self.assertEqual(second['input_tokens'],512)
        self.assertEqual(first['turn_id'],second['turn_id'])

    def test_part_only_activity_discovers_session_with_stale_session_and_message_rows(self):
        old=1794650400000-2*86400000
        self.edit('UPDATE session_v2 SET time_updated=?',(old,))
        self.edit('UPDATE session_message SET time_updated=?,time_created=?',(old,old))
        self.poll(); self.assertEqual(self.reader.discovered_sessions,1)
        self.assertTrue(self.reader.sessions)


class CurrentPartBudgetTests(unittest.TestCase):
    def test_partial_byte_window_rotates_without_retaining_payloads(self):
        from observatory.opencode_turns import read_parts
        with TemporaryDirectory() as directory:
            path=Path(directory)/'source.db';load_current_opencode(path)
            with sqlite3.connect(path) as db:
                db.row_factory=sqlite3.Row; db.execute('DELETE FROM session_part')
                for n in range(6):
                    data=json.dumps(dict(type='text',text='x'*180+str(n)))
                    db.execute('INSERT INTO session_part VALUES(?,?,?,?,?,?,?,?)',
                        ('msg_step_one','ses_schema_fixture',n,'text',None,1,1,data))
                reader=SimpleNamespace(schema_errors=0,skipped_oversized=0,skipped_budget=0,budget_excluded_bytes=0,
                    backlog_records=0,payload_bytes=0,_decode=json.loads)
                cursors={};seen=set()
                with patch('observatory.opencode.MAX_POLL_PAYLOAD_BYTES',512):
                    for poll in range(4):
                        reader.payload_bytes=0
                        _,_,parts,complete,_=read_parts(reader,db,'ses_schema_fixture','msg_step_one',cursors=cursors)
                        seen.update(row['text'][-1] for row in parts)
                        self.assertFalse(complete);self.assertLessEqual(reader.payload_bytes,512)
                self.assertEqual(seen,set('012345'))
                self.assertTrue(all(type(value) is int for value in cursors.values()))
                self.assertGreater(reader.backlog_records,0)


if __name__ == '__main__':
    unittest.main()
