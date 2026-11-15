"""Early startup observations require real-pane and published-state agreement.

These contracts exercise the production observer and driver IO boundaries. They
are not a retained-corpus terminal run or a resource acceptance result.
"""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import time
import unittest
from unittest.mock import patch

from scripts import benchmark_monitor_idle as idle
from scripts.validation_support import published_heartbeat


class FirstFrameAcceptanceTests(unittest.TestCase):
    def row(self,partial=True):
        return dict(kind='heartbeat',harness_version=2,pid=123,monotonic_at=1.,at=100.,
            receiver_id='rx',display_receiver_id='rx',screen='ObservatoryScreen',focus='agent-table',
            terminal_size=[120,36],visible_agent_rows=4,initial_poll_complete=True,
            states=[dict(session=name)for name in sorted(idle.FIRST_FRAME_SESSIONS)],
            native_sessions=sorted(idle.FIRST_FRAME_SESSIONS),native_bootstrap=partial,
            display_native_bootstrap=partial,bootstrap_updated=99.8,display_bootstrap_updated=99.8,
            updated=0 if partial else 99.8,display_updated=0 if partial else 99.8,
            indexing_ready=not partial,error='',warning='',published_sources_clipped=False,
            native_inventory_supported=True,native_inventory_warning='',read_errors={},sessions_error='',
            native_read_errors=0,native_schema_errors=0,opencode_read_errors=0,opencode_diagnostics={},
            pending_storage_records=0,source_read_error=False,source_backlog_bytes=0,source_skipped_records=0,
            source_omitted=0,source_hierarchy_write_errors=0,source_schema_errors=0)

    def frame(self,partial=True):
        return ('Agents\nNative observations · 4 shown · partial\nHistory indexing\n'
                'Codex agent · session soak-parent\nCodex child · session soak-child')if partial else 'Agents\nTask / source State Updated\nLoaded generated task'

    def test_frame_requires_both_terminal_and_fresh_metadata_not_health_only(self):
        row=self.row()
        self.assertEqual(idle.first_frame_kind(row,self.frame(),pid=123,receiver='rx',observed=1.1),'native_partial')
        for change in (dict(pid=9),dict(receiver_id='foreign'),dict(display_receiver_id='foreign'),
                dict(visible_agent_rows=0),dict(initial_poll_complete=False),dict(screen='HelpScreen'),
                dict(focus='search-agents'),dict(terminal_size=[60,18]),dict(monotonic_at=-10),
                dict(monotonic_at=5),dict(native_bootstrap=False),dict(updated=1),dict(updated=False),dict(display_updated=1),
                dict(indexing_ready=True),dict(bootstrap_updated=90),dict(display_bootstrap_updated=101),
                dict(native_sessions=[]),dict(states=[]),dict(error='fatal'),dict(warning='failed')):
            with self.subTest(change=change):
                changed=deepcopy(row);changed.update(change)
                self.assertIsNone(idle.first_frame_kind(changed,self.frame(),pid=123,receiver='rx',observed=1.1))
        for frame in ('','Agents loading','History indexing','Agents\nHistory indexing\nNative observations'):
            self.assertIsNone(idle.first_frame_kind(row,frame,pid=123,receiver='rx',observed=1.1))
        with self.assertRaisesRegex(ValueError,'source content'):
            idle.first_frame_kind(row,self.frame()+idle.FIRST_FRAME_PRIVATE,pid=123,receiver='rx',observed=1.1)

    def run_observer(self,rows,*,health=None,identity=(123,50)):
        moment=[1.];retained=[];calls=[];pending=iter(rows)
        def observe():
            row=deepcopy(next(pending));row['monotonic_at']=moment[0]
            partial=row['native_bootstrap']
            return dict(identity=identity,heartbeat=row,frame=self.frame(partial),
                retain=lambda name,proof:retained.append((name,proof)))
        def confirm(timeout):
            calls.append(('confirm',moment[0],timeout));moment[0]+=.3
            return health or dict(ok=True,receiver_id='rx',indexing={'ready':True},local_agents={'initial_poll_complete':True})
        result=idle.observe_first_frames(observe,confirm,started=0,clock=lambda:moment[0],pause=lambda _:moment.__setitem__(0,moment[0]+.1))
        return result,retained,calls

    def test_partial_capture_precedes_independent_readiness_then_fresh_coherent_capture(self):
        result,retained,calls=self.run_observer([self.row(),self.row(False)])
        self.assertEqual([name for name,_ in retained],['first-native-partial-screen.txt','first-coherent-screen.txt'])
        self.assertEqual(result['partial_observation'],'observed')
        self.assertLess(result['first_native_partial']['observed_seconds'],result['full_readiness_seconds'])
        self.assertGreaterEqual(result['first_coherent']['observed_seconds'],result['full_readiness_seconds'])
        self.assertEqual(len(calls),1)

    def test_coherent_first_reports_partial_unobserved_without_inventing_timestamp(self):
        result,retained,_=self.run_observer([self.row(False),self.row(False)])
        self.assertEqual(result['partial_observation'],'unobserved')
        self.assertIsNone(result['first_native_partial'])
        self.assertEqual([name for name,_ in retained],['first-coherent-screen.txt'])
        self.assertGreaterEqual(result['first_coherent']['observed_seconds'],result['full_readiness_seconds'])

    def test_unknown_fatal_or_foreign_confirmation_never_returns_success(self):
        for health in (dict(ok=True,receiver_id='other',indexing={'ready':True},local_agents={'initial_poll_complete':True}),
                dict(ok=True,receiver_id='rx',indexing={'ready':False},local_agents={'initial_poll_complete':True}),
                dict(ok=True,receiver_id='rx',indexing={'ready':True,'error':'fatal'},local_agents={'initial_poll_complete':True}),
                dict(ok=True,receiver_id='rx',indexing={'ready':True},local_agents={})):
            with self.subTest(health=health),self.assertRaisesRegex(ValueError,'readiness failed'):
                self.run_observer([self.row()],health=health)

    def test_process_receiver_and_deadline_changes_do_not_pass(self):
        row=self.row();clock=[1.];identities=iter(((123,50),(123,51)))
        def observe():return dict(identity=next(identities),heartbeat=row,frame='',retain=lambda *_:None)
        with self.assertRaisesRegex(ValueError,'process identity changed'):
            idle.observe_first_frames(observe,lambda _:None,started=0,clock=lambda:clock[0],pause=lambda _:None)
        for identity in ((123,None),(True,50),(123,0)):
            with self.subTest(identity=identity),self.assertRaisesRegex(ValueError,'Invalid cockpit'):
                self.run_observer([row],identity=identity)
        second=deepcopy(row);second['receiver_id']='other';rows=iter((row,second))
        def changed():return dict(identity=(123,50),heartbeat=next(rows),frame='',retain=lambda *_:None)
        with self.assertRaisesRegex(ValueError,'Receiver identity changed'):
            idle.observe_first_frames(changed,lambda _:None,started=0,clock=lambda:1.,pause=lambda _:None)
        with self.assertRaises(TimeoutError):
            idle.observe_first_frames(lambda:None,lambda _:None,started=0,clock=lambda:601.)

    def test_dispatch_is_opt_in_and_does_not_run_idle_driver(self):
        with patch.object(idle,'measure_first_frame',return_value=17)as first,patch.object(idle,'bind_application',side_effect=AssertionError('default driver')):
            self.assertEqual(idle.measure(SimpleNamespace(first_frame_only=True)),17);first.assert_called_once()
            with self.assertRaisesRegex(AssertionError,'default driver'):
                idle.measure(SimpleNamespace(first_frame_only=False,app_root=Path('/generated')))
            self.assertEqual(first.call_count,1)

    def test_heartbeat_additions_are_metadata_only_and_do_not_prepare_sources(self):
        snapshot=SimpleNamespace(local_status={'sessions':[]},error='',receiver_id='rx',updated=0,
            read_errors={},sessions_error='',sessions=[],mcp_summary={},native_bootstrap=True,bootstrap_updated=99.)
        def forbidden():raise AssertionError('Source preparation ran')
        app=SimpleNamespace(feed=SimpleNamespace(snapshot=snapshot),data=snapshot,
            source_map={'x':dict(task=idle.FIRST_FRAME_PRIVATE)},workflow_items=forbidden,
            workflow_ledger=SimpleNamespace(rows=[]),workflow_controller=SimpleNamespace(warning=''))
        result=published_heartbeat(app,at=100)
        self.assertTrue(result['native_bootstrap']);self.assertTrue(result['display_native_bootstrap'])
        self.assertEqual(result['bootstrap_updated'],99.)
        self.assertIsInstance(result['monotonic_at'],float)
        self.assertNotIn(idle.FIRST_FRAME_PRIVATE,json.dumps(result))

    def driver(self,*,mutate_source=False,mutate_seed=False,replace_file=False):
        """Run the production driver/observer; substitute external IO only."""
        from contextlib import ExitStack
        with tempfile.TemporaryDirectory()as directory,ExitStack()as stack:
            base=Path(directory)/'run';seed=Path(directory)/'seed.db';seed.write_bytes(b'generated')
            args=SimpleNamespace(output=base,app_root=Path(directory),minimum_free_mib=1,seed=seed,
                history=True,expected_history_sessions=1,profile=False,allow_dirty=False,smoke=True)
            calls=[];state={'closed':False,'coherent':False};owner=self
            def publish(coherent=False):
                row=owner.row(not coherent);row['monotonic_at']=time.monotonic()
                (base/'ui-events.jsonl').write_text(json.dumps(row)+'\n')
            def workload(path):
                (path/'project').mkdir();(path/'fixture-home').mkdir()
                return SimpleNamespace(paths={name:path/(name+'.jsonl')for name in ('parent','child')})
            class Receiver:
                pid=321;returncode=None
                def __init__(self,argv,**kwargs):
                    calls.append('receiver-spawn')
                    token=base/'state/agenturm'/('receiver-'+argv[-1]+'.token')
                    token.parent.mkdir(parents=True);token.write_text('generated-private-token')
                def poll(self):return self.returncode
                def send_signal(self,_):
                    self.returncode=0
                    (base/'receiver-exits.jsonl').write_text(json.dumps(dict(pending_after=0))+'\n')
                def wait(self,**kwargs):return self.returncode
                def kill(self):self.returncode=-9
            health=dict(ok=True,receiver_id='rx',indexing={'ready':True},local_agents={'enabled':True,'initial_poll_complete':True,
                'sessions':[dict(session_id=name)for name in sorted(idle.FIRST_FRAME_SESSIONS)]})
            class Api:
                def __init__(self,*_):pass
                def get(self,path):
                    calls.append(path)
                    if path=='/api/health':return deepcopy(health)
                    if path.startswith('/api/sessions'):return dict(sessions=[{}])
                    if path.startswith('/api/mcp'):return dict(summary={'calls':1})
                    raise AssertionError(path)
            def ready(api,**kwargs):
                calls.append('full-ready-wait');value=api.get('/api/health')
                state['coherent']=True;publish(True);return value
            def run(argv,**kwargs):
                result=''
                if 'new-session'in argv:
                    calls.append('ui-start');self.assertNotIn('full-ready-wait',calls);self.assertNotIn('/api/health',calls)
                    publish()
                elif 'display-message'in argv:
                    field=argv[-1]
                    if field=='#{pane_pid}':result='123'
                    elif field=='#{pane_pid}:#{pane_dead}':result='123:0'
                    else:result='1:0:'
                elif 'capture-pane'in argv:
                    calls.append('terminal-capture');result=owner.frame(not state['coherent'])
                    if replace_file and not state['coherent']:
                        replacement=base/'replacement.db';replacement.write_bytes(b'replacement');replacement.replace(base/'telemetry.db')
                elif 'send-keys'in argv and argv[-1]=='q':
                    with (base/'ui-events.jsonl').open('a')as stream:stream.write(json.dumps(dict(kind='exit',pid=123,code=0,warning=''))+'\n')
                elif 'kill-server'in argv:state['closed']=True
                return SimpleNamespace(stdout=result,returncode=0)
            def revision(_):
                count=calls.count('revision');calls.append('revision')
                return dict(dirty=False,head='b'*40 if mutate_source and count else'a'*40)
            def clone(*_,**__):
                (base/'telemetry.db').write_bytes(b'generated clone')
                return dict(counts={'spans':12,'logs':20,'trace_activity':3})
            replacements={
                'bind_application':lambda _:None,'disk_gate':lambda *_,**__: {},'Workload':workload,
                'revision':revision,'clone_seed':clone,'fresh_schema_manifest':lambda *_,**__: {'schema_meta_sql':'fixture'},
                'prepare_raw_clone':lambda *_,**__: {'seed_after':{'raw_sha256':'initial'}},
                'verify_named_indexes':lambda *_,**__: {'equal':True},'storage_snapshot':lambda _: {'rows':{}},
                'process_sample':lambda pid:None if pid==123 and state['closed']else dict(start_ticks=50 if pid==123 else 70),
            }
            for name,value in replacements.items():stack.enter_context(patch.object(idle,name,value))
            stack.enter_context(patch('observatory.terminal.Api',Api))
            stack.enter_context(patch('scripts.soak_monitor.native',lambda *_,**__:None))
            stack.enter_context(patch('scripts.soak_monitor.wait_ready',ready))
            stack.enter_context(patch('scripts.soak_monitor.index_proof',return_value={key:True for key in
                ('receiver_matches','health_matches','derived_marker_complete','file_unchanged')}))
            stack.enter_context(patch('scripts.validation_support.raw_manifest',return_value={'raw_sha256':'changed'if mutate_seed else'initial'}))
            stack.enter_context(patch.object(idle.subprocess,'Popen',Receiver))
            stack.enter_context(patch.object(idle.subprocess,'run',run))
            stack.enter_context(patch('builtins.print'))
            code=idle.measure_first_frame(args)
            result=json.loads((base/'results.json').read_text())
            return code,result,calls

    def test_production_driver_starts_ui_and_captures_before_full_readiness(self):
        code,result,calls=self.driver()
        self.assertEqual(code,0,result['errors'])
        self.assertLess(calls.index('ui-start'),calls.index('full-ready-wait'))
        self.assertLess(calls.index('terminal-capture'),calls.index('full-ready-wait'))
        self.assertEqual(result['proof']['partial_observation'],'observed')
        self.assertEqual(result['exit_status'],{'cockpit':{'status':'0','signal':''},'receiver':0})
        self.assertFalse(result['publishable']);self.assertFalse(result['idle_budget'])
        self.assertFalse(result['observation_valid'],'Smoke IO fixtures are not acceptance')
        self.assertNotIn('generated-private-token',json.dumps(result))

    def test_source_seed_and_same_receiver_file_changes_remain_failed(self):
        for option in ('mutate_source','mutate_seed','replace_file'):
            with self.subTest(change=option):
                code,result,_=self.driver(**{option:True})
                self.assertEqual(code,1);self.assertTrue(result['errors'])
                self.assertFalse(result['observation_valid']);self.assertFalse(result['publishable'])


if __name__=='__main__':unittest.main()
