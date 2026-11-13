"""Shared traces and provider-local call IDs never override source ownership."""
from contextlib import contextmanager
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store, normalize_native

TRACE='a'*32


def record(number,actor,session,*,tool=False,execution=False,output='',parent=None,resources=None,service='svc'):
    attrs={'agent.runtime':'SDK','agent.namespace':service}
    if actor:attrs['agent.id']=actor
    if session:attrs['session.id']=session
    if tool:attrs.update({'mcp.method.name':'tools/call','mcp.server.name':'fixture','mcp.tool.name':'search','output.value':output})
    if execution:attrs.update({'gen_ai.tool.type':'MCPTool','gen_ai.tool.name':'search',
        'gen_ai.tool.call.arguments':json.dumps({'query':'PRIVATE_CHILD_ARGS'}),
        'gen_ai.tool.call.result':json.dumps({'isError':True,'content':[{'type':'text','text':'PRIVATE_CHILD_OUTPUT'}]})})
    return normalize_native(dict(trace_id=TRACE,span_id=f'{number:016x}',parent_span_id=parent,
        name='agent.run' if not(tool or execution) else 'MCP call' if tool else 'execute tool',
        start_unix_nano=number*10,end_unix_nano=number*10+5,status='UNSET',attributes=attrs,
        resource_attributes=resources or {}),service)


