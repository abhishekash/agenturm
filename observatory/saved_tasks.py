"""User-owned task records that survive agent sessions and telemetry retention."""
from __future__ import annotations

from datetime import datetime, timezone
from contextlib import contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import stat
import time
import uuid

from .task_workspace import capture_workspace, display_text

MAX_NOTES = 32000
MAX_LINKS = 64
MAX_TASK_BYTES = 4 * 1024 * 1024
MAX_DECISIONS = 128
BRIEF_FIELDS = ('intent', 'acceptance_criteria', 'constraints', 'open_questions', 'next_action')
DECISION_KINDS = ('accepted', 'rejected')
TOKEN_FIELDS = ('input_tokens', 'output_tokens', 'cache_read_input_tokens',
                'cache_write_input_tokens', 'reasoning_output_tokens')


class TaskError(ValueError):
    pass


class TaskConflict(TaskError):
    pass


def default_path():
    override = os.environ.get('AGENTURM_TASKS_DB')
    return Path(override).expanduser() if override else Path(
        os.environ.get('XDG_STATE_HOME', Path.home() / '.local' / 'state')) / 'agenturm' / 'tasks.sqlite3'


def backup_task_database(path, destination):
    """SQLite backup API works before initialization or migration of side tables."""
    source_path = Path(path).expanduser().resolve()
    target = Path(destination).expanduser()
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(fd)
    try:
        source = sqlite3.connect(source_path.as_uri() + '?mode=ro', uri=True, timeout=3)
        try:
            with sqlite3.connect(target) as backup:
                source.backup(backup)
        finally:
            source.close()
    except BaseException:
        target.unlink(missing_ok=True)
        raise
    return str(target)


def _text(value, name, limit, *, required=False, multiline=False):
    if not isinstance(value, str):
        raise TaskError(f'{name} must be text.')
    value = value.strip() if not multiline else value
    if required and not value:
        raise TaskError(f'{name} is required.')
    if len(value) > limit:
        raise TaskError(f'{name} must be at most {limit:,} characters.')
    if any(ord(char) < 32 and not (multiline and char in '\n\t') or 127 <= ord(char) <= 159 for char in value):
        raise TaskError(f'{name} contains unsupported control characters.')
    return value


def _number(value, name):
    if (isinstance(value, bool) or not isinstance(value, (int, float)) or
            not 0 <= value <= 10**18 or isinstance(value, float) and not math.isfinite(value)):
        raise TaskError(f'{name} must be a finite non-negative number.')
    return value


def _timestamp(value, name):
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 2**63 - 1:
        raise TaskError(f'{name} must be a non-negative integer nanosecond timestamp.')
    return value


def _workspace(value):
    value = _text(value, 'Workspace', 4096)
    return str(Path(value).expanduser().resolve()) if value else ''


