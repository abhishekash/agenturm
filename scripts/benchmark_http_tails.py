"""Authenticated HTTP tails from two complete source trees, on generated-only clones.

No native monitor, watcher, launcher, provider or notification is started. Long
runs require explicit invocation after a coordinated quiet window. Smoke results
are nonpublishable. Each tree initializes its own ordinary production schema.
"""
from __future__ import annotations
import argparse
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import re
import sqlite3
import subprocess
import sys
import threading
import time

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from scripts.validation_support import (application_revision,bind_application,cleanup_clone,clone_seed,
    create_owned_directory,disk_gate,file_hash,harness_manifest,isolated_env,nearest_rank,
    raw_manifest,readonly,storage_snapshot,fresh_schema_manifest,prepare_raw_clone,verify_named_indexes)

BASELINE='5e0c397'
CANDIDATE='55c6244'
ENDPOINTS=('/api/sessions?limit=500','/api/mcp?limit=100')
PROTOCOL='agenturm-authenticated-http-tails-v1'
TRACE=hashlib.sha256(PROTOCOL.encode()).hexdigest()[:32]
MAX_RESPONSE=16*1024**2


def utc():return datetime.now(timezone.utc).isoformat()


def invalid_constant(value):raise ValueError('Nonfinite JSON value')


def protocol(smoke=False):
    return dict(smoke=smoke,warmups=1 if smoke else 5,idle_pairs=2 if smoke else 20,
        loaded_pairs=2 if smoke else 40,read_period_s=.05 if smoke else 1.,
        write_period_s=.05 if smoke else .1,write_batches=10 if smoke else 420,
        read_offset_s=.05 if smoke else .3,records_per_batch=2,
        endpoints=list(ENDPOINTS),request_timeout_s=5.,ready_timeout_s=600.,
        deadline_policy='No skipped slots or burst catchup; lateness over one respective period invalidates the run.',
        preparation_protocol='clean-raw-schema-v1, registered clone only; full source production Store builds its own derived schema.',
        process_boundary='External coordinator clients/writer; separate receiver-only child per full source tree.',
        namespace_contract='Every GET requires exact namespace header and compatible body; /api/spans POST is not atomically namespace-fenced.',
        latency_definition='New loopback HTTP connection: immediately before request until full response body received; JSON validation follows timing.',
        loaded_definition='All attempts inside fixed scheduled writer window; literal write intersections are recorded, never used to select samples.')


def write_json(path,value):
    path=Path(path);temporary=path.with_name('.'+path.name+'.'+secrets.token_hex(8)+'.tmp')
    try:
        with temporary.open('x')as stream:
            temporary.chmod(0o600);json.dump(value,stream,indent=2,allow_nan=False);stream.write('\n')
        os.replace(temporary,path)
    finally:
        if temporary.exists():temporary.unlink()


def source_state(root):
    value=application_revision(root)
    files={}
    for path in sorted((Path(root)/'observatory').rglob('*')):
        if path.is_file() and path.suffix in {'.py','.tcss'}:files[str(path.relative_to(root))]=file_hash(path)
    value['files']=files
    return value


def harness_state():
    value=harness_manifest()
    value['http_tails_sha256']=file_hash(Path(__file__))
    return value


def payload(sequence,base_ns):
    stamp=base_ns+sequence*100_000_000
    owner={'session.id':'http-tail-write-session-v1','agent.id':'http-tail-write-worker-v1'}
    return dict(service='http-tail-generated',spans=[
        dict(trace_id=TRACE,span_id='0000000000000001',name='agent.run',start_unix_nano=base_ns,
            end_unix_nano=stamp+1,status='OK',attributes={**owner,'agent.run.state':'running'}),
        dict(trace_id=TRACE,span_id=f'{sequence+2:016x}',parent_span_id='0000000000000001',
            name='MCP generated search',start_unix_nano=stamp,end_unix_nano=stamp+1,status='OK',
            attributes={**owner,'mcp.method.name':'tools/call','mcp.server.name':'http-tail-generated',
                'mcp.tool.name':'search','input.value':{'query':'synthetic','sequence':sequence},
                'output.value':{'count':2,'sequence':sequence}})])


