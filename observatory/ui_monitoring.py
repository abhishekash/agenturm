"""Small, bounded view models for local monitoring and investigations."""
from rich.text import Text
from textual import on
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, RichLog

FAILURES = {'error', 'failed', 'denied', 'blocked', 'policy_denied', 'incomplete'}


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


class UsageScreen(ModalScreen):
    DEFAULT_CSS = '''
    UsageScreen { align: center middle; background: $background 70%; }
    #usage-dialog { width: 90%; height: 90%; border: round $primary; background: $surface; padding: 1 2; }
    #usage-log { height: 1fr; }
    #usage-close { width: 100%; }
    '''
    BINDINGS = [('escape', 'dismiss', 'Close')]

    def __init__(self, totals):
        super().__init__()
        self.details = usage_summary(totals)[1]

    def compose(self):
        with Vertical(id='usage-dialog'):
            yield RichLog(id='usage-log', wrap=True, markup=False)
            yield Button('Close · Esc', id='usage-close')

    def on_mount(self):
        self.query_one(RichLog).write(Text(self.details))

    @on(Button.Pressed, '#usage-close')
    def close_dialog(self):
        self.dismiss()
