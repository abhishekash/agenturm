import argparse
from contextlib import redirect_stdout, redirect_stderr
from copy import deepcopy
import io
import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from observatory.review_cli import add_review_parser, collect_review, run_review_command
from observatory.review_facts import refreshed_review_item
from observatory.workflow_evidence import ExecutionScope, ReviewHistory, check_kind
from observatory.work_receipts import freeze_receipt, export_receipt
from tests.test_workflow_evidence import event as source_event


def event(identity='one',**changes):
    row=source_event(identity,**changes)
    row['attributes']={'process.command_line':row['subject']}
    return row


class ReceiptTests(unittest.TestCase):
    def projection(self):
        history=ReviewHistory(ExecutionScope('receiver','codex','worker','session','run',kind='run'))
        history.accept({'receiver_id':'receiver','activity':[event(),
            {**event('edit'),'domain':'file','action':'edit','subject':'retry.py','time_unix_nano':11},
            {**event('request'),'domain':'message','action':'user','subject':'Fix retry errors'},
            {**event('reply'),'domain':'message','action':'assistant','subject':'All tests pass'}]})
        return history.projection({'task':'First session request','state':'Completed'})

    def test_information_and_collection_commands_do_not_validate(self):
        for command in ('pytest --version','ruff --help','python3.12 -m pytest --collect-only',
            'pytest --co','vitest list','jest --listTests','tsc --showConfig','eslint -v',
            'ruff version','ruff clean','ruff format .','cargo test -- --list','go test -list .',
            'pytest --fixtures','pytest --markers','ruff check --show-files','tsc --listFilesOnly',
            'jest --listTests=true','npm test -- --listTests','make -n test'):
            with self.subTest(command=command):self.assertIsNone(check_kind(command))
        for command in ('pytest tests/test_help.py','python3.12 -m pytest tests/test_version.py',
                        'ruff check .','ruff format --check .','uv run --offline pytest tests'):
            with self.subTest(command=command):self.assertIsNotNone(check_kind(command))

    def test_receipt_has_request_claim_source_and_gap_without_verification_badge(self):
        projection=self.projection();formats=freeze_receipt(projection)
        self.assertEqual(projection['request']['text'],'Fix retry errors')
        self.assertEqual(projection['title'],'First session request')
        self.assertEqual(projection['agent_update']['text'],'All tests pass')
        self.assertEqual(len(projection['checks']),1)
        self.assertEqual(projection['checks'][0]['result_basis'],'producer result label')
        for expected in ('later recorded edits','rerun the affected checks','Source references','receiver','worker','session','run'):
            self.assertIn(expected,formats['markdown'])
        projection['checks'][0]['command']='MUTATED'
        self.assertNotIn('MUTATED',formats['json'])

    def test_metadata_only_projection_suppresses_cached_content(self):
        history=ReviewHistory(ExecutionScope('receiver','codex','worker','session','run',kind='run'))
        history.accept({'receiver_id':'receiver','activity':[event(subject_private='PRIVATE')]})
        projection=history.projection({'capture_content':False,'task':'PRIVATE_TASK'})
        self.assertEqual(projection['window']['count'],0)
        self.assertFalse(projection['checks']);self.assertIsNone(projection['request'])
        self.assertNotIn('PRIVATE',json.dumps(projection))
        for content in freeze_receipt(projection).values():self.assertNotIn('PRIVATE',content)

    def test_operational_only_user_record_does_not_replace_actual_run_request(self):
        history=ReviewHistory(ExecutionScope('receiver','codex','worker','session','run',kind='run'))
        history.accept({'receiver_id':'receiver','activity':[
            {**event('first'),'domain':'message','action':'user','subject':'continue','time_unix_nano':10},
            {**event('control'),'domain':'message','action':'user','time_unix_nano':11,
             'subject':'<command-name>/model</command-name><command-message>model</command-message><command-args>fast</command-args>'}]})
        self.assertEqual(history.projection({})['request']['text'],'continue')

    def test_captured_markdown_html_and_terminal_controls_remain_literal(self):
        projection=self.projection()
        projection['request']['text']='```\n<script>alert(1)</script>\n![x](https://secret.test/x)\n\x1b[31mred\x00'
        formats=freeze_receipt(projection)
        self.assertNotIn('<script>',formats['html']);self.assertNotIn('<img',formats['html'])
        self.assertIn('&lt;script&gt;',formats['html']);self.assertNotIn('\x1b',formats['markdown'])
        self.assertIn('prefers-color-scheme:dark',formats['html'])

    def test_exports_are_frozen_private_nonoverwriting_and_rollback_owned_files(self):
        formats=freeze_receipt(self.projection())
        with TemporaryDirectory() as directory:
            target=Path(directory)/'review.md'
            paths=export_receipt(target,formats)
            self.assertEqual(len(paths),2)
            self.assertEqual(target.read_text(),formats['markdown'])
            if os.name!='nt':self.assertEqual(target.stat().st_mode&0o777,0o600)
            with self.assertRaises(FileExistsError):export_receipt(target,formats)
            blocked=Path(directory)/'blocked.html';blocked.write_text('KEEP')
            with self.assertRaises(FileExistsError):export_receipt(blocked.with_suffix('.md'),formats)
            self.assertFalse(blocked.with_suffix('.md').exists());self.assertEqual(blocked.read_text(),'KEEP')
            self.assertFalse(list(Path(directory).glob('.agenturm-review-*')))
            (Path(directory)/'symlink.md').symlink_to(target)
            with self.assertRaises(FileExistsError):export_receipt(Path(directory)/'symlink.md',formats)
            self.assertEqual(target.read_text(),formats['markdown'])
            self.assertEqual(len(export_receipt(Path(directory)/'machine.json',formats)),3)

    def test_many_checks_keep_total_later_edit_count_without_quadratic_links(self):
        history=ReviewHistory(ExecutionScope('receiver','codex','worker','session','run',kind='run'))
        for page in range(5):
            rows=[{**event(str(i)),'time_unix_nano':i+1} if i<500 else
                {**event(str(i)),'domain':'file','action':'edit','subject':'file.py','time_unix_nano':i+1}
                for i in range(page*200,(page+1)*200)]
            history.accept({'receiver_id':'receiver','activity':rows})
        projection=history.projection({})
        self.assertEqual(projection['checks'][0]['later_edit_count'],500)
        self.assertTrue(projection['checks'][0]['later_edits_clipped'])
        self.assertLess(len(json.dumps(projection).encode()),1024*1024)

    def test_run_usage_requires_explicit_producer_agent_session_ownership(self):
        history=ReviewHistory(ExecutionScope('receiver','codex','worker','session','run',kind='run'))
        usage={'trace_id':'run','agent_count':1,'agent_count_complete':True,'input_tokens':100}
        self.assertEqual(history.projection({'_review_usage':usage})['usage']['values'],{})
        for key,value in (('producer','claude'),('agent_id','sibling'),('session_id','other')):
            owner={'producer':'codex','agent_id':'worker','session_id':'session',key:value}
            self.assertEqual(history.projection({'_review_usage':{**usage,'review_owner':owner}})['usage']['values'],{})