def http_attempt(port,path,token,*,method='GET',body=None,receiver=None,timeout=5.):
    """One real direct HTTP attempt; failures remain observations, never retries."""
    headers={'Accept':'application/json'}
    if token is not None:headers['Authorization']='Bearer '+token
    if body is not None:headers['Content-Type']='application/json'
    if receiver:headers['X-Agenturm-Expected-Receiver-ID']=receiver
    row=dict(path=path,method=method,started_ns=time.monotonic_ns(),status=None,bytes=0,error=None)
    connection=http.client.HTTPConnection('127.0.0.1',port,timeout=timeout)
    data=None
    try:
        connection.request(method,path,body=body,headers=headers)
        response=connection.getresponse();row['status']=response.status
        row['receiver_header']=response.getheader('X-Agenturm-Receiver-ID')
        raw=response.read(MAX_RESPONSE+1)
        row.update(bytes=len(raw),finished_ns=time.monotonic_ns(),body_sha256=hashlib.sha256(raw).hexdigest())
        if len(raw)>MAX_RESPONSE:raise ValueError('Response byte cap exceeded')
        length=response.getheader('Content-Length')
        if length is None or int(length)!=len(raw):raise ValueError('Response length is absent or inconsistent')
        if response.getheader('Content-Type','').split(';')[0]!='application/json':raise ValueError('Response is not JSON')
        data=json.loads(raw,parse_constant=invalid_constant)
        if not isinstance(data,dict):raise ValueError('Response is not an object')
    except Exception as exc:
        row['error']=type(exc).__name__+': '+str(exc)
    finally:
        connection.close()
        row.setdefault('finished_ns',time.monotonic_ns())
    row['elapsed_ms']=(row['finished_ns']-row['started_ns'])/1e6
    row['valid']=row['error'] is None and row['status']==200
    return row,data


def validate_read(row,data,path,receiver):
    issues=[]
    if not row['valid']:issues.append('HTTP attempt failed')
    if not isinstance(data,dict):issues.append('Missing response object');data={}
    if row.get('receiver_header')!=receiver or ('receiver_id'in data and data['receiver_id']!=receiver):
        issues.append('Receiver namespace mismatch')
    key='sessions' if path.startswith('/api/sessions?') else 'calls'
    rows=data.get(key)
    if not isinstance(rows,list) or not rows or any(not isinstance(item,dict) for item in rows):
        issues.append('Expected nonempty object rows');rows=[]
    limit=500 if key=='sessions' else 100
    if len(rows)>limit:issues.append('Response exceeds selected limit')
    summary=data.get('summary')
    if key=='calls' and (not isinstance(summary,dict) or type(summary.get('calls'))is not int or summary['calls']<len(rows)):
        issues.append('Missing or incoherent MCP inventory')
    row.update(valid=not issues,validation_errors=issues,shape=dict(top_keys=sorted(data),rows_key=key,
        returned_rows=len(rows),row_keys=sorted({key for item in rows for key in item}),
        nonnull_counts={key:sum(item.get(key)is not None for item in rows)for key in
            ('service','services','agent_id','session_id','trace_id','result','arguments','output')},
        summary_calls=summary.get('calls') if isinstance(summary,dict) else None))
    return row


def summarize(rows):
    return {path:dict(attempts=len(selected:=[row for row in rows if row['path']==path]),
        valid_attempts=sum(row['valid']for row in selected),
        median_ms=nearest_rank([row['elapsed_ms']for row in selected],.5),
        p95_ms=nearest_rank([row['elapsed_ms']for row in selected],.95),
        maximum_ms=max((row['elapsed_ms']for row in selected),default=None))for path in ENDPOINTS}


def intersect_writes(reads,writes):
    for row in reads:
        row['overlapping_write_attempts']=[write['sequence']for write in writes
            if write['started_ns']<row['finished_ns'] and write['finished_ns']>row['started_ns']]
        row['accepted_writes_finished_during']=[write['sequence']for write in writes
            if write['valid'] and row['started_ns']<=write['finished_ns']<=row['finished_ns']]