def _link(evidence):
    if not isinstance(evidence, dict):
        raise TaskError('Linked evidence must be an object.')
    if evidence.get('format')=='agenturm-review-v1':
        scope=evidence.get('scope')
        if not isinstance(scope,dict):raise TaskError('Review scope must be an object.')
        evidence={**{key:scope.get(key) for key in ('receiver','producer','agent_id','session_id','run_start_ns','run_end_ns')},
            'trace_id':'' if scope.get('turn_id') else scope.get('run_id'),'turn_id':scope.get('turn_id'),
            'step_trace_ids':(evidence.get('usage') or {}).get('step_trace_ids',[]),
            'usage':(evidence.get('usage') or {}).get('values',{}),'task':evidence.get('title'),'state':evidence.get('reported_outcome'),
            'review_snapshot':evidence}
    result = {}
    for key, limit in (('session_id', 512), ('agent_id', 512), ('parent_agent_id', 512), ('runtime', 120),
                       ('receiver', 2000), ('producer', 512),
                       ('task', 500), ('state', 120), ('working_directory', 4096),
                       ('branch', 512), ('resume_command', 2000)):
        value = evidence.get(key)
        if value is not None:
            result[key] = _text(value, key, limit, multiline=key == 'task')
    trace = evidence.get('trace_id') or ''
    if trace and (not isinstance(trace, str) or not re.fullmatch(r'[0-9a-fA-F]{32}', trace)):
        raise TaskError('trace_id must contain 32 hexadecimal characters.')
    result['trace_id'] = trace.lower()
    turn=evidence.get('turn_id') or ''
    if turn:
        if trace or not isinstance(turn,str) or not re.fullmatch(r'[0-9a-f]{32}',turn):raise TaskError('Choose a recorded turn or a step trace; turn_id must be 32 hexadecimal characters.')
        result['turn_id']=turn
        members=evidence.get('step_trace_ids') or []
        if not isinstance(members,list) or len(members)>1000 or any(not isinstance(value,str) or not re.fullmatch(r'[0-9a-f]{32}',value) for value in members):raise TaskError('Turn step identities must be bounded recorded trace IDs.')
        result['step_trace_ids']=sorted(set(members))
    start = evidence.get('run_start_ns')
    if start is not None:
        start = _timestamp(start, 'Run lifecycle start')
        if not start:
            raise TaskError('Run lifecycle start must be a positive recorded timestamp.')
        if not result.get('receiver') or not result.get('producer'):
            raise TaskError('A native lifecycle run needs its receiver and producer namespace.')
        result['run_start_ns'] = start
    end = evidence.get('run_end_ns')
    if end is not None:
        end = _timestamp(end, 'Run lifecycle end')
        if not start or end < start:
            raise TaskError('Run lifecycle end must follow its recorded start.')
        result['run_end_ns'] = end
    if not (trace or result.get('session_id') or result.get('agent_id')):
        raise TaskError('Link a recorded trace or an identified session.')
    identity = '\0'.join(result.get(key, '') for key in ('runtime', 'session_id', 'agent_id'))
    result['key'] = ('turn:'+turn if turn else 'trace:' + trace.lower() if trace else
        'native-run:' + hashlib.sha256(('\0'.join(result.get(key, '') for key in ('session_id', 'agent_id'))+'\0'+str(start)).encode()).hexdigest()[:24] if start else
        'session:' + hashlib.sha256(identity.encode()).hexdigest()[:24])
    if result.get('receiver'):
        namespace = hashlib.sha256('\0'.join(result.get(key, '') for key in
            ('receiver', 'producer', 'agent_id', 'session_id')).encode()).hexdigest()[:24]
        result['key'] += '@' + namespace
    result['captured_ns'] = _timestamp(evidence.get('captured_ns', time.time_ns()), 'captured_ns')
    usage = evidence.get('usage')
    if usage is None:
        usage = {}
    if not isinstance(usage, dict):
        raise TaskError('Usage must be an object.')
    result['usage'] = {}
    # Session associations cannot be safely added to exact run totals.
    if trace or turn and result.get("step_trace_ids"):
        for field in TOKEN_FIELDS:
            if usage.get(field) is not None:
                value = _number(usage[field], field)
                if value != int(value):
                    raise TaskError(f'{field} must be a whole token count.')
                result['usage'][field] = int(value)
                result['usage'][field + '_reported'] = usage.get(field + '_reported') is not False
            elif field + '_reported' in usage:
                result['usage'][field + '_reported'] = False
        if usage.get('known_cost_usd') is not None:
            result['usage']['known_cost_usd'] = float(_number(usage['known_cost_usd'], 'known_cost_usd'))
        for flag in ('cost_complete', 'usage_complete'):
            result['usage'][flag] = usage.get(flag) is True
    checks = evidence.get('checks')
    if checks is None:
        checks = []
    if not isinstance(checks, list) or len(checks) > 32:
        raise TaskError('Linked evidence may contain at most 32 recorded checks.')
    result['checks'] = []
    for check in checks:
        if not isinstance(check, dict):
            raise TaskError('Each recorded check must be an object.')
        check_trace = check.get('trace_id') or ''
        if trace and check_trace and check_trace != trace:
            continue
        command = _text(check.get('command', ''), 'Check command', 2000, multiline=True)
        if command:
            check_time = _timestamp(check.get('time_ns') or check.get('time_unix_nano') or 0, 'Check time')
            if start and not trace and (end is None or not start <= check_time <= end):
                continue
            result['checks'].append({'command': command,
                'result': _text(str(check.get('result') or 'not reported'), 'Check result', 200),
                'time_ns': check_time,
                'trace_id': _text(check_trace, 'Check trace', 64),
                'scope': 'linked run' if trace and check_trace else 'native lifecycle run; captured interval may be partial' if start else 'session activity; run assignment unverified'})
    if evidence.get('review_snapshot') is not None:
        projection = evidence['review_snapshot']
        if not isinstance(projection, dict):
            raise TaskError('Review snapshot must be an object.')
        scope = projection.get('scope') or {}
        if (not (trace or start or turn) or (scope.get('turn_id') != turn if turn else scope.get('run_id') != trace if trace else scope.get('run_start_ns') != start) or
                result.get('receiver') and scope.get('receiver') != result['receiver'] or
                result.get('producer') and scope.get('producer') != result['producer'] or
                result.get('agent_id') and scope.get('agent_id') != result['agent_id'] or
                result.get('session_id') and scope.get('session_id') != result['session_id']):
            raise TaskError('Review snapshot must match the exact linked run and agent.')
        try:
            serialized = json.dumps(projection, ensure_ascii=False, allow_nan=False)
        except (ValueError, TypeError, RecursionError) as exc:
            raise TaskError('Review snapshot must contain bounded JSON evidence.') from exc
        if len(serialized.encode()) > 3 * 1024 * 1024:
            raise TaskError('Review snapshot exceeds 3 MiB. Save a smaller explicit evidence selection or create the task without this link.')
        try:
            from .workflow_evidence import render_review
            if projection.get('format') != 'agenturm-review-v1':
                raise ValueError('Unsupported review format')
            render_review(projection)
            if (not trace and not turn and not scope.get('run_end_ns') and
                    any(projection[key] for key in ('checks', 'edits', 'errors', 'context_events'))):
                raise ValueError('Native review needs a recorded lifecycle end')
            if ('Metadata-only capture' in projection['window']['reasons'] and
                    any(projection[key] for key in ('checks', 'edits', 'errors', 'context_events'))):
                raise ValueError('Metadata-only capture cannot contain review payloads')
        except (KeyError, TypeError, ValueError, AttributeError) as exc:
            raise TaskError('Review snapshot is malformed or exceeds its capture permissions.') from exc
        result['review_snapshot'] = json.loads(serialized)
    return result


