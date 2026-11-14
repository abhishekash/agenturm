"""Typed search index over Activity identities; callers own transactions.

No raw span/log JSON is scanned at query time. FTS5 is required explicitly;
unsupported SQLite builds fail closed rather than falling back to history scans.
"""
from __future__ import annotations

import base64
import hashlib
import json
import sqlite3

from .investigation import prepare_terms, source_path, source_reference, validate_query

VERSION = 1
MARKER = 'investigation_terms_v1'


def initialize(db):
    """Install durable index hooks; return whether Activity needs re-projection."""
    try:
        db.execute('''CREATE VIRTUAL TABLE IF NOT EXISTS investigation_fts_v1 USING fts5(
            text, content='investigation_terms_v1', content_rowid='term_id',
            tokenize='unicode61 remove_diacritics 0')''')
    except sqlite3.OperationalError as exc:
        raise ValueError('Typed investigations require SQLite FTS5; no history-scan fallback is available.') from exc
    db.execute('''CREATE TABLE IF NOT EXISTS investigation_terms_v1(
        term_id INTEGER PRIMARY KEY, event_key TEXT NOT NULL, kind TEXT NOT NULL,
        text TEXT NOT NULL, role TEXT NOT NULL, complete INTEGER NOT NULL,
        UNIQUE(event_key,kind,text,role))''')
    db.execute('CREATE INDEX IF NOT EXISTS investigation_exact_v1 ON investigation_terms_v1(kind,text,event_key)')
    db.execute('CREATE INDEX IF NOT EXISTS investigation_event_v1 ON investigation_terms_v1(event_key)')
    db.execute('''CREATE TRIGGER IF NOT EXISTS investigation_insert_v1
        AFTER INSERT ON investigation_terms_v1 BEGIN
        INSERT INTO investigation_fts_v1(rowid,text) VALUES(NEW.term_id,NEW.text); END''')
    db.execute('''CREATE TRIGGER IF NOT EXISTS investigation_delete_v1
        AFTER DELETE ON investigation_terms_v1 BEGIN
        INSERT INTO investigation_fts_v1(investigation_fts_v1,rowid,text)
            VALUES('delete',OLD.term_id,OLD.text); END''')
    db.execute('''CREATE TRIGGER IF NOT EXISTS investigation_activity_delete_v1
        AFTER DELETE ON activity_index_v1 BEGIN
        DELETE FROM investigation_terms_v1 WHERE event_key=OLD.event_key; END''')
    # The integrating receiver queues metadata IDs using its existing durable
    # bounded drain. It must set MARKER only in that initialization transaction.
    return not bool(db.execute('SELECT 1 FROM schema_meta WHERE key=?', (MARKER,)).fetchone())


def replace_owner(db, kind, owner_key, event_rows):
    """Called after authoritative Activity writes, in the same writer snapshot."""
    db.execute('''DELETE FROM investigation_terms_v1 WHERE event_key IN
        (SELECT event_key FROM activity_index_v1 WHERE kind=? AND owner_key=?)''', (kind, owner_key))
    count = 0
    for event_key, row in event_rows:
        for term in prepare_terms(row):
            db.execute('''INSERT OR IGNORE INTO investigation_terms_v1
                (event_key,kind,text,role,complete) VALUES(?,?,?,?,?)''',
                (event_key, term['kind'], term['text'], term['role'], int(term['complete'])))
            count += 1
    return count


def _signature(kind, query, failed, service, agent_ids, receiver):
    return hashlib.sha256(json.dumps((VERSION, kind, query, failed, service,
        sorted(agent_ids) if agent_ids is not None else None, receiver), ensure_ascii=False).encode()).hexdigest()


def _before(cursor, signature):
    if cursor is None:
        return None
    try:
        if not isinstance(cursor, str) or len(cursor) > 4096:
            raise ValueError
        value = json.loads(base64.urlsafe_b64decode(cursor + '=' * (-len(cursor) % 4)))
        if (not isinstance(value, list) or len(value) != 4 or value[0] != VERSION
                or value[1] != signature or type(value[2]) is not int or value[2] < 0
                or not isinstance(value[3], str) or not 0 < len(value[3]) <= 512):
            raise ValueError
        return value[2], value[3]
    except (ValueError, TypeError, UnicodeError) as exc:
        raise ValueError('Invalid investigation cursor or changed query/receiver/scope.') from exc


