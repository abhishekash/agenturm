"""Pure, exactly scoped review evidence. Observations never establish correctness."""
from __future__ import annotations
from collections import OrderedDict
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import json
import re
import time
import unicodedata
from urllib.parse import urlencode, urlsplit
from .session_text import local_command_only, message_text, request_text
from .command_evidence import check_kind as check_kind, analyze_command, modifying_action

PAGE_SIZE = 200
MAX_EVENTS = 1000
MAX_BYTES = 2 * 1024 * 1024
CACHE_SELECTIONS = 4
CACHE_BYTES = 8 * 1024 * 1024
LATER_REFERENCE_BYTES = 128 * 1024
LATER_REFERENCES_PER_CHECK = 8


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
    turn_id: str = ""

    @property
    def namespace(self): return self.receiver

    @property
    def run_anchor(self):
        if self.turn_id:return 'turn:'+self.turn_id
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
        if self.turn_id and event.get('turn_id')!=self.turn_id:return False
        if not self.turn_id and self.run_id and event.get('trace_id') != self.run_id: return False
        if not self.turn_id and self.run_start_ns and (event.get('time_unix_nano') or 0) < self.run_start_ns: return False
        if not self.run_id and self.run_end_ns and (event.get('time_unix_nano') or 0)>self.run_end_ns:return False
        return True

    def path(self, cursor=None):
        if self.kind == 'unavailable': raise ValueError('No unambiguous execution identity')
        return '/api/activity?' + urlencode(dict(limit=PAGE_SIZE, service=self.producer,
            agent_id=self.agent_id, session_id=self.session_id, trace_id=self.run_id if not self.turn_id else '',turn_id=self.turn_id,
            subtree=0, review=1, cursor=cursor or ''))


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
    turn=str(item.get('turn_id') or (item.get('_review_usage') or {}).get('turn_id') or '')
    return ExecutionScope(str(receiver), producer, agent, session, trace, None if turn else start, 'turn' if turn else kind, fallback,None if turn else end,turn)


def reference(event):
    return {key: event[key] for key in ('id', 'source_id', 'trace_id', 'span_id', 'service', 'agent_id', 'session_id', 'time_unix_nano') if event.get(key) is not None}


def event_key(event):
    return str(event.get('trace_id') or 'log') + ':' + str(event.get('source_id') or event.get('id') or hashlib.sha256(json.dumps(event, sort_keys=True, default=str).encode()).hexdigest())



