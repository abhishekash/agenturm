"""Recent, unseen error reminders. Raw history and task outcomes stay intact."""
from __future__ import annotations

import hashlib
import json

from .workflow_evidence import scope_for

FAILURES = {'error', 'failed', 'denied', 'blocked', 'policy_denied', 'incomplete'}


def failure(event):
    return str(event.get('result') or event.get('status') or '').casefold() in FAILURES


def operation(event):
    return tuple(event.get(key) for key in
                 ('service', 'agent_id', 'session_id', 'trace_id', 'domain', 'action', 'subject'))


def pending_failures(events, limit=5):
    """A later identical successful operation need not interrupt the user.

    This only suppresses a reminder, never changes the recorded task outcome.
    Unknown/missing success is not evidence that a task is blocked.
    """
    successful = {}
    result = []
    seen = set()
    for event in sorted(events, key=lambda e: (e.get('time_unix_nano') or 0,
                                              str(e.get('id') or e.get('source_id') or '')), reverse=True):
        key = operation(event)
        identity = event.get('id') or event.get('source_id')
        if not identity or identity in seen:
            continue
        seen.add(identity)
        matchable = bool(event.get('subject') and (event.get('agent_id') or event.get('session_id')))
        if str(event.get('result') or '').casefold() in {'ok', 'success'} and matchable:
            successful[key] = max(successful.get(key, 0), event.get('time_unix_nano') or 0)
        elif failure(event) and (not matchable or successful.get(key, 0) <= (event.get('time_unix_nano') or 0)):
            result.append(event)
    return result[:limit]


def error_reference(event):
    reference = {key: event.get(key) for key in
        ('id', 'source_id', 'trace_id', 'span_id', 'service', 'session_id', 'agent_id',
         'time_unix_nano', 'domain', 'action', 'result', 'source', 'subject')}
    reference['subject'] = str(reference.get('subject') or '')[:500]
    return reference


def trace_error_references(activity, trace_id):
    events = [{**event, 'trace_id': trace_id,
               'id': trace_id + ':' + str(event.get('source_id') or '')} for event in activity
              if event.get('source_id')]
    return [error_reference(event) for event in pending_failures(events, limit=20)]


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
