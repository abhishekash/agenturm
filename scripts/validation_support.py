"""Generated-only validation helpers; no product import or work at import time."""
from __future__ import annotations
from contextlib import closing
import hashlib
import importlib
import importlib.metadata
import importlib.util
import json
import math
import os
import platform
from pathlib import Path
import secrets
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from tempfile import TemporaryDirectory
from urllib.parse import quote

HARNESS_ROOT=Path(__file__).resolve().parents[1]
GIB=1024**3
MARKER='agenturm-validation-owned-v1'
HARNESSES=('validation_support.py','soak_monitor.py','benchmark_monitor_idle.py',
    'benchmark_http_tails.py','verify_keyboard_tmux.py','validate_combined_monitor.py','benchmark_history.py','benchmark_app.py','soak_workflows.py','benchmark_resources.py')


def file_hash(path):
    digest=hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda:stream.read(1024*1024),b''):digest.update(block)
    return digest.hexdigest()


def application_revision(root):
    root=Path(root).resolve()
    head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
    dirty=bool(subprocess.run(['git','diff','HEAD','--quiet'],cwd=root).returncode)
    untracked=subprocess.check_output(['git','ls-files','--others','--exclude-standard','--','*.py','*.toml','*.tcss'],cwd=root,text=True).splitlines()
    return dict(root=str(root),head=head,dirty=dirty or bool(untracked),untracked_code=untracked)


def harness_manifest():
    return dict(root=str(HARNESS_ROOT),revision=application_revision(HARNESS_ROOT),files={name:file_hash(HARNESS_ROOT/'scripts'/name)
        for name in HARNESSES if (HARNESS_ROOT/'scripts'/name).is_file()},
        python=sys.version,executable=str(Path(sys.executable).resolve()),
        host=dict(system=platform.system(),kernel=platform.release(),machine=platform.machine(),cpus=os.cpu_count()),
        terminal=dict(term='xterm-256color',tmux=subprocess.check_output(['tmux','-V'],text=True).strip() if shutil.which('tmux') else None),
        lock_sha256=file_hash(HARNESS_ROOT/'uv.lock') if (HARNESS_ROOT/'uv.lock').is_file() else None,
        packages={name:importlib.metadata.version(name) for name in ('textual','rich','markdown-it-py')})


def bind_application(root):
    """One application tree per child; reject an already contaminated import."""
    root=Path(root).resolve()
    loaded=sys.modules.get('observatory')
    if loaded is not None and not Path(loaded.__file__).resolve().is_relative_to(root):
        raise RuntimeError('A different application tree was already imported')
    sys.path.insert(0,str(root))
    module=importlib.import_module('observatory')
    if not Path(module.__file__).resolve().is_relative_to(root):
        raise RuntimeError('Application import escaped the registered tree')
    return dict(root=str(root),module=str(Path(module.__file__).resolve()))


def load_harness(name):
    if name not in HARNESSES:raise ValueError('Unregistered harness helper')
    path=HARNESS_ROOT/'scripts'/name
    spec=importlib.util.spec_from_file_location('_agenturm_validation_'+name.removesuffix('.py'),path)
    module=importlib.util.module_from_spec(spec)
    sys.modules[spec.name]=module;spec.loader.exec_module(module)
    return module


def isolated_env(base,app_root):
    base=Path(base).resolve()
    env={key:os.environ[key] for key in ('PATH','LANG','LC_ALL','LD_LIBRARY_PATH') if key in os.environ}
    env.update(XDG_STATE_HOME=str(base/'state'),XDG_CONFIG_HOME=str(base/'config'),
        XDG_DATA_HOME=str(base/'data'),OPENCODE_DB=str(base/'opencode.db'),TERM='xterm-256color',
        GEMINI_CLI_SYSTEM_SETTINGS_PATH=str(base/'config/gemini-system.json'),PYTHONUNBUFFERED='1',
        AGENTURM_VALIDATION_APP_ROOT=str(Path(app_root).resolve()))
    return env


