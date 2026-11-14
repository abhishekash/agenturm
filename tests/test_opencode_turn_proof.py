"""Schema2 membership proof gates real Store/HTTP Review usage, independently of adapter parsing."""
import json
import sqlite3
import threading
import time
import unittest
from tests import test_opencode as fixture
from observatory.opencode import OpenCodeReader
from observatory.opencode_turn_proof import PREFIX,membership_digest,turn_coverage
from observatory.local_agents import identity
from observatory.review_cli import collect_review
from observatory.server import BoundedHTTPServer,handler_for
from observatory.terminal import Api
from observatory.work_receipts import freeze_receipt


class TurnProofTests(unittest.TestCase):
    setUp=fixture.OpenCodeReaderTests.setUp

    def prepare(self,*,closed=True):
        with sqlite3.connect(self.database) as db:
            db.execute('DELETE FROM todo')
            data=json.loads(db.execute("SELECT data FROM message WHERE id='msg_test'").fetchone()[0]);stamp=data['time']['created']
            data['parentID']='user-one'
            db.execute("UPDATE message SET data=? WHERE id='msg_test'",(json.dumps(data),))
            db.execute('INSERT INTO message VALUES(?,?,?,?,?)',('user-one','ses_test',stamp-1,stamp-1,json.dumps({'role':'user','content':[{'type':'text','text':'Implement fix'}]})))
            db.execute('INSERT INTO message VALUES(?,?,?,?,?)',('step-two','ses_test',stamp+1,stamp+1,json.dumps({**data,'content':[{'type':'text','text':'Done'}],'tokens':{'input':5,'output':3}})))
        OpenCodeReader(self.store,path=self.database).poll()
        self.traces=[identity('opencode-trace','ses_test',mid) for mid in ('msg_test','step-two')]
        self.proof=dict(schema=2,source_epoch='a'*32,boundary_kind='idle',boundary_id='idle-before',boundary_seq=12,
            closed=closed,expected_assistants=2,membership_sha256=membership_digest(self.traces),source_complete=True)
        if closed:self.proof.update(closed_id='idle-after',closed_seq=27)
        for trace in self.traces:self.write_proof(trace,self.proof)
        self.store._refresh_summaries(self.traces)
        return self.store.review_turn(self.traces[0])

    def write_proof(self,trace,proof):
        with self.store._connect() as db:
            rows=db.execute("SELECT span_id,attributes FROM spans WHERE trace_id=? AND name='agent.run'",(trace,)).fetchall()
            for row in rows:
                attrs={key:value for key,value in json.loads(row['attributes']).items() if not key.startswith(PREFIX)}
                attrs.update({PREFIX+key:value for key,value in proof.items()})
                db.execute('UPDATE spans SET attributes=? WHERE trace_id=? AND span_id=?',(json.dumps(attrs),trace,row['span_id']))

    def test_closed_and_open_complete_membership_retain_reported_step_totals(self):
        for closed in (True,False):
            with self.subTest(closed=closed):
                if not hasattr(self,'traces'):self.prepare(closed=closed)
                else:
                    self.proof['closed']=closed;self.proof.pop('closed_id');self.proof.pop('closed_seq')
                    for trace in self.traces:self.write_proof(trace,self.proof)
                turn=self.store.review_turn(self.traces[0])
                self.assertTrue(turn['turn_usage_complete']);self.assertEqual(turn['input_tokens'],17)
                self.assertEqual(turn['turn_closed'],closed);self.assertEqual(turn['turn_coverage_reasons'],[])

    def test_partial_missing_changed_and_ambiguous_proofs_withhold_totals(self):
        self.prepare()
        for patch in ({'source_complete':False},{'expected_assistants':3},{'membership_sha256':'f'*64},
                      {'schema':True},{'source_epoch':'bad'},{'boundary_kind':'unknown-prefix'},
                      {'boundary_seq':True},{'closed_seq':12}):
            with self.subTest(patch=patch):
                for trace in self.traces:self.write_proof(trace,{**self.proof,**patch})
                turn=self.store.review_turn(self.traces[0])
                self.assertFalse(turn['turn_usage_complete']);self.assertNotIn('input_tokens',turn)
                self.assertTrue(turn['turn_coverage_reasons'])
        self.write_proof(self.traces[0],self.proof)
        self.write_proof(self.traces[1],{**self.proof,'source_epoch':'b'*32})
        self.assertFalse(self.store.review_turn(self.traces[0])['turn_usage_complete'])
        self.write_proof(self.traces[1],{})
        self.assertFalse(self.store.review_turn(self.traces[0])['turn_usage_complete'])

    def test_missing_member_and_disagreement_remain_explicit_in_http_review_export(self):
        self.prepare()
        self.write_proof(self.traces[1],{**self.proof,'expected_assistants':3})
        turn=self.store.review_turn(self.traces[0]);owner=turn['scope_owner']
        deadline=time.monotonic()+5
        while not self.store.index_status()['ready'] and time.monotonic()<deadline:self.store._drain_external_activity()
        self.assertTrue(self.store.index_status()['ready'])
        server=BoundedHTTPServer(('127.0.0.1',0),handler_for(self.store,'synthetic',quiet=True))
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        self.addCleanup(server.server_close);self.addCleanup(server.shutdown)
        projection=collect_review(Api(f'http://127.0.0.1:{server.server_port}','synthetic'),
            service='opencode',agent_id=owner['agent_id'],session_id='ses_test',trace_id=self.traces[0],whole_turn=True,pages=5)
        self.assertEqual(projection['usage']['values'],{})
        self.assertTrue(projection['usage']['turn_coverage_reasons'])
        self.assertIn('Assistant steps disagree', ' '.join(projection['window']['reasons']))
        receipt=freeze_receipt(projection)
        for name in ('json','markdown','html'):
            self.assertIn('Assistant steps disagree',receipt[name])

    def test_pure_clipping_and_duplicate_roots_fail_closed(self):
        proof={PREFIX+key:value for key,value in dict(schema=2,source_epoch='a'*32,boundary_kind='idle',boundary_id='idle',boundary_seq=0,
            closed=False,expected_assistants=1,membership_sha256=membership_digest(['a'*32]),source_complete=True).items()}
        self.assertTrue(turn_coverage(proof,[proof],['a'*32],clipped=True))
        self.assertTrue(turn_coverage(proof,[proof,proof],['a'*32,'a'*32]))
        self.assertEqual(turn_coverage({},[{}],['a'*32]),[])


if __name__=='__main__':unittest.main()