def usage_summary(links):
    runs = {(link.get('receiver', ''), link.get('producer', ''), link.get('agent_id', ''),
             link.get('session_id', ''), ('turn:'+link['turn_id']) if link.get('turn_id') else link.get('trace_id') or link.get('run_start_ns')): link
            for link in links if link.get('turn_id') or link.get('trace_id') or link.get('run_start_ns')}
    shared = {}
    for identity, link in runs.items():
        members=link.get('step_trace_ids') if link.get('turn_id') else [link['trace_id']] if link.get('trace_id') else []
        for member in members or []:shared.setdefault((link.get('receiver',''),member),set()).add(identity)
    overlapping = sum(len(scopes) > 1 for scopes in shared.values())
    recorded_agents = {(link.get('receiver', ''), link.get('agent_id')) for link in runs.values() if link.get('agent_id')}
    related = sum(bool(link.get('parent_agent_id')) and
                  (link.get('receiver', ''), link['parent_agent_id']) in recorded_agents
                  for link in runs.values())
    ambiguous = bool(overlapping or related)
    unscoped = sum(not (link.get('turn_id') or link.get('trace_id') or link.get('run_start_ns')) for link in links)
    result = {'scope': 'Recorded linked runs only; snapshots update when linked evidence is refreshed.',
              'linked_runs': len(runs), 'unscoped_links': unscoped,
              'ambiguous_shared_traces': overlapping, 'related_run_scopes': related,
              'aggregation_ambiguous': ambiguous,
              'relationship_coverage': 'Recorded parent links only; missing relationships remain unknown.'}
    if overlapping:
        result['scope'] += ' Multiple agent/session scopes share a trace; aggregate usage is unavailable because their accounting may overlap. Review each linked scope separately.'
    elif related:
        result['scope'] += ' Recorded parent and child agents are linked; aggregate usage is unavailable because their accounting may overlap. Review each linked scope separately.'
    result['scope'] += ' Relationship coverage includes recorded parent links only; missing relationships remain unknown.'
    for field in TOKEN_FIELDS:
        reported = [link['usage'][field] for link in runs.values()
                    if link.get('usage', {}).get(field + '_reported') and field in link['usage']]
        result[field] = sum(reported) if reported and not ambiguous else None
        result[field + '_complete'] = bool(runs) and not ambiguous and len(reported) == len(runs) and not unscoped and all(
            link.get('usage', {}).get('usage_complete') for link in runs.values())
    costs = [link['usage']['known_cost_usd'] for link in runs.values()
             if link.get('usage', {}).get('known_cost_usd') is not None]
    result['known_cost_usd'] = round(sum(costs), 8) if costs and not ambiguous else None
    result['cost_complete'] = bool(runs) and not ambiguous and not unscoped and len(costs) == len(runs) and all(
        link.get('usage', {}).get('cost_complete') for link in runs.values())
    result['usage_complete'] = result['input_tokens_complete'] and result['output_tokens_complete'] and all(
        link.get('usage', {}).get('usage_complete') for link in runs.values())
    return result


