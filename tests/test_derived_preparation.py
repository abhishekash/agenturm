"""Connection scratch reuse preserves frozen e795 derived facts and rollback."""
import hashlib
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from observatory.core import Store,normalize_native
from observatory.error_reminders import trace_error_references
from observatory import storage_derived

FIXTURE=Path(__file__).parent/'fixtures/derived_preparation.json'


def seed(path):
    store=Store(path)
    with store._connect() as db:db.execute("UPDATE schema_meta SET value=? WHERE key='receiver_id'",('f'*32,))
    records=[]
    for trace in range(1,6):
        for number in range(1,5):
            attrs={'session.id':'fixture-session','agent.id':'fixture-worker','gen_ai.request.model':'fixture-model'}
            if number>1:attrs.update({'gen_ai.usage.input_tokens':trace*number,
                'gen_ai.usage.output_tokens':number,'llm.cost.total':number/100})
            if trace==3 and number==3:attrs['gen_ai.usage.input_tokens']='invalid'
            if trace==4 and number==4:attrs['captured.padding']='x'*65537
            if trace==5 and number==2:attrs.update({'tool.name':'shell','tool.command':'pytest fixture',
                'tool.exit_code':1,'tool.error':'Synthetic failure','output.value':'Synthetic assertion failed'})
            records.append(normalize_native(dict(trace_id=f'{trace:032x}',span_id=f'{number:016x}',
                parent_span_id=f'{number-1:016x}'if number>1 else None,
                name='agent.run'if number==1 else 'model',start_unix_nano=number*100,end_unix_nano=number*100+50,
                status='ERROR'if trace==5 and number==2 else 'OK',attributes=attrs,events=[]),'fixture'))
    store.ingest(records)
    store.ingest_logs([dict(trace_id='6'.zfill(32),span_id=None,time_unix_nano=200,service='fixture',severity_number=9,
        severity_text='INFO',event_name='agent.activity',body='Generated log-only request',
        attributes={'session.id':'fixture-log-session','agent.activity.domain':'message','agent.activity.action':'user'},
        resource_attributes={},scope_name='fixture')])
    return store


def projection(db,module):
    """Ordered complete result tuples plus exact error references, no field mask."""
    results={}
    def capture(name,trace):
        result=module.derive_trace(db,f'{trace:032x}')
        refs=trace_error_references(result[1],f'{trace:032x}',receiver='f'*32)
        raw=json.dumps({'derived':result,'error_references':refs},sort_keys=True,separators=(',',':'))
        results[name]=hashlib.sha256(raw.encode()).hexdigest()
    for trace in range(1,7):capture('trace-'+str(trace),trace)
    # Reparent usage away from an ancestor without changing the cached stats.
    db.execute("UPDATE spans SET parent_span_id=? WHERE trace_id=? AND span_id=?",('1'.zfill(16),'1'.zfill(32),'4'.zfill(16)))
    capture('reparent',1)
    db.execute("UPDATE spans SET attributes=json_set(attributes,'$.\"gen_ai.usage.input_tokens\"',91) WHERE trace_id=? AND span_id=?",('1'.zfill(32),'3'.zfill(16)))
    capture('upsert-usage',1)
    db.execute('UPDATE spans SET parent_span_id=? WHERE trace_id=? AND span_id=?',('4'.zfill(16),'2'.zfill(32),'2'.zfill(16)))
    capture('cycle',2)
    db.execute('DELETE FROM spans WHERE trace_id=? AND span_id=?',('2'.zfill(32),'4'.zfill(16)))
    capture('delete-child',2)
    return results


class RecordingConnection(sqlite3.Connection):
    def __init__(self,*args,**kwargs):
        super().__init__(*args,**kwargs);self.functions=[];self.ddl=[];self.fail_usage=False;self.peak=0
    def create_function(self,*args,**kwargs):
        self.functions.append((args[0],kwargs.get('deterministic',False)))
        return super().create_function(*args,**kwargs)
    def execute(self,sql,*args):
        if self.fail_usage and sql.startswith('UPDATE derived_usage SET leaves'):
            self.fail_usage=False;raise RuntimeError('Synthetic failure after scratch fill')
        result=super().execute(sql,*args)
        if 'CREATE TEMP TABLE derived_usage' in sql or 'DROP TABLE' in sql:self.ddl.append(sql)
        if sql.startswith('INSERT INTO derived_usage'):
            self.peak=max(self.peak,super().execute('SELECT COUNT(*) FROM derived_usage').fetchone()[0])
        return result


class DerivedPreparationTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.path=Path(self.temp.name)/'fixture.db';self.store=seed(self.path);self.addCleanup(self.store.close)
    def connection(self):
        db=sqlite3.connect(self.path,factory=RecordingConnection);db.row_factory=sqlite3.Row
        self.addCleanup(db.close);return db

    def test_complete_frozen_projection_and_error_references_match_original(self):
        db=self.connection();db.execute('BEGIN IMMEDIATE');db.execute('DELETE FROM derived_span_stats_v1')
        before=db.execute("SELECT type,name,sql FROM sqlite_master ORDER BY type,name").fetchall()
        self.assertEqual(projection(db,storage_derived),json.loads(FIXTURE.read_text())['sha256_by_projection'])
        self.assertEqual(before,db.execute("SELECT type,name,sql FROM sqlite_master ORDER BY type,name").fetchall())
        self.assertEqual(len(db.ddl),1)
        self.assertEqual(db.functions,[('derived_statistics',True),('derived_session_value',True)])
        self.assertEqual(db.peak,4)
        self.assertEqual(db.execute('SELECT COUNT(*) FROM temp.derived_usage').fetchone()[0],0)

    def test_scratch_does_not_cross_traces_error_or_rolled_back_create(self):
        db=self.connection();db.execute('BEGIN IMMEDIATE')
        first=storage_derived.derive_trace(db,'1'.zfill(32));db.commit()
        db.execute('BEGIN IMMEDIATE');db.fail_usage=True
        with self.assertRaisesRegex(RuntimeError,'Synthetic failure'):storage_derived.derive_trace(db,'2'.zfill(32))
        self.assertEqual(db.execute('SELECT COUNT(*) FROM temp.derived_usage').fetchone()[0],0)
        db.rollback();self.assertEqual(storage_derived.derive_trace(db,'1'.zfill(32)),first)
        self.assertEqual(len(db.functions),2)
        db.commit()
        other=self.connection();other.execute('BEGIN IMMEDIATE')
        storage_derived.derive_trace(other,'2'.zfill(32));other.rollback()
        self.assertFalse(other.execute("SELECT 1 FROM sqlite_temp_master WHERE name='derived_usage'").fetchone())
        storage_derived.derive_trace(other,'2'.zfill(32))
        self.assertEqual(len(other.functions),4)
        self.assertEqual(other.execute('SELECT COUNT(*) FROM temp.derived_usage').fetchone()[0],0)


if __name__=='__main__':unittest.main()
