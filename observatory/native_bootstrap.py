"""Metadata-only native observations while the initial history index is pending.

No task, path, project, prompt, tool/input reason, raw event or stored context is
copied. This is a display projection, never evidence that history is complete.
"""
from .agent_hierarchy import AGENT_ID
import re

MAX_ROWS=500
_CLOCK_MAX=2**63
_NAMES=('source_id','service','runtime','agent_id','session_id','parent_agent_id',
    'native_session_id','native_worker_id','trace_id','turn_id','request_id','outcome_id',
    'relationship_kind','relationship_state','discovery_state','coverage')
_TIMES=('state_since_ns','last_record_ns','last_update_ns','run_start_ns','request_time_ns',
    'outcome_time_ns','observed_ns')
_STATES={'running','waiting','completed','interrupted','error','failed','incomplete','idle','unknown','announced'}
_ID=re.compile(r'[A-Za-z0-9_.:@-]{1,512}')
_RUNTIMES={'Codex','Claude','Claude Code','OpenCode','Pi','Kiro','Gemini CLI','Cursor'}
_ENUMS={'relationship_kind':{'delegated','review','unknown'},
    'relationship_state':{'confirmed','unresolved','conflicted','invalid'},
    'discovery_state':{'observed','announced'},'coverage':{'complete','partial','unknown'}}
_INDEX_STAGES={'Preparing indexed history','Refreshing ownership and run summaries',
    'Refreshing incoming finding evidence','Indexing incoming source changes'}


def indexing_in_progress(indexing):
    if not isinstance(indexing,dict)or indexing.get('ready')is not False or indexing.get('error'):return False
    stage=indexing.get('stage')
    return isinstance(stage,str)and len(stage)<=512 and(stage in _INDEX_STAGES or
        bool(re.fullmatch(r'Indexing source records · [0-9,]{1,30} indexed|Refreshing run evidence · [0-9,]{1,30}/[0-9,]{1,30} runs',stage)))


def identity_fields(entry):
    row={key:entry[key]for key in _NAMES if isinstance(entry.get(key),str)and
        ((key=='runtime'and entry[key]in _RUNTIMES)or key in _ENUMS and entry[key]in _ENUMS[key]
         or key!='runtime'and key not in _ENUMS and _ID.fullmatch(entry[key]))}
    return row


def clock(value):
    return value if type(value)is int and 0<value<_CLOCK_MAX else 0


def ready_native(health):
    if not isinstance(health,dict):return False
    local=health.get('local_agents')if isinstance(health,dict)else None
    indexing=health.get('indexing')if isinstance(health,dict)else None
    return (health.get('ok')is True and indexing_in_progress(indexing)
        and isinstance(local,dict)and local.get('enabled')is True and
        local.get('initial_poll_complete')is True and isinstance(local.get('sessions'),list)and
        isinstance(health.get('receiver_id'),str)and 0<len(health['receiver_id'])<=512 and
        bool(clock(health.get('observed_time_unix_nano'))))