class TaskStore:
    def __init__(self, path=None):
        self.path = Path(path).expanduser() if path is not None else default_path()
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        try:
            fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            mode = self.path.lstat().st_mode
            if not stat.S_ISREG(mode):
                raise TaskError('Task database must be a regular file, not a symlink or directory.')
        else:
            os.close(fd)
        os.chmod(self.path, 0o600)
        with self._connect() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('''CREATE TABLE IF NOT EXISTS saved_tasks_v1 (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, workspace TEXT NOT NULL,
                notes TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0,
                created_ns INTEGER NOT NULL, updated_ns INTEGER NOT NULL,
                revision INTEGER NOT NULL, links_json TEXT NOT NULL,
                snapshot_json TEXT NOT NULL)''')
            db.execute('CREATE INDEX IF NOT EXISTS saved_tasks_recent ON saved_tasks_v1(archived,updated_ns DESC,id)')
            db.execute('CREATE TABLE IF NOT EXISTS task_schema_v1 (version INTEGER NOT NULL)')
            version = db.execute('SELECT version FROM task_schema_v1').fetchone()
            if version and version[0] != 1:
                raise TaskError('Unsupported task schema version. Use a compatible Agenturm build.')
            if not version:
                db.execute('INSERT INTO task_schema_v1 VALUES(1)')
            db.execute('CREATE TABLE IF NOT EXISTS task_briefs_v1 (task_id TEXT PRIMARY KEY, brief_json TEXT NOT NULL)')
            db.execute('''CREATE TABLE IF NOT EXISTS task_decisions_v1 (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL, decision_json TEXT NOT NULL)''')
            db.execute('CREATE INDEX IF NOT EXISTS task_decisions_task ON task_decisions_v1(task_id)')

    @contextmanager
    def _connect(self):
        db = sqlite3.connect(self.path, timeout=3)
        db.row_factory = sqlite3.Row
        try:
            with db:
                yield db
        finally:
            db.close()

    def close(self):
        """Connections are short lived; retained for context-independent callers."""

    def list(self, include_archived=False, *, offset=0):
        if isinstance(offset, bool) or not isinstance(offset, int) or not 0 <= offset <= 2**63 - 1:
            raise TaskError('List offset must be a non-negative integer.')
        with self._connect() as db:
            rows = db.execute('''SELECT id,title,workspace,archived,created_ns,updated_ns,revision,
                json_array_length(links_json) AS link_count FROM saved_tasks_v1''' +
                ('' if include_archived else ' WHERE archived=0') +
                ' ORDER BY updated_ns DESC,id LIMIT 1000 OFFSET ?', (offset,)).fetchall()
        return [{**dict(row), 'archived': bool(row['archived'])} for row in rows]

    def count(self, include_archived=False):
        with self._connect() as db:
            return db.execute('SELECT COUNT(*) FROM saved_tasks_v1' +
                              ('' if include_archived else ' WHERE archived=0')).fetchone()[0]

    def _read(self, db, task_id):
        if not isinstance(task_id, str) or len(task_id) > 80:
            raise TaskError('Invalid saved task ID.')
        row = db.execute('SELECT * FROM saved_tasks_v1 WHERE id=?', (task_id,)).fetchone()
        if row is None:
            raise TaskError('Saved task was not found. Use `agenturm task list` to find its ID.')
        result = dict(row)
        result['archived'] = bool(result['archived'])
        try:
            result['links'] = json.loads(result.pop('links_json'))
            result['workspace_snapshot'] = json.loads(result.pop('snapshot_json'))
        except (ValueError, RecursionError) as exc:
            raise TaskError('Saved task data is damaged. Restore the task database from a backup.') from exc
        result['usage_summary'] = usage_summary(result['links'])
        try:
            brief = db.execute('SELECT brief_json FROM task_briefs_v1 WHERE task_id=?', (task_id,)).fetchone()
            result['brief'] = json.loads(brief[0]) if brief else dict.fromkeys(BRIEF_FIELDS, '')
            result['decisions'] = [json.loads(row[0]) for row in db.execute(
                'SELECT decision_json FROM task_decisions_v1 WHERE task_id=? ORDER BY rowid', (task_id,))]
        except (ValueError, RecursionError) as exc:
            raise TaskError('Structured task data is damaged. Restore a task database backup.') from exc
        return result

    def get(self, task_id):
        with self._connect() as db:
            return self._read(db, task_id)

    @staticmethod
    def _check_size(task):
        without_receipts={**task,'links':[{key:value for key,value in link.items() if key!='review_snapshot'} for link in task.get('links',[])]}
        if len(json.dumps(without_receipts,ensure_ascii=False).encode())>512*1024:
            raise TaskError('Saved task fields exceed the 512 KiB limit. Shorten this draft, unlink older evidence, or export it and start a new task; no saved decisions were removed.')
        if len(json.dumps(task, ensure_ascii=False).encode()) > MAX_TASK_BYTES:
            raise TaskError('Saved task including review snapshots exceeds the 4 MiB limit. Export and unlink older receipts before adding more evidence.')

    def create(self, title, workspace='', notes='', link=None):
        now = time.time_ns()
        task = {'id': str(uuid.uuid4()), 'title': _text(title, 'Title', 200, required=True),
                'workspace': _workspace(workspace), 'notes': _text(notes, 'Notes', MAX_NOTES, multiline=True),
                'archived': False, 'created_ns': now, 'updated_ns': now, 'revision': 1,
                'links': [_link(link)] if link is not None else [], 'workspace_snapshot': {}}
        self._check_size(task)
        with self._connect() as db:
            db.execute('INSERT INTO saved_tasks_v1 VALUES(?,?,?,?,?,?,?,?,?,?)',
                       (task['id'], task['title'], task['workspace'], task['notes'], 0,
                        now, now, 1, json.dumps(task['links']), '{}'))
        return self.get(task['id'])

    def _edit(self, task_id, modify, expected_revision=None):
        with self._connect() as db:
            db.execute('BEGIN IMMEDIATE')
            task = self._read(db, task_id)
            if expected_revision is not None and task['revision'] != expected_revision:
                raise TaskConflict('This task changed elsewhere. Reload it before saving your edits.')
            modify(task)
            task['updated_ns'] = time.time_ns()
            task['revision'] += 1
            self._check_size(task)
            db.execute('''UPDATE saved_tasks_v1 SET title=?,workspace=?,notes=?,archived=?,
                updated_ns=?,revision=?,links_json=?,snapshot_json=? WHERE id=?''',
                (task['title'], task['workspace'], task['notes'], int(task['archived']),
                 task['updated_ns'], task['revision'], json.dumps(task['links']),
                 json.dumps(task['workspace_snapshot']), task_id))
            db.execute('INSERT OR REPLACE INTO task_briefs_v1 VALUES(?,?)',
                       (task_id, json.dumps(task.get('brief', {}), ensure_ascii=False)))
            for decision in task.get('decisions', []):
                # Decisions are immutable; an edit appends a superseding version.
                db.execute('INSERT OR IGNORE INTO task_decisions_v1 VALUES(?,?,?)',
                           (decision['id'], task_id, json.dumps(decision, ensure_ascii=False)))
        return self.get(task_id)

    def update_brief(self, task_id, brief, *, expected_revision=None):
        if not isinstance(brief, dict) or set(brief) - set(BRIEF_FIELDS):
            raise TaskError('Brief must contain only: ' + ', '.join(BRIEF_FIELDS))
        values = {key: _text(value, key, 4000, multiline=True) for key, value in brief.items()}
        return self._edit(task_id, lambda task: task['brief'].update(values), expected_revision)

    def add_decision(self, task_id, *, kind, text, rationale='', scope='', source=None,
                     supersedes_id=None, expected_revision=None):
        if kind not in DECISION_KINDS:
            raise TaskError('Decision kind must be accepted or rejected.')
        if not isinstance(text, str) or not text.strip():
            raise TaskError('Decision text is required.')
        if supersedes_id is not None and (not isinstance(supersedes_id, str) or not supersedes_id or len(supersedes_id) > 80):
            raise TaskError('Supersedes must be an existing decision ID in this task.')
        decision = {'id': str(uuid.uuid4()), 'kind': kind,
                    'text': _text(text, 'Decision', 4000, required=True, multiline=True),
                    'rationale': _text(rationale, 'Rationale', 4000, multiline=True),
                    'scope': _text(scope, 'Scope', 4000, multiline=True),
                    'created_ns': time.time_ns(), 'author_kind': 'user',
                    'supersedes_id': supersedes_id or None, 'source': None}
        if source is not None:
            if not isinstance(source, dict):
                raise TaskError('Decision source must be an object.')
            allowed = ('receiver', 'producer', 'agent_id', 'session_id', 'trace_id', 'event_id', 'coverage')
            if set(source) - set(allowed) - {'captured_ns', 'content_available', 'run_start_ns'}:
                raise TaskError('Unsupported decision source fields.')
            if source.get('content_available') is False:
                raise TaskError('Metadata-only evidence cannot supply a decision excerpt. Enter a manual decision without a source.')
            decision['source'] = {key: _text(source[key], 'Source '+key, 2000) for key in allowed if key in source}
            decision['source']['captured_ns'] = _timestamp(source.get('captured_ns', time.time_ns()), 'Source capture time')
            if source.get('run_start_ns') is not None:
                value = _timestamp(source['run_start_ns'], 'Source lifecycle start')
                if not value:
                    raise TaskError('Source lifecycle start must be positive.')
                decision['source']['run_start_ns'] = value
            decision['source']['availability'] = 'Reference saved; retained source has not been revalidated'
        def modify(task):
            decisions = task['decisions']
            if len(decisions) >= MAX_DECISIONS:
                raise TaskError('A task can retain at most 128 decision versions. Export the task and start a new task to continue; no decisions were removed.')
            if supersedes_id:
                if not isinstance(supersedes_id, str) or not any(item['id'] == supersedes_id for item in decisions):
                    raise TaskError('A superseded decision must belong to this saved task.')
                if any(item.get('supersedes_id') == supersedes_id for item in decisions):
                    raise TaskError('That decision already has a newer version. Supersede its current version instead.')
            # The new UUID can reference only an existing version, so a cycle
            # or a reference into another task cannot be introduced.
            decisions.append(decision)
        return self._edit(task_id, modify, expected_revision)

    def backup(self, destination):
        """Make an explicit consistent private backup without overwriting a file."""
        return backup_task_database(self.path, destination)

    def update(self, task_id, *, title=None, workspace=None, notes=None, archived=None, expected_revision=None):
        def modify(task):
            if title is not None:
                task['title'] = _text(title, 'Title', 200, required=True)
            if workspace is not None:
                value = _workspace(workspace)
                if value != task['workspace']:
                    task['workspace_snapshot'] = {}
                task['workspace'] = value
            if notes is not None:
                task['notes'] = _text(notes, 'Notes', MAX_NOTES, multiline=True)
            if archived is not None:
                if not isinstance(archived, bool):
                    raise TaskError('Archived must be true or false.')
                task['archived'] = archived
        return self._edit(task_id, modify, expected_revision)

    def link(self, task_id, evidence, *, expected_revision=None):
        entry = _link(evidence)
        def modify(task):
            def same_scope(link):
                # Recognize an older namespaced key only through all of its
                # recorded scope fields. Unnamespaced legacy keys stay exact.
                return (bool(entry.get('receiver')) and bool(link.get('receiver')) and
                    all(link.get(key) == entry.get(key) for key in
                        ('receiver','producer','agent_id','session_id','trace_id','run_start_ns','turn_id')))
            index = next((i for i, link in enumerate(task['links']) if link['key'] == entry['key'] or same_scope(link)), None)
            if index is None:
                if len(task['links']) >= MAX_LINKS:
                    raise TaskError(f'A task can link at most {MAX_LINKS} runs or sessions. Unlink older evidence first.')
                task['links'].append(entry)
            elif task['links'][index]['captured_ns'] <= entry['captured_ns']:
                previous = task['links'][index]
                if previous.get('parent_agent_id') and not entry.get('parent_agent_id'):
                    # Missing metadata in a refresh is not evidence that a
                    # recorded relationship or possible accounting overlap ended.
                    entry['parent_agent_id'] = previous['parent_agent_id']
                    entry['parent_identity_retained'] = True
                task['links'][index] = entry
        return self._edit(task_id, modify, expected_revision)

    def unlink(self, task_id, link_key, *, expected_revision=None):
        def modify(task):
            task['links'] = [link for link in task['links'] if link['key'] != link_key]
        return self._edit(task_id, modify, expected_revision)

    def snapshot(self, task_id, *, expected_revision=None):
        task = self.get(task_id)
        if expected_revision is not None and task['revision'] != expected_revision:
            raise TaskConflict('This task changed elsewhere. Reload it before capturing the workspace.')
        snapshot = capture_workspace(task['workspace'], task['workspace_snapshot'])
        return self._edit(task_id, lambda value: value.update(workspace_snapshot=snapshot), task['revision'])

    def handoff(self, task_id, format='markdown'):
        return render_handoff(self.get(task_id), format)