def disk_gate(path,*,required_bytes=0,reserve_bytes=4*GIB):
    if type(required_bytes) is not int or required_bytes<0 or type(reserve_bytes) is not int or reserve_bytes<0:
        raise ValueError('Invalid disk budget')
    path=Path(path).resolve()
    while not path.exists():path=path.parent
    free=shutil.disk_usage(path).free
    result=dict(free_bytes=free,required_bytes=required_bytes,reserve_bytes=reserve_bytes,
        allowed=free>=required_bytes+reserve_bytes)
    if not result['allowed']:raise RuntimeError('Insufficient validation disk headroom: '+json.dumps(result))
    return result


def create_owned_directory(base):
    base=Path(base).resolve();base.mkdir(mode=0o700)
    marker=dict(format=MARKER,nonce=secrets.token_hex(16),created_at=time.time(),clones={})
    (base/'ownership.json').write_text(json.dumps(marker,indent=2)+'\n')
    return marker


def readonly(path):
    return sqlite3.connect('file:'+quote(str(Path(path).resolve()),safe='/')+'?mode=ro',uri=True)


def raw_manifest(path):
    """Hash only canonical raw rows, independently of derived schema/page layout."""
    result={};digest=hashlib.sha256()
    with closing(readonly(path)) as db:
        for table,order in (('spans','trace_id,span_id'),('logs','digest'),('trace_activity','trace_id')):
            if not db.execute('SELECT 1 FROM sqlite_master WHERE type=\'table\' AND name=?',(table,)).fetchone():
                raise ValueError('Raw seed is missing '+table)
            count=0
            for row in db.execute('SELECT * FROM '+table+' ORDER BY '+order):
                digest.update(json.dumps([table,*row],ensure_ascii=True,separators=(',',':')).encode());digest.update(b'\n');count+=1
            result[table]=count
    return dict(path=str(Path(path).resolve()),sha256=file_hash(path),raw_sha256=digest.hexdigest(),counts=result,bytes=Path(path).stat().st_size)


def clone_seed(seed,target,*,reserve_bytes=4*GIB):
    seed=Path(seed).resolve();target=Path(target).absolute();base=target.parent
    marker=json.loads((base/'ownership.json').read_text())
    if marker.get('format')!=MARKER or base.is_symlink() or target.is_symlink() or target.exists():
        raise ValueError('Working clone must be a new file inside an owned directory')
    if Path(str(seed)+'-wal').exists() and Path(str(seed)+'-wal').stat().st_size:
        raise ValueError('Frozen seed still has a live WAL')
    before=raw_manifest(seed);disk_gate(base,required_bytes=max(before['bytes']*8,32*1024**2),reserve_bytes=reserve_bytes)
    with closing(readonly(seed)) as source,closing(sqlite3.connect(target)) as destination:
        source.backup(destination);destination.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    after=raw_manifest(target)
    if before['raw_sha256']!=after['raw_sha256'] or before!=raw_manifest(seed):
        raise RuntimeError('Frozen seed or cloned raw facts changed')
    marker['clones'][target.name]=dict(role='working_clone',seed=before,clone=after)
    (base/'ownership.json').write_text(json.dumps(marker,indent=2)+'\n')
    return after


