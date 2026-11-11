"""Pure, exactly scoped review evidence. Observations never establish correctness."""
from __future__ import annotations
from collections import OrderedDict
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import json
import re
import shlex
import time
import unicodedata
from urllib.parse import urlencode, urlsplit

PAGE_SIZE = 200
MAX_EVENTS = 1000
MAX_BYTES = 2 * 1024 * 1024
CACHE_SELECTIONS = 4
CACHE_BYTES = 8 * 1024 * 1024


def safe_text(value):
    value = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', str(value or ''))
    return ''.join(c for c in value if c in '\n\t' or unicodedata.category(c) not in {'Cc','Cf'})


@dataclass(frozen=True)
class ExecutionScope:
    receiver: str
    producer: str
    agent_id: str = ''
    session_id: str = ''
    run_id: str = ''
    run_start_ns: int | None = None
    kind: str = 'unavailable'
    endpoint_fallback: bool = False
    run_end_ns: int | None = None

    @property
    def namespace(self): return self.receiver

    @property
    def run_anchor(self):
        return 'trace:' + self.run_id if self.run_id else 'start:' + str(self.run_start_ns) if self.run_start_ns else ''

    @property
    def scope_key(self):
        values = (self.receiver, self.producer, self.agent_id, self.session_id, self.run_anchor, self.kind)
        return hashlib.sha256(json.dumps(values).encode()).hexdigest()

    @property
    def persistent(self):
        return bool(self.receiver and self.producer and self.run_anchor and self.kind != 'unavailable')

    def matches(self, event):
        if self.kind == 'unavailable' or event.get('service') != self.producer: return False
        if self.agent_id:
            if event.get('agent_id') != self.agent_id: return False
        elif not self.session_id or event.get('session_id') != self.session_id or event.get('agent_id'):
            return False
        if self.session_id and event.get('session_id') != self.session_id: return False
        if self.run_id and event.get('trace_id') != self.run_id: return False
        if self.run_start_ns and (event.get('time_unix_nano') or 0) < self.run_start_ns: return False
        if not self.run_id and self.run_end_ns and (event.get('time_unix_nano') or 0)>self.run_end_ns:return False
        return True

    def path(self, cursor=None):
        if self.kind == 'unavailable': raise ValueError('No unambiguous execution identity')
        return '/api/activity?' + urlencode(dict(limit=PAGE_SIZE, service=self.producer,
            agent_id=self.agent_id, session_id=self.session_id, trace_id=self.run_id,
            subtree=0, cursor=cursor or ''))


def scope_for(item, snapshot=None, endpoint=''):
    receiver = getattr(snapshot, 'receiver_id', '') or (getattr(snapshot, 'local_status', {}) or {}).get('receiver_id', '')
    fallback = not bool(receiver)
    if fallback:
        parsed = urlsplit(endpoint)
        # Credentials, query strings, fragments never enter durable identities.
        host = parsed.hostname or ''
        receiver = 'endpoint:' + parsed.scheme + '://' + host + (':' + str(parsed.port) if parsed.port else '') + parsed.path.rstrip('/') if host else ''
    producer = str(item.get('service') or '')
    agent = '' if item.get('agent_id_inferred') else str(item.get('agent_id') or '')
    session = str(item.get('session_id') or '')
    trace = str(item.get('trace_id') or (item.get('latest_trace_id') if not item.get('run_start_ns') else '') or '')
    start = item.get('run_start_ns')
    # Only explicit lifecycle anchors qualify, never display/update timestamps.
    start = int(start) if isinstance(start, int) and start > 0 else None
    kind = ('run' if trace or start else 'agent' if agent else 'legacy_session') if producer and (agent or session) else 'unavailable'
    end=item.get('run_end_ns') or item.get('outcome_time_ns') or item.get('task_outcome_ns')
    end=end if isinstance(end,int) and start and end>=start else None
    return ExecutionScope(str(receiver), producer, agent, session, trace, start, kind, fallback,end)


def reference(event):
    return {key: event[key] for key in ('id', 'source_id', 'trace_id', 'span_id', 'service', 'agent_id', 'session_id', 'time_unix_nano') if event.get(key) is not None}