def _iso(ns):
    return datetime.fromtimestamp(ns / 1e9, timezone.utc).isoformat(timespec='seconds').replace('+00:00', 'Z') if ns else 'Not recorded'


def _fence(text, language=''):
    text = display_text(text)
    runs = re.findall(r'`+', text)
    marker = '`' * max(3, max(map(len, runs), default=0) + 1)
    return f'{marker}{language}\n{text.rstrip()}\n{marker}'


def _md(text):
    return re.sub(r'([\\`*_{}\[\]<>()#+.!|~-])', r'\\\1', display_text(text))


def render_handoff(task, format='markdown', *, include_decision_history=False):
    """Render an already captured record without refreshing or mutating anything."""
    if format == 'json':
        return json.dumps({'format': 'agenturm-task-v1', 'task': task}, ensure_ascii=True, indent=2) + '\n'
    if format not in {'markdown', 'html'}:
        raise TaskError('Handoff format must be markdown, html, or json.')
    usage = usage_summary(task.get('links', []))
    snapshot = task.get('workspace_snapshot') or {}
    lines = ['# ' + _md(task['title']), '', f"Saved task: `{task['id']}`", '',
             f"Updated: {_iso(task['updated_ns'])}", '', '## Intent, decisions, and next steps', '',
             _fence(task['notes']) if task.get('notes') else 'Add your intent, decisions, and next steps before handing this task over.',
             '', '## Workspace', '', _fence(task.get('workspace') or 'No workspace selected')]
    structured = []
    for key in BRIEF_FIELDS:
        value = task.get('brief', {}).get(key)
        if value:
            structured += ['', '### ' + key.replace('_', ' ').title(), '', _fence(value)]
    decisions = task.get('decisions', [])
    replaced = {item.get('supersedes_id') for item in decisions}
    if decisions:
        structured += ['', '### Decisions and rejected approaches', '',
                       'User-owned decisions; recorded agent statements are not promoted automatically.']
        for decision in decisions:
            if decision['id'] in replaced and not include_decision_history:
                continue
            structured += ['', '#### ' + decision['kind'].title() + (' · superseded' if decision['id'] in replaced else ''),
                           '', _fence(decision['text']), '', 'Decision: ' + decision['id'],
                           'Created: ' + _iso(decision['created_ns'])]
            if decision.get('rationale'):
                structured += ['', 'Rationale:', _fence(decision['rationale'])]
            if decision.get('scope'):
                structured += ['', 'Scope:', _fence(decision['scope'])]
            if decision.get('supersedes_id'):
                structured += ['', 'Supersedes: ' + decision['supersedes_id']]
            if decision.get('source'):
                structured += ['', 'Saved source reference (availability not revalidated):',
                               _fence(json.dumps(decision['source'], ensure_ascii=False, indent=2))]
        if replaced and not include_decision_history:
            structured += ['', f'{len(replaced)} superseded decision version(s) retained; use Decisions or --include-decision-history to read history.']
    if structured:
        index = lines.index('## Workspace')
        lines[index:index] = ['## Structured brief', *structured, '']
    if snapshot:
        lines += ['', f"- Captured: {_iso(snapshot.get('captured_ns'))}",
                  '- Branch: ' + _md(snapshot.get('branch') or 'Not available'),
                  '- Saved base: ' + _md(snapshot.get('base_commit') or 'Not available'),
                  '- Observed HEAD: ' + _md(snapshot.get('head') or 'Not available'), '',
                  snapshot.get('scope', ''), '', snapshot.get('checks', '')]
        if snapshot.get('error'):
            lines += ['', 'Capture issue: ' + _md(snapshot['error'])]
        lines += ['', 'Capture: ' + ('within output limits' if snapshot.get('complete') else 'partial or unavailable')]
        for warning in snapshot.get('warnings', []):
            lines.append('- ' + _md(warning))
        if snapshot.get('files'):
            lines += ['', 'Changed paths:', ''] + [
                '- ' + _md(entry['status'] + ' ' + entry['path']) for entry in snapshot['files']]
        if snapshot.get('stat'):
            lines += ['', _fence(snapshot['stat'])]
        if snapshot.get('diff'):
            lines += ['', '### Captured diff', '', _fence(snapshot['diff'], 'diff')]
    else:
        lines += ['', 'No Git snapshot saved. Capture the workspace before reviewing or continuing.']
    lines += ['', '## Recorded linked runs', '', usage['scope'], '',
              f"Exact runs: {usage['linked_runs']} · Session associations without a run: {usage['unscoped_links']}"]
    for field, label in (('input_tokens', 'Input tokens'), ('output_tokens', 'Output tokens'),
                         ('cache_read_input_tokens', 'Cached input'), ('cache_write_input_tokens', 'Cache writes')):
        value = usage[field]
        lines.append('- ' + label + ': ' + ('Not reported' if value is None else f'{value:,}' + ('' if usage[field + '_complete'] else ' (partial)')))
    cost = usage['known_cost_usd']
    lines.append('- Reported cost: ' + ('Not reported' if cost is None else f'${cost:,.4f}' + ('' if usage['cost_complete'] else ' (partial)')))
    for link in task.get('links', []):
        lines += ['', '### ' + _md(link.get('runtime') or 'Recorded agent'), '',
                  '- Receiver: ' + _md(link.get('receiver') or 'Legacy association; namespace not recorded'),
                  '- Producer: ' + _md(link.get('producer') or 'Legacy association; producer not recorded'),
                  '- Session: ' + _md(link.get('session_id') or 'Not reported'),
                  '- Agent: ' + _md(link.get('agent_id') or 'Legacy session scope; worker not recorded'),
                  '- Run: ' + _md(link.get('trace_id') or ('Recorded native lifecycle start: '+_iso(link['run_start_ns'])
                      if link.get('run_start_ns') else 'Unscoped; excluded from usage totals')),
                  '- Reported state: ' + _md(link.get('state') or 'Unknown'),
                  '- Evidence captured: ' + _iso(link.get('captured_ns'))]
        if link.get('parent_agent_id'):
            lines += ['- Recorded parent' + (' (retained from an earlier capture)' if link.get('parent_identity_retained') else '')
                      + ': ' + _md(link['parent_agent_id'])]
        if usage['aggregation_ambiguous'] and link.get('trace_id'):
            individual = link.get('usage', {})
            values = []
            for field, label in (('input_tokens','Input tokens'),('output_tokens','Output tokens'),
                                 ('cache_read_input_tokens','Cached input'),('cache_write_input_tokens','Cache writes')):
                if individual.get(field + '_reported') and individual.get(field) is not None:
                    values.append('- ' + label + ': ' + f'{individual[field]:,}')
            if individual.get('known_cost_usd') is not None:
                values.append('- Reported cost: ' + f"${individual['known_cost_usd']:,.4f}" +
                              ('' if individual.get('cost_complete') else ' (partial)'))
            lines += ['', 'Recorded usage for this scope only (excluded from an overlapping aggregate):',
                      *values] if values else ['', 'No usage was reported for this linked scope.']
        if link.get('task'):
            lines += ['- Recorded request: ' + _md(link['task'])]
        if link.get('resume_command'):
            lines += ['', 'Native resume command (check for an existing active session first):', '', _fence(link['resume_command'], 'sh')]
        if link.get('review_snapshot'):
            from .workflow_evidence import render_review
            lines += ['', 'Captured review · exact preview at link time', '', _fence(render_review(link['review_snapshot']))]
        for check in ([] if link.get('review_snapshot') else link.get('checks', [])):
            lines += ['', 'Recorded check: ' + _md(check['result']) + ' · ' + _iso(check['time_ns']),
                      _md(check['scope']) + '; freshness against the current workspace is unverified.',
                      '', _fence(check['command'], 'sh')]
    lines += ['', 'This brief contains saved evidence, not a live transcript. Review it before sharing or supplying it to another agent. '
              'Agenturm has not executed any command in this handoff.', '']
    markdown = '\n'.join(lines)
    if format == 'markdown':
        return markdown
    from markdown_it import MarkdownIt
    body = MarkdownIt('commonmark', {'html': False}).disable('image').render(markdown)
    style = '''
:root{color-scheme:light dark;--bg:#f6f8f5;--fg:#243128;--panel:#e8eee8;--link:#176b48}
@media(prefers-color-scheme:dark){:root{--bg:#151d18;--fg:#e2ece5;--panel:#243229;--link:#9bdcba}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.65 system-ui,sans-serif}
main{max-width:960px;margin:auto;padding:32px 24px 72px}h1{line-height:1.2;font-size:clamp(28px,5vw,40px)}
h2{margin-top:2em;line-height:1.3}h3{margin-top:1.7em}a{color:var(--link)}
pre{background:var(--panel);padding:16px;border-radius:6px;overflow:auto;white-space:pre-wrap;overflow-wrap:anywhere}
code{font-family:ui-monospace,monospace;font-size:.9em}p,li{overflow-wrap:anywhere}li{margin:.4em 0}
@media(max-width:600px){main{padding:24px 16px}}'''
    from html import escape
    return '<!doctype html>\n<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>' + escape(task['title']) + '</title><style>' + style + '</style></head><body><main>' + body + '</main></body></html>\n'


