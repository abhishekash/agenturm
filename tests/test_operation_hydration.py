"""Raw semantic/native evidence survives indexed paging without owner borrowing."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store,normalize_native
from observatory.operation_facts import operation_facts,operation_key


class OperationHydrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.path=Path(self.tmp.name)/'source.db'
        self.store=Store(self.path);self.addCleanup(lambda:self.store.close())
        self.trace='a'*32;self.span='1'*16
        self.parent=canonical_agent_id('SDK','fixture','parent')
        self.child=canonical_agent_id('SDK','fixture','child')

    def ingest(self,**changes):
        attrs={'agent.id':self.parent,'session.id':'parent-session','tool.name':'Bash',
            'gen_ai.tool.type':'shell','gen_ai.operation.name':'execute_tool','tool.action':'execute',
            'process.command_line':'pytest tests','process.working_directory':'/work','process.exit.code':0}
        attrs.update(changes.pop('attributes',{}))
        row=dict(trace_id=self.trace,span_id=self.span,name='Bash',start_unix_nano=1,end_unix_nano=2,
            status='OK',attributes=attrs,events=[])
        row.update(changes)
        self.store.ingest([normalize_native(row,'sdk')])

    def own(self,owner=None,session=None):
        return self.store.activity_page(agent_id=owner or self.parent,session_id=session or 'parent-session')['activity']

    def test_empty_old_presentation_still_hydrates_semantic_command_and_terminal_zero(self):
        self.ingest()
        rows=self.own();self.assertEqual(len(rows),1)
        facts=operation_facts(rows[0])
        self.assertEqual(facts['command'],'pytest tests')
        self.assertEqual(facts['exit_code'],0)
        self.assertEqual(facts['result'],'passed')
        self.assertTrue(operation_key(rows[0]))
        # Emulate an old index with no typed-operation presence pointer.
        with self.store._connect() as db:
            db.execute("UPDATE activity_index_v1 SET payload=json_remove(json_set(payload,'$.detail_present',0),"
                       "'$.operation_present','$.detail_role')")
            self.assertFalse(db.execute('SELECT 1 FROM activity_dirty_v1').fetchone())
        from observatory.core import _activity_for
        with patch('observatory.core._activity_for',wraps=_activity_for) as hydrate:
            rows=self.own()
            self.assertEqual(hydrate.call_count,1)
        self.assertEqual(operation_facts(rows[0])['exit_code'],0)

    def test_foreign_or_unqualified_linked_log_cannot_hide_typed_span_or_borrow_exit(self):
        self.ingest()
        self.store.ingest_logs([dict(trace_id=self.trace,span_id=self.span,time_unix_nano=3,
            service='sdk',event_name='agent.activity',body='{"command":"false","exit_code":9}',
            scope_name='fixture',resource_attributes={},severity_number=17,severity_text='ERROR',
            attributes={'agent.id':self.child,'session.id':'child-session','agent.activity.domain':'shell',
                'agent.activity.action':'execute','agent.activity.subject':'Other worker output',
                'agent.activity.result':'error'})])
        parent=self.own();child=self.own(self.child,'child-session')
        self.assertEqual(len(parent),1);self.assertEqual(len(child),1)
        self.assertEqual(operation_facts(parent[0])['exit_code'],0)
        self.assertIsNone(operation_facts(child[0])['exit_code'])
        self.assertNotEqual(operation_facts(child[0])['command'],'false')
        # Reopen as if an older receiver had omitted that parent index row.
        with self.store._connect() as db:
            db.execute('DELETE FROM activity_index_v1 WHERE agent_id=?',(self.parent,))
            db.execute("DELETE FROM schema_meta WHERE key='activity_operation_facts_v2'")
        self.store.close();self.store=Store(self.path)
        self.assertEqual(operation_facts(self.own()[0])['exit_code'],0)
        self.assertEqual(self.own(self.child,'child-session')[0]['agent_id'],self.child)

    def test_exact_event_owner_gets_its_exception_message_without_parent_tool_fields(self):
        exception=dict(name='exception',time_unix_nano=3,attributes={'agent.id':self.child,
            'session.id':'child-session','exception.type':'TimeoutError',
            'exception.message':'Connection timed out after 30 seconds'})
        self.ingest(events=[exception])
        parent=self.own();child=self.own(self.child,'child-session')
        self.assertEqual(len(parent),1);self.assertEqual(len(child),1)
        facts=operation_facts(child[0])
        self.assertIn('Connection timed out',facts['output'])
        self.assertIsNone(facts['command'])
        self.assertIsNone(facts['exit_code'])
        self.assertIsNone(facts['operation_fingerprint'])
        self.assertTrue(facts['error_fingerprint'])
        self.assertTrue(child[0]['source_id'].endswith(':event:0'))

    def test_clipping_one_side_keeps_other_side_and_terminal_metadata(self):
        self.ingest(attributes={'output.value':'x'*1000000})
        facts=operation_facts(self.own()[0])
        self.assertTrue(facts['operation_fingerprint'])
        self.assertTrue(facts['output_clipped'])
        self.assertEqual(facts['exit_code'],0)
        self.ingest(attributes={'input.value':{'extra':'x'*1000000},'output.value':'Useful recorded output'})
        row=self.own()[0];facts=operation_facts(row)
        self.assertIsNone(facts['operation_fingerprint'])
        self.assertIn('Useful recorded output',facts['output'])
        self.assertEqual(facts['exit_code'],0)
        self.assertLessEqual(len(row['detail'].encode()),8000)

    def test_index_payload_retains_only_role_and_presence_not_arguments_or_output(self):
        self.ingest(attributes={'input.value':{'cmd':'pytest tests','cwd':'/work','note':'PRIVATE_ARGUMENT'},
                                'output.value':'PRIVATE_OUTPUT'})
        self.own()
        with self.store._connect() as db:
            payload=db.execute('SELECT payload FROM activity_index_v1').fetchone()[0]
        self.assertNotIn('PRIVATE_ARGUMENT',payload)
        self.assertNotIn('PRIVATE_OUTPUT',payload)
        self.assertTrue(json.loads(payload)['operation_present'])
        self.assertEqual(json.loads(payload)['detail'],'')

    def test_upgrade_reindexes_old_success_label_using_typed_terminal_failure(self):
        self.ingest(status='UNSET',attributes={'process.exit.code':1,
            'process.command_line':'python migrate.py','output.value':'Migration rejected'})
        with self.store._connect() as db:
            db.execute("UPDATE activity_index_v1 SET result='unknown',payload=json_set(payload,'$.result','unknown')")
            db.execute("DELETE FROM schema_meta WHERE key='activity_operation_facts_v2'")
        self.store.close();self.store=Store(self.path)
        rows=self.store.activity_page(agent_id=self.parent,session_id='parent-session',errors_only=True)['activity']
        self.assertEqual(len(rows),1)
        self.assertEqual(rows[0]['result'],'error')
        self.assertEqual(operation_facts(rows[0])['exit_code'],1)
        from observatory.workflow_evidence import ExecutionScope,ReviewHistory
        scope=ExecutionScope(self.store.receiver_id,'sdk',self.parent,'parent-session',self.trace,kind='run')
        history=ReviewHistory(scope)
        history.accept(self.store.activity_page(agent_id=self.parent,session_id='parent-session',trace_id=self.trace))
        self.assertEqual(len(history.projection({})['errors']),1)
        summary=next(item for item in self.store.sessions() if item['agent_id']==self.parent)
        self.assertEqual(len(summary['recent_errors']),1)

    def test_session_reminders_observe_retry_across_steps_only_with_exact_turn(self):
        self.ingest(status='UNSET',attributes={'process.exit.code':1,'agent.turn.id':'turn',
            'process.command_line':'python migrate.py','output.value':'Migration rejected'})
        failed=next(item for item in self.store.sessions() if item['agent_id']==self.parent)
        self.assertEqual(len(failed['recent_errors']),1)
        self.ingest(trace_id='b'*32,start_unix_nano=3,end_unix_nano=4,
            attributes={'agent.turn.id':'turn','process.command_line':'python migrate.py'})
        summary=next(item for item in self.store.sessions() if item['agent_id']==self.parent)
        self.assertEqual(summary['recent_errors'],[])
        self.ingest(trace_id='b'*32,start_unix_nano=3,end_unix_nano=4,
            attributes={'agent.turn.id':'other-turn','process.command_line':'python migrate.py'})
        summary=next(item for item in self.store.sessions() if item['agent_id']==self.parent)
        self.assertEqual(len(summary['recent_errors']),1)

    def test_native_emit_marks_omitted_container_values_unmatchable(self):
        from observatory.local_agents import Session
        from observatory.local_activity import emit
        path=Path(self.tmp.name)/'codex.jsonl';path.touch()
        session=Session(self.store,path,'Codex',capture_details=True)
        for arguments in ([*range(60),'PRIVATE_61ST_ARGUMENT'],
                          {**{str(i):i for i in range(60)},'61':'PRIVATE_61ST_ARGUMENT'}):
            log=emit(session,{'ordinal':1},3,'mcp','call','test',
                {'arguments':arguments,'result':{'isError':True,'message':'Call failed'}},'error',
                {'mcp.server.name':'fixture','mcp.tool.name':'test'})
            self.store.ingest_logs([log])
            row=self.store.activity_page(agent_id=session.agent_id)['activity'][0]
            facts=operation_facts(row)
            self.assertIsNone(facts['operation_fingerprint'])
            self.assertNotIn('PRIVATE_61ST_ARGUMENT',row['detail'])

    def test_mcp_selected_output_preserves_falsy_presence_and_strict_error_flag(self):
        from observatory.core import _tool_activity,_activity_row
        for value in (None,False,0,{},[],{'isError':False},{'isError':1},{'isError':'true'}):
            attrs={'rpc.system':'mcp','mcp.server.name':'fixture','mcp.tool.name':'lookup',
                'output.value':value,'gen_ai.tool.call.result':{'isError':True,'message':'Ignored alias'}}
            span={'span_id':self.span,'service':'sdk','status':'UNSET','attributes':attrs}
            facts=operation_facts(_tool_activity(span))
            log=_activity_row({**attrs,'agent.activity.domain':'mcp','agent.activity.result':'unknown'},
                2,'log','source',self.span,'sdk')
            self.assertEqual(facts['result'],'unknown',value)
            self.assertEqual(facts['output'],json.dumps(value,sort_keys=True))
            self.assertEqual(facts['output_origin'],'output.value')
            self.assertNotIn('Ignored alias',facts['output'])
            self.assertIsNone(facts['error_fingerprint'])
            self.assertEqual(facts['result'],operation_facts(log)['result'])
            native=_activity_row({**attrs,'agent.activity.domain':'mcp','agent.activity.result':'unknown',
                'observatory.data_source':'local-session-log','observatory.operation.capture_version':1},
                2,'log','native',self.span,'sdk',body={'result':{'isError':True,'message':'Ignored body'}})
            self.assertEqual(operation_facts(native)['result'],'unknown',value)
            self.assertNotIn('Ignored body',operation_facts(native)['output'])
            self.assertTrue(facts['output_reported'])
            span['status']='ERROR'
            self.assertEqual(operation_facts(_tool_activity(span))['result'],'failed')
        attrs['output.value']={'isError':True,'message':'Selected failure'}
        span['status']='UNSET'
        self.assertEqual(operation_facts(_tool_activity(span))['result'],'failed')

    def test_event_identity_family_override_does_not_inherit_parent_execution(self):
        from observatory.core import _event_activity_identity,_agent_id_from_attributes
        parent={'service':'sdk','attributes':{'agent.id':'shared','agent.runtime':'SDK',
            'agent.namespace':'fixture','session.id':'parent-session','agent.turn.id':'parent-turn'},
            'resource_attributes':{}}
        for key,value in (('agent.namespace','other'),('agent.runtime','Other')):
            attrs,resource=_event_activity_identity(parent,{key:value})
            self.assertNotEqual(_agent_id_from_attributes(attrs,resource,'sdk'),
                _agent_id_from_attributes(parent['attributes'],{},'sdk'))
            self.assertNotIn('session.id',attrs)
            self.assertNotIn('agent.turn.id',attrs)
        attrs,_=_event_activity_identity(parent,{'agent.worker_id':'child'})
        self.assertEqual(attrs['session.id'],'parent-session')
        for claim in ({'session.id':False},{'session.id':'one','gen_ai.conversation.id':'two'}):
            attrs,_=_event_activity_identity(parent,claim)
            self.assertNotIn('session.id',attrs)
            self.assertNotIn('agent.turn.id',attrs)


if __name__=='__main__':unittest.main()
