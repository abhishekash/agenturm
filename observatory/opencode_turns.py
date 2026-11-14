"""Versioned OpenCode SQLite identity and idle-boundary metadata, never payloads."""
from __future__ import annotations
from dataclasses import dataclass
import hashlib
import json
import math

from .local_agents import identity

PREFIX = 'observatory.opencode.turn.'
MAX_MEMBERS = 1000
MAX_TOKENS = 2**53 - 1


def epoch(source_id, fingerprint):
    return identity('opencode-db-epoch', source_id, *fingerprint[-2:])


def namespace(source_id, source_epoch):
    return identity('OpenCode-v2-actor', source_id, source_epoch)


def trace_id(source_id, source_epoch, session_id, message_id):
    return identity('opencode-v2-trace', source_id, source_epoch, session_id, message_id)


def member_digest(traces):
    return hashlib.sha256(json.dumps(sorted(set(traces)), ensure_ascii=True,
                                    separators=(',', ':')).encode('ascii')).hexdigest()


def valid_id(value):
    return isinstance(value, str) and value.startswith('msg_') and len(value) <= 512


def usage_attributes(tokens):
    """Current TokenUsage.Info components are disjoint; OTel totals include them.

    Never fabricate a total when one of its required source components is absent.
    Legacy schemas have their own unchanged interpretation in the caller.
    """
    cache = tokens.get('cache') if isinstance(tokens.get('cache'), dict) else {}
    values = {'input': tokens.get('input'), 'output': tokens.get('output'),
              'reasoning': tokens.get('reasoning'), 'read': cache.get('read'), 'write': cache.get('write')}
    valid = {key: int(value) for key, value in values.items()
             if type(value) in (int, float) and 0 <= value <= MAX_TOKENS and math.isfinite(value) and int(value) == value}
    result = {}
    for key, target in (('reasoning','gen_ai.usage.reasoning_tokens'),
                        ('read','gen_ai.usage.cache_read.input_tokens'),
                        ('write','gen_ai.usage.cache_write.input_tokens')):
        if key in valid: result[target] = valid[key]
    for keys, target in ((('input','read','write'),'gen_ai.usage.input_tokens'),
                         (('output','reasoning'),'gen_ai.usage.output_tokens')):
        if all(key in valid for key in keys):
            total=sum(valid[key] for key in keys)
            if total <= MAX_TOKENS: result[target] = total
    return result


@dataclass(frozen=True)
class Turn:
    turn_id: str | None
    attrs: dict
    outcome: str = ''


class Resolver:
    """Poll-local bounded scalar proof cache; all queries share the source snapshot.

    A prior idle is positive boundary evidence. Missing prefix history never
    becomes a guessed public turn. Sequence gaps are normal durable-event gaps.
    """
    def __init__(self, db, source_id, source_epoch, session_id, membership_budget):
        self.db, self.source_id, self.epoch, self.session = db, source_id, source_epoch, session_id
        self.budget = membership_budget
        self.cache = {}

    def for_message(self, message_id, seq):
        if type(seq) is not int or seq < 0 or not valid_id(message_id):
            return Turn(None, {PREFIX+'schema': 2, PREFIX+'source_epoch': self.epoch,
                        PREFIX+'source_complete': False, PREFIX+'coverage_reason': 'Invalid native message identity/sequence.'})
        before = self.db.execute('''SELECT substr(id,1,513) AS id,seq FROM session_message
            WHERE session_id=? AND type='idle' AND seq<? ORDER BY seq DESC LIMIT 1''',
            (self.session, seq)).fetchone()
        after = self.db.execute('''SELECT substr(id,1,513) AS id,seq,CASE WHEN length(CAST(data AS BLOB))<=65536
            AND json_valid(data) THEN json_extract(data,'$.outcome') END AS outcome
            FROM session_message WHERE session_id=? AND type='idle' AND seq>?
            ORDER BY seq LIMIT 1''', (self.session, seq)).fetchone()
        key = (tuple(before) if before else None, tuple(after) if after else None)
        if key in self.cache:
            return self.cache[key]
        known = bool(before and valid_id(before['id']) and type(before['seq']) is int and before['seq']>=0)
        if before:
            boundary_id, boundary_seq = before['id'], before['seq']
        else:
            first = self.db.execute('SELECT substr(id,1,513) AS id,seq FROM session_message WHERE session_id=? ORDER BY seq LIMIT 1',
                                    (self.session,)).fetchone()
            boundary_id, boundary_seq = (first['id'], first['seq']) if first else ('', 0)
        closed = bool(after and valid_id(after['id']) and after['outcome'] in {'succeeded', 'failed', 'interrupted'})
        attrs = {PREFIX+'schema': 2, PREFIX+'source_epoch': self.epoch,
                 PREFIX+'boundary_kind': 'idle' if known else 'unknown-prefix',
                 PREFIX+'boundary_id': str(boundary_id)[:512], PREFIX+'boundary_seq': boundary_seq,
                 PREFIX+'closed': closed, PREFIX+'source_complete': False,
                 PREFIX+'coverage_reason': 'Earlier idle boundary unavailable; exact-step scope only.'}
        if after:
            attrs.update({PREFIX+'closed_id': str(after['id'])[:512], PREFIX+'closed_seq': after['seq']})
        turn_id = identity('opencode-v2-turn', self.source_id, self.epoch, self.session, boundary_id) if known else None
        if known:
            where = 'session_id=? AND type=\'assistant\' AND seq>?'
            params = [self.session, boundary_seq]
            if after:
                where += ' AND seq<?'
                params.append(after['seq'])
            count = self.db.execute('SELECT COUNT(*) FROM session_message WHERE '+where, params).fetchone()[0]
            attrs[PREFIX+'expected_assistants'] = count
            reason = ''
            if count > MAX_MEMBERS:
                reason = 'Native turn exceeds the 1,000-assistant membership limit.'
            elif count > self.budget[0]:
                reason = 'Native turn membership metadata deferred by this poll budget.'
            else:
                members = self.db.execute('SELECT substr(id,1,513) AS id FROM session_message WHERE '+where+' ORDER BY id LIMIT 1001', params).fetchall()
                self.budget[0] -= len(members)
                if len(members) != count or not all(valid_id(row['id']) for row in members):
                    reason = 'Native turn membership identity is incomplete or malformed.'
                elif after and not closed:
                    reason = 'Native closing idle outcome is malformed or unavailable.'
                else:
                    traces = [trace_id(self.source_id, self.epoch, self.session, row['id']) for row in members]
                    attrs[PREFIX+'membership_sha256'] = member_digest(traces)
                    attrs[PREFIX+'source_complete'] = True
            attrs[PREFIX+'coverage_reason'] = reason
        result = Turn(turn_id, attrs, after['outcome'] if closed else '')
        self.cache[key] = result
        return result


