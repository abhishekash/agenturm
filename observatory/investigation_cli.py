"""Bounded, noninteractive retained-history investigations; never launches agents."""
from __future__ import annotations

import json
import os
import sys
from types import SimpleNamespace
from urllib.parse import urlencode

from .investigation import render_investigation, validate_query
from .review_facts import review_capture_policy
from .workflow_evidence import scope_for

MAX_BYTES = 2 * 1024 * 1024


def add_investigation_parser(sub):
    parser = sub.add_parser('investigate', help='find recorded paths, commands or user instructions across retained history',
        epilog='Examples: agenturm investigate path src/retry.py; agenturm investigate command pytest --failed; '
               'agenturm investigate instruction "keep the public API"; agenturm investigate source EVENT_ID. '
               'Exit 0: bounded result, including no matches; 2: invalid request/unavailable receiver. No agent or receiver is started.')
    parser.add_argument('kind', choices=('path', 'command', 'instruction', 'source'))
    parser.add_argument('query', help='exact recorded path, literal word phrase, or source EVENT_ID')
    parser.add_argument('--endpoint', default=os.environ.get('OBSERVATORY_ENDPOINT', 'http://127.0.0.1:14318'))
    parser.add_argument('--service', help='restrict matches to an exact producer')
    parser.add_argument('--failed', action='store_true', help='only recorded failures; informational exit 1 and conflicting results are excluded')
    parser.add_argument('--limit', type=int, choices=range(1, 101), default=50, metavar='1..100')
    parser.add_argument('--pages', type=int, choices=range(1, 6), default=1, metavar='1..5')
    parser.add_argument('--cursor', help='resume the same query and receiver from a previous result')
    parser.add_argument('--format', choices=('text', 'json'), default='text')
    return parser


def _health(api, receiver=None):
    value = api.get('/api/health')
    if not isinstance(value, dict) or not isinstance(value.get('receiver_id'), str) or not value['receiver_id']:
        raise ValueError('Receiver identity is unavailable; update or reconnect to the intended receiver.')
    if receiver is not None and value['receiver_id'] != receiver:
        raise ValueError('Receiver identity changed; no investigation emitted.')
    if value.get('investigation_index_v1') is not True:
        raise ValueError('Receiver does not support typed investigations; update it first. No broad search fallback was used.')
    snapshot = SimpleNamespace(receiver_id=value['receiver_id'], local_status=value.get('local_agents') or {})
    if snapshot.local_status.get('capture_content') is False:
        raise ValueError('Source capture is disabled; investigation content is withheld.')
    return snapshot


def _allowed(reference, snapshot):
    return (reference.get('receiver_id') == snapshot.receiver_id and
            review_capture_policy(reference, snapshot, scope_for(reference, snapshot)).get('capture_content') is not False)


def collect_investigation(api, *, kind, query, limit=50, pages=1, cursor=None, service=None, failed=False):
    validate_query(kind, query)
    if not 1 <= limit <= 100 or not 1 <= pages <= 5:
        raise ValueError('Read 1..5 pages of 1..100 records each.')
    snapshot = _health(api)
    receiver = snapshot.receiver_id
    rows = []
    seen = set()
    used = 0
    count = 0
    result = None
    cursors = {cursor} if cursor is not None else set()
    for _ in range(pages):
        path = '/api/investigations?' + urlencode(dict(kind=kind, q=query, limit=limit,
            cursor=cursor or '', service=service or '', failed=int(failed)))
        response = api.get(path)
        if (not isinstance(response, dict) or response.get('receiver_id') != receiver
                or response.get('format') != 'agenturm-investigation-v1'
                or response.get('kind') != kind or response.get('query') != query
                or not isinstance(response.get('matches'), list) or len(response['matches']) > limit):
            raise ValueError('Investigation response does not match the requested receiver/query.')
        size = len(json.dumps(response, ensure_ascii=False).encode())
        if used + size > MAX_BYTES:
            if result is None:
                raise ValueError('Receiver page exceeds the 2 MiB investigation limit; lower --limit.')
            break  # cursor still identifies the first unread page.
        for row in response['matches']:
            if not isinstance(row, dict) or not isinstance(row.get('source_reference'), dict):
                raise ValueError('Malformed investigation source reference.')
            reference = row['source_reference']
            if (reference.get('event_id') != row.get('event_id') or not row.get('event_id')
                    or service and reference.get('service') != service or not _allowed(reference, snapshot)):
                raise ValueError('Source identity or capture policy changed; no investigation emitted.')
            if row['event_id'] in seen:
                raise ValueError('Receiver repeated an investigation event; no ambiguous result emitted.')
            seen.add(row['event_id'])
        result = response
        rows.extend(response['matches'])
        used += size
        count += 1
        next_cursor = response.get('next_cursor')
        if next_cursor is not None and (not isinstance(next_cursor, str) or not next_cursor or len(next_cursor) > 4096):
            raise ValueError('Malformed investigation cursor.')
        if next_cursor is not None and next_cursor in cursors:
            raise ValueError('Receiver repeated an investigation cursor.')
        if next_cursor is not None:
            cursors.add(next_cursor)
        cursor = next_cursor
        if cursor is None:
            break
    after = _health(api, receiver)
    if any(not _allowed(row['source_reference'], after) for row in rows):
        raise ValueError('Source capture policy changed while reading; no investigation emitted.')
    return {**result, 'matches': rows, 'next_cursor': cursor, 'pages_read': count,
            'limited': cursor is not None, 'payload_limit_bytes': MAX_BYTES}


