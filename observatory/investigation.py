"""Pure typed investigation terms and source references shared by CLI and UI."""
from __future__ import annotations

import base64
import json
import shlex
from urllib.parse import urlencode

from .operation_facts import operation_facts, safe_value
from .recorded_paths import path_facts
from .session_text import local_command_only, message_text, request_text

KINDS = {'path', 'command', 'instruction'}
MAX_TERM_BYTES = 4096
REFERENCE_FIELDS = ('receiver_id', 'event_id', 'source', 'source_id', 'trace_id',
                    'span_id', 'turn_id', 'service', 'agent_id', 'session_id')


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


def _reference(value):
    if not isinstance(value, dict) or any(key not in REFERENCE_FIELDS for key in value):
        raise ValueError('Malformed exact source reference.')
    result = {key: value.get(key) for key in REFERENCE_FIELDS}
    if (any(item is not None and (not isinstance(item, str) or len(item) > 2048
            or any(ord(char) < 32 or ord(char) == 127 for char in item)) for item in result.values())
            or any(not result[key] for key in ('receiver_id', 'event_id', 'source', 'source_id'))
            or result['source'] not in {'span', 'log'}):
        raise ValueError('Malformed exact source identity.')
    return result


def encode_reference(reference):
    value = json.dumps(_reference(reference), ensure_ascii=False, separators=(',', ':')).encode()
    token = base64.urlsafe_b64encode(value).decode().rstrip('=')
    if len(token) > 8192:
        raise ValueError('Exact source reference exceeds its bounded size.')
    return token


def decode_reference(token):
    if isinstance(token, dict):
        return _reference(token)
    try:
        if not isinstance(token, str) or not 0 < len(token) <= 8192:
            raise ValueError
        value = json.loads(base64.b64decode(token + '=' * (-len(token) % 4), altchars=b'-_', validate=True))
        return _reference(value)
    except (ValueError, TypeError, UnicodeError) as exc:
        raise ValueError('Invalid exact source reference; use the reference returned by the investigation.') from exc


def reference_matches(expected, current):
    try:
        return decode_reference(expected) == _reference(current)
    except (ValueError, TypeError):
        return False


def source_path(reference):
    return '/api/investigations/source?' + urlencode(dict(event_id=reference['event_id'],
        receiver_id=reference['receiver_id'], reference=encode_reference(reference)))


def source_command(reference):
    return shlex.join(['agenturm', 'investigate', 'source', reference['event_id'],
                       '--reference', encode_reference(reference)])


def render_investigation(response):
    rows = response.get('matches') or []
    lines = [f"{len(rows)} retained {response['kind']} observation(s) · {response['match_mode']}"]
    for item in rows:
        source = item['source_reference']
        owner = ' / '.join(str(source.get(key) or 'Not reported')
                           for key in ('service', 'agent_id', 'session_id'))
        lines.extend(['', item['text'], owner,
                      'Recorded result: ' + str(item.get('result') or 'unknown'),
                      'Exact source: ' + item.get('source_command', item['source_path'])])
        if not item.get('term_complete'):
            lines.append('Only a bounded or partial input/path was available.')
    lines.extend(['', response.get('coverage') or 'Retained indexed observations only.'])
    if response.get('next_cursor'):
        lines.append('More observations remain; resume with --cursor ' + response['next_cursor'])
    return '\n'.join(lines)
