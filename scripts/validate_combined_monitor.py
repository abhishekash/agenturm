"""Frozen generated validation: prepare/smoke, native race, modal retention, cleanup.

These are functional harnesses, not substitutes for the registered untouched
idle budget. No long run starts without an explicit command. All outputs are
new private directories; cleanup only removes registered working DB clones.
"""
from __future__ import annotations
import argparse
from contextlib import closing
from unittest.mock import patch
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import urllib.request

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.validation_support import (bind_application,cleanup_clone,clone_seed,create_owned_directory,
    disk_gate,harness_manifest,isolated_env,raw_manifest,storage_snapshot,application_revision,nearest_rank,strict_heartbeat_errors)
from scripts.soak_monitor import Workload,process_sample,seed_history,append,write_json,event,native


def rows(path):
    if not Path(path).exists():return []
    lines=Path(path).read_text().splitlines();result=[]
    for index,line in enumerate(lines):
        try:result.append(json.loads(line))
        except ValueError:
            if index!=len(lines)-1:raise
    return result


def until(predicate,label,*,timeout=30):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        value=predicate()
        if value:return value
        time.sleep(.15)
    raise RuntimeError(label+' deadline exceeded')


class FixtureSession:
    """Own real receiver/UI processes and one private tmux server."""
    def __init__(self,args):
        self.args=args;self.base=args.output.resolve();self.roles={};self.streams=[];self.exits=[]
        self.disk=disk_gate(self.base,reserve_bytes=args.minimum_free_mib*1024**2)
        create_owned_directory(self.base);self.workload=Workload(self.base)
        if args.mode=='modals':
            for index in range(5):
                path=self.base/'fixture-home/.codex/sessions'/f'modal-{index}.jsonl'
                append(path,event('session_meta',dict(id=f'validation-modal-{index}',cwd=str(self.base/'project'))))
                native(path,'event_msg',type='task_started',turn_id=f'modal-turn-{index}')
                native(path,'event_msg',type='item_completed',item=dict(type='UserMessage',id=f'modal-prompt-{index}',
                    content=[dict(type='text',text=f'Modal validation {index}')]))
                native(path,'event_msg',type='item_completed',item=dict(type='CommandExecution',id=f'modal-check-{index}',
                    command=['pytest','-q'],status='completed',exit_code=0,aggregated_output='Generated check passed'))
                native(path,'event_msg',type='task_complete',turn_id=f'modal-turn-{index}')
        self.seed=clone_seed(args.seed,self.base/'telemetry.db',reserve_bytes=args.minimum_free_mib*1024**2) if args.seed else None
        self.env=isolated_env(self.base,args.app_root);self.channel='agenturm-validation-'+str(os.getpid())
        with socket.socket() as sock:sock.bind(('127.0.0.1',0));self.port=sock.getsockname()[1]
        self.command=[sys.executable,str(ROOT/'scripts/soak_monitor.py'),'_child']
        self.before=dict(application=application_revision(args.app_root),harness=harness_manifest())
        self.report=dict(protocol='agenturm-final-validation-v1',mode=args.mode,source=self.before,disk=self.disk,
            seed=self.seed,no_provider_calls=True,notification_sink='fixture only',errors=[])
    def tmux(self,*args):
        return subprocess.check_output(['tmux','-L',self.channel,'-f','/dev/null',*args],env=self.env,cwd=self.base/'project',text=True,stderr=subprocess.STDOUT,timeout=5)
    def spawn(self,role,mode):
        env={**self.env,'AGENTURM_VALIDATION_ROLE':role};stream=(self.base/(role+'.log')).open('a');self.streams.append(stream)
        process=subprocess.Popen([*self.command,mode,str(self.base),str(self.port)],env=env,cwd=self.base/'project',stdout=stream,stderr=subprocess.STDOUT)
        self.roles[role]=process;return process
    def start(self):
        self.tmux('new-session','-d','-s','control','-x','120','-y','36','/bin/cat')
        self.spawn('receiver','receiver')
        token=self.base/'state/agenturm'/f'receiver-{self.port}.token'
        until(token.exists,'Receiver token')
        from observatory.terminal import Api
        self.api=Api(f'http://127.0.0.1:{self.port}',token.read_text().strip())
        def ready():
            if self.roles['receiver'].poll() is not None:raise RuntimeError('Receiver exited before readiness')
            try:value=self.api.get('/api/health')
            except OSError:return None
            return value if value.get('indexing',{}).get('ready') is True and value.get('local_agents',{}).get('initial_poll_complete') is True else None
        self.health=until(ready,'Receiver readiness',timeout=600)
        self.workload.phase(0,0)
        if self.args.mode=='modals':
            for phase in range(1,5):self.workload.phase(0,phase)
        until(lambda:any(row.get('session_id')=='soak-parent' and row.get('trace_id') for row in self.api.get('/api/health')['local_agents']['sessions']),
            'Recorded parent execution')
    def ui(self,role,*,notify=False):
        env={**self.env,'AGENTURM_VALIDATION_ROLE':role}
        # tmux direct argv preserves both the fixture cwd and absence of shell startup files.
        command=['env',*['%s=%s'%(key,value) for key,value in env.items()],*self.command,
                 'ui-observer' if notify else 'ui',str(self.base),str(self.port)]
        self.tmux('new-session','-d','-s',role,'-x','120','-y','36',*command)
        self.tmux('set-option','-t',role,'remain-on-exit','on')
        self.roles[role]=int(self.tmux('display-message','-p','-t',role,'#{pane_pid}').strip())
        return self.wait_ui(role)
    def beats(self,role):return [row for row in rows(self.base/('ui-events-'+role+'.jsonl')) if row.get('kind')=='heartbeat']
    def wait_ui(self,role):
        def ready():
            beats=self.beats(role)
            if not beats:return None
            row=beats[-1]
            if row.get('pid')!=self.roles[role]:return None
            failures=strict_heartbeat_errors(row,self.health['receiver_id'])
            if failures:raise RuntimeError('Cockpit validation failure: '+ '; '.join(failures))
            now=time.time()
            if not 0<=now-row['at']<=2.5 or not 0<=now-row['updated']<=6:return None
            return row if row.get('states') else None
        return until(ready,'Cockpit '+role)
    def keys(self,role,*keys):
        for key in keys:self.tmux('send-keys','-t',role,key);time.sleep(.15)
    def text(self,role,value):self.tmux('send-keys','-l','-t',role,value)
    def frame(self,role,label):
        text=self.tmux('capture-pane','-p','-t',role)
        append(self.base/'frames.jsonl',dict(role=role,label=label,at=time.time(),text=text))
        return text
    def wait_screen(self,role,text):return until(lambda:text in self.frame(role,text),text)
    def stop_ui(self,role):
        self.keys(role,'Escape','Escape','q')
        def exited():
            values=self.tmux('display-message','-p','-t',role,'#{pane_dead}:#{pane_dead_status}:#{pane_dead_signal}').strip().split(':')
            return values if values[0]=='1' and (values[1] or values[2]) else None
        value=until(exited,'Cockpit clean exit',timeout=12)
        self.exits.append(dict(role=role,status=value[1],signal=value[2]))
        if value[1]!='0' or value[2]:raise RuntimeError('Cockpit did not exit normally')
        self.tmux('kill-session','-t',role);self.roles.pop(role,None)
    def close(self):
        for role,process in sorted(self.roles.items(),key=lambda row:row[0]=='receiver'):
            if isinstance(process,int):
                try:self.stop_ui(role)
                except Exception as exc:self.report['errors'].append(str(exc))
            else:
                if process.poll() is None:
                    process.send_signal(signal.SIGINT)
                    try:process.wait(timeout=20)
                    except subprocess.TimeoutExpired:process.kill();process.wait();self.report['errors'].append(role+' forced termination')
                self.exits.append(dict(role=role,status=process.returncode))
                if process.returncode:self.report['errors'].append(role+' unsuccessful exit')
        subprocess.run(['tmux','-L',self.channel,'-f','/dev/null','kill-server'],env=self.env,capture_output=True)
        for stream in self.streams:stream.close()
        if self.before!=dict(application=application_revision(self.args.app_root),harness=harness_manifest()):self.report['errors'].append('Application/harness changed')
        self.report.update(exit_statuses=self.exits,storage=storage_snapshot(self.base/'telemetry.db'))
        if any(value for name,value in self.report['storage'].get('rows',{}).items() if 'dirty' in name):self.report['errors'].append('Indexes did not drain')
        self.report['passed']=not self.report['errors'];write_json(self.base/'results.json',self.report)