def until_ready(port,token,*,timeout,process_error=lambda:None):
    deadline=time.monotonic()+timeout;observations=[];receiver=None
    while time.monotonic()<deadline:
        if problem:=process_error():raise RuntimeError(problem)
        row,data=http_attempt(port,'/api/health',token)
        observations.append(dict(status=row['status'],error=row['error'],at_ns=row['started_ns'],
            indexing=data.get('indexing') if isinstance(data,dict) else None))
        if row['valid'] and isinstance(data,dict):
            current=data.get('receiver_id')
            if not isinstance(current,str)or not re.fullmatch('[0-9a-f]{32}',current):
                raise RuntimeError('Receiver identity is malformed')
            if row.get('receiver_header')!=current:raise RuntimeError('Health namespace header disagrees')
            if receiver and current!=receiver:raise RuntimeError('Receiver changed during readiness')
            if current:receiver=current
            if data.get('local_agents',{}).get('enabled')is not False:raise RuntimeError('Native monitor unexpectedly enabled')
            if data.get('security',{}).get('authentication_required')is not True:raise RuntimeError('Authentication is not enabled')
            if data.get('indexing',{}).get('ready')is True and receiver:
                return receiver,observations
        time.sleep(.05)
    raise RuntimeError('Strict authenticated/index readiness deadline exceeded')


def run_reads(port,token,receiver,count,*,period=None,start_ns=None,missed_slots=None):
    rows=[]
    for pair in range(count):
        scheduled=start_ns+int(pair*period*1e9) if period is not None else None
        if scheduled is not None:
            time.sleep(max(0,(scheduled-time.monotonic_ns())/1e9))
            observed=time.monotonic_ns()
            if observed-scheduled>int(period*1e9):
                if missed_slots is not None:missed_slots.append(dict(kind='read',pair=pair,scheduled_ns=scheduled,observed_ns=observed))
                break
        paths=ENDPOINTS if pair%2==0 else tuple(reversed(ENDPOINTS))
        pair_started=time.monotonic_ns()
        for path in paths:
            row,data=http_attempt(port,path,token,receiver=receiver)
            row.update(pair=pair,pair_scheduled_ns=scheduled,pair_started_ns=pair_started,
                pair_lateness_ms=max(0,(pair_started-scheduled)/1e6)if scheduled is not None else 0)
            rows.append(validate_read(row,data,path,receiver))
    return rows


def phase_loaded(port,token,receiver,settings,base_ns):
    reads=[];writes=[];errors=[];missed=[];stop=threading.Event()
    prepared=time.monotonic_ns()
    bodies=[json.dumps(payload(index,base_ns),sort_keys=True,separators=(',',':')).encode()
            for index in range(settings['write_batches'])]
    payload_preparation_ms=(time.monotonic_ns()-prepared)/1e6
    start_ns=time.monotonic_ns()+100_000_000
    last_ns=start_ns+int(settings['write_batches']*settings['write_period_s']*1e9)
    def writer():
        for sequence,body in enumerate(bodies):
            scheduled=start_ns+int(sequence*settings['write_period_s']*1e9)
            if stop.wait(max(0,(scheduled-time.monotonic_ns())/1e9)):break
            if time.monotonic_ns()-scheduled>int(settings['write_period_s']*1e9):
                missed.append(dict(kind='write',sequence=sequence,scheduled_ns=scheduled,observed_ns=time.monotonic_ns()))
                errors.append('Writer missed its slot period; refusing burst catchup');stop.set();break
            row,data=http_attempt(port,'/api/spans',token,method='POST',body=body,receiver=receiver)
            accepted=data.get('accepted') if isinstance(data,dict) else None
            row.update(sequence=sequence,scheduled_ns=scheduled,lateness_ms=max(0,(row['started_ns']-scheduled)/1e6),
                request_sha256=hashlib.sha256(body).hexdigest(),request_bytes=len(body),accepted=accepted)
            row['valid']=row['valid'] and type(accepted)is int and accepted==settings['records_per_batch']
            writes.append(row)
            if row['lateness_ms']>settings['write_period_s']*1000:
                errors.append('Writer missed its slot period; refusing burst catchup');stop.set();break
    thread=threading.Thread(target=writer,name='generated-http-writer');thread.start()
    try:
        reads=run_reads(port,token,receiver,settings['loaded_pairs'],period=settings['read_period_s'],
            start_ns=start_ns+int(settings['read_offset_s']*1e9),missed_slots=missed)
        remaining=(last_ns-time.monotonic_ns())/1e9
        thread.join(max(0,remaining)+settings['request_timeout_s']+2)
        if thread.is_alive():errors.append('Writer did not finish its fixed window');stop.set();thread.join(7)
    finally:
        if thread.is_alive():stop.set();thread.join(7)
    if thread.is_alive():raise RuntimeError('Owned writer did not stop')
    intersect_writes(reads,writes)
    if len(reads)!=2*settings['loaded_pairs']:errors.append('Fixed read count/slot deadline failed; no catchup attempted')
    if len(writes)!=settings['write_batches']or any(not row['valid']for row in writes):errors.append('Fixed writer count/acceptance failed')
    if any(row['started_ns']<start_ns or row['finished_ns']>last_ns for row in reads):errors.append('Read escaped scheduled writer window')
    if any(row['pair_lateness_ms']>settings['read_period_s']*1000 for row in reads):errors.append('Reader missed its pair slot period')
    if any(not row['valid']for row in reads):errors.append('Loaded HTTP response failed')
    progress={};accepted=[row for row in writes if row['valid']]
    for path in ENDPOINTS:
        selected=[row for row in reads if row['path']==path]
        if not selected:continue
        first=min(row['started_ns']for row in selected);last=max(row['finished_ns']for row in selected)
        progress[path]=dict(accepted_before=sum(row['finished_ns']<=first for row in accepted),
            accepted_during=sum(first<row['finished_ns']<last for row in accepted),
            accepted_after=sum(row['finished_ns']>=last for row in accepted))
        if not all(progress[path].values()):errors.append('Accepted writer progress did not span endpoint sample window')
    return dict(reads=reads,writes=writes,errors=errors,start_ns=start_ns,scheduled_end_ns=last_ns,
        accepted_records=sum(row['accepted']for row in writes if row['valid']),statistics=summarize(reads),
        payload_preparation_ms=payload_preparation_ms,accepted_progress=progress,missed_slots=missed)