class FakeReviewApi:
    endpoint='http://receiver.invalid'
    def __init__(self):
        self.requests=[];self.receiver='receiver';self.capable=True;self.capture=True
        self.rows=[event(),{**event('other'),'agent_id':'sibling','subject':'pytest SIBLING'},
            {**event('old'),'trace_id':'old','subject':'pytest OLD'},
            {**event('producer'),'service':'claude','subject':'pytest OTHER_PRODUCER'}]
        self.cursor=None
    def get(self,path):
        self.requests.append(path)
        if path=='/api/health':return {'receiver_id':self.receiver,'review_trace_filter':self.capable,
            'local_agents':{'capture_content':self.capture,'sessions':[]}}
        return {'receiver_id':self.receiver,'sessions':[]}
    def get_review(self,path):
        self.requests.append(path)
        return {'receiver_id':self.receiver,'activity':self.rows,'next_cursor':self.cursor}


class HeadlessReviewTests(unittest.TestCase):
    def collect(self,api,**kw):return collect_review(api,service='codex',agent_id='worker',session_id='session',trace_id='run',**kw)

    def test_exact_scope_filters_even_a_receiver_returning_foreign_rows(self):
        api=FakeReviewApi();projection=self.collect(api)
        self.assertEqual(len(projection['checks']),1)
        serialized=json.dumps(projection)
        for text in ('SIBLING','OLD','OTHER_PRODUCER'):self.assertNotIn(text,serialized)
        self.assertIn('subtree=0',api.requests[-2]);self.assertIn('trace_id=run',api.requests[-2])

    def test_capability_namespace_and_missing_identity_fail_closed(self):
        api=FakeReviewApi();api.capable=False
        with self.assertRaisesRegex(ValueError,'exact trace'):self.collect(api)
        self.assertEqual(len(api.requests),1)
        with self.assertRaisesRegex(ValueError,'agent-id or --session-id'):
            collect_review(FakeReviewApi(),service='codex',trace_id='run')
        api=FakeReviewApi();original=api.get_review
        api.get_review=lambda path:{**original(path),'receiver_id':'replacement'}
        with self.assertRaisesRegex(ValueError,'identity changed'):self.collect(api)
        api=FakeReviewApi();original=api.get
        api.get=lambda path:{**original(path),'receiver_id':'replacement'} if 'sessions?' in path else original(path)
        with self.assertRaisesRegex(ValueError,'identity changed'):self.collect(api)

    def test_metadata_only_never_fetches_historical_content(self):
        api=FakeReviewApi();api.capture=False
        projection=self.collect(api)
        self.assertEqual(api.requests,['/api/health'])
        self.assertEqual(projection['window']['count'],0)
        self.assertIn('Metadata-only capture',projection['window']['reasons'])

    def test_paging_is_explicit_bounded_and_repeated_cursor_is_disclosed(self):
        api=FakeReviewApi();api.cursor='older'
        projection=self.collect(api)
        self.assertEqual(len(api.requests),4)
        self.assertEqual(projection['window']['next_cursor'],'older')
        api=FakeReviewApi();api.cursor='same'
        projection=self.collect(api,pages=5)
        self.assertEqual(len(api.requests),5)
        self.assertIn('Receiver repeated an activity cursor',projection['window']['reasons'])

    def test_capture_policy_flip_while_reading_emits_no_review(self):
        api=FakeReviewApi();original=api.get_review
        def flip(path):
            response=original(path);api.capture=False;return response
        api.get_review=flip
        with self.assertRaisesRegex(ValueError,'Capture policy changed'):self.collect(api)

    def test_empty_receiver_window_is_qualified_not_a_complete_execution_claim(self):
        api=FakeReviewApi();api.rows=[]
        projection=self.collect(api)
        self.assertEqual(projection['window']['coverage'],'partial')
        self.assertIn('indexing may still be pending',' '.join(projection['window']['reasons']))

    def test_noninteractive_json_stdout_errors_and_status_contract(self):
        parser=argparse.ArgumentParser();add_review_parser(parser.add_subparsers())
        args=parser.parse_args(['review','--service','codex','--agent-id','worker','--trace-id','run','--format','json'])
        out=io.StringIO();err=io.StringIO()
        with patch('observatory.terminal.Api',return_value=FakeReviewApi()),redirect_stdout(out),redirect_stderr(err):
            self.assertEqual(run_review_command(args),0)
        self.assertEqual(json.loads(out.getvalue())['format'],'agenturm-review-v1');self.assertEqual(err.getvalue(),'')
        api=FakeReviewApi();api.capable=False;out=io.StringIO();err=io.StringIO()
        with patch('observatory.terminal.Api',return_value=api),redirect_stdout(out),redirect_stderr(err):
            self.assertEqual(run_review_command(args),2)
        self.assertEqual(out.getvalue(),'');self.assertIn('update',err.getvalue())


