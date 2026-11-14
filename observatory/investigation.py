"""Pure typed investigation terms and source references shared by CLI and UI."""
from __future__ import annotations

from urllib.parse import urlencode

from .operation_facts import operation_facts, safe_value
from .recorded_paths import path_facts
from .session_text import local_command_only, message_text, request_text

KINDS = {'path', 'command', 'instruction'}
MAX_TERM_BYTES = 4096


def validate_query(kind, query):
    if kind not in KINDS:
        raise ValueError('Choose path, command or instruction.')
    if not isinstance(query, str) or not query.strip():
        raise ValueError('Provide a nonempty investigation query.')
    try:
        encoded = query.encode()
    except UnicodeError as exc:
        raise ValueError('Query must contain valid Unicode.') from exc
    if len(encoded) > MAX_TERM_BYTES:
        raise ValueError('Investigation queries are limited to 4,096 UTF-8 bytes.')
    if any(ord(char) < 32 or ord(char) == 127 for char in query):
        raise ValueError('Use a single-line query without control characters.')
    if kind != 'path' and not any(char.isalnum() for char in query):
        raise ValueError('Phrase queries need at least one letter or number.')
    return query


def prepare_terms(event):
    """Extract known input roles only; stdout and assistant claims stay out."""
    if event.get('capture_content') is False or event.get('service') == 'codex-review':
        return []
    candidates = []
    paths = path_facts(event)
    for value in paths['paths']:
        candidates.append(('path', value['path'], value['role'], paths['complete']))
    if event.get('domain') == 'shell':
        facts = operation_facts(event)
        if facts.get('command_complete'):
            candidates.append(('command', facts['command'], 'command', True))
    if event.get('domain') == 'message' and event.get('action') == 'user':
        value = event.get('detail') or event.get('subject') or ''
        # Do not expand arbitrary structures before applying the source budget.
        bounded, complete = safe_value(value)
        text = message_text(bounded)
        if not local_command_only(text):
            text = request_text(text).body
            if text.strip():
                candidates.append(('instruction', text, 'user', complete and bool(event.get('detail'))))
    terms = []
    for kind, text, role, complete in candidates[:8]:
        sanitized, safe = safe_value(text)
        if not isinstance(sanitized, str) or not sanitized or not safe:
            continue
        raw = sanitized.encode()
        if len(raw) > MAX_TERM_BYTES:
            sanitized = raw[:MAX_TERM_BYTES].decode('utf-8', 'ignore')
            complete = False
        terms.append(dict(kind=kind, text=sanitized, role=role, complete=bool(complete)))
    return terms


def source_reference(event, receiver):
    identity = event.get('id') or ((event.get('trace_id') or 'log') + ':' + str(event.get('source_id') or ''))
    return dict(receiver_id=receiver, event_id=identity,
                **{key: event.get(key) for key in ('source', 'source_id', 'trace_id', 'span_id',
                    'turn_id', 'service', 'agent_id', 'session_id')})


def source_path(reference):
    return '/api/investigations/source?' + urlencode({'event_id': reference['event_id']})


def render_investigation(response):
    rows = response.get('matches') or []
    lines = [f"{len(rows)} retained {response['kind']} observation(s) · {response['match_mode']}"]
    for item in rows:
        source = item['source_reference']
        owner = ' / '.join(str(source.get(key) or 'Not reported')
                           for key in ('service', 'agent_id', 'session_id'))
        lines.extend(['', item['text'], owner,
                      'Recorded result: ' + str(item.get('result') or 'unknown'),
                      'Exact source: ' + item['source_path']])
        if not item.get('term_complete'):
            lines.append('Only a bounded or partial input/path was available.')
    lines.extend(['', response.get('coverage') or 'Retained indexed observations only.'])
    if response.get('next_cursor'):
        lines.append('More observations remain; resume with --cursor ' + response['next_cursor'])
    return '\n'.join(lines)