def cleanup_clone(base,name,*,children_stopped=False):
    """Delete only a registered working clone, never seeds/sources/raw results."""
    base=Path(base).absolute()
    if base.is_symlink() or not children_stopped:raise ValueError('Owned children must be stopped before clone cleanup')
    marker=json.loads((base/'ownership.json').read_text())
    if marker.get('format')!=MARKER or Path(name).name!=name or marker.get('clones',{}).get(name,{}).get('role')!='working_clone':
        raise ValueError('Not a registered owned working clone')
    if not (base/'results.json').is_file():raise ValueError('Preserve raw results before clone cleanup')
    paths=[base/(name+suffix) for suffix in ('','-wal','-shm')]
    # Validate the whole set before deleting any member. A hostile sidecar must
    # not turn a refused cleanup into partial database loss.
    for path in paths:
        if path.is_symlink():raise ValueError('Refusing symlink cleanup')
        if path.exists() and not path.is_file():raise ValueError('Refusing non-file cleanup')
    removed=[dict(path=path.name,bytes=path.stat().st_size,sha256=file_hash(path)) for path in paths if path.exists()]
    marker['clones'][name]['cleanup_intent']=removed
    (base/'ownership.json').write_text(json.dumps(marker,indent=2)+'\n')
    for path in paths:
        if path.exists():path.unlink()
    marker['clones'][name]['removed']=removed
    (base/'ownership.json').write_text(json.dumps(marker,indent=2)+'\n')
    return removed