def event_key(event):
    return str(event.get('trace_id') or 'log') + ':' + str(event.get('source_id') or event.get('id') or hashlib.sha256(json.dumps(event, sort_keys=True, default=str).encode()).hexdigest())


def check_kind(command):
    """A bounded allowlist; prose or arbitrary commands containing 'test' do not qualify."""
    if any(token in command for token in ('&&','||',';','|','`','$(','\n')): return None
    try: words = shlex.split(command[:8000])
    except ValueError: return None
    while words and re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*=.*', words[0]): words.pop(0)
    if not words: return None
    if words[0] in ('uv', 'poetry') and words[1:2] == ['run']:
        words = words[2:]
        while words and words[0] in ('--no-sync','--offline','--frozen','--locked','--no-project','--active'):words.pop(0)
    if not words: return None
    executable = words[0].rsplit('/', 1)[-1]
    if executable in ('pytest', 'ruff', 'mypy', 'pyright', 'tsc', 'eslint', 'flake8', 'pylint', 'jest', 'vitest'): return executable
    if re.fullmatch(r'python(?:\d+(?:\.\d+)*)?', executable) and words[1:3] in (['-m', 'unittest'], ['-m', 'pytest']): return words[2]
    if executable in ('npm', 'pnpm', 'yarn', 'bun') and len(words) > 1:
        action = words[2] if words[1] == 'run' and len(words) > 2 else words[1]
        if action in ('test', 'check', 'lint', 'typecheck', 'type-check'): return action
    if executable in ('cargo', 'go') and words[1:2] in (['test'], ['check'], ['clippy'], ['vet']): return executable + ' ' + words[1]
    if executable == 'make' and words[1:2] in (['test'], ['check'], ['lint']): return 'make ' + words[1]
    return None


def check_observation(event):
    if event.get('domain') != 'shell': return None
    command = safe_text(event.get('subject'))
    kind = check_kind(command)
    if not kind: return None
    detail = event.get('detail')
    if isinstance(detail, str):
        try: detail = json.loads(detail)
        except (ValueError, TypeError): detail = {}
    detail = detail if isinstance(detail, dict) else {}
    exit_code = detail.get('exit_code', event.get('exit_code'))
    if isinstance(exit_code, bool) or not isinstance(exit_code, int): exit_code = None
    result = str(event.get('result') or '').lower()
    status = 'passed' if exit_code == 0 else 'failed' if exit_code is not None else 'passed' if result in ('success', 'ok') else 'failed' if result in ('failed', 'error', 'failure') else 'unknown'
    return dict(source=reference(event), command=command, kind=kind, result=status, exit_code=exit_code,
                completed_ns=event.get('end_unix_nano') or event.get('time_unix_nano') if status != 'unknown' else None)


