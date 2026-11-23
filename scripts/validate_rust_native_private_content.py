"""Disposable, root-only broker private evidence qualification.

No system accounts are created. Ownership changes stay inside one temporary
fixture; execute only in an isolated Linux VM or container.
"""
import argparse
import grp
import pwd
import fcntl
import pty
import select
import struct
import termios
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bin-dir',type=Path,required=True)
args=parser.parse_args()
if os.geteuid()!=0:
    parser.error('run only in a disposable root-capable Linux VM or container')
unused=[]
for candidate in range(45000,46000):
    try: pwd.getpwuid(candidate)
    except KeyError: unused.append(candidate)
    if len(unused)==4: break
if len(unused)!=4: parser.error('four unused numeric identities are required')
BROKER,COLLECTOR,READER,ADMIN=unused
for candidate in range(45000,46000):
    try: grp.getgrgid(candidate)
    except KeyError:
        GID=candidate
        break
else: parser.error('an unused primary group is required')
SOURCE_GID=next(candidate for candidate in range(GID+1,46000) if all(group.gr_gid!=candidate for group in grp.getgrall()))
binaries=args.bin_dir.resolve(strict=True)
APP=str(binaries/'agenturm')
SERVICE=str(binaries/'agenturm-storage-service')
ADAPTER=str(binaries/'agenturm-pi-harness')
CODEX=str(binaries/'agenturm-codex-harness')
HOST=str(binaries/'agenturm-plugin-host')

def drop(uid,gid=GID,groups=()):
    def prepare():
        os.setgroups(groups)
        os.setgid(gid)
        os.setuid(uid)
    return prepare