def fresh_schema_manifest(base,store_factory):
    """Inspect an empty, private, normally initialized same-version Store."""
    base=Path(base).resolve()
    if json.loads((base/'ownership.json').read_text()).get('format')!=MARKER:
        raise ValueError('Fresh schema needs an owned validation directory')
    with TemporaryDirectory(prefix='empty-schema-',dir=base) as directory:
        path=Path(directory)/'schema.db';store=store_factory(path,workflow_patterns=False)
        try:
            deadline=time.monotonic()+30
            while store.index_status().get('ready') is not True:
                if store.last_derived_error or time.monotonic()>deadline:raise RuntimeError('Empty production schema did not become ready')
                time.sleep(.01)
            with closing(readonly(path)) as db:
                result=dict(named_indexes=dict(db.execute("SELECT name,sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL")),
                    schema_meta_sql=db.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name='schema_meta'").fetchone()[0])
        finally:
            workers=[getattr(store,name,None) for name in ('_migration_thread','_derived_thread')]
            store.close()
            if any(worker and worker.is_alive() for worker in workers):raise RuntimeError('Empty schema worker did not stop')
    return result


def prepare_raw_clone(base,name,*,schema_meta_sql):
    """Remove inherited derived schema ONLY from a registered stopped clone.

    The three raw tables and their constraints/rows remain untouched. Real
    nonempty source/configuration inputs refuse preparation rather than being
    silently discarded. Dirty queues are derived and are rebuilt normally.
    """
    started=time.monotonic();base=Path(base).absolute();target=base/name
    marker=json.loads((base/'ownership.json').read_text())
    if (base.is_symlink() or Path(name).name!=name or target.is_symlink() or
        marker.get('format')!=MARKER or marker.get('clones',{}).get(name,{}).get('role')!='working_clone'):
        raise ValueError('Raw preparation needs a registered owned clone')
    if marker['clones'][name].get('raw_preparation'):raise ValueError('Clone was already prepared')
    seed_path=Path(marker['clones'][name]['seed']['path'])
    if any(Path(str(target)+suffix).is_symlink()for suffix in ('','-wal','-shm'))or os.path.samefile(target,seed_path):
        raise ValueError('Clone/seed alias or linked sidecar is unsafe')
    if not isinstance(schema_meta_sql,str)or not re.match(r'^CREATE TABLE(?: IF NOT EXISTS)?\s+schema_meta\s*\(',schema_meta_sql,re.I):
        raise ValueError('Expected fresh production schema_meta table DDL')
    seed_before=raw_manifest(marker['clones'][name]['seed']['path']);before=raw_manifest(target)
    if seed_before!=marker['clones'][name]['seed']or before!=marker['clones'][name]['clone']:
        raise ValueError('Seed or registered clone changed before preparation')
    raw={'spans','logs','trace_activity'};removed=[]
    with closing(sqlite3.connect(target)) as db:
        db.execute('PRAGMA foreign_keys=OFF');db.execute('BEGIN IMMEDIATE')
        schema=list(db.execute("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name"))
        tables={row[1]for row in schema if row[0]=='table'}
        raw_ddl={row[1]:row[2]for row in schema if row[0]=='table'and row[1]in raw}
        # These are source evidence or durable policy/configuration, rather
        # than derived projection caches. Unknown future input additions must
        # be reviewed before extending this generated-only protocol.
        source_tables={'progress','metric_points','outcomes','agent_connections','agent_nodes','agent_identity','native_source_policy_v1'}
        for table in source_tables&tables:
            if db.execute('SELECT 1 FROM "'+table+'" LIMIT 1').fetchone():raise ValueError('Nonempty canonical input table: '+table)
        derived={'endpoints','endpoint_services','endpoint_traces','trace_summaries','finding_cache','finding_dirty_v1',
            'derived_span_stats_v1','activity_index_v1','activity_dirty_v1','live_run_stats_v1','live_log_usage_v1',
            'live_summaries_v1','investigation_fts_v1','investigation_fts_v1_data','investigation_fts_v1_idx',
            'investigation_fts_v1_docsize','investigation_fts_v1_config','investigation_terms_v1',
            'investigation_dirty_v1','native_lifecycle_index_v1','native_lifecycle_dirty_v1','schema_meta'}
        for table in tables-raw-source_tables-derived:
            if db.execute('SELECT 1 FROM "'+table.replace('"','""')+'" LIMIT 1').fetchone():
                raise ValueError('Unknown nonempty input table: '+table)
        if 'schema_meta'in tables:
            policies=db.execute("SELECT key FROM schema_meta WHERE key LIKE 'native_source_policy%' LIMIT 1").fetchone()
            if policies:raise ValueError('Durable native source policy cannot be discarded')
        for kind in ('trigger','view','index','table'):
            for actual,name_,sql in schema:
                if actual!=kind or (kind=='table' and name_ in raw)or(kind=='index'and sql is None):continue
                if kind=='index' and name_.startswith('sqlite_'):continue
                db.execute('DROP '+kind.upper()+' IF EXISTS "'+name_.replace('"','""')+'"')
                removed.append(dict(type=kind,name=name_,sql=sql))
        db.execute(schema_meta_sql)
        db.execute("INSERT INTO schema_meta(key,value) VALUES('trace_activity_v1','1')")
        db.commit();db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    after=raw_manifest(target);seed_after=raw_manifest(seed_before['path'])
    if before['raw_sha256']!=after['raw_sha256']or before['counts']!=after['counts']or seed_before!=seed_after:
        raise RuntimeError('Raw facts or immutable seed changed during preparation')
    result=dict(protocol='clean-raw-schema-v1',before=before,after=after,seed_before=seed_before,seed_after=seed_after,
        removed_schema=removed,retained_tables=sorted(raw),retained_markers=['trace_activity_v1'],
        raw_table_ddl=raw_ddl,
        seconds=time.monotonic()-started)
    marker['clones'][name]['raw_preparation']=result
    (base/'ownership.json').write_text(json.dumps(marker,indent=2)+'\n')
    return result


def verify_named_indexes(path,manifest):
    with closing(readonly(path)) as db:
        actual=dict(db.execute("SELECT name,sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL"))
    expected=manifest['named_indexes']
    result=dict(equal=actual==expected,expected=expected,actual=actual)
    if not result['equal']:raise RuntimeError('Initialized clone indexes differ from fresh same-version production schema')
    return result


def storage_snapshot(path):
    path=Path(path);result=dict(files={suffix or 'db':Path(str(path)+suffix).stat().st_size
        if Path(str(path)+suffix).exists() else 0 for suffix in ('','-wal','-shm')})
    if not path.exists():return result
    with closing(readonly(path)) as db:
        result.update(page_size=db.execute('PRAGMA page_size').fetchone()[0],
            pages=db.execute('PRAGMA page_count').fetchone()[0],freelist=db.execute('PRAGMA freelist_count').fetchone()[0])
        names=[row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
        result['rows']={name:db.execute('SELECT COUNT(*) FROM "'+name.replace('"','""')+'"').fetchone()[0] for name in names}
        try:result['index_bytes']=dict(db.execute('SELECT name,SUM(pgsize) FROM dbstat GROUP BY name'))
        except sqlite3.OperationalError:result['index_bytes']=None
    return result


def published_heartbeat(app,*,at=None):
    """Observe already published data; never prepare sources or query a widget."""
    snapshot=app.feed.snapshot;local=snapshot.local_status;display=getattr(app,'data',snapshot)
    diagnostics=[row.get('diagnostics') or {} for row in local.get('sessions',())]
    sources=list(getattr(app,'source_map',{}).values())
    screen=getattr(app,'screen',None);scope=getattr(screen,'scope',None)
    return dict(kind='heartbeat',harness_version=2,at=time.time() if at is None else at,pid=os.getpid(),
        error=snapshot.error,receiver_id=snapshot.receiver_id,updated=snapshot.updated,
        display_receiver_id=getattr(display,'receiver_id',''),display_updated=getattr(display,'updated',0),
        read_errors=snapshot.read_errors,sessions_error=snapshot.sessions_error,indexing_ready=getattr(snapshot,'indexing_ready',None),
        native_inventory_supported=hasattr(snapshot,'native_lifecycle_warning'),
        native_inventory_warning=getattr(snapshot,'native_lifecycle_warning',None),
        session_count=len(snapshot.sessions),mcp_calls=snapshot.mcp_summary.get('calls'),
        native_sessions=[row.get('session_id') for row in local.get('sessions',())],
        initial_poll_complete=local.get('initial_poll_complete'),native_read_errors=local.get('read_errors'),native_schema_errors=local.get('schema_errors'),
        opencode_read_errors=local.get('opencode_read_errors'),opencode_diagnostics=local.get('opencode_diagnostics',{}),
        pending_storage_records=sum(int(row.get('pending_storage_records') or 0) for row in diagnostics),
        source_read_error=any(bool(row.get('read_error')) for row in diagnostics),
        source_backlog_bytes=local.get('backlog_bytes'),
        source_skipped_records=local.get('skipped_records'),source_omitted=local.get('omitted_sources'),
        source_hierarchy_write_errors=(local.get('hierarchy') or {}).get('write_errors'),
        source_schema_errors=sum(int(row.get('schema_errors') or 0) for row in diagnostics),
        states=[dict(session=item.get('session_id'),state=item.get('state'),request=item.get('request_id'),parent=item.get('parent_agent_id')) for item in sources[:500]],
        published_sources_clipped=len(sources)>500,screen=type(screen).__name__ if screen else '',
        visible_agent_rows=len(getattr(app,'table_data',{}).get('agent-table',{})),
        selected_source=getattr(app,'selected_source',None),
        selected_session=getattr(app,'source_map',{}).get(getattr(app,'selected_source',None),{}).get('session_id'),
        review_scope={key:getattr(scope,key,None) for key in ('receiver','producer','agent_id','session_id','run_id','turn_id')} if scope is not None else None,
        review_cache_entries=len(getattr(getattr(app,'review_cache',None),'entries',{})),
        review_cache_bytes=sum(getattr(value,'payload_bytes',0) for value in getattr(getattr(app,'review_cache',None),'entries',{}).values()),
        modal_stack=len(getattr(app,'screen_stack',())),
        investigation_rows=len(getattr(screen,'rows',())),investigation_pages=getattr(screen,'pages',None),
        investigation_bytes=getattr(screen,'payload_bytes',None),
        native_snapshot_entries=len(getattr(getattr(app,'native_snapshot_projection',None),'entries',())),
        approval_requests=[item.get('request_id') for item,_,_ in app.workflow_ledger.rows if item.get('_attention_kind')=='approval'],
        attention=[dict(id=item.get('_attention_id'),kind=item.get('_attention_kind'),request=item.get('request_id'),
            seen=item.get('_attention_seen'),stale=item.get('_attention_stale'),delivery=item.get('_attention_delivery'))
            for item,_,_ in app.workflow_ledger.rows[:1000]],
        warning=app.workflow_controller.warning)


def strict_heartbeat_errors(row,receiver):
    errors=[]
    if row.get('harness_version')!=2:return ['Missing v2 validation heartbeat']
    if row.get('receiver_id')!=receiver or row.get('display_receiver_id')!=receiver:errors.append('Receiver/display namespace mismatch')
    if row.get('indexing_ready') is not True:errors.append('Index readiness is unknown/incomplete')
    if type(row.get('native_inventory_supported')) is not bool:errors.append('Native capability is unknown')
    if row.get('native_inventory_supported') is True and row.get('native_inventory_warning')!='':errors.append('Native inventory is unknown/incomplete')
    if row.get('initial_poll_complete') is not True:errors.append('Initial inventory is unknown/incomplete')
    if any(row.get(key) for key in ('error','read_errors','sessions_error','native_read_errors','native_schema_errors','opencode_read_errors')):
        errors.append('Receiver/source read or schema failure')
    diagnostic=row.get('opencode_diagnostics') or {}
    if any(diagnostic.get(key) for key in ('read_errors','schema_errors','backlog_records','skipped_oversized_records','skipped_budget_records')):
        errors.append('OpenCode coverage is incomplete')
    if any(row.get(key) for key in ('pending_storage_records','source_read_error','source_backlog_bytes','source_skipped_records',
        'source_omitted','source_hierarchy_write_errors','source_schema_errors','published_sources_clipped','warning')):
        errors.append('Published/source/storage coverage is incomplete')
    times=(row.get('at'),row.get('updated'),row.get('display_updated'))
    if any(type(value) not in (int,float) or not math.isfinite(value) for value in times) or any(not 0<=times[0]-value<=6 for value in times[1:]):
        errors.append('Displayed/received data is stale or unknown')
    return errors


def nearest_rank(values,fraction):
    return sorted(values)[max(0,math.ceil(len(values)*fraction)-1)] if values else None


def monitor_budgets(samples):
    budgets={}
    if len(samples)<2:return budgets,None
    if not {'receiver','cockpit'}.issubset(samples[0]):raise ValueError('Complete monitor roles are missing')
    for role in samples[0]:
        first,last=samples[0][role],samples[-1][role]
        for row in samples:
            point=row.get(role,{})
            if any(type(point.get(key)) is not int or point[key]<=0 for key in ('pid','start_ticks')):
                raise ValueError('Process identity is unknown')
            if any(type(point.get(key)) not in (int,float) or not math.isfinite(point[key]) or point[key]<0 for key in ('sampled_at','cpu_seconds','rss_mib')):
                raise ValueError('Invalid process metric')
        if any((row[role]['pid'],row[role].get('start_ticks'))!=(first['pid'],first.get('start_ticks')) for row in samples):
            raise ValueError('Process identity changed within idle interval')
        wall=last['sampled_at']-first['sampled_at'];cpu=last['cpu_seconds']-first['cpu_seconds']
        if wall<=0 or cpu<0:raise ValueError('Invalid process CPU sample interval')
        budgets[role]=dict(cpu_one_core_percent=cpu/wall*100,sampled_seconds=wall,
            peak_sampled_rss_mib=max(row[role]['rss_mib'] for row in samples),
            last_rss_mib=last['rss_mib'])
    combined=[sum(row[role]['rss_mib'] for role in ('receiver','cockpit')) for row in samples]
    return budgets,dict(cpu_one_core_percent=sum(budgets[role]['cpu_one_core_percent'] for role in ('receiver','cockpit')),
        peak_sampled_rss_mib=max(combined),median_sampled_rss_mib=nearest_rank(combined,.5),last_rss_mib=combined[-1])