@dataclass
class ReviewHistory:
    scope: ExecutionScope
    revision: object = None
    capture_revision: object = None
    events: dict = field(default_factory=dict)
    cursor: str | None = None
    loaded: bool = False
    busy: bool = False
    error: str = ''
    limited: list = field(default_factory=list)
    payload_bytes: int = 0
    captured_at: float = 0
    generation: int = 0
    failed_cursor: str | None = None
    receiver_confirmed: bool = False

    @property
    def key(self): return (self.scope.scope_key, str(self.revision), str(self.capture_revision))

    def accept(self, response):
        if not isinstance(response, dict) or not isinstance(response.get('activity'), list): raise ValueError('Malformed activity response')
        response_receiver=response.get('receiver_id')
        if response_receiver and not self.scope.endpoint_fallback and response_receiver!=self.scope.receiver:
            raise ValueError('Receiver identity changed; reopen the review before loading more evidence')
        cursor=response.get('next_cursor')
        if cursor is not None and not isinstance(cursor,str):raise ValueError('Malformed activity cursor')
        rows = response['activity']
        if any(not isinstance(row, dict) for row in rows): raise ValueError('Malformed activity record')
        for event in rows[:PAGE_SIZE]:
            if not self.scope.matches(event): continue
            key = event_key(event)
            if key in self.events: continue
            size = len(json.dumps(event, ensure_ascii=False, default=str).encode())
            if len(self.events) >= MAX_EVENTS:
                self.limited.append('1,000-event review limit'); break
            if self.payload_bytes + size > MAX_BYTES:
                self.limited.append('2 MiB review payload limit'); break
            # Freeze captured payload so later producer mutation cannot alter preview/export.
            self.events[key] = json.loads(json.dumps(event, default=str))
            self.payload_bytes += size
        if len(rows) > PAGE_SIZE: self.limited.append('Oversized receiver page')
        if len(self.events)>=MAX_EVENTS and cursor:self.limited.append('1,000-event review limit')
        self.cursor = None if self.limited else cursor
        if self.cursor is not None and not isinstance(self.cursor, str): raise ValueError('Malformed activity cursor')
        self.loaded = True; self.error = ''; self.failed_cursor = None; self.captured_at = time.time()
        self.receiver_confirmed=bool(response_receiver and response_receiver==self.scope.receiver and not self.scope.endpoint_fallback)

    def projection(self, item):
        rows = sorted(self.events.values(), key=lambda e: (e.get('time_unix_nano') or 0, event_key(e)))
        edits = [e for e in rows if e.get('domain') == 'file' and e.get('action') in ('write', 'edit', 'delete', 'create', 'patch', 'rename')]
        checks = [check for event in rows if (check := check_observation(event))]
        for check in checks:
            stamp = check['completed_ns']
            later = [reference(e) for e in edits if stamp and (e.get('time_unix_nano') or 0) > stamp]
            check['later_edits'] = later
            check['freshness'] = 'later recorded edits' if later else 'freshness unknown'
        errors = [e for e in rows if str(e.get('result') or '').lower() in ('error', 'failed', 'failure', 'blocked', 'denied')]
        context = [e for e in rows if e.get('action') in ('compaction', 'compact', 'truncation', 'context_truncated')]
        stamps = [e['time_unix_nano'] for e in rows if isinstance(e.get('time_unix_nano'), int)]
        reasons = list(dict.fromkeys(self.limited))
        if not self.loaded: reasons.append('Evidence not loaded')
        if not self.receiver_confirmed:reasons.append('Receiver response identity unavailable; namespace confirmation is limited')
        if self.cursor: reasons.append('Earlier evidence available; load explicitly')
        if self.error: reasons.append('Refresh failed; retained evidence may be stale')
        if self.scope.run_start_ns and not self.scope.run_id and not self.scope.run_end_ns:reasons.append('Native lifecycle end is unavailable; exact run interval cannot be isolated')
        if self.scope.kind != 'run': reasons.append('Session/agent evidence; exact run identity unavailable')
        if item.get('capture_content') is False: reasons.append('Metadata-only capture')
        if not stamps or len(stamps) != len(rows): reasons.append('Some timestamps unavailable')
        window = dict(earliest_ns=min(stamps) if stamps else None, latest_ns=max(stamps) if stamps else None,
            count=len(rows), next_cursor=self.cursor, scope_revision=self.revision, capture_revision=self.capture_revision,
            coverage='partial' if reasons else 'loaded captured interval', reasons=reasons,
            note='Captured records do not establish complete runtime capture or the current worktree state.')
        candidate=item.get('_review_usage') or {}
        exact_usage=bool(self.scope.run_id and candidate.get('trace_id')==self.scope.run_id and
            type(candidate.get('agent_count')) is int and candidate['agent_count']==1 and candidate.get('agent_count_complete') is True and
            (not candidate.get('agent_id') or candidate.get('agent_id')==self.scope.agent_id))
        usage=dict(scope='exact run' if exact_usage else 'unavailable',trace_id=self.scope.run_id or None,
            agent_count=candidate.get('agent_count'),allocation=candidate.get('usage_allocation') or ('single recorded agent' if exact_usage else 'agent allocation unverified'),
            values={key:candidate[key] for key in ('input_tokens','output_tokens','cache_read_input_tokens',
                'cache_write_input_tokens','cost_usd','known_cost_usd','input_tokens_reported','output_tokens_reported',
                'cost_reported','cost_coverage','cost_complete','usage_complete','unknown_cost_runs',
                'cache_read_input_tokens_reported','cache_write_input_tokens_reported') if exact_usage and key in candidate},
            note='Recorded run usage only; session lifetime, descendant totals and provider billing are separate.')
        return dict(format='agenturm-review-v1', title=safe_text(item.get('task') or 'Selected execution'),
            scope=asdict(self.scope), captured_at=self.captured_at, reported_outcome=item.get('state') or 'Unknown',
            edits=[dict(source=reference(e), path=safe_text(e.get('subject')), action=e.get('action')) for e in edits],
            checks=checks, usage=usage, errors=[dict(source=reference(e), subject=safe_text(e.get('subject'))) for e in errors],
            context_events=[dict(source=reference(e), subject=safe_text(e.get('subject'))) for e in context], window=window)


