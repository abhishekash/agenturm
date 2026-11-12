"""Small, bounded view models for local monitoring and investigations."""
import math
import time
from rich.text import Text
from textual import on
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, RichLog, Static

FAILURES = {'error', 'failed', 'denied', 'blocked', 'policy_denied', 'incomplete'}


def session_history_feedback(snapshot):
    notes=[]
    for kind,error in snapshot.read_errors.items():
        stamp=snapshot.read_updated.get(kind,0)
        label={'connections':'SDK connections','activity':'Activity','traces':'Traces'}[kind]
        notes.append(label+(f' {max(0,int(time.time()-stamp))}s old' if stamp else ' unavailable'))
    if notes:
        notes.append('Counts cover available agents; missing evidence is unknown.')
    if not snapshot.sessions_error:return ' · '.join(notes)
    if not snapshot.sessions_updated:
        notes.append('Stored history unavailable · showing available agents')
    else:
        notes.append(f'Stored history {max(0,int(time.time()-snapshot.sessions_updated))}s old · totals may be stale')
    return ' · '.join(notes)


def connection_feedback(feed):
    progress=feed.connection
    elapsed=max(0,int(time.monotonic()-progress.started))
    snapshot=feed.snapshot
    if snapshot.read_errors and snapshot.updated and not snapshot.error:
        heading=('Retry requested · waiting for current request' if feed.retry_requested else
                 'Partial data · showing available agents')
        return 'Partial data',heading+'\n'+session_history_feedback(snapshot)+'\n'+\
            '\n'.join(f'{kind}: {error}' for kind,error in snapshot.read_errors.items()),True
    if snapshot.sessions_error and snapshot.updated and not snapshot.error:
        heading=('Retry requested · waiting for current request' if feed.retry_requested else
                 'History delayed · '+('stored history unavailable' if not snapshot.sessions_updated else
                     f'stored history {max(0,int(time.time()-snapshot.sessions_updated))}s old'))
        return 'History delayed',heading+'\n'+session_history_feedback(snapshot)+' · '+snapshot.sessions_error,True
    slow=progress.phase=='loading' and elapsed>=1
    visible=bool(snapshot.error or not snapshot.updated or slow)
    label=('Authentication' if progress.phase=='auth' else 'Loading delayed' if snapshot.error.startswith('Loading delayed:') else 'Reconnecting' if snapshot.error
           else 'Connecting' if not snapshot.updated else 'Updating' if slow else 'Live')
    if not visible:return label,'',False
    if feed.retry_requested:
        heading='Retry requested · waiting for current request'
    elif progress.phase=='waiting':
        remaining=max(0,math.ceil(progress.retry_at-time.monotonic()))
        heading=f'{label} · attempt {progress.attempt} · retry in {remaining}s'
    elif progress.phase=='auth':
        heading='Authentication required · check receiver credentials'
    else:
        heading=f'{label} · {progress.stage} · {elapsed}s'
    stale=(f'Showing saved data · {max(0,int(time.time()-snapshot.updated))}s old'
           if snapshot.updated else 'Waiting for the first snapshot')
    detail=stale+(' · '+snapshot.error if snapshot.error else '')
    return label,heading+'\n'+detail,True


def failed(event):
    return str(event.get('result') or event.get('status') or '').casefold() in FAILURES


def event_id(event):
    return event.get('id') or event.get('source_id')


def operation_key(event):
    # Matching a tool name alone would conflate different commands or sessions.
    identity = event.get('agent_id') or (event.get('service'), event.get('session_id'))
    return identity, event.get('domain'), event.get('action'), event.get('subject')


def failure_groups(events, context=()):
    """Group exact repeated operations within a loaded page, retaining evidence.

    A later successful identical operation is evidence, not proof of a retry or
    resolution. Never infer recovery across sessions or from generic success.
    """
    groups = {}
    successes = {}
    for event in context:
        if str(event.get('result') or event.get('status') or '').casefold() in {'ok', 'success'}:
            key=operation_key(event)
            if event.get('time_unix_nano',0)>successes.get(key,{}).get('time_unix_nano',0):
                successes[key]=event
    for event in sorted(events, key=lambda e: e.get('time_unix_nano', 0), reverse=True):
        if failed(event):
            event={key:value for key,value in event.items() if not key.startswith('_')}
            key = operation_key(event)
            # Missing identity/subject cannot establish the same operation.
            if not (event.get('agent_id') or event.get('session_id')) or not event.get('subject'):
                key = (key, event_id(event))
            groups.setdefault(key, []).append(event)
    result = []
    for occurrences in groups.values():
        latest = occurrences[0]
        success=successes.get(operation_key(latest))
        if not (latest.get('subject') and (latest.get('agent_id') or latest.get('session_id'))
                and success and success.get('time_unix_nano',0)>latest.get('time_unix_nano',0)):
            success=None
        result.append({**latest, '_failures': occurrences, '_later_success': success})
    return result


