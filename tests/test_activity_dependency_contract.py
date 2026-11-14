"""Generated Activity dependency proofs; no receiver/workload benchmark.

Certified HTTP Activity reads isolate their exact dependencies while preserving
complete serialized Store rows/cursors/scopes and strict scoped readiness.
"""
from contextlib import closing,contextmanager
from copy import deepcopy
import json
import os
import sqlite3
import sys
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request,urlopen

from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store,normalize_native
from observatory.server import BoundedHTTPServer,handler_for

TRACE='a'*32
SOURCE='c'*32


class ActivityDependencyContractTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.path=Path(self.tmp.name)/'activity.db'
        self.store=Store(self.path);self.addCleanup(self.store.close)
        self.parent=canonical_agent_id('SDK','activity-proof','parent')
        self.child=canonical_agent_id('SDK','activity-proof','child')
        stamp=time.time_ns()-10**9
        def attrs(name):return {'agent.id':name,'agent.runtime':'SDK',
            'agent.namespace':'activity-proof','session.id':'shared',
            **({'agent.parent_id':'parent'}if name=='child'else{})}
        self.records=[]
        for number,name in ((1,'parent'),(2,'child')):
            events=[dict(name='agent.activity',time_unix_nano=stamp+number*100+offset,
                attributes={'agent.activity.domain':domain,'agent.activity.action':action,
                    'agent.activity.subject':subject,'agent.activity.detail':'GENERATED_CAPTURED_DETAIL_'+name,
                    'agent.activity.result':result})for offset,domain,action,subject,result in (
                    (1,'shell','execute','pytest '+name,'success'),
                    (2,'file','edit','src/'+name+'.py','success'),
                    (3,'message','user','Validate '+name,'success'))]
            self.records.append(normalize_native(dict(trace_id=TRACE,span_id=f'{number:016x}',
                parent_span_id=None if number==1 else f'{1:016x}',name='agent.run',
                start_unix_nano=stamp+number,end_unix_nano=stamp+number*100+5,status='OK',
                attributes=attrs(name),resource_attributes={'observatory.source_id':SOURCE},events=events),'fixture'))
        sibling=deepcopy(self.records[0]);sibling.update(trace_id='b'*32,span_id=f'{3:016x}',parent_span_id=None)
        sibling['resource_attributes']['observatory.source_id']='d'*32
        for event in sibling['events']:event['attributes']['agent.activity.detail']='GENERATED_SIBLING_DETAIL'
        self.store.ingest([*self.records,sibling])
        self.assertTrue(self.store.index_status()['ready'])
        self.handler=handler_for(self.store,'generated-activity-token',quiet=True)
        self.server_errors=[]
        owner=self
        class RecordingServer(BoundedHTTPServer):
            def handle_error(self,*args):owner.server_errors.append(sys.exc_info()[1])
        self.server=RecordingServer(('127.0.0.1',0),self.handler)
        self.thread=threading.Thread(target=self.server.serve_forever);self.thread.start()
        self.addCleanup(self.stop_server)

    def stop_server(self):
        self.server.shutdown();self.server.server_close();self.thread.join(3)
        self.assertFalse(self.thread.is_alive())
        self.assertEqual(self.server_errors,[],'HTTP handler exceptions must not be silently accepted')

    def get(self,params=None,*,token='generated-activity-token'):
        path='/api/activity'+('?' +urlencode(params)if params else'')
        req=Request(f'http://127.0.0.1:{self.server.server_port}'+path,
            headers={'Authorization':'Bearer '+token})
        try:
            with urlopen(req,timeout=3)as response:
                return response.status,json.load(response),dict(response.headers)
        except HTTPError as exc:
            with closing(exc):return exc.code,json.load(exc),dict(exc.headers)

    def pages(self,kwargs):
        result=[];cursor=None
        while True:
            params={**kwargs,**({'cursor':cursor}if cursor else{})}
            page=self.store.activity_page(**params);result.append(page)
            cursor=page['next_cursor']
            if not cursor:return result
            self.assertLess(len(result),20,'Generated fixture cursor did not terminate')

    def queue(self,kind):
        with self.store._connect()as db:
            if kind=='findings':db.execute('INSERT OR IGNORE INTO finding_dirty_v1 VALUES(?)',(TRACE,))
            else:db.execute('INSERT OR IGNORE INTO '+kind+"_dirty_v1 VALUES('log','generated-unrelated')")

    @contextmanager
    def queued(self,kind):
        self.queue(kind)
        try:yield
        finally:
            with self.store._connect()as db:
                for table in ('finding_dirty_v1','investigation_dirty_v1','native_lifecycle_dirty_v1'):
                    db.execute('DELETE FROM '+table)

    def test_only_unrelated_queues_are_ignored_and_complete_activity_payloads_match(self):
        revision=self.store.hierarchy_revision()
        selectors=[dict(limit=2),dict(limit=2,trace_id=TRACE),dict(limit=2,service='fixture',domain='shell'),
            dict(limit=2,query='src/'),dict(limit=2,session_id='shared'),
            dict(limit=2,domain='file',action='edit'),dict(limit=2,errors_only=False,include_reviews=False),
            dict(limit=2,agent_id=self.child,session_id='shared',trace_id=TRACE),
            dict(limit=2,agent_id=self.parent,subtree=True,scope_revision=revision,review=True)]
        before=[self.pages(kwargs)for kwargs in selectors]
        self.assertGreater(len(before[0]),1)
        all_rows=[row for page in before[0]for row in page['activity']]
        self.assertEqual(len(all_rows),9)
        self.assertEqual(len({row['id']for row in all_rows}),9)
        for kind in ('findings','investigation','native_lifecycle'):
            with self.subTest(queue=kind),self.queued(kind):
                self.assertFalse(self.store.index_status()['ready'])
                self.assertEqual([self.pages(kwargs)for kwargs in selectors],before,
                    'Every complete row, cursor, context, receiver and scope must remain equal')
                with self.store._read()as db:
                    self.assertFalse(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
                for kwargs,expected_pages in zip(selectors,before):
                    wire={('q'if key=='query'else key):('1'if value is True else'0'if value is False else value)for key,value in kwargs.items()}
                    code,body,_=self.get(wire)
                    if {'agent_id','subtree','scope_revision','trace_id','turn_id','review'}.intersection(kwargs):
                        self.assertEqual(code,503);self.assertNotIn('activity',body)
                    else:self.assertEqual((code,body),(200,expected_pages[0]))
        for kwargs,pages in zip(selectors,before):
            cursor=None
            for expected in pages:
                wire={('q'if key=='query'else key):('1'if value is True else'0'if value is False else value)for key,value in kwargs.items()}
                if cursor:wire['cursor']=cursor
                code,actual,_=self.get(wire);self.assertEqual(code,200,actual)
                self.assertEqual(actual,expected)
                cursor=expected['next_cursor']

    def test_real_source_dirty_is_never_served_from_the_old_cached_page(self):
        params=dict(limit=2,agent_id=self.child,trace_id=TRACE)
        before=self.store.activity_page(**params)
        self.assertEqual(self.get(params)[:2],(200,before))
        with self.store._connect()as db:
            events=json.loads(db.execute('SELECT events FROM spans WHERE span_id=?',(f'{2:016x}',)).fetchone()[0])
            events[2]['attributes']['agent.activity.subject']='Changed top-page child request'
            db.execute('UPDATE spans SET events=? WHERE span_id=?',(json.dumps(events),f'{2:016x}'))
            self.assertTrue(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
        self.assertEqual(self.get(params)[0],503)
        # The documented bounded Store read repairs Activity only. Findings
        # remains dirty; no test clears that queue before this proof.
        current=self.store.activity_page(**params)
        self.assertNotEqual(current,before)
        with self.store._read()as db:
            self.assertFalse(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
            self.assertTrue(db.execute('SELECT 1 FROM finding_dirty_v1').fetchone())
        self.assertEqual(self.get(params)[0],503)
        self.store._drain_external_activity()
        self.assertEqual(self.get(params)[:2],(200,current))

    def test_initial_fatal_stopping_and_missing_marker_are_explicit_controls(self):
        baseline=self.store.activity_page(limit=2)
        for state in (dict(ready=False,stage='Preparing history'),dict(ready=False,stage='History migration failed',error='generated fatal')):
            with self.subTest(state=state),patch.object(self.store,'migration_status',state):
                code,body,_=self.get(dict(limit=2));self.assertEqual(code,503)
                self.assertNotIn('activity',body)
                self.assertEqual(self.store.activity_page(limit=2),baseline)
        # Initial-source proof is mandatory even with a warmed response cache.
        with patch.object(self.store,'_derived_stopping',True):
            self.assertEqual(self.get(dict(limit=2))[0],503)
        with self.store._connect()as db:db.execute("DELETE FROM schema_meta WHERE key='trace_derived_v15'")
        self.assertEqual(self.get(dict(limit=2))[0],503)
        self.assertEqual(self.get(dict(limit=2),token='wrong')[0],401)

    def test_ready_probe_source_race_is_503_without_http_repair(self):
        params=dict(limit=2,agent_id=self.child,trace_id=TRACE)
        baseline=self.store.activity_page(**params);original=self.store.index_status;changed=[]
        def status():
            result=original()
            if not changed:
                with self.store._connect()as db:
                    events=json.loads(db.execute('SELECT events FROM spans WHERE span_id=?',(f'{2:016x}',)).fetchone()[0])
                    events[2]['attributes']['agent.activity.subject']='Changed exact child request'
                    db.execute('UPDATE spans SET events=? WHERE span_id=?',(json.dumps(events),f'{2:016x}'))
                changed.append(True)
            return result
        with patch.object(self.store,'index_status',status):code,page,_=self.get(params)
        self.assertEqual(code,503,page)
        self.assertNotIn('activity',page)
        self.assertNotEqual(self.store.activity_page(**params),baseline)

    def test_cached_receiver_and_file_replacement_races_never_publish_old_namespace(self):
        baseline=self.store.activity_page(limit=2);self.assertEqual(self.get(dict(limit=2))[:2],(200,baseline))
        old=self.handler.response_cache.get
        def change_receiver(key):
            cached=old(key);self.assertIsNotNone(cached[1])
            with self.store._connect()as db:db.execute("UPDATE schema_meta SET value=? WHERE key='receiver_id'",('f'*32,))
            # receiver_namespace caches same-file identity; explicit receiver
            # refresh is a production-supported observable identity change.
            self.store.receiver_id='f'*32
            return cached
        with patch.object(self.handler.response_cache,'get',change_receiver):
            code,body,_=self.get(dict(limit=2))
        self.assertEqual(code,409);self.assertNotIn('activity',body)
        with self.store._connect()as db:db.execute("UPDATE schema_meta SET value=? WHERE key='receiver_id'",(baseline['receiver_id'],))
        self.store.receiver_id=baseline['receiver_id']
        self.assertEqual(self.get(dict(limit=2))[0],200)
        with self.store._connect()as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        replacement=Path(self.tmp.name)/'replacement.db'
        other=Store(replacement);other.close()
        self.assertEqual(self.get(dict(limit=2))[0],200)
        def replace_file(key):
            cached=old(key);self.assertIsNotNone(cached[1]);os.replace(replacement,self.path);return cached
        with patch.object(self.handler.response_cache,'get',replace_file):
            code,body,_=self.get(dict(limit=2))
        self.assertEqual(code,409);self.assertNotIn('activity',body)

    def test_capture_revocation_before_cache_read_scrubs_exact_source(self):
        before=self.store.activity_page(limit=500)
        self.assertIn('GENERATED_CAPTURED_DETAIL',json.dumps(before))
        siblings=[row for row in before['activity']if row['trace_id']=='b'*32]
        self.assertTrue(siblings)
        self.assertEqual(self.get(dict(limit=500))[:2],(200,before))
        original=self.store.index_status;changed=[]
        def status():
            result=original()
            if not changed:self.store.redact_internal_source(SOURCE);changed.append(True)
            return result
        with patch.object(self.store,'index_status',status):code,page,_=self.get(dict(limit=500))
        self.assertEqual(code,503,page);self.assertNotIn('activity',page)
        with self.store._read()as db:self.assertTrue(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
        self.store._drain_external_activity()
        code,page,_=self.get(dict(limit=500));self.assertEqual(code,200,page)
        self.assertNotIn('GENERATED_CAPTURED_DETAIL',json.dumps(page))
        self.assertEqual([row for row in page['activity']if row['trace_id']=='b'*32],siblings)
        self.assertEqual(page,self.store.activity_page(limit=500))

    def test_every_cache_hit_certifies_all_semantic_markers_and_runtime_state(self):
        self.assertEqual(self.get(dict(limit=2))[0],200)
        with patch.object(self.store,'activity_page',wraps=self.store.activity_page)as read,patch.object(self.store,'certify_initial_activity',wraps=self.store.certify_initial_activity)as certify:
            self.assertEqual(self.get(dict(limit=2))[0],200)
            self.assertEqual(self.get(dict(limit=2))[0],200)
            self.assertEqual(read.call_count,0);self.assertEqual(certify.call_count,2)
            for marker in ('trace_derived_v15','activity_sources_v2','activity_operation_facts_v2'):
                with self.subTest(marker=marker):
                    with self.store._connect()as db:db.execute('DELETE FROM schema_meta WHERE key=?',(marker,))
                    try:self.assertEqual(self.get(dict(limit=2))[0],503)
                    finally:
                        with self.store._connect()as db:db.execute('INSERT INTO schema_meta VALUES(?,?)',(marker,'1'))
            for key,value in (('migration_status',dict(ready=True,error='generated fatal')),
                    ('last_derived_error','generated failure'),('_derived_stopping',True)):
                with self.subTest(field=key),patch.object(self.store,key,value):
                    self.assertEqual(self.get(dict(limit=2))[0],503)
            self.store._migration_stop.set()
            try:self.assertEqual(self.get(dict(limit=2))[0],503)
            finally:self.store._migration_stop.clear()

    def test_selector_presence_is_strict_including_blank_false_and_duplicate(self):
        self.queue('findings')
        with patch.object(self.handler.response_cache,'get',side_effect=AssertionError('Unrelated dirty used cache')),patch.object(self.handler.response_cache,'put',side_effect=AssertionError('Unrelated dirty cached body')):
            self.assertEqual(self.get(dict(limit=2,session_id='shared'))[0],200)
            for key,value in (('agent_id',''),('subtree','0'),('subtree','false'),
                    ('scope_revision',''),('trace_id',''),('turn_id',''),('review','0')):
                for pairs in ([(key,value)],[(key,value),(key,value)]):
                    with self.subTest(selector=pairs):
                        code,body,_=self.get(pairs);self.assertEqual(code,503);self.assertNotIn('activity',body)

    def test_same_receiver_file_replacement_after_cache_lookup_is_rejected(self):
        self.assertEqual(self.get(dict(limit=2))[0],200)
        replacement=Path(self.tmp.name)/'same-receiver.db'
        with self.store._connect()as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        with closing(sqlite3.connect(self.path))as source,closing(sqlite3.connect(replacement))as target:source.backup(target)
        self.assertEqual(self.get(dict(limit=2))[0],200)
        original=self.handler.response_cache.get
        def replace(key):
            cached=original(key);self.assertIsNotNone(cached[1]);os.replace(replacement,self.path);return cached
        with patch.object(self.handler.response_cache,'get',replace):code,body,_=self.get(dict(limit=2))
        self.assertEqual(code,409);self.assertNotIn('activity',body)

    def test_source_commit_after_read_begin_keeps_one_complete_old_payload(self):
        expected=self.store.activity_page(limit=2)
        original=__import__('observatory.storage_activity',fromlist=['hydrate']).hydrate
        fired=[]
        def hydrate(db,rows):
            self.assertTrue(db.in_transaction)
            self.assertEqual(db.execute('PRAGMA query_only').fetchone()[0],1)
            if not fired:
                fired.append(True)
                with self.store._connect()as writer:
                    events=json.loads(writer.execute('SELECT events FROM spans WHERE span_id=?',(f'{2:016x}',)).fetchone()[0])
                    events[2]['attributes'].update({'agent.activity.subject':'changed after BEGIN','agent.activity.detail':'changed detail after BEGIN'})
                    writer.execute('UPDATE spans SET events=? WHERE span_id=?',(json.dumps(events),f'{2:016x}'))
            return original(db,rows)
        with patch('observatory.storage_activity.hydrate',hydrate):
            code,page,_=self.get(dict(limit=2))
        self.assertEqual(code,200,page);self.assertEqual(page,expected)
        self.assertEqual(fired,[True])
        # A subsequent request sees the committed dirty state, not old cache.
        self.assertEqual(self.get(dict(limit=2))[0],503)
        self.assertNotEqual(self.store.activity_page(limit=2),expected)

    def test_owner_change_invalidates_old_scope_cache_and_old_hierarchy_revision(self):
        params=dict(limit=2,agent_id=self.parent,subtree=True,scope_revision=self.store.hierarchy_revision())
        before=self.store.activity_page(**params);self.assertEqual(self.get({**params,'subtree':'1'})[:2],(200,before))
        with self.store._connect()as db:
            attrs=json.loads(db.execute('SELECT attributes FROM spans WHERE span_id=?',(f'{2:016x}',)).fetchone()[0])
            attrs['agent.id']='foreign';attrs['session.id']='foreign-session';attrs.pop('agent.parent_id')
            db.execute('UPDATE spans SET attributes=? WHERE span_id=?',(json.dumps(attrs),f'{2:016x}'))
        self.store._drain_external_activity()
        current=self.store.activity_page(limit=500,agent_id=self.child,session_id='shared',trace_id=TRACE)
        self.assertFalse(current['activity'])
        # Explicit hierarchy revision changes are rejected, never widened.
        with self.store._connect()as db:self.store._bump_hierarchy_revision(db)
        code,body,_=self.get({**params,'subtree':'1'})
        self.assertEqual(code,400);self.assertNotIn('activity',body)


if __name__=='__main__':unittest.main()