class RefreshFactsTests(unittest.TestCase):
    def test_internal_classification_revokes_capture_only_for_same_identity(self):
        from observatory.review_facts import review_capture_policy
        scope=ExecutionScope('receiver','codex','worker','session','run',kind='run')
        original={'capture_content':True}
        row=dict(service='codex-review',runtime='Codex',agent_id='worker',session_id='session',capture_content=False)
        snapshot=SimpleNamespace(receiver_id='receiver',local_status={'sessions':[row]})
        self.assertFalse(review_capture_policy(original,snapshot,scope)['capture_content'])
        for change in ({'agent_id':'other'},{'session_id':'other'},{'runtime':'Other'}):
            snapshot.local_status={'sessions':[{**row,**change}]}
            self.assertTrue(review_capture_policy(original,snapshot,scope)['capture_content'])
        snapshot.receiver_id='other';snapshot.local_status={'sessions':[row]}
        self.assertTrue(review_capture_policy(original,snapshot,scope)['capture_content'])

    def test_same_run_facts_update_and_newer_run_does_not_bleed_back(self):
        original=dict(service='codex',agent_id='worker',session_id='session',trace_id='run',state='Working',capture_content=True,
            _review_usage=dict(trace_id='run',input_tokens=100))
        scope=ExecutionScope('receiver','codex','worker','session','run',kind='run')
        native={**original,'run_state':'completed','last_record_ns':time.time_ns(),'diagnostics':{'last_success_ns':time.time_ns()}}
        usage=dict(trace_id='run',agent_count=1,agent_count_complete=True,input_tokens=500)
        snapshot=SimpleNamespace(receiver_id='receiver',local_status={'sessions':[native]},sessions=[dict(
            agent_id='worker',session_id='session',services=['codex'],latest_run=usage)],traces=[],updated=time.time(),error='',read_errors={})
        refreshed=refreshed_review_item(original,snapshot,scope)
        self.assertEqual(refreshed['state'],'Completed');self.assertEqual(refreshed['_review_usage']['input_tokens'],500)
        native['trace_id']='newer'
        retained=refreshed_review_item(original,snapshot,scope)
        self.assertEqual(retained['state'],'Working');self.assertEqual(retained['_review_usage']['input_tokens'],100)
        self.assertIn('previous snapshot',retained['_review_metadata_note'])
        self.assertEqual(original['state'],'Working')

    def test_newer_run_or_global_policy_revokes_content_independently_of_facts(self):
        original=dict(service='codex',agent_id='worker',session_id='session',trace_id='run',state='Completed',capture_content=True)
        scope=ExecutionScope('receiver','codex','worker','session','run',kind='run')
        snapshot=SimpleNamespace(receiver_id='receiver',local_status={'sessions':[
            {**original,'trace_id':'new-run','capture_content':False,'capture_revision':2}]},
            sessions=[],traces=[],updated=time.time(),error='',read_errors={})
        selected=refreshed_review_item(original,snapshot,scope)
        self.assertFalse(selected['capture_content']);self.assertEqual(selected['capture_revision'],2)
        self.assertEqual(selected['state'],'Completed')
        snapshot.local_status={'sessions':[],'capture_content':False}
        self.assertFalse(refreshed_review_item(original,snapshot,scope)['capture_content'])