def changed_records(before, after, page):
    """Count distinct changed/new records in the loaded window, not polls."""
    def records(snapshot):
        if page == 'agents':
            values = {}
            for item in snapshot.sessions:
                key=item.get('agent_id') or item.get('session_id')
                values[key]=tuple(item.get(k) for k in ('last_unix_nano','latest_status','trace_count','errors'))
            for item in snapshot.local_status.get('sessions', []):
                key = item.get('agent_id') or item.get('session_id') or item.get('source_id')
                values[key] = tuple(item.get(k) for k in
                    ('last_record_ns', 'run_state', 'task', 'last_activity', 'active'))
            return values
        if page == 'activity':
            rows = snapshot.activity_results if snapshot.activity_results is not None else snapshot.activity
        elif page == 'host':
            rows=(snapshot.local_status.get('runtime') or {}).get('processes',[])
        else:
            rows = getattr(snapshot, {'traces': 'traces', 'mcp': 'mcp_results',
                                      'findings': 'finding_results', 'metrics': 'metric_points'}.get(page, 'agents'), []) or []
        return {event_id(row) or row.get('trace_id') or row.get('pid') or str(index): row for index, row in enumerate(rows)}
    old, new = records(before), records(after)
    return sum(key not in old or old[key] != value for key, value in new.items())


def usage_summary(totals):
    known = lambda key: totals.get(key) is not None and totals.get(key + '_reported') is not False
    tokens = (f"{int(totals['input_tokens']) + int(totals['output_tokens']):,}"
              if known('input_tokens') and known('output_tokens') else 'Not reported')
    cost = totals.get('known_cost_usd')
    unknown = totals.get('unknown_cost_runs', 0)
    priced = max(0, totals.get('runs', 0) - unknown)
    money = lambda value: 'Not reported' if value is None else f'${value:,.4f}'
    reported_cost = money(cost if priced or cost else None)
    if unknown and (priced or cost):
        reported_cost += '+'
    average = money(totals.get('average_task_cost_usd'))
    count = totals.get('costed_tasks', 0)
    lines = [f'Tokens {tokens} · all stored history',
             f'Cost {reported_cost} · Avg/task {average}'+(f' ({count} runs)' if count else '')]
    details = '\n'.join([
        'LOCAL USAGE · ALL STORED HISTORY', *lines, '',
        f"Input tokens: {int(totals['input_tokens']):,}" if known('input_tokens') else 'Input tokens: Not reported',
        f"Output tokens: {int(totals['output_tokens']):,}" if known('output_tokens') else 'Output tokens: Not reported',
        'Cached input and reasoning output are already included; they are not added twice.', '',
        f"Average denominator: {count} fully priced completed runs of {totals.get('completed_tasks', 0)} completed runs.",
        'For this average, one task = one recorded run (trace), including finished failures.',
        'Running tasks and runs with incomplete prices are excluded from the average.',
        f'{unknown} runs have missing or incomplete cost data. A + marks a partial cost subtotal.',
        'Log-only token usage is included in tokens; log-only sessions have no run cost average.',
        'These are reported USD costs, not a bill estimate or subscription charge.',
        'Totals cover retained database history, including reviews; session/search filters do not change them.',
        'Press Esc to return.',
    ])
    return '\n'.join(lines), details


def session_usage(sessions, item):
    """Require producer/session provenance even when canonical agent IDs match."""
    if not item:return {}
    def same_producer(row):
        producer=item.get('service')
        return not producer or producer in (row.get('services') or [row.get('service')])
    def same_session(row):
        return not item.get('session_id') or row.get('session_id')==item['session_id']
    if item.get('agent_id'):
        exact=next((row for row in sessions if row.get('agent_id')==item['agent_id']
                    and same_producer(row) and same_session(row)),None)
        if exact is not None:return exact
        if not item.get('agent_id_inferred'):return {}
    return next((row for row in sessions if item.get('session_id')
                 and row.get('session_id')==item['session_id']
                 and same_producer(row) and not row.get('agent_id')), {})


