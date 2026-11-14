"""Opt-in generated-only SQL/lock diagnostic, not latency acceptance evidence.

Receiver, instrumented SQLite calls and real HTTP clients share one process.
All configured writes finish; missed schedules are retained as deadline failures,
never interpreted as a publishable fixed-cadence run. No native sources launch.
"""
from contextlib import closing,contextmanager
from pathlib import Path
import argparse,collections,json,sqlite3,sys,tempfile,threading,time
ROOT=Path(__file__).resolve().parents[1];sys.path.insert(0,str(ROOT))
from observatory.core import Store
from observatory.server import BoundedHTTPServer,handler_for
from scripts.benchmark_http_tails import payload,http_attempt
import observatory.storage_activity as activity
import observatory.storage_derived as derived
parser=argparse.ArgumentParser();parser.add_argument('--batches',type=int,default=60);parser.add_argument('--readers',action='store_true');parser.add_argument('--output',required=True);args=parser.parse_args()
if not 1<=args.batches<=200:parser.error('--batches must be 1..200')
records=[];gate=threading.Lock();active=False

def record(kind,started,wall,cpu,detail):
    if active:
        with gate:records.append(dict(kind=kind,thread=threading.current_thread().name,started=started,ms=wall*1000,cpu_ms=cpu*1000,detail=detail))

class Timed(sqlite3.Connection):
    def execute(self,sql,parameters=()):
        start=time.monotonic();cpu=time.thread_time()
        try:return super().execute(sql,parameters)
        finally:record('sql',start,time.monotonic()-start,time.thread_time()-cpu,' '.join(sql.split())[:180])
    def executemany(self,sql,parameters):
        start=time.monotonic();cpu=time.thread_time()
        try:return super().executemany(sql,parameters)
        finally:record('many',start,time.monotonic()-start,time.thread_time()-cpu,' '.join(sql.split())[:180])

@contextmanager
def connect(self):
    start=time.monotonic();cpu=time.thread_time();db=sqlite3.connect(self.path,timeout=10,factory=Timed);db.row_factory=sqlite3.Row
    try:
        yield db
        before=time.monotonic();beforecpu=time.thread_time();db.commit();record('commit',before,time.monotonic()-before,time.thread_time()-beforecpu,'')
    except BaseException:db.rollback();raise
    finally:
        db.close();record('connection',start,time.monotonic()-start,time.thread_time()-cpu,'')
Store._connect=connect

def wrap(module,name):
    original=getattr(module,name)
    def measured(*a,**kw):
        start=time.monotonic();cpu=time.thread_time()
        try:return original(*a,**kw)
        finally:record(name,start,time.monotonic()-start,time.thread_time()-cpu,str(a[1:2])+str(kw))
    setattr(module,name,measured)
wrap(activity,'drain');wrap(derived,'derive_trace')
with tempfile.TemporaryDirectory(prefix='agenturm-ingest-diag-')as directory:
    path=Path(directory)/'tiny.db';store=Store(path,async_derived_refresh=True)
    deadline=time.monotonic()+600
    while not store.index_status()['ready']:
        if time.monotonic()>deadline:raise RuntimeError('Diagnostic readiness timeout')
        time.sleep(.05)
    server=BoundedHTTPServer(('127.0.0.1',0),handler_for(store,'synthetic-diagnostic-token',quiet=True));serverthread=threading.Thread(target=server.serve_forever,name='diag-http');serverthread.start()
    writes=[];reads=[];stop=threading.Event();base=time.time_ns();start=time.monotonic();active=True
    def reader():
        for seq in range(args.batches//10):
            if stop.wait(max(0,start+seq-time.monotonic())):break
            for endpoint in ('/api/sessions?limit=500','/api/mcp?limit=100'):
                row,data=http_attempt(server.server_port,endpoint,'synthetic-diagnostic-token');reads.append(row)
    readthread=threading.Thread(target=reader,name='diag-read-client')if args.readers else None
    if readthread:readthread.start()
    try:
        for seq in range(args.batches):
            time.sleep(max(0,start+seq*.1-time.monotonic()))
            lateness=(time.monotonic()-(start+seq*.1))*1000
            row,data=http_attempt(server.server_port,'/api/spans','synthetic-diagnostic-token',method='POST',body=json.dumps(payload(seq,base)).encode())
            row.update(sequence=seq,lateness_ms=lateness,accepted=data.get('accepted')if isinstance(data,dict)else None);writes.append(row)
    finally:
        stop.set()
        if readthread:readthread.join(10)
        server.shutdown();server.server_close();serverthread.join(10)
        workers=[store._migration_thread,store._derived_thread]
        store.close();active=False
        if serverthread.is_alive()or any(worker and worker.is_alive()for worker in workers):
            raise RuntimeError('Diagnostic receiver workers did not drain')
    with closing(sqlite3.connect(path))as db:
        counts={table:db.execute('SELECT COUNT(*) FROM '+table).fetchone()[0]for table in ('spans','trace_activity','agent_nodes')}
    totals=collections.defaultdict(lambda:dict(calls=0,ms=0,cpu_ms=0,max_ms=0))
    for row in records:
        key=(row['kind'],row['thread'].split(' ')[-1]if row['thread'].startswith('Thread-')else row['thread'],row['detail'])
        value=totals[key];value['calls']+=1;value['ms']+=row['ms'];value['cpu_ms']+=row['cpu_ms'];value['max_ms']=max(value['max_ms'],row['ms'])
    stats=[dict(kind=k[0],thread=k[1],detail=k[2],**v)for k,v in totals.items()]
    result=dict(diagnostic=True,publishable=False,shared_process=True,counts=counts,database='tiny generated fixture',batches=args.batches,readers=args.readers,elapsed_s=time.monotonic()-start,deadline_failures=sum(row['lateness_ms']>100 for row in writes),source_files={str(path.relative_to(ROOT)):__import__('hashlib').sha256(path.read_bytes()).hexdigest() for path in (ROOT/'observatory').rglob('*.py')},writes=writes,reads=reads,events=records,top=sorted(stats,key=lambda x:x['ms'],reverse=True)[:80],derived_error=store.last_derived_error)
    Path(args.output).write_text(json.dumps(result,indent=2))
    print(json.dumps(dict(elapsed_s=result['elapsed_s'],write_durations=[round(x['elapsed_ms'],2)for x in writes],top=result['top'][:15]),indent=2))