def message_window(reader, db, session_id, fields, windows, cursors, source_revision=None):
    """Keep recent work fresh while walking older source rows with a small keyset.

    State contains scalar IDs/sequences only. Payloads still use the reader's
    shared preflight and per-poll byte budget. Unrelated sessions cannot reset
    the cursor or starve this session's older messages.
    """
    from .opencode import MAX_MESSAGES_PER_SESSION, MAX_PAYLOAD_BYTES
    valid = "length(CAST(id AS BLOB))<=512 AND id GLOB 'msg_*' AND typeof(seq)='integer' AND seq>=0"
    roles = "type IN ('user','assistant','idle') AND "+valid
    latest = db.execute('SELECT id,seq,time_updated FROM session_message WHERE session_id=? AND '+roles+
                        ' ORDER BY seq DESC LIMIT 1', (session_id,)).fetchone()
    modified = db.execute('SELECT id,seq,time_updated FROM session_message WHERE session_id=? AND '+roles+
                          ' ORDER BY time_updated DESC,seq DESC LIMIT 1', (session_id,)).fetchone()
    assistant = db.execute("SELECT id,seq,time_updated FROM session_message WHERE session_id=? AND type='assistant' AND "+valid+" ORDER BY seq DESC LIMIT 1", (session_id,)).fetchone()
    part = None
    tables = reader._tables(db)
    if 'session_part' in tables and {'message_id','session_id','ordinal','time_updated','data','type'} <= reader._columns(db,'session_part'):
        part = db.execute('''SELECT m.id,m.seq,p.time_updated FROM session_part p JOIN session_message m
            ON m.id=p.message_id AND m.session_id=p.session_id WHERE p.session_id=? AND m.type='assistant'
            AND length(CAST(m.id AS BLOB))<=512 AND m.id GLOB 'msg_*' AND typeof(m.seq)='integer' AND m.seq>=0
            ORDER BY p.time_updated DESC,m.seq DESC,p.ordinal DESC LIMIT 1''', (session_id,)).fetchone()
    # An external writer can change equal-length part text without changing its
    # timestamp. Revisit a completed walk when the source file changes too.
    signature = (source_revision, *(tuple(row) if row else None for row in (latest, modified, part)))
    state = dict(windows.get(session_id) or {})
    if not state or (state.get('done') and state.get('signature') != signature):
        state = dict(before=None, pending=[], done=False)
    # Streaming updates do not rewind a still-incomplete historical walk.
    # Fresh/changed rows are serviced through the independent priority slots.
    state['signature'] = signature
    priority = list(dict.fromkeys(row['id'] for row in (latest, assistant, modified, part) if row))
    if not state.get('pending') and not state.get('done'):
        where = 'session_id=? AND '+roles
        args = [session_id]
        if state.get('before') is not None:
            where += ' AND seq<?';args.append(state['before'])
        if priority:
            where += ' AND id NOT IN ('+','.join('?' for _ in priority)+')';args.extend(priority)
        page = db.execute('SELECT id,seq FROM session_message WHERE '+where+
                          ' ORDER BY seq DESC LIMIT ?', [*args, MAX_MESSAGES_PER_SESSION-len(priority)]).fetchall()
        state['pending'] = [row['id'] for row in page]
        state['next_before'] = min((row['seq'] for row in page), default=state.get('before'))
        if not page:
            state['done'] = True
    selected = list(dict.fromkeys([*priority, *state.get('pending',())]))[:MAX_MESSAGES_PER_SESSION]
    if not selected:
        windows[session_id] = state
        return [], set(), assistant
    marks = ','.join('?' for _ in selected)
    messages, ids = reader._payload_rows(db,'session_message',fields,
        ' WHERE session_id=? AND id IN ('+marks+') ORDER BY seq DESC LIMIT ?',
        [session_id,*selected,MAX_MESSAGES_PER_SESSION],cursors=cursors,
        cursor_key=('session_message',session_id,None),messages=True,advance=False)
    loaded = {str(row['id']) for row in messages}
    oversized = {row['id'] for row in db.execute('SELECT id FROM session_message WHERE session_id=? AND id IN ('+marks+') AND length(CAST(data AS BLOB))>?',
                                                [session_id,*selected,MAX_PAYLOAD_BYTES])}
    state['pending'] = [mid for mid in state.get('pending',()) if mid not in loaded and mid not in oversized]
    if not state['pending'] and not state.get('done'):
        state['before'] = state.get('next_before')
        state['done'] = not bool(db.execute('SELECT 1 FROM session_message WHERE session_id=? AND '+roles+' AND seq<? LIMIT 1',
                                           (session_id,state['before'])).fetchone()) if state['before'] is not None else True
    if not state.get('done') or state['pending']:
        reader.backlog_records += 1
    windows[session_id] = state
    return messages, ids, assistant


