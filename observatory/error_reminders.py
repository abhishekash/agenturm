"""Recent, unseen error reminders. Raw history and task outcomes stay intact."""
from __future__ import annotations

import hashlib
import json

from .workflow_evidence import scope_for
from .operation_facts import correlation_facts,operation_key,operation_metadata,operation_stamp

FAILURES = {'error', 'failed', 'denied', 'blocked', 'policy_denied', 'incomplete'}


def failure(event):
    return str(event.get('result') or event.get('status') or '').casefold() in FAILURES


def operation(event):
    return operation_key(event,facts=correlation_facts(event))


def pending_failures(events, limit=5, *, receiver=''):
    """A later identical successful operation need not interrupt the user.

    This only suppresses a reminder, never changes the recorded task outcome.
    Unknown/missing success is not evidence that a task is blocked.
    """
    successful = {}
    result = []
    seen = set()
    for event in sorted(events, key=lambda e: (operation_stamp(e) or 0,
                                              str(e.get('id') or e.get('source_id') or '')), reverse=True):
        facts=correlation_facts(event)
        key=operation_key(event,receiver=receiver,facts=facts)
        stamp=operation_stamp(event)
        identity = event.get('id') or event.get('source_id')
        occurrence=tuple(event.get(key) if isinstance(event.get(key),str) else ''
            for key in ('receiver_id','service','agent_id','session_id','trace_id','turn_id'))+(identity,)
        if not isinstance(identity,str) or not identity or occurrence in seen:
            continue
        seen.add(occurrence)
        if facts['result']=='passed' and not facts['result_conflict'] and key and stamp:
            successful[key]=max(successful.get(key,0),stamp)
        elif (failure(event) or facts['result']=='failed') and not facts['expected_nonzero'] and (
                not key or not stamp or successful.get(key,0)<=stamp):
            result.append(event)
    return result[:limit]


def error_reference(event):
    reference = {key: event.get(key) for key in
        ('id', 'source_id', 'trace_id', 'span_id', 'service', 'session_id', 'agent_id',
         'time_unix_nano','domain','action','result','source','turn_id','receiver_id')}
    domain=event.get('domain')
    reference['subject']=('Shell operation' if domain=='shell' else 'MCP operation' if domain=='mcp' else 'Recorded operation')
    reference.update(operation_metadata(event))
    return reference


def trace_error_references(activity, trace_id, *, receiver=''):
    events = [{**event, 'trace_id': trace_id,
               'id': trace_id + ':' + str(event.get('source_id') or ''),'receiver_id':receiver} for event in activity
              if event.get('source_id')]
    references = [error_reference(event) for event in pending_failures(events,limit=20,receiver=receiver)]
    # Carry bounded, payload-free retry context across traces in one turn.
    # Otherwise a successful step is discarded before owner-level correlation.
    successful = set()
    for event in sorted(events,key=lambda event: operation_stamp(event) or 0,reverse=True):
        facts=correlation_facts(event)
        key=operation_key(event,receiver=receiver,facts=facts)
        if facts['result']!='passed' or facts['result_conflict'] or not key or not operation_stamp(event):continue
        if key in successful:continue
        successful.add(key)
        references.append(error_reference(event))
        if len(successful)>=20:break
    return references