def read_page(db, kind, query, *, limit=50, cursor=None, receiver='',
              agent_ids=None, service=None, failed=False):
    """Read one globally retained or explicitly scoped page in one caller snapshot."""
    query = validate_query(kind, query)
    if type(limit) is not int or not 1 <= limit <= 100:
        raise ValueError('Investigation page size must be between 1 and 100.')
    if service is not None and (not isinstance(service, str) or not 0 < len(service) <= 128):
        raise ValueError('Invalid investigation producer.')
    if agent_ids is not None and (not isinstance(agent_ids, (list, tuple)) or len(agent_ids) > 10000
            or any(not isinstance(item, str) or not 0 < len(item) <= 512 for item in agent_ids)):
        raise ValueError('Invalid investigation agent scope.')
    signature = _signature(kind, query, failed, service, agent_ids, receiver)
    before = _before(cursor, signature)
    params = [kind]
    if kind == 'path':
        match = 't.text=? COLLATE BINARY'
        params.append(query)
    else:
        # User query is a literal FTS phrase, never executable FTS syntax.
        match = 't.term_id IN (SELECT rowid FROM investigation_fts_v1 WHERE investigation_fts_v1 MATCH ?)'
        params.append('"' + query.replace('"', '""') + '"')
    filters = ['t.kind=?', match]
    if failed:
        filters.append("a.result IN ('error','failed','failure','denied','blocked','policy_denied','incomplete')")
    if service is not None:
        filters.append('a.service=?')
        params.append(service)
    if agent_ids is not None:
        filters.append('a.agent_id IN (' + ','.join('?' for _ in agent_ids) + ')' if agent_ids else '0')
        params.extend(agent_ids)
    if before:
        filters.append('(a.stamp,a.event_key)<(?,?)')
        params.extend(before)
    rows = db.execute('''SELECT a.event_key,a.stamp,a.payload,a.service,a.agent_id,a.session_id,
        a.trace_id,a.turn_id,a.result,a.domain,a.action,MIN(t.text) AS matched_text,
        MIN(t.complete) AS term_complete FROM investigation_terms_v1 t
        JOIN activity_index_v1 a ON a.event_key=t.event_key WHERE ''' + ' AND '.join(filters)
        + ' GROUP BY a.event_key ORDER BY a.stamp DESC,a.event_key DESC LIMIT ?', [*params, limit + 1]).fetchall()
    matches = []
    for row in rows[:limit]:
        event = json.loads(row['payload'])
        event.update({key: row[key] for key in ('service', 'agent_id', 'session_id',
            'trace_id', 'turn_id', 'result', 'domain', 'action')})
        reference = source_reference({**event, 'id': row['event_key']}, receiver)
        matches.append(dict(event_id=row['event_key'], text=row['matched_text'],
            time_unix_nano=row['stamp'], result=event.get('result') or 'unknown',
            action=event.get('action'), term_complete=bool(row['term_complete']),
            effect='recorded operation; effect requires source review',
            source_reference=reference, source_path=source_path(reference)))
    next_cursor = None
    if len(rows) > limit:
        last = rows[limit - 1]
        next_cursor = base64.urlsafe_b64encode(json.dumps(
            [VERSION, signature, last['stamp'], last['event_key']]).encode()).decode().rstrip('=')
    return dict(format='agenturm-investigation-v1', kind=kind, query=query,
        match_mode='exact recorded path; case and Unicode sensitive' if kind == 'path' else 'literal word phrase; case insensitive, accents preserved',
        receiver_id=receiver, matches=matches, next_cursor=next_cursor,
        scope='global retained Activity history' if agent_ids is None and service is None else 'explicit Activity ownership filter',
        coverage='Retained indexed source observations only. Clipped, uncaptured, unsupported or pruned evidence can be absent; no matches does not prove no activity. Recorded file attempts do not prove a persisted change.')