with tempfile.TemporaryDirectory(prefix='broker-private-') as tmp:
    root=Path(tmp);root.chmod(0o755)
    homes={}
    for uid in [COLLECTOR,READER,ADMIN]:
        home=root/f'home-{uid}';home.mkdir();os.chown(home,uid,GID);home.chmod(0o700);homes[uid]=home
    data=root/'data';data.mkdir();os.chown(data,BROKER,BROKER);data.chmod(0o700)
    transport=root/'transport';transport.mkdir();os.chown(transport,BROKER,GID);transport.chmod(0o2710)
    source=root/'pi.jsonl'
    original='{"type":"session","id":"private-session"}\n{"type":"message","id":"prompt-1","timestamp":"2026-11-20T12:00:00Z","message":{"role":"user","content":"Review the bounded broker fixture"}}\n'
    source.write_text(original);os.chown(source,COLLECTOR,SOURCE_GID);source.chmod(0o640)
    codex=root/'codex.jsonl'
    codex.write_text(''.join(json.dumps(row)+'\n' for row in [
        {'type':'session_meta','payload':{'id':'edit-session'}},
        {'type':'event_msg','timestamp':'2026-11-19T12:00:01Z','payload':{
            'type':'item_completed','thread_id':'edit-session','turn_id':'edit-turn',
            'item':{'type':'FileChange','id':'fixture-edit','status':'completed','changes':{
                'src/before.txt':{'type':'update','unified_diff':'@@ -1 +1 @@\n-old\n+verified line\n','move_path':'src/after.txt'}
            }}
        }},
        {'type':'event_msg','timestamp':'2026-11-19T12:00:02Z','payload':{
            'type':'item_completed','thread_id':'edit-session','turn_id':'command-turn',
            'item':{'type':'CommandExecution','id':'fixture-command','status':'completed',
                    'command':['/bin/bash','-lc','pytest isolated smoke'],
                    'aggregated_output':'private command output'}
        }},
    ]))
    os.chown(codex,COLLECTOR,SOURCE_GID);codex.chmod(0o640)
    sources=[('harness.pi','example.rust-pi-harness',ADAPTER,source),
             ('harness.codex','example.rust-codex-harness',CODEX,codex)]
    pins={artifact:hashlib.sha256(Path(artifact).read_bytes()).hexdigest() for _,_,artifact,_ in sources}
    features=['source-adapter/1','configured-root/1']
    catalog=root/'catalog.json';catalog.write_text(json.dumps(dict(version=1,plugins=[dict(plugin_id=plugin,adapter=artifact,sha256=pins[artifact],protocol=1,features=features,observation_format='agenturm-observations/1') for _,plugin,artifact,_ in sources])))
    config=root/'sources.json';config.write_text(json.dumps(dict(version=2,plugin_host=HOST,plugin_catalog=str(catalog),sources=[dict(plugin_id=plugin,source_root=str(path),source_id=source_id) for source_id,plugin,_,path in sources])))
    private=root/'content.json';private.write_text(json.dumps(dict(version=1,sources=[dict(source_id=source_id,plugin_id=plugin,plugin_host=HOST,adapter=artifact,artifact_sha256=pins[artifact],features=features,source_root=str(path)) for source_id,plugin,artifact,path in sources])));os.chown(private,BROKER,BROKER);private.chmod(0o600)
    endpoint=root/'broker.json';endpoint.write_text(json.dumps(dict(version=1,socket=str(transport/'agenturm-native-storage-v1.sock'),receiver_id='receiver:private',service_uid=BROKER,service_gid=BROKER)))
    service=subprocess.Popen([SERVICE,'--data-root',data,'--transport-dir',transport,'--receiver-id','receiver:private','--client-uid',str(COLLECTOR),'--client-gid',str(GID),'--native-sources','harness.pi,harness.codex','--native-reader',f'{READER}:{GID}','--native-grant-admin',f'{ADMIN}:{GID}','--native-content-config',private],preexec_fn=drop(BROKER,BROKER,(SOURCE_GID,)))
    def run(uid,*args,ok=True,broker=True):
        command=[APP]
        if broker: command+=['broker']
        command+=list(map(str,args))
        if broker: command+=['--broker-config',str(endpoint)]
        result=subprocess.run(command,preexec_fn=drop(uid),env={'HOME':str(homes[uid]),'PATH':''},capture_output=True,text=True,timeout=15)
        assert (result.returncode==0)==ok,(command,result.returncode,result.stdout,result.stderr)
        if not ok:
            assert 'Review the bounded broker fixture' not in result.stdout
            assert 'verified line' not in result.stdout
            assert 'pytest isolated smoke' not in result.stdout
        return result
    try:
        for _ in range(250):
            if (transport/'agenturm-native-storage-v1.sock').exists():break
            assert service.poll() is None
            time.sleep(.04)
        run(COLLECTOR,'collect','--config',config)
        # The second poll has shell activity only, without a message/edit/MCP
        # sharing its receipt. Its proof must stand on its own after collection.
        with codex.open('a') as stream:
            stream.write(json.dumps({'type':'event_msg','timestamp':'2026-11-19T12:00:03Z','payload':{
                'type':'item_completed','thread_id':'edit-session','turn_id':'solo-command',
                'item':{'type':'CommandExecution','id':'solo-operation','status':'completed',
                        'command':['pytest','solo-control'],'aggregated_output':'private solo output'}
            }})+'\n')
        run(COLLECTOR,'collect','--config',config)
        query=run(READER,'query').stdout
        assert 'Review the bounded broker fixture' not in query
        rows=json.loads(query)['observations']
        prompt=next(row for row in rows if row['observation']['kind']=='message')
        listing=['content','list','--source-id','harness.pi','--session-id','private-session']
        run(READER,*listing,ok=False)
        instruction=['investigate','instruction','bounded broker','--source-id','harness.pi','--session-id','private-session']
        path_search=['investigate','path','src/after.txt','--source-id','harness.codex','--session-id','edit-session']
        command_search=['investigate','command','isolated smoke','--source-id','harness.codex','--session-id','edit-session','--turn-id','command-turn']
        solo_search=['investigate','command','solo-control','--source-id','harness.codex','--session-id','edit-session','--turn-id','solo-command']
        for investigation in [instruction,path_search,command_search]:
            coverage=json.loads(run(READER,*investigation).stdout)
            assert coverage['matched']==0 and coverage['unavailable_candidates']==1,coverage
        binding=root/'binding.json';binding.write_text(run(COLLECTOR,'content','binding','--config',config,'--source-id','harness.pi',broker=False).stdout)
        run(ADMIN,'grant','approve','--binding',binding)
        references=json.loads(run(READER,*listing).stdout)
        assert len(references)==1,references
        assert references[0]['event_id']==prompt['event_id']
        for uid in [COLLECTOR,ADMIN]:run(uid,*listing,ok=False)
        selection=root/'selection.json';selection.write_text(json.dumps(dict(source_id='harness.pi',session_id='private-session',event_id=prompt['event_id'],revision=prompt['revision'],kind=dict(kind='user_text'))))
        preview=['content','preview','--selection',selection]
        assert json.loads(run(READER,*preview).stdout)=='Review the bounded broker fixture'
        for uid in [COLLECTOR,ADMIN]:run(uid,*preview,ok=False)
        match,coverage=map(json.loads,run(READER,*instruction).stdout.splitlines())
        assert match['event_id']==prompt['event_id'] and coverage['matched']==1,(match,coverage)
        assert 'Review the bounded broker fixture' not in match['source_reference']
        prompt_source=['investigate','source',prompt['event_id'],'--reference',match['source_reference']]
        assert json.loads(run(READER,*prompt_source).stdout)['evidence']['text']==match['excerpt']
        for uid in [COLLECTOR,ADMIN]:
            run(uid,*instruction,ok=False)
            run(uid,*prompt_source,ok=False)
        edit_binding=root/'edit-binding.json'
        edit_binding.write_text(run(COLLECTOR,'content','binding','--config',config,
                                    '--source-id','harness.codex','--kind','codex.edit_detail',broker=False).stdout)
        run(ADMIN,'grant','approve','--binding',edit_binding)
        found,coverage=map(json.loads,run(READER,*path_search).stdout.splitlines())
        assert found['scope']['turn_id']=='edit-turn' and coverage['matched']==1,(found,coverage)
        assert 'verified line' not in json.dumps(found)
        edit_source=['investigate','source',found['event_id'],'--reference',found['source_reference']]
        change=json.loads(run(READER,*edit_source).stdout)['evidence']['changes'][0]
        assert change['path']=='src/before.txt' and change['move_path']=='src/after.txt',change
        assert 'verified line' in change['diff'],change
        run(ADMIN,'grant','revoke','--binding',edit_binding)
        run(READER,*edit_source,ok=False)
        command_binding=root/'command-binding.json'
        command_binding.write_text(run(COLLECTOR,'content','binding','--config',config,
                                       '--source-id','harness.codex','--kind','codex.command_detail',broker=False).stdout)
        run(ADMIN,'grant','approve','--binding',command_binding)
        command,coverage=map(json.loads,run(READER,*command_search).stdout.splitlines())
        expected=['/bin/bash','-lc','pytest isolated smoke']
        assert command['recorded_invocation']==expected and coverage['matched']==1,(command,coverage)
        command_source=['investigate','source',command['event_id'],'--reference',command['source_reference']]
        assert json.loads(run(READER,*command_source).stdout)['evidence']=={'recorded_invocation':expected}
        assert 'private command output' not in json.dumps(command)
        solo,solo_coverage=map(json.loads,run(READER,*solo_search).stdout.splitlines())
        assert solo['recorded_invocation']==['pytest','solo-control'] and solo_coverage['matched']==1,(solo,solo_coverage)
        solo_source=['investigate','source',solo['event_id'],'--reference',solo['source_reference']]
        assert json.loads(run(READER,*solo_source).stdout)['evidence']=={'recorded_invocation':['pytest','solo-control']}
        assert 'private solo output' not in json.dumps(solo)
        run(ADMIN,'grant','revoke','--binding',command_binding)
        run(READER,*command_source,ok=False)
        run(READER,*solo_source,ok=False)
        draft=json.loads(run(READER,'task','draft','--source-id','harness.pi','--session-id','private-session','--turn-id','prompt-1','--event-id',prompt['event_id'],'--revision',prompt['revision']).stdout)
        assert draft['excerpt']=='Review the bounded broker fixture'
        draft['continuation']='Verify the next bounded step'
        draftfile=homes[READER]/'task-draft.json';draftfile.write_text(json.dumps(draft));os.chown(draftfile,READER,GID);draftfile.chmod(0o600)
        approved=json.loads(run(READER,'task','approve-draft',draftfile,'--title','Broker continuity task').stdout)
        task_id=approved['task']['id'];approval_id=approved['approval']['id']
        assert approved['approval']['continuation']=='Verify the next bounded step'
        assert 'Review the bounded broker fixture' not in json.dumps(approved)
        run(READER,'task','approve-draft',draftfile,ok=False)
        assert len(json.loads(run(READER,'task','list').stdout)['items'])==1
        revealed=json.loads(run(READER,'task','continuity',task_id,'--approval-id',approval_id).stdout)
        assert revealed['excerpt']==draft['excerpt']
        decision=json.loads(run(READER,'task','decide',task_id,'--kind','accepted','--text','Proceed with the bounded next step','--source-file',draftfile,'--expected-revision',approved['task']['revision']).stdout)
        assert decision['decision']['source']['event_id']==prompt['event_id'],decision
        handoff=run(READER,'task','handoff',task_id,'--format','json').stdout
        assert draft['excerpt'] not in handoff
        assert 'Verify the next bounded step' in handoff
        stale=run(READER,'task','update',task_id,'--title','Stale update','--expected-revision',approved['task']['revision'],ok=False)
        assert stale.returncode==3,(stale.returncode,stale.stderr)
        for index,close_key in enumerate([b'\x1b',b'q',b'\x03']):
            if index:run(ADMIN,'grant','approve','--binding',binding)
            home=homes[READER]
            master,slave=pty.openpty()
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',18,60,0,0))
            original_termios=termios.tcgetattr(slave)
            cockpit=subprocess.Popen([APP,'broker','cockpit','--broker-config',endpoint],preexec_fn=drop(READER),stdin=slave,stdout=slave,stderr=slave,env={'HOME':str(home),'PATH':'','TERM':'xterm-256color'})
            def screen_until(expected):
                screen=b'';deadline=time.monotonic()+15
                while time.monotonic()<deadline:
                    if select.select([master],[],[],.1)[0]:screen+=os.read(master,65536)
                    if expected in screen:return screen
                    assert cockpit.poll() is None,screen
                raise AssertionError((expected,screen))
            revoke=None
            try:
                screen_until(b'private-session')
                os.write(master,b'p');screen_until(b'prompt-1')
                os.write(master,b'\r');screen_until(b'Review the bounded broker fixture')
                revoke=subprocess.Popen([APP,'broker','grant','revoke','--binding',binding,'--broker-config',endpoint],preexec_fn=drop(ADMIN),stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                time.sleep(.3)
                assert revoke.poll() is None,'revocation completed before terminal redaction'
                os.write(master,close_key)
                output,errors=revoke.communicate(timeout=10)
                assert revoke.returncode==0,(output,errors)
                if close_key==b'\x1b':
                    time.sleep(.2)
                    os.write(master,b'\x1b');time.sleep(.2);os.write(master,b'q')
                assert cockpit.wait(timeout=10)==0
                after=b''
                while select.select([master],[],[],.05)[0]:
                    try: after+=os.read(master,65536)
                    except OSError: break
                assert b'Prompts' in after,after
                assert b'\x1b[?1049l' in after,after
                assert after.index(b'Prompts')<after.index(b'\x1b[?1049l'),after
                assert termios.tcgetattr(slave)==original_termios
            finally:
                if cockpit.poll() is None:cockpit.kill();cockpit.wait(timeout=5)
                if revoke is not None and revoke.poll() is None:revoke.kill();revoke.wait(timeout=5)
                os.close(master);os.close(slave)
        run(READER,*preview,ok=False)
        run(READER,*prompt_source,ok=False)
        run(READER,'task','continuity',task_id,'--approval-id',approval_id,ok=False)
        assert draft['excerpt'] not in run(READER,'task','continuity',task_id).stdout
        run(ADMIN,'grant','approve','--binding',binding)
        read_end,write_end=os.pipe()
        os.set_blocking(write_end,False)
        filled=0
        while True:
            try:filled+=os.write(write_end,b'X'*4096)
            except BlockingIOError:break
        os.set_blocking(write_end,True)
        before_flags=fcntl.fcntl(write_end,fcntl.F_GETFL)
        blocked=None;pipe_revoke=None
        try:
            blocked=subprocess.Popen([APP,'broker','content','preview','--selection',selection,'--broker-config',endpoint],preexec_fn=drop(READER),stdout=write_end,stderr=subprocess.PIPE)
            time.sleep(.3)
            assert blocked.poll() is None
            _,errors=blocked.communicate(timeout=30)
            assert blocked.returncode!=0 and b'Review the bounded broker fixture' not in errors,errors
            # Revoke after the client expires, before the pipe consumer drains.
            # Ordinary broker RPCs have a shorter two-second reply deadline.
            run(ADMIN,'grant','revoke','--binding',binding)
            assert json.loads(run(READER,'grant','check','--binding',binding).stdout)['epoch'] is None
            assert fcntl.fcntl(write_end,fcntl.F_GETFL)==before_flags
            os.set_blocking(read_end,False)
            drained=b''
            while True:
                try:drained+=os.read(read_end,65536)
                except BlockingIOError:break
            assert drained==b'X'*filled,(len(drained),filled)
        finally:
            for process in [blocked,pipe_revoke]:
                if process is not None and process.poll() is None:process.kill();process.wait(timeout=5)
            os.close(read_end);os.close(write_end)
        run(ADMIN,'grant','approve','--binding',binding)
        run(READER,*prompt_source,ok=False)
        wrong=json.loads(selection.read_text());wrong['session_id']='other';selection.write_text(json.dumps(wrong));run(READER,*preview,ok=False)
        wrong['session_id']='private-session';selection.write_text(json.dumps(wrong))
        with source.open('a') as transcript:
            for index in range(13):
                transcript.write(json.dumps({
                    'type':'message','id':f'budget-{index}',
                    'timestamp':f'2026-11-20T12:01:{index:02}Z',
                    'message':{'role':'user','content':'scan budget fixture'}
                })+'\n')
        run(COLLECTOR,'collect','--config',config)
        budget=['investigate','instruction','scan budget','--source-id','harness.pi',
                '--session-id','private-session','--limit','100']
        first=list(map(json.loads,run(READER,*budget).stdout.splitlines()))
        assert len(first)==13 and first[-1]['matched']==12 and first[-1]['next_cursor'],first[-1]
        second=list(map(json.loads,run(READER,*budget,'--cursor',first[-1]['next_cursor']).stdout.splitlines()))
        assert len(second)==2 and second[-1]['matched']==1 and second[-1]['scope_exhausted'],second[-1]
        assert first[-1]['snapshot_id']==second[-1]['snapshot_id']
        assert len({row['event_id'] for row in first[:-1]+second[:-1]})==13
        source.rename(root/'old-source.jsonl');source.write_text(original);os.chown(source,COLLECTOR,SOURCE_GID);source.chmod(0o640)
        run(READER,*preview,ok=False)
        deny_script='import pathlib,sys\ntry: pathlib.Path(sys.argv[1]).read_bytes()\nexcept PermissionError: sys.exit(73)\nelse: sys.exit(1)'
        for uid in [COLLECTOR,READER,ADMIN]:
            denied=subprocess.run([os.sys.executable,'-c',deny_script,str(data/'history.sqlite')],preexec_fn=drop(uid),capture_output=True)
            assert denied.returncode==73,(uid,denied.returncode,denied.stdout,denied.stderr)
        for uid in [READER,ADMIN]:
            for path in [source,codex]:
                denied=subprocess.run([os.sys.executable,'-c',deny_script,str(path)],preexec_fn=drop(uid),capture_output=True)
                assert denied.returncode==73,(uid,denied.returncode,denied.stdout,denied.stderr)
        print('PASS bounded private pipe and broker instruction/path/command/source investigation, source-backed task draft/approval/decision/CAS, 60x18 cockpit display/redaction/revocation fence, private delivery, distinct reader, grant revocation, exact session/inode proof and metadata-only queries',flush=True)
    finally:
        service.terminate()
        assert service.wait(timeout=10)==0