class HeadlessReceiverJourney(unittest.TestCase):
    def test_actual_http_auth_worker_scope_stdout_and_frozen_exports(self):
        import subprocess
        import sys
        from observatory.core import normalize_native
        from observatory.agent_hierarchy import canonical_agent_id
        from observatory.terminal import LocalReceiver
        with TemporaryDirectory() as directory:
            receiver=LocalReceiver(Path(directory)/'telemetry.db',0,'test-token',local_agents=False)
            self.addCleanup(receiver.close)
            trace='a'*32
            def span(worker,number):
                attrs={'agent.id':worker,'agent.runtime':'SDK','agent.namespace':'svc','session.id':'shared'}
                def activity(domain,action,subject,stamp):
                    return dict(name='agent.activity',time_unix_nano=stamp,attributes={
                        'agent.activity.domain':domain,'agent.activity.action':action,
                        'agent.activity.subject':subject,'agent.activity.result':'success',
                        **({'process.command_line':subject,'process.exit.code':0} if domain=='shell' else {})})
                return normalize_native(dict(trace_id=trace,span_id=f'{number:016x}',name='agent.run',
                    start_unix_nano=1,end_unix_nano=9,status='OK',attributes=attrs,events=[
                    activity('message','user','Fix retries '+worker,2),activity('shell','execute','pytest '+worker,3),
                    activity('file','edit','retry.py',4)]),'svc')
            receiver.store.ingest([span('parent',1),span('worker',2)])
            # The live receiver indexes asynchronously. Prepare the indexed
            # fixture deterministically; this journey tests HTTP scope/auth,
            # not a read-after-ingest timing guarantee.
            receiver.store._refresh_summaries([trace])
            endpoint='http://127.0.0.1:'+str(receiver.server.server_address[1])
            destination=Path(directory)/'review.json'
            command=[sys.executable,'-m','observatory','review','--endpoint',endpoint,'--service','svc',
                '--agent-id',canonical_agent_id('SDK','svc','worker'),'--session-id','shared','--trace-id',trace,
                '--format','json','--output',str(destination)]
            result=subprocess.run(command,env={**os.environ,'OBSERVATORY_TOKEN':'test-token','NO_COLOR':'1'},capture_output=True,text=True,timeout=15)
            self.assertEqual(result.returncode,0,result.stderr)
            projection=json.loads(result.stdout)
            self.assertEqual([check['command'] for check in projection['checks']],['pytest worker'])
            self.assertEqual(projection['checks'][0]['later_edit_count'],1)
            self.assertEqual(projection['request']['text'],'Fix retries worker')
            self.assertEqual(projection['usage']['values'],{})
            self.assertEqual(destination.read_text(),result.stdout)
            self.assertIn('rerun the affected checks',destination.with_suffix('.md').read_text())
            self.assertTrue(destination.with_suffix('.html').exists())
            unauthorized=subprocess.run(command,env={**os.environ,'OBSERVATORY_TOKEN':'wrong'},capture_output=True,text=True,timeout=15)
            self.assertEqual(unauthorized.returncode,2);self.assertEqual(unauthorized.stdout,'')
            self.assertNotIn('test-token',unauthorized.stderr)