def native_metadata(health):
    """Return only bounded, attributed metadata from the final health response."""
    if not ready_native(health):return None
    original=health['local_agents'];now=clock(health['observed_time_unix_nano'])
    result={'enabled':True,'initial_poll_complete':True,'sessions':[]}
    if type(original.get('capture_content'))is bool:result['capture_content']=original['capture_content']
    seen={};conflicted=set();omitted=0
    for entry in original['sessions'][:MAX_ROWS]:
        if not isinstance(entry,dict):omitted+=1;continue
        row=identity_fields(entry)
        if (not row.get('source_id')or not row.get('service')or not row.get('runtime')or
                not AGENT_ID.fullmatch(row.get('agent_id',''))):
            omitted+=1;continue
        if row.get('parent_agent_id')and not AGENT_ID.fullmatch(row['parent_agent_id']):
            row.pop('parent_agent_id',None);row['relationship_state']='unresolved'
        identity=tuple(row.get(key)or ''for key in ('service','agent_id','session_id'))
        previous=seen.get(row['source_id'])
        if previous is not None:
            conflicted.add(row['source_id'])
            omitted+=1;continue
        seen[row['source_id']]=identity
        for key in _TIMES:
            value=clock(entry.get(key))
            if value:row[key]=value
        if type(entry.get('capture_content'))is bool:row['capture_content']=entry['capture_content']
        diagnostics=entry.get('diagnostics')if isinstance(entry.get('diagnostics'),dict)else {}
        safe={key:value for key in ('last_success_ns','backlog_bytes','pending_storage_records')
            if type(value:=diagnostics.get(key))is int and 0<=value<_CLOCK_MAX}
        safe['read_error']=bool(diagnostics.get('read_error'))
        read=clock(safe.get('last_success_ns'))
        current=bool(all(key in safe for key in ('last_success_ns','backlog_bytes','pending_storage_records'))and
            read and 0<=now-read<=15_000_000_000 and not safe.get('backlog_bytes')and
            not safe.get('pending_storage_records')and not safe['read_error'])
        state=entry.get('run_state')
        if any(row.get(key,0)>now for key in ('state_since_ns','run_start_ns','last_record_ns','outcome_time_ns')):current=False
        if isinstance(state,str)and state in {'completed','interrupted','error','failed','incomplete'}:
            outcome=row.get('outcome_time_ns')or row.get('state_since_ns')
            if not outcome or outcome>read or outcome<row.get('run_start_ns',0):current=False
        row['run_state']=state if current and isinstance(state,str)and state in _STATES else 'unknown'
        row['collector_updated_ns']=read
        row['diagnostics']=safe
        row.update(project='',model='',working_directory='',branch='',task='',initial_task='',last_activity={},waiting_for='')
        # No waiting_for text: it can contain a command or user input reason.
        result['sessions'].append(row)
    result['sessions']=[row for row in result['sessions']if row['source_id']not in conflicted]
    omitted+=len(original['sessions'])-min(len(original['sessions']),MAX_ROWS)+len(conflicted)
    result['bootstrap_omitted_rows']=omitted
    return result


def published_metadata(local):
    """Policy tombstones are not display rows; strip their broader ID metadata.

    Feed retains the authoritative private policy registry. This final pass
    prevents its namespace/path fields from entering a bootstrap Snapshot.
    """
    result={key:local[key]for key in ('enabled','initial_poll_complete','capture_content',
        'capture_policy_schema','capture_global_revision','bootstrap_omitted_rows')
        if type(local.get(key))in(bool,int)and 0<=local[key]<_CLOCK_MAX}
    fields=(*_TIMES,'collector_updated_ns','run_state','capture_content','capture_safety_revision')
    rows=[]
    for entry in local.get('sessions',())[:MAX_ROWS]:
        row=identity_fields(entry)
        row.update({key:entry[key]for key in fields if (key=='run_state'and isinstance(entry.get(key),str)and entry[key]in _STATES
            or key=='capture_safety_revision'and isinstance(entry.get(key),str)and re.fullmatch(r'[0-9]{1,20}:[0-9]{1,20}',entry[key])
            or type(entry.get(key))in(bool,int)and 0<=entry[key]<_CLOCK_MAX)})
        if not row.get('source_id')or not row.get('service')or not AGENT_ID.fullmatch(str(row.get('agent_id')or '')):continue
        if row.get('parent_agent_id')and not AGENT_ID.fullmatch(row['parent_agent_id']):row.pop('parent_agent_id',None)
        diagnostics=entry.get('diagnostics')or {}
        row['diagnostics']={key:diagnostics[key]for key in ('last_success_ns','backlog_bytes','pending_storage_records','read_error')
            if type(diagnostics.get(key))in(bool,int)and 0<=diagnostics[key]<_CLOCK_MAX}
        row.update(project='',model='',working_directory='',branch='',task='',initial_task='',last_activity={},waiting_for='')
        rows.append(row)
    result['sessions']=rows
    revisions=[]
    for row in local.get('capture_policy_revisions',())[:MAX_ROWS]:
        if not isinstance(row,dict):continue
        revisions.append({**{key:value for key,value in identity_fields(row).items()if key in {'service','runtime','agent_id','session_id'}},
            **({'capture_safety_revision':row['capture_safety_revision']}if type(row.get('capture_safety_revision'))is int and 0<=row['capture_safety_revision']<_CLOCK_MAX else {})})
    if revisions:result['capture_policy_revisions']=revisions
    return result
