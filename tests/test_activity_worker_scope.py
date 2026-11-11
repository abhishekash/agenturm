"""Actual stored shared traces retain the recorded owner of each activity row."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store, normalize_native
from observatory.workflow_evidence import ExecutionScope, ReviewHistory


class SharedTraceActivityTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'shared.db');self.addCleanup(self.store.close)
        self.trace='a'*32
        self.parent=canonical_agent_id('SDK','svc','parent')
        self.child=canonical_agent_id('SDK','svc','child')

    def install(self, *, child_session='shared'):
        def attrs(native,session):
            result={'agent.id':native,'agent.runtime':'SDK','agent.namespace':'svc','session.id':session}
            if native=='child':result['agent.parent_id']='parent'
            return result
        def activity(subject,**extra):
            return dict(name='agent.activity',time_unix_nano=4,attributes={
                'agent.activity.domain':'shell','agent.activity.action':'execute',
                'agent.activity.subject':subject,'agent.activity.result':'success',**extra})
        records=[normalize_native(dict(trace_id=self.trace,span_id='1'*16,parent_span_id=None,
            name='agent.run',start_unix_nano=1,end_unix_nano=5,status='OK',attributes=attrs('parent','shared'),
            events=[activity('pytest parent'),activity('pytest explicit child',**{'agent.id':'child','session.id':child_session})]),'svc'),
            normalize_native(dict(trace_id=self.trace,span_id='2'*16,parent_span_id='1'*16,
            name='agent.run',start_unix_nano=2,end_unix_nano=6,status='OK',attributes=attrs('child',child_session),
            events=[activity('pytest child'),dict(name='github.copilot.session.compaction_complete',time_unix_nano=5,
                attributes={'github.copilot.success':True})]),'svc'),
            normalize_native(dict(trace_id=self.trace,span_id='3'*16,parent_span_id='2'*16,
            name='test tool',start_unix_nano=3,end_unix_nano=7,status='OK',attributes={
                **attrs('child',child_session),'gen_ai.tool.name':'pytest inferred child','gen_ai.tool.type':'shell'}),'svc')]
        self.store.ingest(records)
        def log(native,session,subject,stamp):
            return dict(trace_id=self.trace,span_id=None,time_unix_nano=stamp,service='svc',severity_number=9,
                severity_text='INFO',event_name='agent.activity',body='recorded output',resource_attributes={},scope_name='fixture',
                attributes={**attrs(native,session),'agent.activity.domain':'shell','agent.activity.action':'execute',
                    'agent.activity.subject':subject,'agent.activity.result':'success'})
        self.store.ingest_logs([log('child',child_session,'pytest child log',8),
                                log('parent','parent-other-session','pytest parent other session',9)])

    def test_malformed_explicit_event_agent_is_never_relabelled_as_span_owner(self):
        self.install()
        malformed=normalize_native(dict(trace_id=self.trace,span_id='4'*16,parent_span_id='1'*16,
            name='parent container',start_unix_nano=3,end_unix_nano=8,status='OK',
            attributes={'agent.id':'parent','agent.runtime':'SDK','agent.namespace':'svc','session.id':'shared'},
            events=[dict(name='agent.activity',time_unix_nano=7,attributes={
                'agent.id':{'invalid':'id'},'agent.activity.domain':'shell','agent.activity.action':'execute',
                'agent.activity.subject':'pytest UNKNOWN_OWNER','agent.activity.result':'success'})]),'svc')
        self.store.ingest([malformed])
        all_rows=self.store.activity_page(trace_id=self.trace)['activity']
        unknown=next(e for e in all_rows if e['subject']=='pytest UNKNOWN_OWNER')
        self.assertIsNone(unknown['agent_id'])
        for owner in (self.parent,self.child):
            rows=self.store.activity_page(agent_id=owner,trace_id=self.trace)['activity']
            self.assertNotIn('pytest UNKNOWN_OWNER',{e['subject'] for e in rows})

    def test_shared_trace_worker_queries_return_their_own_canonical_evidence(self):
        self.install()
        parent=self.store.activity_page(agent_id=self.parent,session_id='shared',trace_id=self.trace)['activity']
        child=self.store.activity_page(agent_id=self.child,session_id='shared',trace_id=self.trace)['activity']
        self.assertEqual([e['subject'] for e in parent],['pytest parent'])
        self.assertEqual({e['agent_id'] for e in child},{self.child})
        self.assertTrue({'pytest child','pytest inferred child','pytest explicit child','pytest child log'} <= {e['subject'] for e in child})
        subtree=self.store.activity_page(agent_id=self.parent,subtree=True,trace_id=self.trace)['activity']
        self.assertEqual({e['agent_id'] for e in subtree},{self.parent,self.child})
        scoped=ExecutionScope(self.store.receiver_id,'svc',self.child,'shared',self.trace,kind='run')
        history=ReviewHistory(scoped);history.accept(self.store.activity_page(agent_id=self.child,session_id='shared',trace_id=self.trace))
        self.assertEqual(len(history.projection({})['checks']),4)
        other=self.store.activity_page(agent_id=self.parent,session_id='parent-other-session',trace_id=self.trace)['activity']
        self.assertEqual([e['subject'] for e in other],['pytest parent other session'])

    def test_worker_owned_sessions_win_over_trace_owner_session(self):
        self.install(child_session='child-session')
        child=self.store.activity_page(agent_id=self.child,session_id='child-session',trace_id=self.trace)['activity']
        self.assertTrue(child)
        self.assertEqual({e['session_id'] for e in child},{'child-session'})
        self.assertEqual({e['agent_id'] for e in child},{self.child})
        self.assertIn('pytest child log',{e['subject'] for e in child})
        self.assertEqual(self.store.activity_page(agent_id=self.child,session_id='shared',trace_id=self.trace)['activity'],[])

    def test_canonical_workers_without_session_never_inherit_trace_session(self):
        self.install(child_session=None)
        rows=self.store.activity_page(agent_id=self.child,trace_id=self.trace)['activity']
        self.assertTrue(rows)
        self.assertTrue(all(row['session_id'] is None for row in rows))
        self.assertEqual(self.store.activity_page(agent_id=self.child,session_id='shared',trace_id=self.trace)['activity'],[])

    def test_canonical_log_resource_session_is_its_own_session(self):
        self.install(child_session='child-session')
        with self.store._connect() as db:
            row=db.execute("SELECT digest,attributes FROM logs WHERE json_extract(attributes,'$.\"agent.activity.subject\"')='pytest child log'").fetchone()
            attrs=json.loads(row['attributes']);attrs.pop('session.id')
            db.execute('UPDATE logs SET attributes=?,resource_attributes=? WHERE digest=?',
                       (json.dumps(attrs),json.dumps({'session.id':'resource-only-session'}),row['digest']))
        rows=self.store.activity_page(agent_id=self.child,session_id='resource-only-session',trace_id=self.trace)['activity']
        self.assertEqual([row['subject'] for row in rows],['pytest child log'])
        self.assertEqual(rows[0]['session_id'],'resource-only-session')
        other=self.store.activity_page(agent_id=self.child,session_id='child-session',trace_id=self.trace)['activity']
        self.assertNotIn('pytest child log',{row['subject'] for row in other})

    def test_explicit_unknown_session_stays_unknown_but_missing_legacy_field_uses_own_span(self):
        self.install(child_session='child-session')
        with self.store._connect() as db:
            activity=json.loads(db.execute('SELECT activity_json FROM trace_summaries WHERE trace_id=?',(self.trace,)).fetchone()[0])
            selected=next(row for row in activity if row['subject']=='pytest child')
            selected['session_id']=None
            db.execute('UPDATE trace_summaries SET activity_json=? WHERE trace_id=?',(json.dumps(activity),self.trace))
        rows=self.store.activity_page(agent_id=self.child,trace_id=self.trace)['activity']
        self.assertIsNone(next(row for row in rows if row['subject']=='pytest child')['session_id'])
        rows=self.store.activity_page(agent_id=self.child,session_id='child-session',trace_id=self.trace)['activity']
        self.assertNotIn('pytest child',{row['subject'] for row in rows})
        selected.pop('session_id')
        selected.pop('activity_scope_version',None)
        with self.store._connect() as db:
            db.execute('UPDATE trace_summaries SET activity_json=? WHERE trace_id=?',(json.dumps(activity),self.trace))
        rows=self.store.activity_page(agent_id=self.child,session_id='child-session',trace_id=self.trace)['activity']
        self.assertIn('pytest child',{row['subject'] for row in rows})

    def test_current_canonical_cache_does_not_parse_owner_span_payloads(self):
        from contextlib import contextmanager
        import sqlite3
        from unittest.mock import patch
        self.install(child_session='child-session')
        marker='retained-owner-payload-not-needed-for-current-cache'
        with self.store._connect() as db:
            for row in db.execute('SELECT span_id,attributes FROM spans WHERE trace_id=?',(self.trace,)).fetchall():
                attrs=json.loads(row['attributes']);attrs['large.retained.payload']=marker+'x'*64000
                db.execute('UPDATE spans SET attributes=? WHERE trace_id=? AND span_id=?',
                           (json.dumps(attrs),self.trace,row['span_id']))
            activity=json.loads(db.execute('SELECT activity_json FROM trace_summaries WHERE trace_id=?',(self.trace,)).fetchone()[0])
            # An explicit unknown in a current cache must also remain unknown;
            # it cannot cause source payload decoding or infer the span owner.
            selected=next(row for row in activity if row['subject']=='pytest child')
            selected['session_id']=None
            db.execute('UPDATE trace_summaries SET activity_json=? WHERE trace_id=?',(json.dumps(activity),self.trace))
        delegate=sqlite3.connect(':memory:');self.addCleanup(delegate.close)
        parsed_owner=[]
        def counted_extract(value,*paths):
            if isinstance(value,str) and marker in value:parsed_owner.append(value[:80])
            placeholders=','.join('?' for _ in range(len(paths)+1))
            return delegate.execute('SELECT json_extract('+placeholders+')',(value,*paths)).fetchone()[0]
        original=self.store._connect
        @contextmanager
        def connection():
            with original() as db:
                db.create_function('json_extract',-1,counted_extract,deterministic=True)
                yield db
        with patch.object(self.store,'_connect',connection):
            rows=self.store.activity_page(trace_id=self.trace,limit=2)['activity']
        self.assertEqual(len(rows),2)
        self.assertEqual(parsed_owner,[],'Current cache metadata must precede LIMIT without parsing retained owner payloads')

    def test_old_cached_span_rows_use_own_span_identity_without_trace_owner_substitution(self):
        self.install(child_session='child-session')
        with self.store._connect() as db:
            row=db.execute('SELECT activity_json FROM trace_summaries WHERE trace_id=?',(self.trace,)).fetchone()
            activity=json.loads(row[0])
            for event in activity:
                if event['source']!='span':continue
                event.pop('activity_scope_version',None)
                event.pop('session_id',None)
                # Earlier caches did not inherit span identity, and explicit
                # event native IDs were raw. Preserve explicit overrides so
                # they cannot be relabelled as the parent span's owner.
                if event['subject']=='pytest explicit child':event['agent_id']='child'
                elif event['span_id']=='2'*16:event['agent_id']='child'
                else:event.pop('agent_id',None)
            db.execute('UPDATE trace_summaries SET activity_json=? WHERE trace_id=?',(json.dumps(activity),self.trace))
        child=self.store.activity_page(agent_id=self.child,session_id='child-session',trace_id=self.trace)['activity']
        self.assertTrue({'pytest child','pytest inferred child','pytest child log'} <= {e['subject'] for e in child})
        parent=self.store.activity_page(agent_id=self.parent,session_id='shared',trace_id=self.trace)['activity']
        self.assertEqual([e['subject'] for e in parent],['pytest parent'])
        self.assertEqual({e['agent_id'] for e in child},{self.child})