def post_noise(session,sequence):
    records=[];stamp=time.time_ns()
    for index in range(210):
        attrs={'agent.activity.domain':'message','agent.activity.action':'assistant','agent.activity.subject':'Generated unrelated record',
            'observatory.record_id':f'validation-noise-{sequence}-{index}'}
        records.append(dict(timeUnixNano=str(stamp+index),severityNumber=9,severityText='INFO',body={'stringValue':''},
            attributes=[dict(key=key,value={'stringValue':value}) for key,value in attrs.items()]))
    payload=dict(resourceLogs=[dict(resource={'attributes':[dict(key='service.name',value={'stringValue':'validation-noise'})]},
        scopeLogs=[dict(scope={'name':'validation.fixture'},logRecords=records)])])
    request=urllib.request.Request(session.api.endpoint+'/v1/logs',data=json.dumps(payload).encode(),
        headers={'Content-Type':'application/json','Authorization':'Bearer '+session.api.token,
                 'X-Agenturm-Expected-Receiver-ID':session.health['receiver_id']})
    with urllib.request.urlopen(request,timeout=10) as response:
        value=json.load(response)
        if value.get('accepted')!=210:raise RuntimeError('Fixture writer batch was not accepted')


def native_race(session):
    from observatory.native_lifecycle import validate_binding,normalize_event,event_log,post_event,verify_binding
    from observatory.workflow_store import WorkflowStore
    if session.health.get('native_lifecycle_v1') is not True:
        raise RuntimeError('Native lifecycle inventory unsupported by this application revision; case not exercised')
    session.ui('cockpit-a',notify=True);session.ui('cockpit-b',notify=True);session.spawn('watcher','watcher')
    until(lambda:any(row.get('ready') is True for row in rows(session.base/'watcher-events.jsonl')),'Headless baseline')
    store=WorkflowStore(session.base/'state/agenturm/workflow.sqlite3')
    until(lambda:session.health['receiver_id'] in store.attention_baselines(),'Shared inventory baseline')
    if rows(session.base/'notification-events.jsonl'):raise RuntimeError('Initial inventory was not quiet')
    observed=next(row for row in session.api.get('/api/health')['local_agents']['sessions'] if row['session_id']=='soak-parent')
    configured=validate_binding(dict(schema='agenturm-native-binding-v1',sender_id='validation-sender',endpoint=session.api.endpoint,
        adapter='generic-v1',native_turn_id='',follow_recorded_turns=False,source_clock='receiver',receiver_id=session.health['receiver_id'],
        service=observed['service'],agent_id=observed['agent_id'],session_id=observed['session_id'],trace_id=observed['trace_id'],
        turn_id=observed.get('turn_id') or '',source_id=observed['source_id'],parent_agent_id=observed.get('parent_agent_id') or ''))
    verify_binding(session.api,configured)
    def emit(sequence,kind):
        value=dict(schema='agenturm-native-event-v1',**{key:configured[key] for key in
            ('sender_id','receiver_id','service','agent_id','session_id','trace_id','turn_id','source_id','parent_agent_id')},
            event_id='validation-'+str(sequence),sequence=sequence,kind=kind,request_id='validation-request',time_unix_nano=time.time_ns())
        event=normalize_event(value,configured);event.update(request_kind='approval_requested',received_time_unix_nano=time.time_ns())
        post_event(session.api,configured,event_log(event));post_noise(session,sequence)
    emit(1,'approval_requested')
    until(lambda:len(rows(session.base/'notification-events.jsonl'))>=1,'One notification claim')
    def incident():return next((row for row in store.load() if row['kind']=='approval' and row['anchor']=='request:validation-request'),None)
    first=until(incident,'Durable request receipt')
    if len(rows(session.base/'notification-events.jsonl'))!=1:raise RuntimeError('Duplicate notification delivery')
    store.acknowledge(first['namespace'],first['id'],first['scope_key'],action='seen')
    session.stop_ui('cockpit-b');session.ui('cockpit-b',notify=True)
    watcher=session.roles['watcher'];watcher.send_signal(signal.SIGINT);watcher.wait(timeout=10)
    session.exits.append(dict(role='watcher-before-restart',status=watcher.returncode));session.roles.pop('watcher')
    if watcher.returncode:raise RuntimeError('Headless observer did not stop cleanly')
    before=len(rows(session.base/'watcher-events.jsonl'));session.spawn('watcher','watcher')
    until(lambda:len(rows(session.base/'watcher-events.jsonl'))>before and rows(session.base/'watcher-events.jsonl')[-1].get('ready') is True,'Restarted headless inventory')
    retained=incident()
    if retained.get('acknowledgement')!='seen' or retained.get('resolved_at') is not None or retained.get('delivery_status')!='delivered':raise RuntimeError('Seen request/delivery did not remain durable')
    if len(rows(session.base/'notification-events.jsonl'))!=1:raise RuntimeError('Restart repeated notification')
    emit(2,'request_resolved')
    until(lambda:incident().get('resolved_at') is not None,'Exact native request resolution')
    if store.native_observations(session.health['receiver_id']):raise RuntimeError('Remote case unexpectedly used a local hook binding')
    notifications=rows(session.base/'notification-events.jsonl')
    if len(notifications)!=1 or any('Validate parent' in json.dumps(row) for row in notifications):raise RuntimeError('Notification count/privacy contract failed')
    session.report.update(observers=['two real cockpits','headless watcher'],notification_attempts=1,
        acknowledgement_preserved=True,remote_inventory_catchup=True,unrelated_records_per_transition=210,exact_resolution=True)