def task_run_usage(sessions, item, traces=()):
    session=session_usage(sessions,item)
    if item.get('run_start_ns') and not item.get('trace_id'):return {}
    selected=item.get('trace_id') or item.get('latest_trace_id')
    latest=session.get('latest_run') or {}
    def allocated(row):
        if not row.get('trace_id'):return {}
        count=row.get('agent_count')
        if type(count) is int and count==1 and row.get('agent_count_complete') is True:return row
        return {key:row[key] for key in ('trace_id','agent_id','agent_count','agent_count_complete') if key in row} | {
            'usage_allocation':'shared trace; per-agent usage unavailable' if type(count) is int and count>1 else
                'agent allocation unverified; per-agent usage unavailable'}
    if latest and (not selected or latest.get('trace_id')==selected):return allocated(latest)
    trace_id=selected or session.get('latest_trace_id')
    def exact(row):
        if not trace_id or row.get('trace_id')!=trace_id:return False
        if item.get('service') and item['service'] not in (row.get('services') or [row.get('service')]):return False
        if item.get('session_id') and row.get('session_id') and row['session_id']!=item['session_id']:return False
        if item.get('agent_id') and not item.get('agent_id_inferred') and row.get('agent_id')!=item['agent_id']:return False
        return True
    candidate=next((row for row in traces if exact(row)),{})
    return allocated(candidate) if candidate else {}


def task_usage_label(usage, *, compact=False):
    text,_=task_usage_summary(usage,scope='trace',compact=True)
    first=text.splitlines()[0]
    money,tokens=first.removeprefix('Task cost ').split(' · Tokens ')
    if compact:
        if money.startswith('$'):
            suffix='+' if money.endswith('+') else ''
            money=money.rstrip('+').rstrip('0').rstrip('.')+suffix
        tokens=tokens.replace('.0k','k').replace('.0m','m')
        return tokens.replace('Not reported','—')+' tok · '+money.replace('Not reported','—')
    return f'Tokens {tokens} · Cost {money}'.replace('Not reported','—')


def task_usage_summary(usage, title='', *, scope='session', identity='', compact=False):
    known=lambda key: usage.get(key) is not None and usage.get(key+'_reported') is not False
    partial=usage.get('usage_complete') is False
    token=lambda key: f"{int(usage[key]):,}"+('+' if partial else '') if known(key) else 'Not reported'
    tokens=(f"{int(usage['input_tokens'])+int(usage['output_tokens']):,}"+('+' if partial else '')
            if known('input_tokens') and known('output_tokens') else 'Not reported')
    runs=usage.get('trace_count',1 if scope=='trace' and usage else 0)
    missing=usage.get('unknown_cost_runs',int(usage.get('cost_complete') is False))
    cost=usage.get('known_cost_usd')
    priced=max(0,runs-missing)
    cost_known=cost is not None and (priced>0 or cost>0)
    complete=usage.get('cost_complete') is not False and not missing
    money=f'${cost:,.4f}'+('+' if not complete else '') if cost_known else 'Not reported'
    ratio=(f"{100*usage['cache_read_input_tokens']/usage['input_tokens']:.0f}%"
           if not partial and known('input_tokens') and known('cache_read_input_tokens')
           and usage['input_tokens']>0 and 0<=usage['cache_read_input_tokens']<=usage['input_tokens'] else 'Not reported')
    def short(value):
        if not compact:return f'{value:,}'
        return f'{value/1_000_000:.1f}m' if value>=1_000_000 else f'{value/1000:.1f}k' if value>=1000 else str(value)
    screen_token=lambda key: short(int(usage[key]))+('+' if partial else '') if known(key) else 'Not reported'
    screen_total=(short(int(usage['input_tokens'])+int(usage['output_tokens']))+('+' if partial else '')
                  if known('input_tokens') and known('output_tokens') else 'Not reported')
    cached_suffix=f' · {ratio} of input' if ratio!='Not reported' else ''
    lines=[f'Task cost {money} · Tokens {screen_total}',
           f"Input {screen_token('input_tokens')} · Output {screen_token('output_tokens')}",
           f"Cached input {screen_token('cache_read_input_tokens')}"+cached_suffix]
    if known('cache_write_input_tokens'):
        lines[-1]+=f" · Cache write {screen_token('cache_write_input_tokens')}"
    if usage.get('usage_allocation'):lines.append(usage['usage_allocation']+' · Trace view retains whole-trace totals.')
    details='\n'.join([
        'SELECTED TASK USAGE',title or 'No task selected','',
        f'Cost: {money}',f'Tokens used: {tokens}',
        f"Input: {token('input_tokens')}",f"Output: {token('output_tokens')}",
        f"Cached input: {token('cache_read_input_tokens')}"+cached_suffix,
        *([f"Cache write: {token('cache_write_input_tokens')}"] if known('cache_write_input_tokens') else []),
        f"Reasoning output: {token('reasoning_output_tokens')}",'',
        ('Scope: this recorded run (trace).' if scope=='trace' else
         'Scope: this agent session across its recorded runs; excludes other agents and sessions.'),
        ('Trace: ' if scope=='trace' else 'Session: ')+(identity or 'Not reported'),'',
        'Cache reads/writes are input breakdowns; reasoning is an output breakdown. Do not add them to total tokens.',
        'Cached percentage means cached input divided by reported input tokens, not a request hit rate.',
        'A + marks a partial reported subtotal. Missing telemetry is not zero.',
        'Cost is reported USD for this task, not an all-history total or a subscription bill estimate.',
        *([usage['usage_allocation']+' · Whole-trace totals remain available in Traces.'] if usage.get('usage_allocation') else []),
        'Press Esc to return.'])
    return '\n'.join(lines),details