def check_observation(event):
    if event.get('domain') != 'shell': return None
    command = safe_text(event.get('subject'))
    detail = event.get('detail')
    if isinstance(detail, str):
        try: detail = json.loads(detail)
        except (ValueError, TypeError): detail = {}
    detail = detail if isinstance(detail, dict) else {}
    command=safe_text(detail.get('command') or command)
    analysis=analyze_command(command)
    if not analysis:return None
    kind=analysis['kind']
    exit_code = detail.get('exit_code', event.get('exit_code'))
    # Older native content redaction serialized primitive detail values as text.
    if isinstance(exit_code,str) and re.fullmatch(r'-?\d{1,9}',exit_code):exit_code=int(exit_code)
    if isinstance(exit_code, bool) or not isinstance(exit_code, int): exit_code = None
    result = str(event.get('result') or '').lower()
    status = 'passed' if exit_code == 0 else 'failed' if exit_code is not None else 'passed' if result in ('success', 'ok') else 'failed' if result in ('failed', 'error', 'failure') else 'unknown'
    if analysis['compound'] and status!='passed':status='unknown'
    return dict(source=reference(event), command=command, normalized_command=analysis['command'], compound=analysis['compound'], kind=kind, result=status, exit_code=exit_code,
                result_basis='compound shell result; individual failure unavailable' if analysis['compound'] and status=='unknown' else 'recorded exit code' if exit_code is not None else 'producer result label' if status!='unknown' else 'no terminal result',
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
        context=response.get('review_context',[])
        if not isinstance(context,list) or len(context)>2 or any(not isinstance(row,dict) for row in context):raise ValueError('Malformed review context')
        for event in context+rows[:PAGE_SIZE]:
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
        rows = sorted(self.events.values(), key=lambda e: (e.get('time_unix_nano') or 0, event_key(e))) if item.get('capture_content') is not False else []
        edits = [e for e in rows if modifying_action(e)]
        checks = [check for event in rows if (check := check_observation(event))]
        reference_budget=LATER_REFERENCE_BYTES
        edit_refs=[(e.get('time_unix_nano') or 0,reference(e)) for e in edits]
        ref_sizes={id(ref):len(json.dumps(ref,ensure_ascii=False).encode()) for _,ref in edit_refs}
        for check in checks:
            stamp = check['completed_ns']
            later = [ref for time_ns,ref in edit_refs if stamp and time_ns > stamp]
            linked=[]
            for ref in later[:LATER_REFERENCES_PER_CHECK]:
                size=ref_sizes[id(ref)]
                if size>reference_budget:break
                linked.append(ref);reference_budget-=size
            check['later_edits'] = linked
            check['later_edit_count']=len(later)
            check['later_edits_clipped']=len(linked)<len(later)
            check['freshness'] = 'later recorded edits' if later else 'freshness unknown'
        errors = [e for e in rows if str(e.get('result') or '').lower() in ('error', 'failed', 'failure', 'blocked', 'denied')]
        detailed_spans={(e.get('trace_id'),e.get('span_id')) for e in errors if e.get('span_id') and e.get('domain')=='shell' and e.get('subject') not in {'Shell command','CommandExecution'}}
        errors=[e for e in errors if not (e.get('subject') in {'Shell command','CommandExecution'} and (e.get('trace_id'),e.get('span_id')) in detailed_spans)]
        def error_fact(event):
            detail=event.get('detail') or ''
            if isinstance(detail,str):
                try:detail=json.loads(detail)
                except (ValueError,TypeError):pass
            code=event.get('exit_code')
            if isinstance(detail,dict):
                code=detail.get('exit_code',code)
                output=detail.get('stderr') or detail.get('output') or detail.get('error') or detail.get('message') or ''
            else:output=detail
            return dict(source=reference(event),subject=safe_text(event.get('subject')),exit_code=code,
                detail=safe_text(output)[:1500],detail_clipped=len(str(output))>1500)
        context = [e for e in rows if e.get('action') in ('compaction', 'compact', 'truncation', 'context_truncated')]
        def latest_message(action):
            if item.get('capture_content') is False:return None
            for event in reversed(rows):
                if event.get('domain')!='message' or event.get('action')!=action:continue
                text=message_text(event.get('detail')) or message_text(event.get('subject'))
                if not text or (not event.get('detail') and text.casefold() in {'assistant message','user message','message'}) or (action=='user' and local_command_only(text)):continue
                parsed=request_text(text) if action=='user' else None
                body=parsed.body if parsed else text
                if not body:continue
                return dict(text=body[:2000],source=reference(event),clipped=len(body)>2000,
                            context_omitted=list(parsed.context) if parsed else [])
            return None
        stamps = [e['time_unix_nano'] for e in rows if isinstance(e.get('time_unix_nano'), int)]
        reasons = list(dict.fromkeys(self.limited))
        if not self.loaded: reasons.append('Evidence not loaded')
        if not self.receiver_confirmed:reasons.append('Receiver response identity unavailable; namespace confirmation is limited')
        if self.cursor: reasons.append('Earlier evidence available; load explicitly')
        if self.error: reasons.append('Refresh failed; retained evidence may be stale')
        if self.scope.run_start_ns and not self.scope.run_id and not self.scope.run_end_ns:reasons.append('Native lifecycle end is unavailable; exact run interval cannot be isolated')
        if self.scope.kind not in {'run','turn'}: reasons.append('Session/agent evidence; exact run identity unavailable')
        if item.get('capture_content') is False: reasons.append('Metadata-only capture')
        elif self.loaded and not rows: reasons.append('No scoped activity loaded; collection or indexing may still be pending, or capture may be unavailable. Retry explicitly.')
        if any(event.get('detail_clipped') for event in rows):reasons.append('Some source text is clipped to the displayed excerpt; consult the original event for full output')
        if item.get('_review_metadata_note'): reasons.append(item['_review_metadata_note'])
        if not stamps or len(stamps) != len(rows): reasons.append('Some timestamps unavailable')
        window = dict(earliest_ns=min(stamps) if stamps else None, latest_ns=max(stamps) if stamps else None,
            count=len(rows), next_cursor=self.cursor, scope_revision=self.revision, capture_revision=self.capture_revision,
            coverage='partial' if reasons else 'loaded captured interval', reasons=reasons,
            note='Captured records do not establish complete runtime capture or the current worktree state.')
        candidate=item.get('_review_usage') or {}
        owner=candidate.get('review_owner') or {}
        exact_usage=bool(((self.scope.turn_id and candidate.get('turn_id')==self.scope.turn_id and candidate.get('turn_usage_complete') is True) or (not self.scope.turn_id and self.scope.run_id and candidate.get('trace_id')==self.scope.run_id)) and
            type(candidate.get('agent_count')) is int and candidate['agent_count']==1 and candidate.get('agent_count_complete') is True and
            owner.get('producer')==self.scope.producer and owner.get('agent_id')==self.scope.agent_id and
            (not self.scope.session_id or owner.get('session_id')==self.scope.session_id))
        usage=dict(scope=('recorded user turn' if self.scope.turn_id else 'exact run') if exact_usage else 'unavailable',trace_id=self.scope.run_id or None,
            step_trace_ids=list(candidate.get('step_trace_ids') or ()),agent_count=candidate.get('agent_count'),allocation=candidate.get('usage_allocation') or ('single recorded agent' if exact_usage else 'agent allocation unverified'),
            values={key:candidate[key] for key in ('input_tokens','output_tokens','cache_read_input_tokens',
                'cache_write_input_tokens','cost_usd','known_cost_usd','input_tokens_reported','output_tokens_reported',
                'cost_reported','cost_coverage','cost_complete','usage_complete','unknown_cost_runs',
                'cache_read_input_tokens_reported','cache_write_input_tokens_reported') if exact_usage and key in candidate},
            note='Recorded run usage only; session lifetime, descendant totals and provider billing are separate.')
        return dict(format='agenturm-review-v1', title='Selected execution (metadata-only)' if item.get('capture_content') is False else safe_text(item.get('task') or 'Selected execution'),
            scope=asdict(self.scope), captured_at=self.captured_at, reported_outcome=item.get('state') or 'Unknown',
            edits=[dict(source=reference(e), path=safe_text(e.get('subject')), action=modifying_action(e),source_action=e.get('action'),
                result=safe_text(e.get('result') or 'not reported'),effect='recorded success' if e.get('result') in {'success','ok','completed'} else 'attempt; effect unknown') for e in edits],
            checks=checks, unclassified_commands=[dict(source=reference(e),command=safe_text(e.get('subject')),result=safe_text(e.get('result'))) for e in rows if e.get('domain')=='shell' and not check_observation(e)], usage=usage, errors=[error_fact(e) for e in errors],
            context_events=[dict(source=reference(e), subject=safe_text(e.get('subject'))) for e in context], window=window,
            request=latest_message('user'),agent_update=latest_message('assistant'))


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
    lines = [projection['title'], '', 'Scope: '+scope['kind']+' · '+('turn '+scope['turn_id'] if scope.get('turn_id') else 'trace '+scope['run_id'] if scope.get('run_id') else 'native lifecycle'), 'Agent reported: ' + projection['reported_outcome'],
        'Evidence: '+window['coverage']+' · '+str(window['count'])+' captured records',
        'Checks: '+' · '.join(str(counts[state])+' '+state for state in counts),
        'Review next step: '+review_guidance(projection), '', 'LATEST CAPTURED REQUEST FOR THIS RUN']
    request=projection.get('request')
    lines += [request['text']+('… (excerpt clipped)' if request.get('clipped') else '')] if request else ['Not available in loaded evidence; the task label is not a reconstructed request.']
    if request and request.get('context_omitted'):lines.append('Captured context separated: '+', '.join(request['context_omitted']))
    if projection.get('agent_update'):
        lines+=['','AGENT-REPORTED UPDATE',projection['agent_update']['text']]
    lines += ['', 'RECORDED CHECKS']
    for check in projection['checks']:
        basis=check.get('result_basis','recorded producer result')
        if check.get('exit_code') is not None:basis+=' '+str(check['exit_code'])
        freshness=check['freshness']
        if check.get('later_edit_count'):freshness=str(check['later_edit_count'])+' '+freshness
        lines += [check['result'].upper() + ' · ' + check['command'], '  ' + basis+' · '+freshness]
    if not projection['checks']: lines.append('No recognized validation commands in the loaded evidence. This does not establish that no validation ran.')
    unknown=projection.get('unclassified_commands') or []
    if unknown:
        lines += ['', str(len(unknown))+' UNCLASSIFIED SHELL RECORDS · inspect original command results']
        lines += [e['command']+' · '+e['result'] for e in unknown[:12]]
    lines += ['', 'RECORDED MODIFYING OPERATIONS'] + [e['action'] + ' · ' + e['path']+' · '+e.get('result','not reported')+(' · '+e['effect'] if e.get('effect')=='attempt; effect unknown' else '') for e in projection['edits']]
    if not projection['edits']: lines.append('No file operations captured in this interval.')
    lines += ['', 'RECORDED ERRORS · later recovery is not inferred from completion']
    for error in projection['errors']:
        lines.append(error['subject']+(' · recorded exit '+str(error['exit_code']) if error.get('exit_code') is not None else ''))
        if error.get('detail'):lines.append('  '+error['detail']+('… (excerpt clipped)' if error.get('detail_clipped') else ''))
    if not projection['errors']:lines.append('None in the loaded interval.')
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


def review_guidance(projection):
    """A next inspection step, never a correctness or merge-readiness verdict."""
    checks=projection.get('checks') or []
    if projection.get('reported_outcome')=='Waiting':return 'Read the request and respond in the original agent terminal.'
    if any(check['result']=='failed' for check in checks):return 'Inspect the failed validation commands and their source records.'
    if any(check.get('later_edit_count',len(check.get('later_edits',()))) for check in checks):return 'Inspect the later edits and rerun the affected checks in your terminal.'
    if any(check['result']=='unknown' for check in checks):return 'Find the missing command results before relying on these checks.'
    if not checks:return 'No validation was captured here; choose and run the checks this change needs.'
    if projection['window']['coverage']=='partial':return 'Inspect the coverage gaps before drawing conclusions from the captured checks.'
    return 'Review the changes and decide which checks the current worktree still needs.'