def export_handoff(task, path, *, include_decision_history=False):
    """Write the exact previewed task as a new Markdown/HTML pair (and optional JSON)."""
    target = Path(path).expanduser()
    if target.suffix.lower() not in {'.md', '.html', '.json'}:
        raise TaskError('Use a .md, .html, or .json destination; Markdown and HTML companions are always written.')
    formats = [('markdown', target if target.suffix.lower() == '.md' else target.with_suffix('.md')),
               ('html', target if target.suffix.lower() == '.html' else target.with_suffix('.html'))]
    if target.suffix.lower() == '.json':
        formats.append(('json', target))
    contents = [(destination, render_handoff(task, format,
        include_decision_history=include_decision_history).encode()) for format, destination in formats]
    from tempfile import NamedTemporaryFile
    staged = []
    written = []
    try:
        # Prepare complete private files before exposing any final pathname.
        for destination, content in contents:
            with NamedTemporaryFile(dir=destination.parent, prefix='.agenturm-handoff-', delete=False) as stream:
                temporary = Path(stream.name)
                staged.append((temporary, destination))
                os.fchmod(stream.fileno(), 0o600)
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
        for temporary, destination in staged:
            # link is atomic and refuses existing files, including symlinks.
            os.link(temporary, destination)
            written.append((temporary, destination))
    except BaseException:
        for temporary, destination in written:
            try:
                if os.path.samefile(temporary, destination):
                    destination.unlink()
            except OSError:
                pass
        raise
    finally:
        for temporary, _ in staged:
            temporary.unlink(missing_ok=True)
    return [str(destination) for destination, _ in contents]