def child(args):
    """Receiver only; coordinator/writer use real HTTP from another process."""
    config=json.loads(args.config.read_text());base=Path(config['output']);settings=config['settings']
    report=dict(protocol=PROTOCOL,role=config['role'],pid=os.getpid(),settings=settings,started_utc=utc(),errors=[],
        api_only=True,native_monitor_enabled=False,providers_invoked=False)
    store=server=thread=None;started=time.monotonic_ns()
    try:
        report['source_before']=source_state(config['app_root']);report['harness_before']=harness_state()
        if not settings['smoke'] and (report['source_before']['dirty']or report['harness_before']['revision']['dirty']):
            raise ValueError('Controlled runtime/harness sources must be committed and clean')
        report['imports']=bind_application(config['app_root'])
        from observatory.core import Store
        from observatory.server import BoundedHTTPServer,handler_for
        report['seed_before']=raw_manifest(config['seed'])
        report['clone']=clone_seed(config['seed'],base/'working.db')
        report['fresh_schema']=fresh_schema_manifest(base,Store)
        report['raw_schema_preparation']=prepare_raw_clone(base,'working.db',schema_meta_sql=report['fresh_schema']['schema_meta_sql'])
        write_json(base/'fresh-schema.json',report['fresh_schema'])
        store=Store(base/'working.db',async_derived_refresh=True)
        server=BoundedHTTPServer(('127.0.0.1',0),handler_for(store,config['token'],quiet=True))
        thread=threading.Thread(target=server.serve_forever,name='private-http-receiver');thread.start()
        write_json(base/'endpoint.json',dict(port=server.server_port,pid=os.getpid()))
        while not (base/'stop.json').exists():
            if os.getppid()!=config['parent_pid']:raise RuntimeError('Owning coordinator exited')
            time.sleep(.05)
        report['preparation_and_run_ms']=(time.monotonic_ns()-started)/1e6
    except Exception as exc:report['errors'].append(type(exc).__name__+': '+str(exc))
    finally:
        try:
            if server:server.shutdown();server.server_close()
            if thread:thread.join(7)
            if thread and thread.is_alive():raise RuntimeError('HTTP server did not stop')
            if store:
                workers=[getattr(store,name,None)for name in ('_derived_thread','_migration_thread')]
                store.close()
                if any(worker and worker.is_alive()for worker in workers):raise RuntimeError('Store workers did not stop')
                if store.last_derived_error:raise RuntimeError('Derived worker drain error')
            report['drained']=True
        except Exception as exc:report['errors'].append('Shutdown '+type(exc).__name__+': '+str(exc))
        report['source_after']=source_state(config['app_root']);report['harness_after']=harness_state()
        if report.get('source_before')!=report['source_after']or report.get('harness_before')!=report['harness_after']:
            report['errors'].append('Runtime/harness changed')
        report['seed_after']=raw_manifest(config['seed'])
        if report.get('seed_before')!=report['seed_after']:report['errors'].append('Immutable raw seed changed')
        if (base/'working.db').exists():
            report['schema_after']=storage_snapshot(base/'working.db')
            if any(count for name,count in report['schema_after']['rows'].items()if 'dirty'in name):
                report['errors'].append('Derived/index queues remain after shutdown')
        report['finished_utc']=utc();write_json(base/'receiver-results.json',report)
    return 0 if not report['errors']else 1