def modal_cycles(session):
    session.ui('cockpit')
    samples=[];scopes=set();total=session.args.warmup_cycles+session.args.cycles
    for cycle in range(total):
        disk_gate(session.base,reserve_bytes=session.args.minimum_free_mib*1024**2)
        selected=cycle%5
        session.keys('cockpit','1','/','C-a');session.text('cockpit',f'Modal validation {selected}');session.keys('cockpit','Escape')
        until(lambda:session.beats('cockpit') and session.beats('cockpit')[-1].get('selected_session')==f'validation-modal-{selected}',
            'Keyboard exact source selection')
        session.keys('cockpit','C-r');session.wait_screen('cockpit','Review')
        beat=until(lambda:session.beats('cockpit') and session.beats('cockpit')[-1]
            if session.beats('cockpit') and session.beats('cockpit')[-1].get('screen')=='ReviewScreen' and session.beats('cockpit')[-1].get('review_scope') else None,'Frozen Review scope')
        if beat['review_scope'].get('receiver')!=session.health['receiver_id'] or beat['review_scope'].get('session_id')!=f'validation-modal-{selected}':
            raise RuntimeError('Review scope differs from keyboard-selected source')
        scopes.add(json.dumps(beat['review_scope'],sort_keys=True))
        session.keys('cockpit','PageDown','Escape','u');session.wait_screen('cockpit','SELECTED TASK USAGE')
        session.keys('cockpit','PageDown','Escape')
        if session.args.investigate:
            session.keys('cockpit','C-p');session.text('cockpit','Investigate');session.keys('cockpit','Enter')
            session.wait_screen('cockpit','Investigate retained activity')
            session.text('cockpit','src/parent_0.py');session.keys('cockpit','Enter')
            session.wait_screen('cockpit','retained observations')
            investigation=until(lambda:session.beats('cockpit')[-1] if session.beats('cockpit') and session.beats('cockpit')[-1].get('screen')=='InvestigationsScreen' and session.beats('cockpit')[-1].get('investigation_rows') else None,'Bounded Investigate results')
            if investigation['investigation_rows']>250 or investigation['investigation_pages']>5 or investigation['investigation_bytes']>2*1024**2:
                raise RuntimeError('Investigate result retention bound exceeded')
            session.keys('cockpit','Enter')
            session.wait_screen('cockpit','Exact recorded source');session.keys('cockpit','c','x','PageDown','Escape','Escape')
        until(lambda:session.beats('cockpit') and session.beats('cockpit')[-1]['screen']=='ObservatoryScreen','Modal return')
        beat=session.wait_ui('cockpit')
        if beat['review_cache_entries']>4 or beat['review_cache_bytes']>8*1024**2 or beat['modal_stack']!=1 or beat['native_snapshot_entries']>2:
            raise RuntimeError('Modal/cache retention bound exceeded')
        point=process_sample(session.roles['cockpit'])
        if point is None:raise RuntimeError('Cockpit exited in modal loop')
        point.update(cycle=cycle,warmup=cycle<session.args.warmup_cycles,cache_entries=beat['review_cache_entries'],cache_bytes=beat['review_cache_bytes'])
        samples.append(point);append(session.base/'modal-samples.jsonl',point)
        print(json.dumps(dict(kind='modal-progress',cycle=cycle+1,total=total)),flush=True)
    retained=[row['rss_mib'] for row in samples if not row['warmup']]
    if len(scopes)<min(5,total):raise RuntimeError('Modal loop did not exercise distinct Review scopes')
    session.report.update(warmup_cycles=session.args.warmup_cycles,measured_cycles=len(retained),investigate=session.args.investigate,
        forced_gc=False,retained_rss_mib=dict(first=retained[0],last=retained[-1],max=max(retained),median=nearest_rank(retained,.5)),
        distinct_review_scopes=len(scopes),stability_conclusion='RSS plateau requires review of the retained sample series; no universal RSS threshold is inferred.',
        limitations=['Fixed generated inputs; no human task reconstruction or live runtime claim.'])


