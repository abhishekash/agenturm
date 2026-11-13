"""Pure selection of execution facts shared by interactive and headless review."""
from __future__ import annotations
from copy import deepcopy
import time
from .session_status import session_health
from .workflow_evidence import scope_for
from .attention import fresh_observation


def session_usage(sessions, item):
    """Require producer/session provenance even when canonical agent IDs match."""
    if not item:return {}
    def same_producer(row):
        producer=item.get('service')
        return not producer or producer in (row.get('services') or [row.get('service')])
    def same_session(row):
        return not item.get('session_id') or row.get('session_id')==item['session_id']
    if item.get('agent_id'):
        exact=next((row for row in sessions if row.get('agent_id')==item['agent_id']
                    and same_producer(row) and same_session(row)),None)
        if exact is not None:return exact
        if not item.get('agent_id_inferred'):return {}
    return next((row for row in sessions if item.get('session_id')
                 and row.get('session_id')==item['session_id']
                 and same_producer(row) and not row.get('agent_id')), {})


def task_run_usage(sessions, item, traces=()):
    session=session_usage(sessions,item)
    if item.get('run_start_ns') and not item.get('trace_id'):return {}
    selected=item.get('trace_id') or item.get('latest_trace_id')
    latest=session.get('latest_run') or {}
    def allocated(row):
        if row.get('turn_usage'):row=row['turn_usage']
        if not row.get('trace_id'):return {}
        count=row.get('agent_count')
        if type(count) is int and count==1 and row.get('agent_count_complete') is True:return row
        return {key:row[key] for key in ('trace_id','agent_id','agent_count','agent_count_complete','turn_id','scope_owner','step_trace_ids','turn_usage_complete') if key in row} | {
            'usage_allocation':'shared trace; per-agent usage unavailable' if type(count) is int and count>1 else
                'agent allocation unverified; per-agent usage unavailable'}
    if latest and (not selected or latest.get('trace_id')==selected):return allocated(latest)
    trace_id=selected or session.get('latest_trace_id')
    def exact(row):
        if not trace_id or row.get('trace_id')!=trace_id:return False
        if item.get('service') and item['service'] not in (row.get('services') or [row.get('service')]):return False
        if item.get('session_id') and row.get('session_id') and row['session_id']!=item['session_id']:return False
        if item.get('agent_id') and not item.get('agent_id_inferred') and row.get('agent_id')!=item['agent_id']:return False
        return True
    candidate=next((row for row in traces if exact(row)),{})
    return allocated(candidate) if candidate else {}


def review_capture_policy(original, snapshot, scope):
    """Capture policy belongs to the identity, independently of which run is open."""
    policy={key:original[key] for key in ('capture_content','capture_revision') if key in original}
    if getattr(snapshot,'receiver_id','')!=scope.receiver:return policy
    local=getattr(snapshot,'local_status',{}) or {}
    for row in local.get('sessions',()):
        if row.get('service')!=scope.producer:
            if not (scope.producer=='codex' and row.get('service')=='codex-review'
                    and row.get('runtime')=='Codex' and row.get('capture_content') is False):continue
        if scope.agent_id:
            if row.get('agent_id')!=scope.agent_id:continue
        elif row.get('agent_id') or row.get('session_id')!=scope.session_id:continue
        if scope.session_id and row.get('session_id')!=scope.session_id:continue
        policy.update({key:row[key] for key in ('capture_content','capture_revision') if key in row})
        break
    if local.get('capture_content') is False:policy['capture_content']=False
    return policy


def refreshed_review_item(original, snapshot, scope):
    """Refresh facts for exactly this execution; never follow a newer run.

    The caller commits this candidate with a successful matching evidence read.
    """
    policy=review_capture_policy(original,snapshot,scope)
    original={**original,**policy}
    if policy.get('capture_content') is False:original['task']='Selected execution (metadata-only)'
    candidate = None
    for native in (getattr(snapshot, 'local_status', {}) or {}).get('sessions', ()):
        selected = {**native, **session_health(native, time.time_ns())}
        usage = task_run_usage(snapshot.sessions, selected, snapshot.traces)
        if scope.turn_id:
            membership=usage.get('scope_owner') or {}
            if usage.get('turn_id')==scope.turn_id and membership.get('producer')==scope.producer and membership.get('agent_id')==scope.agent_id and membership.get('session_id')==scope.session_id:
                selected['turn_id']=scope.turn_id
                selected['_review_usage']=usage
        if usage.get('trace_id') and not selected.get('trace_id'):
            selected['trace_id'] = usage['trace_id']
        if scope_for(selected, snapshot).scope_key == scope.scope_key:
            candidate = selected
            break
    if candidate is None:
        row = session_usage(snapshot.sessions, original)
        producer = original.get('service')
        latest=row.get('latest_run') or {}
        owner=latest.get('review_owner') or {}
        if (not scope.turn_id and scope.run_id and latest.get('trace_id')==scope.run_id and producer and
                owner.get('producer')==producer and owner.get('agent_id')==scope.agent_id and
                (not scope.session_id or owner.get('session_id')==scope.session_id)):
            outcome = latest.get('task_outcome') or 'unknown'
            health = session_health(dict(run_state=outcome, waiting_for=latest.get('waiting_for') or '',
                last_record_ns=row.get('last_unix_nano') or 0), time.time_ns())
            candidate = {**original, **health, 'trace_id': scope.run_id}
    if candidate is None or not fresh_observation(candidate, snapshot, time.time()):
        return {**deepcopy(original), '_review_metadata_note':
            'Live metadata for this execution is unavailable or stale; outcome and usage remain from the previous snapshot.'}
    if scope_for(candidate, snapshot).scope_key != scope.scope_key:
        return {**deepcopy(original), '_review_metadata_note':'Execution metadata changed scope; previous facts retained.'}
    usage = task_run_usage(snapshot.sessions, candidate, snapshot.traces)
    if getattr(snapshot, 'sessions_error', '') or any(key in getattr(snapshot, 'read_errors', {}) for key in ('sessions','traces')):
        usage = {}
    selected = deepcopy(original)
    for key in ('state','waiting_for','stamp','run_end_ns','outcome_time_ns','task_outcome_ns'):
        if key in candidate:
            selected[key] = deepcopy(candidate[key])
    selected['_review_usage'] = deepcopy(usage)
    selected.pop('_review_metadata_note', None)
    return selected