def read_parts(reader, db, session_id, message_id, *, cursors):
    """Read a bounded composite-key native part window; outputs retain roles."""
    from .opencode import MAX_PARTS_PER_MESSAGE, MAX_PAYLOAD_BYTES, MAX_POLL_PAYLOAD_BYTES
    rows = db.execute('''SELECT ordinal,type,call_id,time_created,time_updated,length(CAST(data AS BLOB)) AS size
        FROM session_part WHERE session_id=? AND message_id=? ORDER BY ordinal LIMIT ?''',
        (session_id,message_id,MAX_PARTS_PER_MESSAGE+1)).fetchall()
    complete = len(rows)<=MAX_PARTS_PER_MESSAGE
    rows = rows[:MAX_PARTS_PER_MESSAGE]
    start = next((i for i,row in enumerate(rows) if row['ordinal']==cursors.get(('session_part',session_id,message_id))),0)
    rows = rows[start:]+rows[:start]
    output,stamps,text = [],{},[]
    deferred = None
    for row in rows:
        pid = message_id+':ordinal:'+str(row['ordinal'])
        if type(row['ordinal']) is not int or row['ordinal']<0:
            reader.schema_errors+=1;complete=False;continue
        size=row['size'] or 0
        if size>MAX_PAYLOAD_BYTES:
            reader.skipped_oversized+=1;complete=False;continue
        if reader.payload_bytes+size>MAX_POLL_PAYLOAD_BYTES:
            reader.skipped_budget+=1;reader.budget_excluded_bytes+=size;reader.backlog_records+=1
            deferred=row['ordinal'] if deferred is None else deferred;complete=False;continue
        raw=db.execute('SELECT data FROM session_part WHERE session_id=? AND message_id=? AND ordinal=?',
                       (session_id,message_id,row['ordinal'])).fetchone()[0]
        reader.payload_bytes+=size
        data=reader._decode(raw)
        # The producer's decoder treats the scalar type column as authoritative.
        data={**data,'type':row['type']}
        if row['type']=='text' and isinstance(data.get('text'),str):
            text.append((row['ordinal'],data))
        if row['type']=='tool':
            state=data.get('state') if isinstance(data.get('state'),dict) else {}
            state=dict(state)
            if 'content' in state:state['output']=state['content']
            timing=data.get('time') if isinstance(data.get('time'),dict) else {}
            state['time']={'start':timing.get('ran',timing.get('created',row['time_created'])),
                           'end':timing.get('completed',row['time_updated'])}
            normalized={'type':'tool','tool':data.get('name') or 'tool','state':state,
                        'metadata':data.get('native') if isinstance(data.get('native'),dict) else {}}
            output.append((pid,json.dumps(normalized,ensure_ascii=False)))
            # Content participates even for equal timestamp/length streaming updates.
            stamps[pid]=hashlib.sha256(raw.encode('utf-8','replace')).hexdigest()
    if rows:
        cursors[('session_part',session_id,message_id)]=deferred if deferred is not None else rows[0]['ordinal']
    text.sort(key=lambda pair:pair[0])
    return output,stamps,[row for _,row in text],complete,bool(rows)