class MCPSourceOwnershipTests(unittest.TestCase):
    def test_log_payloads_are_direct_evidence_and_preserve_false_zero_empty_and_null(self):
        logs=[]
        for index,value in enumerate((False,0,{},[],None),1):
            logs.append(dict(trace_id=TRACE,span_id=None,time_unix_nano=index,service='svc',
                severity_number=9,severity_text='INFO',event_name='agent.activity',body='',
                attributes={'session.id':'session','mcp.server.name':'fixture','mcp.tool.name':f'tool-{index}',
                    'mcp.method.name':'tools/call','input.value':json.dumps(value),'output.value':json.dumps(value)},
                resource_attributes={},scope_name='fixture'))
        self.store.ingest_logs(logs)
        calls=self.store.mcp_page(session_id='session')['calls']
        self.assertEqual(len(calls),5)
        by_tool={call['tool']:call for call in calls}
        for index,value in enumerate((False,0,{},[],None),1):
            call=by_tool[f'tool-{index}']
            self.assertEqual(call['arguments'],value);self.assertEqual(call['output'],value)
            self.assertIs(type(call['arguments']),type(value));self.assertIs(type(call['output']),type(value))
            self.assertTrue(call['arguments_reported']);self.assertTrue(call['output_reported'])
            self.assertEqual(call['source'],'session log')

    def test_owned_log_error_flags_agree_between_inventory_and_call_details(self):
        for location in ('attribute','result_body','string_result_body','io_body'):
            for index,value in enumerate((1,'true',False,True),1):
                with self.subTest(location=location,value=value):
                    session=f'{location}-{index}'
                    attrs={'session.id':session,'mcp.server.name':'fixture','mcp.tool.name':'search','mcp.method.name':'tools/call'}
                    result={'isError':value,'content':[{'type':'text','text':'Recorded result'}]}
                    body=''
                    if location=='attribute':attrs['output.value']=json.dumps(result)
                    elif location=='result_body':body={'result':result}
                    elif location=='string_result_body':body={'result':json.dumps(result)}
                    else:attrs['observatory.mcp.io.direction']='result';body=json.dumps(result)
                    self.store.ingest_logs([dict(trace_id=TRACE,span_id=None,time_unix_nano=index,
                        service='svc',severity_number=9,severity_text='INFO',event_name='agent.activity',
                        body=body,attributes=attrs,resource_attributes={},scope_name='fixture')])
                    page=self.store.mcp_page(session_id=session)
                    self.assertEqual(page['summary']['errors'],int(value is True))
                    self.assertEqual(page['calls'][0]['status'],'ERROR' if value is True else 'OK')

    def test_argument_bodies_cannot_become_result_evidence(self):
        for index,body in enumerate(({'isError':True},{'result':{'isError':True}},
                                      {'arguments':{'a':1},'result':{'isError':True}}),1):
            attrs={'session.id':str(index),'mcp.server.name':'fixture','mcp.tool.name':'search',
                   'mcp.method.name':'tools/call','observatory.mcp.io.direction':'arguments',
                   'agent.activity.result':'success'}
            self.store.ingest_logs([dict(trace_id=TRACE,span_id=None,time_unix_nano=index,
                service='svc',severity_number=9,severity_text='INFO',event_name='agent.activity',
                body=json.dumps(body),attributes=attrs,resource_attributes={},scope_name='fixture')])
            page=self.store.mcp_page(session_id=str(index));call=page['calls'][0]
            self.assertEqual(page['summary']['errors'],0);self.assertEqual(call['status'],'OK')
            self.assertEqual(call['arguments'],body);self.assertFalse(call['output_reported'])
            self.assertEqual(call['output'],'')

    def test_chosen_attribute_result_takes_precedence_over_body_result(self):
        self.store.ingest_logs([dict(trace_id=TRACE,span_id=None,time_unix_nano=1,service='svc',
            severity_number=9,severity_text='INFO',event_name='agent.activity',
            body={'result':{'isError':True}},attributes={'session.id':'s','mcp.server.name':'fixture',
                'mcp.tool.name':'search','mcp.method.name':'tools/call','output.value':json.dumps({'isError':False})},
            resource_attributes={},scope_name='fixture')])
        page=self.store.mcp_page(session_id='s')
        self.assertEqual(page['summary']['errors'],0)
        self.assertEqual(page['calls'][0]['status'],'OK')
        self.assertEqual(page['calls'][0]['output'],{'isError':False})

    def test_result_body_cannot_supply_request_arguments_or_nested_error_flag(self):
        output={'arguments':{'RESPONSE_FIELD':1},'result':{'isError':True},'isError':False}
        self.store.ingest_logs([dict(trace_id=TRACE,span_id=None,time_unix_nano=1,service='svc',
            severity_number=9,severity_text='INFO',event_name='agent.activity',body=json.dumps(output),
            attributes={'session.id':'s','mcp.server.name':'fixture','mcp.tool.name':'search',
                'mcp.method.name':'tools/call','observatory.mcp.io.direction':'result'},
            resource_attributes={},scope_name='fixture')])
        page=self.store.mcp_page(session_id='s');call=page['calls'][0]
        self.assertEqual(page['summary']['errors'],0);self.assertEqual(call['status'],'OK')
        self.assertFalse(call['arguments_reported']);self.assertEqual(call['arguments'],'')
        self.assertEqual(call['output'],output)

    def test_long_scoped_call_identity_can_page_without_cursor_truncation(self):
        for escaped in (False,True):
            with self.subTest(escaped=escaped):
                server='\x02'*512 if escaped else 's'*450
                logs=[self.log_call(i,('\x01'*511 if escaped else 'c\\"'*150)+str(i),server=server) for i in range(3)]
                for log in logs:log['attributes']['session.id']='\x03'*512 if escaped else 'session-'+'x'*440
                self.store.ingest_logs(logs)
                calls=[];cursor=None
                while True:
                    page=self.store.mcp_page(limit=1,cursor=cursor,session_id=logs[0]['attributes']['session.id'])
                    calls.extend(page['calls']);cursor=page['next_cursor']
                    if not cursor:break
                self.assertEqual(len(calls),3)
                self.assertEqual(len({call['id'] for call in calls}),3)

    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.store=Store(Path(self.temp.name)/'mcp.db');self.addCleanup(self.store.close)
        self.root=canonical_agent_id('SDK','svc','parent')

    def test_parent_filters_do_not_import_explicit_child_ownership(self):
        child=record(2,'child','child-session',tool=True,output='PRIVATE_CHILD_OUTPUT')
        child['attributes']['agent.parent_id']='parent'
        self.store.ingest([record(1,'parent','parent-session'),child])
        for filters in (dict(agent_id=self.root),dict(session_id='parent-session'),dict(agent_id=self.root,session_id='parent-session')):
            page=self.store.mcp_page(service='svc',**filters)
            self.assertEqual(page['calls'],[])
            self.assertEqual(page['summary']['calls'],0)
        page=self.store.mcp_page(agent_id=self.root,subtree=True)
        self.assertEqual(len(page['calls']),1)
        self.assertEqual(page['calls'][0]['session_id'],'child-session')

    def test_hydration_and_ownership_use_one_snapshot_during_reassignment(self):
        self.store.ingest([record(1,'parent','session-A',tool=True,output='Output A')])
        original=self.store._connect;changed=[]
        class Proxy:
            def __init__(self,db):self.db=db
            def execute(proxy,sql,args=()):
                if ' AS mcp_key' in sql and ' FROM spans WHERE ' in sql and not changed:
                    self.assertTrue(proxy.db.in_transaction)
                    with original() as writer:
                        row=writer.execute('SELECT attributes FROM spans').fetchone()
                        attrs=json.loads(row[0]);attrs.update({'agent.id':'other','session.id':'session-B','output.value':'PRIVATE_B_OUTPUT'})
                        writer.execute('UPDATE spans SET attributes=?,agent_id=?',(json.dumps(attrs),canonical_agent_id('SDK','svc','other')))
                    changed.append(True)
                return proxy.db.execute(sql,args)
        @contextmanager
        def connection():
            with original() as db:yield Proxy(db)
        with patch.object(self.store,'_connect',connection):
            page=self.store.mcp_page(agent_id=self.root,session_id='session-A')
        self.assertTrue(changed)
        self.assertEqual(page['calls'][0]['output'],'Output A')
        self.assertNotIn('PRIVATE_B_OUTPUT',json.dumps(page))
        self.assertIn('PRIVATE_B_OUTPUT',json.dumps(self.store.mcp_page(session_id='session-B')))

    def test_execution_enrichment_and_error_inventory_cannot_cross_workers(self):
        self.store.ingest([record(1,'parent','parent-session',tool=True),record(2,'child','child-session',execution=True)])
        for page in (self.store.mcp_page(),self.store.mcp_page(agent_id=self.root,session_id='parent-session')):
            self.assertEqual(page['summary']['errors'],0)
            self.assertEqual(page['calls'][0]['arguments'],'')
            self.assertNotIn('PRIVATE_CHILD',json.dumps(page))
            self.assertNotEqual(page['calls'][0]['status'],'ERROR')

    def test_ambiguous_repeated_tools_require_explicit_linkage(self):
        self.store.ingest([record(1,'parent','s',tool=True),record(2,'parent','s',tool=True),record(3,'parent','s',execution=True)])
        page=self.store.mcp_page()
        self.assertEqual(page['summary']['errors'],0)
        self.assertTrue(all(not call['arguments'] for call in page['calls']))
        linked=record(3,'parent','s',execution=True,parent=f'{2:016x}')
        self.store.ingest([linked])
        page=self.store.mcp_page();by_span={call['span_id']:call for call in page['calls']}
        self.assertEqual(page['summary']['errors'],1)
        self.assertEqual(by_span[f'{2:016x}']['execution_basis'],'explicit span linkage')
        self.assertTrue(by_span[f'{2:016x}']['arguments'])
        self.assertFalse(by_span[f'{1:016x}']['arguments'])

    def test_resource_session_identity_is_returned_without_trace_owner_fallback(self):
        self.store.ingest([record(1,'parent','root-session'),record(2,'child',None,tool=True,
            resources={'session.id':'resource-session'},output='Resource output')])
        call,=self.store.mcp_page(session_id='resource-session')['calls']
        self.assertEqual(call['session_id'],'resource-session')
        self.assertEqual(self.store.mcp_page(session_id='root-session')['calls'],[])
        invalid=record(3,'child',None,tool=True,resources={'session.id':{'wrong':'resource-session'}})
        self.store.ingest([invalid])
        self.assertEqual(len(self.store.mcp_page(session_id='resource-session')['calls']),1)

    def test_provider_local_call_ids_do_not_merge_other_services_or_sessions(self):
        logs=[]
        for i,(service,session) in enumerate((('svc','one'),('other','one'),('svc','two')),1):
            logs.append(dict(trace_id=None,span_id=None,time_unix_nano=i,service=service,severity_number=9,
                severity_text='INFO',event_name='agent.activity',scope_name='fixture',resource_attributes={},
                attributes={'session.id':session,'observatory.mcp.call_id':'same-provider-id','mcp.server.name':'fixture',
                    'mcp.tool.name':'search','mcp.method.name':'tools/call','agent.activity.result':'success'},
                body=json.dumps({'result':service+'/'+session})))
        self.store.ingest_logs(logs)
        page=self.store.mcp_page(limit=1);all_calls=list(page['calls'])
        while page['next_cursor']:
            page=self.store.mcp_page(limit=1,cursor=page['next_cursor']);all_calls.extend(page['calls'])
        self.assertEqual(len(all_calls),3)
        self.assertEqual(len({call['id'] for call in all_calls}),3)
        self.assertEqual(self.store.mcp_page(service='svc',session_id='one')['calls'][0]['output'],'svc/one')

    def test_same_span_log_with_foreign_owner_does_not_supply_parent_io(self):
        self.store.ingest([record(1,'parent','parent-session',tool=True)])
        self.store.ingest_logs([dict(trace_id=TRACE,span_id=f'{1:016x}',time_unix_nano=99,service='svc',
            severity_number=9,severity_text='INFO',event_name='agent.activity',scope_name='fixture',resource_attributes={},
            attributes={'agent.id':'child','agent.runtime':'SDK','agent.namespace':'svc','session.id':'child-session',
                'mcp.server.name':'fixture','mcp.tool.name':'search','agent.activity.result':'success'},
            body=json.dumps({'result':'PRIVATE_CHILD_OUTPUT'}))])
        page=self.store.mcp_page(agent_id=self.root,session_id='parent-session')
        self.assertEqual(len(page['calls']),1)
        self.assertNotIn('PRIVATE_CHILD_OUTPUT',json.dumps(page))
        self.assertEqual(len(self.store.mcp_page()['calls']),2)

    def test_unknown_execution_cannot_borrow_known_trace_owner(self):
        self.store.ingest([record(1,'parent','parent-session'),record(2,None,'parent-session',tool=True),record(3,None,None,execution=True)])
        self.assertEqual(self.store.mcp_page(agent_id=self.root)['calls'],[])
        page=self.store.mcp_page()
        self.assertEqual(page['summary']['errors'],0)
        self.assertFalse(page['calls'][0]['arguments'])

    def test_scope_revision_and_resolution_share_the_read_snapshot(self):
        self.store.ingest([record(1,'parent','s',tool=True)])
        original=self.store.resolve_agent_scope;revision=self.store.hierarchy_revision()
        def resolve(*args,**kwargs):
            self.assertTrue(kwargs['_db'].in_transaction)
            result=original(*args,**kwargs)
            with self.store._connect() as writer:
                writer.execute("UPDATE schema_meta SET value=CAST(value AS INTEGER)+1 WHERE key='agent_hierarchy_revision'")
            return result
        with patch.object(self.store,'resolve_agent_scope',side_effect=resolve):
            page=self.store.mcp_page(agent_id=self.root,scope_revision=revision)
        self.assertEqual(page['agent_scope']['revision'],revision)
        with self.assertRaisesRegex(ValueError,'hierarchy scope changed'):
            self.store.mcp_page(agent_id=self.root,scope_revision=revision)

    def test_conflicting_explicit_execution_links_remain_unallocated_everywhere(self):
        self.store.ingest([record(1,'parent','s',execution=True),
            record(2,'parent','s',tool=True,parent=f'{1:016x}'),
            record(3,'parent','s',execution=True,parent=f'{2:016x}')])
        page=self.store.mcp_page()
        self.assertEqual(page['summary']['errors'],0)
        self.assertNotEqual(page['calls'][0]['status'],'ERROR')
        self.assertFalse(page['calls'][0]['arguments'])

    def test_malformed_agent_claims_are_not_legacy_missing_ownership(self):
        for value in ('A'*257,{'invalid':'parent'},['parent'],True,0,''):
            with self.subTest(value=value):
                transport=record(1,None,'s',tool=True)
                transport['attributes']['agent.id']=value
                execution=record(2,None,'s',execution=True)
                execution['resource_attributes']['agent.id']='B'*257
                self.store.ingest([transport,execution])
                page=self.store.mcp_page()
                self.assertFalse(page['calls'][0]['arguments'])
                self.assertEqual(page['summary']['errors'],0)

    def test_malformed_or_multiple_missing_session_contexts_are_not_associated(self):
        for value in ({'invalid':'s'},['s'],True,0,''):
            with self.subTest(value=value):
                transport=record(1,None,None,tool=True)
                transport['resource_attributes']['session.id']=value
                self.store.ingest([transport,record(2,None,None,execution=True)])
                self.assertFalse(self.store.mcp_page()['calls'][0]['arguments'])
        self.store.ingest([record(1,None,None,tool=True),record(2,None,None,execution=True),
            record(3,None,'session-A'),record(4,None,'session-B')])
        self.assertFalse(self.store.mcp_page()['calls'][0]['arguments'])

    def log_call(self,number,call_id,*,trace=TRACE,span=None,server='fixture'):
        return dict(trace_id=trace,span_id=span,time_unix_nano=100,service='svc',
            severity_number=9,severity_text='INFO',event_name='agent.activity',scope_name='fixture',resource_attributes={},
            attributes={'session.id':'s','observatory.mcp.call_id':call_id,'mcp.server.name':server,
                'mcp.tool.name':'search','mcp.method.name':'tools/call','agent.activity.result':'success'},
            body=json.dumps({'result':str(number)}))

    def test_provider_id_cannot_collapse_explicit_traces_or_servers(self):
        self.store.ingest_logs([self.log_call(1,'same',span='1'*16),
            self.log_call(2,'same',trace='b'*32,span='2'*16),
            self.log_call(3,'same',server='different')])
        page=self.store.mcp_page()
        self.assertEqual(page['summary']['calls'],3)
        self.assertEqual({row['output'] for row in page['calls']},{'1','2','3'})

    def test_same_time_does_not_delete_a_distinct_explicit_call(self):
        self.store.ingest_logs([self.log_call(1,'one'),self.log_call(2,'two',span='2'*16)])
        page=self.store.mcp_page()
        self.assertEqual(page['summary']['calls'],2)
        self.assertEqual({row['output'] for row in page['calls']},{'1','2'})

    def test_inventory_and_display_agree_on_boolean_error_flags(self):
        for value in (1,'true',False,True):
            with self.subTest(value=value):
                execution=record(2,None,None,execution=True)
                execution['attributes']['gen_ai.tool.call.result']=json.dumps({'isError':value})
                self.store.ingest([record(1,None,None,tool=True),execution])
                page=self.store.mcp_page()
                self.assertEqual(page['summary']['errors'],int(value is True))
                self.assertEqual(page['calls'][0]['status']=='ERROR',value is True)

    def test_explicit_different_server_cannot_supply_transport_io_or_error(self):
        execution=record(2,'parent','s',execution=True,parent=f'{1:016x}')
        execution['attributes']['mcp.server.name']='different-server'
        self.store.ingest([record(1,'parent','s',tool=True),execution])
        page=self.store.mcp_page()
        transport=next(call for call in page['calls'] if call['span_id']==f'{1:016x}')
        self.assertFalse(transport['arguments'])
        self.assertNotEqual(transport['status'],'ERROR')
        server=next(row for row in page['summary']['servers'] if row['server']=='fixture')
        self.assertEqual(server['errors'],0)

    def test_malformed_owner_session_or_call_id_cannot_merge_distinct_logs(self):
        for field,values in (('agent.id',('A'*257,'B'*257)),('session.id',({'bad':'one'},{'bad':'two'})),
                             ('observatory.mcp.call_id',({'bad':'same'},{'bad':'same'}))):
            with self.subTest(field=field):
                with self.store._connect() as db:db.execute('DELETE FROM logs')
                logs=[self.log_call(1,'same'),self.log_call(2,'same')]
                for log,value in zip(logs,values):log['attributes'][field]=value
                self.store.ingest_logs(logs)
                page=self.store.mcp_page()
                self.assertEqual(page['summary']['calls'],2)
                self.assertEqual({row['output'] for row in page['calls']},{'1','2'})

    def test_nontext_names_are_unknown_in_inventory_and_rows(self):
        item=record(1,None,None,tool=True)
        item['attributes'].update({'mcp.server.name':{'wrong':'server'},'mcp.tool.name':['wrong-tool']})
        self.store.ingest([item])
        page=self.store.mcp_page()
        self.assertEqual(page['summary']['servers'],[])
        self.assertEqual(page['calls'][0]['server'],'Not reported')
        self.assertIsInstance(page['calls'][0]['tool'],str)