def collect_source(api, event_id):
    if not isinstance(event_id, str) or not 0 < len(event_id) <= 512:
        raise ValueError('Provide an exact investigation EVENT_ID.')
    snapshot = _health(api)
    response = api.get('/api/investigations/source?' + urlencode({'event_id': event_id}))
    if (not isinstance(response, dict) or response.get('receiver_id') != snapshot.receiver_id
            or response.get('event_id') != event_id or not isinstance(response.get('sources'), list)
            or len(response['sources']) > 8 or len(json.dumps(response, ensure_ascii=False).encode()) > 65536):
        raise ValueError('Exact source response is unavailable or does not match the selected event.')
    original = response.get('source_reference')
    if (not isinstance(original, dict) or original.get('event_id') != event_id
            or original.get('receiver_id') != snapshot.receiver_id):
        raise ValueError('Exact source reference changed; evidence is withheld.')
    references = [original]
    for entry in response['sources']:
        if (not isinstance(entry, dict) or not isinstance(entry.get('scope'), dict)
                or entry.get('id') != original.get('source_id') or entry.get('type') != original.get('source')):
            raise ValueError('Exact source record differs from the selected reference.')
        references.append({**entry['scope'], 'receiver_id': snapshot.receiver_id})
    if any(not _allowed(reference, snapshot) for reference in references):
        raise ValueError('Source capture policy forbids this evidence.')
    after = _health(api, snapshot.receiver_id)
    if any(not _allowed(reference, after) for reference in references):
        raise ValueError('Source capture policy changed; exact evidence is withheld.')
    return response


def run_investigation_command(args):
    from .terminal import Api, CockpitError
    try:
        api = Api(args.endpoint, os.environ.get('OBSERVATORY_TOKEN') or None)
        if args.kind == 'source':
            if args.failed or args.service or args.cursor or args.pages != 1:
                raise ValueError('Source reads accept one exact event ID, without search filters or paging.')
            result = collect_source(api, args.query)
            text = json.dumps(result, ensure_ascii=False, indent=2)
        else:
            result = collect_investigation(api, kind=args.kind, query=args.query, limit=args.limit,
                pages=args.pages, cursor=args.cursor, service=args.service, failed=args.failed)
            text = render_investigation(result)
        print(json.dumps(result, ensure_ascii=False, indent=2) if args.format == 'json' else text)
        return 0
    except BrokenPipeError:
        return 0
    except (OSError, ValueError, TypeError, CockpitError) as exc:
        message = str(exc) if isinstance(exc, (ValueError, CockpitError)) else 'Cannot read the receiver; check its endpoint, authentication and readiness.'
        if args.format == 'json':
            print(json.dumps({'format': 'agenturm-investigation-v1', 'error': {'code': 'investigation_unavailable', 'message': message}}), file=sys.stderr)
        else:
            print('Agenturm investigate: ' + message, file=sys.stderr)
        return 2