class RecordedOwnershipTests(unittest.TestCase):
    def test_cached_ownership_is_proved_from_all_records_and_rejects_ambiguity(self):
        from observatory.core import Store, normalize_native
        from observatory.agent_hierarchy import canonical_agent_id
        with TemporaryDirectory() as directory:
            store=Store(Path(directory)/'owners.db');self.addCleanup(store.close)
            agent=canonical_agent_id('SDK','svc','worker')
            attrs={'agent.id':agent,'session.id':'session'}
            def record(trace,span,service='svc',attributes=None):
                return normalize_native(dict(trace_id=trace,span_id=f'{span:016x}',name='model',status='OK',
                    start_unix_nano=span,end_unix_nano=span+1,attributes={**attrs,**(attributes or {}),
                    'gen_ai.usage.input_tokens':10}),'svc' if service is None else service)
            for trace,change in [('a'*32,{}),('b'*32,{'service':'other'}),('c'*32,{'attributes':{'session.id':'other'}}),
                                 ('d'*32,{'attributes':{'agent.id':''}})]:
                store.ingest([record(trace,1),record(trace,2,**change)])
            sessions=store.sessions()
            # A mixed lifetime session must not substitute its lifetime service set
            # for the ownership proof attached to each individual latest run.
            by_trace={row['trace_id']:row for row in store.traces()}
            owner={'producer':'svc','agent_id':agent,'session_id':'session'}
            self.assertEqual(by_trace['a'*32]['review_owner'],owner)
            self.assertEqual(store.trace('a'*32)['summary']['review_owner'],owner)
            for trace in ('b'*32,'c'*32,'d'*32):self.assertIsNone(by_trace[trace]['review_owner'])
            self.assertTrue(any('review_owner' in row.get('latest_run',{}) for row in sessions))