def measure_version(config,process,result):
    base=Path(config['output']);settings=config['settings'];token=config['token'];started=time.monotonic_ns()
    result.update(client_pid=os.getpid(),request_origin='External coordinator process; receiver owns Store/server only.')
    deadline=time.monotonic()+settings['ready_timeout_s']
    while not (base/'endpoint.json').exists():
        if process.poll()is not None:raise RuntimeError('Receiver exited before publishing endpoint')
        if time.monotonic()>deadline:raise RuntimeError('Receiver construction deadline exceeded')
        time.sleep(.05)
    endpoint=json.loads((base/'endpoint.json').read_text());port=endpoint['port']
    if endpoint['pid']!=process.pid:raise RuntimeError('Receiver process identity differs')
    bad,_=http_attempt(port,'/api/health',None);result['unauthenticated_status']=bad['status']
    if bad['status']!=401:raise RuntimeError('Unauthenticated API request was not refused')
    problem=lambda:'Receiver exited'if process.poll()is not None else None
    receiver,ready=until_ready(port,token,timeout=settings['ready_timeout_s'],process_error=problem)
    result.update(receiver_id=receiver,readiness=ready,initial_readiness_ms=(time.monotonic_ns()-started)/1e6)
    result['production_indexes']=verify_named_indexes(base/'working.db',json.loads((base/'fresh-schema.json').read_text()))
    result['clone_raw_after_ready']=raw_manifest(base/'working.db')
    if result['clone_raw_after_ready']['raw_sha256']!=raw_manifest(config['seed'])['raw_sha256']:
        raise RuntimeError('Normal initialization changed canonical raw facts')
    result['schema_before']=storage_snapshot(base/'working.db')
    with closing(readonly(base/'working.db'))as db:
        result['schema_manifest']=[dict(type=row[0],name=row[1],sql=row[2])for row in db.execute(
            "SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name")]
        if db.execute('SELECT 1 FROM trace_activity WHERE trace_id=?',(TRACE,)).fetchone():raise ValueError('Writer trace collides with seed')
        base_ns=int(db.execute('SELECT MAX(last_ns) FROM trace_activity').fetchone()[0])+1_000_000_000
    result['warmup']=run_reads(port,token,receiver,settings['warmups'])
    if any(not row['valid']for row in result['warmup']):raise RuntimeError('Warmup response failed')
    result['preparation_ms']=(time.monotonic_ns()-started)/1e6
    result['measurement_started_utc']=utc();result['measurement_started_ns']=time.monotonic_ns()
    result['idle']=run_reads(port,token,receiver,settings['idle_pairs']);result['idle_statistics']=summarize(result['idle'])
    if any(not row['valid']for row in result['idle']):result['errors'].append('Idle HTTP response failed')
    same,_=until_ready(port,token,timeout=settings['ready_timeout_s'],process_error=problem)
    if same!=receiver:raise RuntimeError('Receiver changed before loaded phase')
    result['loaded']=phase_loaded(port,token,receiver,settings,base_ns);result['errors'].extend(result['loaded']['errors'])
    result['measurement_finished_utc']=utc();result['measurement_finished_ns']=time.monotonic_ns()
    same,drain=until_ready(port,token,timeout=settings['ready_timeout_s'],process_error=problem)
    if same!=receiver:raise RuntimeError('Receiver changed after loaded phase')
    result['drain']=drain
    return result


