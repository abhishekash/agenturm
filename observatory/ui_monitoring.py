"""Small, bounded view models for local monitoring and investigations."""
import math
import time
from rich.text import Text
from textual import on, events
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, OptionList, RichLog, Static
from textual.widgets.option_list import Option
from .review_facts import session_usage, task_run_usage
from .terminal import clean
from .themes import PALETTES, CURRENT
from .operation_facts import correlation_facts,operation_key as recorded_operation_key,operation_stamp

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
    return recorded_operation_key(event,facts=correlation_facts(event))


def failure_groups(events, context=()):
    """Group exact repeated operations within a loaded page, retaining evidence.

    A later successful identical operation is evidence, not proof of a retry or
    resolution. Never infer recovery across sessions or from generic success.
    """
    groups = {}
    successes = {}
    for event in context:
        facts=correlation_facts(event)
        key=recorded_operation_key(event,facts=facts);stamp=operation_stamp(event)
        if key and stamp and facts['result']=='passed' and not facts['result_conflict']:
            if stamp>(operation_stamp(successes.get(key,{})) or 0):
                successes[key]=event
    seen=set()
    for ordinal,event in enumerate(sorted(events,key=lambda e:operation_stamp(e) or 0,reverse=True)):
        facts=correlation_facts(event)
        if (failed(event) or facts['result']=='failed') and not facts['expected_nonzero']:
            event={key:value for key,value in event.items() if not key.startswith('_')}
            operation=recorded_operation_key(event,facts=facts)
            identity=event_id(event)
            occurrence=tuple(event.get(key) if isinstance(event.get(key),str) else ''
                for key in ('receiver_id','service','agent_id','session_id','trace_id','turn_id'))+(identity,)
            if isinstance(identity,str):
                if occurrence in seen:continue
                seen.add(occurrence)
            # Missing/redacted/clipped inputs or error signatures remain
            # separate occurrences, even when their display labels match.
            key=(operation,facts['error_fingerprint']) if operation and facts['error_fingerprint'] else ('occurrence',ordinal)
            groups.setdefault(key, []).append(event)
    result = []
    for occurrences in groups.values():
        latest = occurrences[0]
        success=successes.get(operation_key(latest))
        if not (success and operation_stamp(latest) and operation_stamp(success)>operation_stamp(latest)):
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


def task_usage_summary(usage, title='', *, scope='session', identity='', compact=False,
                       rates=None, model='', allocation_proven=False, explanations=False,
                       context=None, scope_owner=None):
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
    if rates is not None:
        from .evidence_explanations import configured_estimate
        estimate=configured_estimate(usage,model=model,rates=rates,allocation_proven=allocation_proven)
        if estimate.get('available'):
            label=f"Configured estimate ${estimate['estimated_usd']:,.4f}"
            lines.append(label+' · explicit user rates')
            details+='\n\n'+label+'\n'+estimate['kind']+'\n'+estimate['assumption']+'\n'+estimate['note']
        else:
            details+='\n\nConfigured estimate unavailable: '+str(estimate.get('reason') or 'allocation or rate not proven')
    if explanations:
        from .evidence_explanations import usage_explanation
        explanation=usage_explanation(usage,scope=scope,context=context,scope_owner=scope_owner)
        cache=explanation['cache'];occupancy=explanation['context']
        fraction=cache['cached_input_fraction']
        cache_label=f'{fraction:.0%} of reported input' if fraction is not None else 'Unavailable from compatible complete token breakdowns'
        context_label=(f"{occupancy['used_tokens']:,} / {occupancy['capacity_tokens']:,} tokens ({occupancy['fraction']:.0%})"
                       if occupancy['available'] else occupancy['reason'])
        details+='\n\nRECORDED USAGE INTERPRETATION\nCached input: '+cache_label+'\n'+cache['note']+'\nContext occupancy: '+context_label+'\n'+explanation['retry_cost']['note']
    return '\n'.join(lines),details