class ReviewCache:
    def __init__(self): self.entries = OrderedDict()

    def get(self, scope, revision=None, capture_revision=None):
        key = (scope.scope_key, str(revision), str(capture_revision))
        if key not in self.entries: self.entries[key] = ReviewHistory(scope, revision, capture_revision)
        self.entries.move_to_end(key)
        self.trim(keep=key)
        return self.entries[key]

    def trim(self, keep=None):
        while len(self.entries) > CACHE_SELECTIONS or sum(e.payload_bytes for e in self.entries.values()) > CACHE_BYTES:
            key = next((key for key in self.entries if key != keep), None)
            if key is None: break
            del self.entries[key]


def render_review(projection):
    scope, window = projection['scope'], projection['window']
    counts={state:sum(check['result']==state for check in projection['checks']) for state in ('passed','failed','unknown')}
    lines = [projection['title'], '', 'Agent reported: ' + projection['reported_outcome'],
        'Evidence: '+window['coverage']+' · '+str(window['count'])+' captured records',
        'Checks: '+' · '.join(str(counts[state])+' '+state for state in counts), '', 'RECORDED CHECKS']
    for check in projection['checks']:
        lines += [check['result'].upper() + ' · ' + check['command'], '  ' + check['freshness']]
    if not projection['checks']: lines.append('No recognized checks in the loaded evidence. Assistant claims are not check results.')
    lines += ['', 'RECORDED EDITS'] + [e['action'] + ' · ' + e['path'] for e in projection['edits']]
    if not projection['edits']: lines.append('No file operations captured in this interval.')
    lines += ['', 'RECORDED ERRORS · later recovery is not inferred from completion']
    lines += [e['subject'] for e in projection['errors']] or ['None in the loaded interval.']
    usage=projection.get('usage') or {};values=usage.get('values') or {}
    lines += ['', 'RECORDED USAGE · '+usage.get('scope','unavailable')]
    if usage.get('scope')=='unavailable':lines.append(usage.get('allocation') or 'Per-agent allocation unavailable.')
    labels={'input_tokens':'Input tokens','output_tokens':'Output tokens','cache_read_input_tokens':'Cached input tokens','cache_write_input_tokens':'Cache write tokens','known_cost_usd':'Recorded USD','cost_usd':'Recorded USD'}
    for key,label in labels.items():
        if key in values and values.get(key+'_reported') is not False:
            value=values[key]
            if key in ('known_cost_usd','cost_usd'):
                shown='Not reported' if value is None or value==0 and values.get('cost_complete') is False else str(value)+(' (partial)' if values.get('cost_complete') is False else '')
            else:shown=str(value)+(' (partial)' if values.get('usage_complete') is False else '')
            lines.append(label+': '+shown)
    if not values:lines.append('Not reported for this exact run.')
    if projection['context_events']:
        lines += ['', 'CONTEXT EVENTS'] + [e['subject'] for e in projection['context_events']]
    def stamp(value):
        try:return datetime.fromtimestamp(value/1e9,timezone.utc).strftime('%Y-%m-%d %H:%M:%S UTC') if value else 'unknown'
        except (ValueError,OverflowError,OSError):return 'unknown'
    lines += ['', 'EVIDENCE SCOPE', 'Scope: '+scope['kind']+' · '+scope['producer'],
        'Receiver: '+scope['receiver']+(' (endpoint fallback)' if scope['endpoint_fallback'] else ''),
        'Run: '+(scope['run_id'] or ('native start '+stamp(scope['run_start_ns']) if scope['run_start_ns'] else 'not recorded')),
        'Window: '+stamp(window['earliest_ns'])+' – '+stamp(window['latest_ns']),
        *window['reasons'], window['note'],usage.get('note','')]
    return safe_text('\n'.join(lines))