def run(args):
    output=args.output.resolve();create_owned_directory(output)
    settings=protocol(args.smoke);seed=raw_manifest(args.seed)
    report=dict(protocol=PROTOCOL,settings=settings,seed=seed,started_utc=utc(),versions={},errors=[],
        harness_before=harness_state(),coordinator_pid=os.getpid())
    try:
        if not args.smoke and seed['counts']!={'spans':31000,'logs':41000,'trace_activity':5751}:
            raise ValueError('Use the registered 5,751-trace common generated corpus; smaller corpora are smoke-only')
        for role,app_root,expected in (('B',args.baseline_root,BASELINE),('C',args.candidate_root,CANDIDATE)):
            app_root=app_root.resolve();actual=application_revision(app_root)
            expected_full=subprocess.check_output(['git','rev-parse',expected],cwd=ROOT,text=True).strip()
            if not args.smoke and actual['head']!=expected_full:raise ValueError('Use exact registered '+role+' source commit')
            base=output/role;create_owned_directory(base)
            config=dict(output=str(base),app_root=str(app_root),seed=str(args.seed.resolve()),role=role,settings=settings,
                token=secrets.token_hex(32),parent_pid=os.getpid())
            write_json(base/'config.json',config);(base/'config.json').chmod(0o600)
            result=dict(errors=[]);process=None
            with (base/'child.log').open('w')as stream:
                process=subprocess.Popen([sys.executable,str(Path(__file__)),'_child','--config',str(base/'config.json')],
                    env=isolated_env(base,app_root),cwd=base,stdout=stream,stderr=subprocess.STDOUT)
                try:measure_version(config,process,result)
                except Exception as exc:result['errors'].append(type(exc).__name__+': '+str(exc))
                finally:
                    write_json(base/'stop.json',{'stop':True})
                    try:process.wait(timeout=40)
                    except subprocess.TimeoutExpired:
                        process.kill();process.wait();result['errors'].append('Receiver required forced termination')
            receiver_result=json.loads((base/'receiver-results.json').read_text())if (base/'receiver-results.json').exists()else dict(errors=['Receiver result unavailable'])
            result['errors'].extend(receiver_result.pop('errors'));result.update(receiver_result)
            result['publishable']=not args.smoke and not result['errors']and 'loaded'in result
            result['loaded_p95_under100ms']={path:(result['loaded']['statistics'][path]['p95_ms']<100
                if result['publishable']else None)for path in ENDPOINTS}
            write_json(base/'results.json',result)
            report['versions'][role]=dict(returncode=process.returncode,result=result)
            if process.returncode or result['errors']:report['errors'].append(role+' invalid: '+'; '.join(result['errors']))
            report['versions'][role]['removed']=cleanup_clone(base,'working.db',children_stopped=True)if result.get('drained')and (base/'working.db').exists()else []
            write_json(output/'results.json',report)
        results=[report['versions'][role]['result']for role in ('B','C')]
        if all('loaded'in result for result in results):
            report['writer_inputs_equal']=[row['request_sha256']for row in results[0]['loaded']['writes']]==[row['request_sha256']for row in results[1]['loaded']['writes']]
            if not report['writer_inputs_equal']:report['errors'].append('B/C writer inputs differ')
        else:report['errors'].append('A version did not complete the registered HTTP protocol')
        if all('pid'in result for result in results)and results[0]['pid']==results[1]['pid']:report['errors'].append('B/C did not use distinct processes')
    except Exception as exc:report['errors'].append(type(exc).__name__+': '+str(exc))
    finally:
        report['harness_after']=harness_state()
        if report['harness_before']!=report['harness_after']:report['errors'].append('Coordinator harness changed')
        report['seed_after']=raw_manifest(args.seed)
        if report['seed_after']!=seed:report['errors'].append('Shared seed changed across runs')
        report['publishable']=not args.smoke and not report['errors']and len(report['versions'])==2 and all(value['result']['publishable']for value in report['versions'].values())
        report['finished_utc']=utc();write_json(output/'results.json',report)
    print(json.dumps(dict(output=str(output/'results.json'),publishable=report['publishable'],errors=report['errors'])))
    return 0 if not report['errors']else 1


def main():
    parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='command',required=True)
    public=sub.add_parser('run');public.add_argument('--seed',type=Path,required=True)
    public.add_argument('--verified-synthetic-source',action='store_true',required=True)
    public.add_argument('--baseline-root',type=Path,required=True);public.add_argument('--candidate-root',type=Path,required=True)
    public.add_argument('--output',type=Path,required=True);public.add_argument('--smoke',action='store_true')
    internal=sub.add_parser('_child');internal.add_argument('--config',type=Path,required=True)
    args=parser.parse_args();return child(args)if args.command=='_child'else run(args)


if __name__=='__main__':raise SystemExit(main())