class UsageScreen(ModalScreen):
    DEFAULT_CSS = '''
    UsageScreen { align: center middle; background: $background 70%; }
    #usage-dialog { width: 90%; height: 90%; border: round $primary; background: $surface; padding: 1 2; }
    #usage-log { height: 1fr; }
    #usage-close { width: 100%; }
    '''
    BINDINGS = [('escape', 'dismiss', 'Close'),('ctrl+c','dismiss','Close'),
                ('c','withhold_action',''),('x','withhold_action','')]

    def __init__(self, totals=None, *, details=None, valid=None):
        super().__init__()
        self.details = details if details is not None else usage_summary(totals or {})[1]
        self.valid=valid;self.invalidated=False

    def compose(self):
        with Vertical(id='usage-dialog'):
            yield RichLog(id='usage-log', wrap=True, markup=False, min_width=1, auto_scroll=False)
            yield Button('Close · Esc', id='usage-close')

    def on_mount(self):
        log=self.query_one(RichLog)
        log.write(Text(self.details))
        log.focus()
        if self.valid is not None:
            self.check_policy();self.set_interval(.25,self.check_policy)

    def check_policy(self):
        try:allowed=not self.invalidated and (self.valid is None or self.valid())
        except Exception:allowed=False
        if allowed:return
        self.invalidated=True;self.details='Source policy or receiver changed. Close and reopen usage from current evidence.'
        log=self.query_one(RichLog);log.clear();log.write(Text(self.details))

    def action_withhold_action(self):
        # Usage has no copy/export workflow; never invoke a background view's
        # selected-record actions from this modal.
        self.check_policy()

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


class SessionScopeSearch(Input):
    BINDINGS=[('ctrl+a','select_all','Select all')]