class UsageScreen(ModalScreen):
    DEFAULT_CSS = '''
    UsageScreen { align: center middle; background: $background 70%; }
    #usage-dialog { width: 90%; height: 90%; border: round $primary; background: $surface; padding: 1 2; }
    #usage-log { height: 1fr; }
    #usage-close { width: 100%; }
    '''
    BINDINGS = [('escape', 'dismiss', 'Close')]

    def __init__(self, totals=None, *, details=None):
        super().__init__()
        self.details = details if details is not None else usage_summary(totals or {})[1]

    def compose(self):
        with Vertical(id='usage-dialog'):
            yield RichLog(id='usage-log', wrap=True, markup=False, min_width=1, auto_scroll=False)
            yield Button('Close · Esc', id='usage-close')

    def on_mount(self):
        log=self.query_one(RichLog)
        log.write(Text(self.details))
        log.focus()

    @on(Button.Pressed, '#usage-close')
    def close_dialog(self):
        self.dismiss()


class SortScreen(ModalScreen):
    DEFAULT_CSS = '''
    SortScreen { align: center middle; background: $background 70%; }
    #sort-dialog { width: 54; max-width: 95%; height: auto; border: round $primary; background: $surface; padding: 1 2; }
    #sort-title { height: 2; }
    #sort-dialog Button { width: 100%; height: 1; padding: 0; margin-bottom: 1; border: none; }
    #sort-hint { height: 1; }
    '''
    OPTIONS=(('working_new','Working first · newest updated'),
             ('working_old','Working first · oldest updated'),
             ('updated_new','Updated · newest first'),
             ('updated_old','Updated · oldest first'),
             ('status','Status · then newest updated'))
    BINDINGS=[('escape','dismiss','Cancel')]+[(str(i),f'choose("{key}")',label)
                                            for i,(key,label) in enumerate(OPTIONS,1)]

    def __init__(self,selected='working_new'):
        super().__init__()
        self.selected=selected

    def compose(self):
        with Vertical(id='sort-dialog'):
            yield Static('Sort agent sessions',id='sort-title')
            for i,(key,label) in enumerate(self.OPTIONS,1):
                yield Button(f'{i}  {label}'+(' ✓' if key==self.selected else ''),id='sort-'+key)
            yield Static('1–5 choose · Esc cancel',id='sort-hint')

    def on_mount(self):
        self.query_one('#sort-'+self.selected,Button).focus()

    def action_choose(self,key):
        self.dismiss(key)

    @on(Button.Pressed)
    def choose_button(self,event):
        self.dismiss(event.button.id.removeprefix('sort-'))