def recent_sources(items):
    """Two most recently active conversations per producer and project.

    Confirmed descendants share their root's slot; workers never evict parents.
    Compute before applying the user's session/search/filter controls.
    """
    by_agent = {item.get('agent_id'): item for item in items if item.get('agent_id')}
    project_paths = {}
    for item in items:
        if item.get('project') and item.get('working_directory'):
            key = (item.get('service') or item.get('runtime'), item['project'])
            project_paths.setdefault(key, set()).add(item['working_directory'].rstrip('/') or '/')
    groups = {}
    membership = {}
    for item in items:
        root = item
        visited = set()
        while (root.get('parent_agent_id') in by_agent and
               root.get('relationship_state') == 'confirmed' and
               root.get('relationship_kind') == 'delegated' and root.get('agent_id') not in visited):
            visited.add(root.get('agent_id'))
            root = by_agent[root['parent_agent_id']]
        producer = root.get('service') or root.get('runtime')
        name = root.get('project') or ''
        paths = project_paths.get((producer, name), set())
        directory = root.get('working_directory') or (next(iter(paths)) if len(paths) == 1 else '')
        project = ('path', directory.rstrip('/') or '/') if directory else ('name', name)
        conversation = root.get('native_session_id') or root.get('session_id') or root.get('id')
        key = (producer, project)
        membership[item['id']] = (key, conversation)
        conversations = groups.setdefault(key, {})
        conversations[conversation] = max(conversations.get(conversation, 0), item.get('stamp') or 0)
    chosen = {key: {identity for identity, _ in sorted(values.items(),
              key=lambda pair: (pair[1], str(pair[0])), reverse=True)[:2]} for key, values in groups.items()}
    return {identity for identity, (key, conversation) in membership.items() if conversation in chosen[key]}


def event_receipt(event, snapshot, endpoint=''):
    source = event.get('id') or event.get('source_id')
    scope = scope_for(event, snapshot, endpoint)
    if event.get('receiver_id') and event['receiver_id']!=scope.receiver:return None
    if not source or not scope.receiver or not scope.producer or not (scope.agent_id or scope.session_id):
        return None
    identity = (scope.receiver, scope.producer, scope.agent_id, scope.session_id,
                event.get('trace_id') or '', str(source))
    return hashlib.sha256(json.dumps(identity).encode()).hexdigest()


def outcome_event(item):
    if item.get('state') not in {'Error', 'Failed', 'Incomplete'}:
        return None
    anchor = item.get('outcome_id') or item.get('trace_id') or item.get('latest_trace_id')
    anchor = anchor or item.get('run_start_ns') or item.get('outcome_time_ns') or item.get('task_outcome_ns')
    if not anchor:
        return None
    return {**{key: item.get(key) for key in ('service', 'agent_id', 'session_id', 'trace_id')},
            'agent_id': None if item.get('agent_id_inferred') else item.get('agent_id'),
            'id': 'outcome:' + str(anchor), 'result': 'failed', 'domain': 'agent', 'action': 'outcome',
            'subject': 'Run reported ' + item['state'].lower(),
            'time_unix_nano': item.get('outcome_time_ns') or item.get('task_outcome_ns') or item.get('stamp') or 0}


def reminders(items, snapshot, seen=(), endpoint=''):
    """No lifetime totals: only concrete, attributable recent error evidence."""
    from .ui_monitoring import session_usage
    eligible = recent_sources(items)
    result = {}
    for item in items:
        if item['id'] not in eligible:
            continue
        summary = session_usage(snapshot.sessions, item)
        events = list(summary.get('recent_errors') or [])
        if 'recent_errors' not in summary:
            # Compatibility with older receivers and a not-yet-loaded sessions
            # snapshot: only use actually available, exactly owned events.
            owned = [event for event in getattr(snapshot, 'activity', ())
                     if event.get('service') == item.get('service')
                     and event.get('session_id') == item.get('session_id')
                     and event.get('agent_id') == (None if item.get('agent_id_inferred') else item.get('agent_id'))]
            events.extend(pending_failures(owned))
        latest = item.get('last_activity') or {}
        if failure(latest) and (latest.get('id') or latest.get('source_id')):
            events.append({**latest, **{key: item.get(key) for key in ('service', 'session_id')},
                           'agent_id': None if item.get('agent_id_inferred') else item.get('agent_id')})
        outcome = outcome_event(item)
        if outcome:
            events.append(outcome)
        pending = []
        for event in events:
            # Even canonical IDs are insufficient without matching producer and session.
            if event.get('service') != item.get('service') or event.get('session_id') != item.get('session_id'):
                continue
            if event.get('agent_id') != (None if item.get('agent_id_inferred') else item.get('agent_id')):
                continue
            receipt = event_receipt(event, snapshot, endpoint)
            if receipt and receipt not in seen:
                pending.append({**event, '_error_receipt': receipt})
        if pending:
            result[item['id']] = pending
    return result