class SessionScopeScreen(ModalScreen):
    """A readable, keyboard-first choice of existing session scope."""
    DEFAULT_CSS='''
    SessionScopeScreen { align: center middle; background: $background 70%; }
    #session-scope-dialog { width: 104; max-width: 96%; height: 80%; max-height: 32; min-height: 14;
        border: round $primary; background: $surface; padding: 1 2; }
    #session-scope-title, #session-scope-count, #session-scope-hint { height: 1; }
    #session-scope-title { text-style: bold; }
    #session-scope-search { height: 1; border: none; padding: 0; background: $agent-panel; }
    #session-scope-options { height: 1fr; min-height: 4; border: none; padding: 0; background: $surface; }
    #session-scope-options > .option-list--option-highlighted { background: $agent-cursor; color: $agent-on-cursor; }
    #session-scope-description { height: 3; color: $agent-muted; overflow-y: auto; }
    #session-scope-small { display: none; height: 2; }
    #session-scope-count, #session-scope-hint { color: $agent-muted; }
    '''
    BINDINGS=[('escape','cancel','Cancel'),('/','focus_search','Search')]

    def __init__(self,entries,selected='all'):
        super().__init__()
        # Freeze the chooser while reading; resolve the chosen identity against
        # the current roster only after returning to the monitor.
        self.entries=sorted((dict(entry) for entry in entries),
                            key=lambda entry:(entry['id']!='all',-int(entry.get('stamp') or 0),entry['id']))
        self.selected=selected
        self.filtered=[]

    def compose(self):
        with Vertical(id='session-scope-dialog'):
            yield Static('Filter agent sessions',id='session-scope-title')
            yield Static('Terminal too small · resize to 60×18 · Esc Cancel',id='session-scope-small')
            yield SessionScopeSearch(placeholder='Search task, project, runtime or session ID…',id='session-scope-search')
            yield Static('',id='session-scope-count')
            yield OptionList(id='session-scope-options',markup=False)
            yield Static('',id='session-scope-description',markup=False)
            yield Static('↑↓ Browse · Enter Apply · Esc Cancel · / Search',id='session-scope-hint')

    def on_mount(self):
        self.app.theme_changed_signal.subscribe(self,self.theme_changed)
        self.fit_dialog()
        self.filter_entries('')
        self.query_one(Input).focus()

    def on_unmount(self):self.app.theme_changed_signal.unsubscribe(self)

    def fit_dialog(self):
        small=self.size.width<60 or self.size.height<18
        self.query_one('#session-scope-small').display=small
        for selector in ('#session-scope-search','#session-scope-count','#session-scope-options',
                         '#session-scope-description','#session-scope-hint'):
            self.query_one(selector).display=not small
        dialog=self.query_one('#session-scope-dialog')
        dialog.styles.min_height=0 if small else 14
        dialog.styles.height=min(self.size.height,32,max(14,12+2*len(self.entries)))
        self.query_one('#session-scope-description').styles.height=2 if self.size.width<90 else 3
        self.query_one('#session-scope-hint',Static).update(
            '↑↓ Browse · Enter Apply · Esc Cancel'+(' · / Search' if self.size.width>=90 else ''))

    def refresh_labels(self):
        options=self.query_one(OptionList)
        index=options.highlighted
        identity=self.filtered[index]['id'] if index is not None and index<len(self.filtered) else self.selected
        self.filter_entries(self.query_one(Input).value,identity)
        options.scroll_to_highlight()

    def on_resize(self):
        self.fit_dialog()
        self.call_after_refresh(self.refresh_labels)

    def theme_changed(self,theme):
        options=self.query_one(OptionList)
        index=options.highlighted
        identity=self.filtered[index]['id'] if index is not None and index<len(self.filtered) else self.selected
        self.filter_entries(self.query_one(Input).value,identity)

    def filter_entries(self,query,highlight=None):
        query=query.casefold().strip()
        self.filtered=[entry for entry in self.entries if not query or query in
            ' '.join(str(value or '') for value in entry.values()).casefold()]
        options=self.query_one(OptionList)
        colors=PALETTES.get(self.app.theme,CURRENT)
        labels=[]
        # OptionList measures renderables before drawing; clip each physical line
        # explicitly so long prompts cannot inflate the height of an option.
        width=max(8,options.content_size.width-4)
        for entry in self.filtered:
            marker='Current · ' if entry['id']==self.selected else ''
            title=Text(marker+' '.join(clean(entry['task']).split()),style='bold')
            title.truncate(width,overflow='ellipsis')
            metadata=entry['description']
            if entry.get('project') and entry.get('runtime'):
                identity=entry.get('identity','')
                runtime=Text(entry['runtime']);runtime.truncate(14,overflow='ellipsis')
                project=Text(entry['project'])
                project.truncate(max(4,width-runtime.cell_len-Text(identity).cell_len-8),overflow='ellipsis')
                metadata=f'{project.plain} · {runtime.plain} · {identity}'
            if entry.get('status') and self.size.width>=90:
                metadata+=' · '+entry['status']+' · '+entry.get('updated','Unknown update')
            description=Text('  '+' '.join(clean(metadata).split()),style=colors['muted'])
            description.truncate(width,overflow='ellipsis')
            text=Text(no_wrap=True,overflow='ellipsis')
            text.append_text(title);text.append('\n');text.append_text(description)
            labels.append(Option(text,id=entry['id']))
        options.clear_options().add_options(labels)
        options.highlighted=next((i for i,entry in enumerate(self.filtered) if entry['id']==(highlight or self.selected)),
                                 0 if self.filtered else None)
        count=sum(entry['id']!='all' for entry in self.filtered)
        self.query_one('#session-scope-count',Static).update(
            f'{count} '+('session' if count==1 else 'sessions')+' · Most recently updated'
            if self.filtered else 'No matches · clear search')
        self.show_description()

    def show_description(self):
        index=self.query_one(OptionList).highlighted
        entry=self.filtered[index] if index is not None and index<len(self.filtered) else None
        if entry:
            lines=(entry.get('detail') or entry['task']).splitlines()
            if entry.get('status'):
                lines.append(entry['status']+' · '+entry.get('updated','Unknown update'))
            preview=Text(no_wrap=True,overflow='ellipsis')
            width=max(8,self.query_one('#session-scope-description').content_size.width)
            for index,line in enumerate(lines):
                part=Text(('Preview: ' if index==0 else '')+' '.join(clean(line).split()))
                part.truncate(width,overflow='ellipsis')
                if index:preview.append('\n')
                preview.append_text(part)
        else:preview=Text('No matching sessions. Clear search to browse again.')
        widget=self.query_one('#session-scope-description',Static)
        widget.update(preview)
        widget.tooltip=entry.get('detail') if entry else None

    @on(Input.Changed,'#session-scope-search')
    def search_changed(self,event):self.filter_entries(event.value)

    @on(OptionList.OptionHighlighted,'#session-scope-options')
    def highlight_changed(self,event):self.show_description()

    @on(OptionList.OptionSelected,'#session-scope-options')
    def choose_option(self,event):self.dismiss(event.option_id)

    @on(Input.Submitted,'#session-scope-search')
    def choose_search_result(self):
        index=self.query_one(OptionList).highlighted
        if index is not None and index<len(self.filtered):self.dismiss(self.filtered[index]['id'])

    def on_key(self,event:events.Key):
        if self.focused is self.query_one(Input) and event.key in {'up','down'}:
            options=self.query_one(OptionList)
            if self.filtered:
                index=options.highlighted if options.highlighted is not None else 0
                options.highlighted=max(0,min(len(self.filtered)-1,index+(1 if event.key=='down' else -1)))
                options.scroll_to_highlight()
            event.stop();event.prevent_default()

    def action_focus_search(self):self.query_one(Input).focus()
    def action_cancel(self):self.dismiss(None)