def prepare(args):
    disk_gate(args.output,reserve_bytes=args.minimum_free_mib*1024**2);create_owned_directory(args.output)
    seed=args.output/'raw-seed.db';seed_history(seed,traces=3 if args.tiny else 5751,span_count=12 if args.tiny else 31000,log_count=20 if args.tiny else 41000)
    with closing(sqlite3.connect(seed)) as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    value=dict(mode='prepare-seed',tiny=args.tiny,application=application_revision(args.app_root),harness=harness_manifest(),seed=raw_manifest(seed))
    write_json(args.output/'results.json',value);print(json.dumps(value,indent=2));return 0


def smoke(args):
    disk_gate(args.output,reserve_bytes=args.minimum_free_mib*1024**2);create_owned_directory(args.output)
    seed=args.output/'raw-seed.db';seed_history(seed,traces=3,span_count=12,log_count=20)
    with closing(sqlite3.connect(seed)) as db:db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    seed_data=raw_manifest(seed);clone=clone_seed(seed,args.output/'working.db',reserve_bytes=args.minimum_free_mib*1024**2)
    if seed_data['raw_sha256']!=clone['raw_sha256']:raise RuntimeError('Smoke raw facts differ')
    value=dict(mode='tiny-fixture-smoke',passed=True,not_a_performance_measurement=True,seed=seed_data,clone=clone,
        application=application_revision(args.app_root),harness=harness_manifest())
    import importlib.util
    if importlib.util.find_spec('observatory.attention_runner') is not None:
        from scripts.validation_support import load_harness
        from types import SimpleNamespace
        value['native_durable_cycle']=load_harness('soak_workflows.py').native_durable_cycle(
            args.output,SimpleNamespace(receiver_id='validation-smoke'),0)
    else:value['native_durable_cycle']='unsupported by this application revision; not exercised'
    write_json(args.output/'results.json',value);cleanup_clone(args.output,'working.db',children_stopped=True)
    print(json.dumps(value,indent=2));return 0


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode',choices=('prepare-seed','smoke','native-race','modals','cleanup'))
    parser.add_argument('--app-root',type=Path,default=ROOT);parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--seed',type=Path);parser.add_argument('--minimum-free-mib',type=int,default=4096)
    parser.add_argument('--tiny',action='store_true');parser.add_argument('--cycles',type=int,default=100)
    parser.add_argument('--warmup-cycles',type=int,default=10);parser.add_argument('--investigate',action='store_true')
    parser.add_argument('--clone-name',default='telemetry.db');parser.add_argument('--children-stopped',action='store_true')
    args=parser.parse_args();args.app_root=args.app_root.resolve();args.output=args.output.resolve()
    if args.minimum_free_mib<1 or args.cycles<1 or args.warmup_cycles<0:parser.error('Positive budgets/cycles required')
    if args.mode=='cleanup':print(json.dumps(cleanup_clone(args.output,args.clone_name,children_stopped=args.children_stopped)));return 0
    if args.output.exists():parser.error('Output directory must be new')
    bind_application(args.app_root)
    if args.mode=='prepare-seed':return prepare(args)
    if args.mode=='smoke':return smoke(args)
    if not shutil.which('tmux') or sys.platform!='linux':parser.error('Real tmux and Linux /proc required')
    session=FixtureSession(args)
    try:
        session.start()
        (native_race if args.mode=='native-race' else modal_cycles)(session)
    except Exception as exc:session.report['errors'].append(type(exc).__name__+': '+str(exc))
    finally:session.close()
    print(json.dumps(session.report,indent=2));return 0 if session.report['passed'] else 1


if __name__=='__main__':
    with patch('urllib.request.getproxies',return_value={}):raise SystemExit(main())
