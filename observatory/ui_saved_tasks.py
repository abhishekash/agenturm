"""Saved local tasks, deliberate workspace capture, and reviewed handoffs."""
from __future__ import annotations

from copy import deepcopy
from dataclasses import asdict
from datetime import datetime
from io import StringIO
import re
import time
from urllib.parse import unquote, urlsplit

from rich.markdown import Markdown
from rich.cells import cell_len
from rich.console import Console
from rich.text import Text
from rich.theme import Theme
from textual import on
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.geometry import Size
from textual.screen import ModalScreen
from textual.scroll_view import ScrollView
from textual.strip import Strip
from textual.widgets import Button, DataTable, Input, RichLog, Select, Static, TextArea

from .saved_tasks import BRIEF_FIELDS, TaskStore, export_handoff, render_handoff, review_scope_issue,continuity_view
from .ui_monitoring import task_run_usage
from .ui_workflow import events_for, resume_command


def plain(value):
    return re.sub(r'[\x00-\x08\x0b-\x1f\x7f-\x9f]', '', str(value or ''))


def one_line(value, limit=500):
    return ' '.join(plain(value).split())[:limit]


def local_workspace(value):
    value = str(value or '')
    if value.startswith('file:'):
        parsed = urlsplit(value)
        return unquote(parsed.path) if parsed.netloc in {'', 'localhost'} else ''
    return value


def current_capture_context(owner, context):
    """Recheck source consent without touching independently authored task fields."""
    if not context:
        return context
    from .review_facts import review_capture_policy
    from .workflow_evidence import ExecutionScope
    result = deepcopy(context)
    scope = ExecutionScope(str(result.get('receiver') or ''), str(result.get('producer') or ''),
        str(result.get('agent_id') or ''), str(result.get('session_id') or ''))
    policy = review_capture_policy(result, owner.feed.snapshot, scope)
    receiver_changed=bool(getattr(owner.feed.snapshot,'receiver_id','') and scope.receiver!=owner.feed.snapshot.receiver_id)
    if policy.get('capture_content') is False or receiver_changed:
        result.update(task='Metadata-only execution', checks=[], capture_content=False, local_workspace='')
        result.pop('review_snapshot', None);result.pop('continuity_draft',None)
    return result


def capture_selected_run(owner, *, item_override=None, review_snapshot=None):
    """Freeze only the chosen identity and exact-run evidence already loaded."""
    item = deepcopy(item_override) if item_override is not None else next((item for item in owner.workflow_items()
                 if item['id'] == owner.selected_source), None)
    if not item:
        return None
    data = owner.feed.snapshot
    from .review_facts import review_capture_policy
    from .workflow_evidence import ExecutionScope, scope_for
    policy_scope = ExecutionScope(**review_snapshot['scope']) if review_snapshot is not None else scope_for(item,data,getattr(owner.feed.api,'endpoint',''))
    item.update(review_capture_policy(item,data,policy_scope))
    from .ui_workflow_controller import prepared_item
    selected = prepared_item(item, data)
    usage = deepcopy(selected['_workflow_usage'])
    if review_snapshot is not None:
        usage = deepcopy((review_snapshot.get('usage') or {}).get('values') or {})
    # Trace summaries use an empty cost sum as zero even when no prices exist.
    # Keep that distinct from a complete, explicitly reported zero-cost run.
    if usage.get('known_cost_usd') == 0 and usage.get('cost_complete') is False:
        usage['known_cost_usd'] = None
    trace = str(review_snapshot['scope'].get('run_id') or '') if review_snapshot is not None else str(item.get('trace_id') or usage.get('trace_id') or item.get('latest_trace_id') or '')
    trace = trace.lower() if re.fullmatch(r'[0-9a-fA-F]{32}', trace) else ''
    from .workflow_evidence import ExecutionScope, scope_for
    scope = scope_for({**selected, 'latest_trace_id': trace}, data, getattr(owner.feed.api, 'endpoint', ''))
    members = deepcopy((review_snapshot.get('usage') or {}).get('step_trace_ids') or []) if review_snapshot is not None else deepcopy(usage.get('step_trace_ids') or [])
    if review_snapshot is not None:
        captured_scope = ExecutionScope(**review_snapshot['scope'])
        if (captured_scope.agent_id != scope.agent_id or captured_scope.session_id != scope.session_id
                or captured_scope.producer != scope.producer
                or not captured_scope.turn_id and item.get('run_start_ns') and captured_scope.run_start_ns != item['run_start_ns']
                or item.get('turn_id') and captured_scope.turn_id != item['turn_id']
                or (item.get('trace_id') or item.get('latest_trace_id')) and captured_scope.run_id
                    and (item.get('trace_id') or item.get('latest_trace_id')) not in
                        ({captured_scope.run_id, *members} if captured_scope.turn_id else {captured_scope.run_id})):
            raise ValueError('Frozen review does not belong to this selected agent.')
        scope = captured_scope
    checks = []
    frozen_review = None
    if (trace or scope.turn_id or scope.run_start_ns and getattr(scope, 'run_end_ns', None)) and item.get('capture_content') is not False:
        from .workflow_evidence import check_observation
        for event in events_for(selected, data.activity):
            exact = scope.matches(event) if scope.turn_id else event.get('trace_id') == trace if trace else scope.matches(event)
            check = check_observation(event) if exact else None
            if check:
                checks.append({'command': plain(check['command'])[:2000],
                    'result': check['result'], 'time_ns': check.get('completed_ns') or 0, 'trace_id': event.get('trace_id') or ''})
        last = {'item_id':item['id'], 'projection':review_snapshot} if review_snapshot is not None else getattr(owner, 'last_review', None) or {}
        projection = last.get('projection') or {}
        if (last.get('item_id') == item['id'] and projection.get('scope') == asdict(scope)
                and item.get('capture_content') is not False):
            frozen_review = deepcopy(projection)
            usage = deepcopy((projection.get('usage') or {}).get('values') or {})
            members = deepcopy((projection.get('usage') or {}).get('step_trace_ids') or [])
            checks = [{'command': check['command'], 'result': check['result'],
                       'time_ns': check.get('completed_ns') or 0,
                       'trace_id': (check.get('source') or {}).get('trace_id') or (trace if not scope.turn_id else '')}
                      for check in projection.get('checks', [])][-32:]
    endpoint = urlsplit(getattr(owner.feed.api, 'endpoint', '')).hostname
    workspace = local_workspace(item.get('working_directory'))
    result = {'session_id': str(item.get('session_id') or ''),
            'receiver': scope.receiver, 'producer': scope.producer,
            'run_start_ns': scope.run_start_ns,
            'run_end_ns': getattr(scope, 'run_end_ns', None),
            'agent_id': scope.agent_id,
            'parent_agent_id': str(item.get('parent_agent_id') or ''),
            'runtime': one_line(item.get('runtime'), 120), 'trace_id': '' if scope.turn_id else trace,
            'task': 'Metadata-only execution' if item.get('capture_content') is False else one_line(item.get('task')), 'state': one_line(item.get('state'), 120),
            'working_directory': workspace,
            'local_workspace': ((frozen_review or {}).get('workspace_observation') or {}).get('binding',{}).get('workspace',''),
            'branch': one_line(item.get('branch'), 512),
            'captured_ns': time.time_ns(), 'usage': deepcopy(usage) if trace or scope.turn_id else {},
            'capture_content': item.get('capture_content'),
            'checks': checks[-8:], 'resume_command': resume_command({**item, 'working_directory': workspace})}
    if frozen_review is not None:
        result['review_snapshot'] = frozen_review

    if scope.turn_id:
        result.update(turn_id=scope.turn_id, step_trace_ids=members)
    return current_capture_context(owner,result)


def task_source_policy(owner):
    """Freeze policy for one render; callers recheck before publishing/writing."""
    from .review_facts import review_capture_policy
    from .workflow_evidence import ExecutionScope
    from types import SimpleNamespace
    live=owner.feed.snapshot
    snapshot=SimpleNamespace(receiver_id=getattr(live,'receiver_id',''),local_status=deepcopy(getattr(live,'local_status',{}) or {}))
    def state(scope):
        if not scope.get('receiver') or snapshot.receiver_id!=scope['receiver']:return dict(receiver_changed=True,receiver_id=snapshot.receiver_id)
        try:execution=ExecutionScope(**scope)
        except TypeError:return dict(receiver_changed=True)
        return dict(receiver_id=snapshot.receiver_id,**review_capture_policy({},snapshot,execution))
    def allowed(scope):
        policy=state(scope)
        return not policy.get('receiver_changed') and policy.get('capture_content') is not False
    allowed.state=state
    return allowed


def task_policy_signature(owner,task):
    allowed=task_source_policy(owner)
    scopes=[row.get('scope') or {} for row in task.get('continuity_drafts',[])]
    scopes.extend((row.get('review_snapshot') or {}).get('scope') or {} for row in task.get('links',[])
        if (row.get('review_snapshot') or {}).get('workspace_observation'))
    import json
    return tuple((str(scope),json.dumps(allowed.state(scope),sort_keys=True)) for scope in scopes)


def stamp(value):
    return datetime.fromtimestamp(value / 1e9).strftime('%b %d %H:%M') if value else 'Not captured'


def task_overview(task,source_policy=None):
    task=continuity_view(task,source_policy)
    usage = task.get('usage_summary') or {}
    snapshot = task.get('workspace_snapshot') or {}
    tokens = [usage.get(field) for field in ('input_tokens', 'output_tokens')]
    total = sum(value for value in tokens if value is not None)
    token_label = f'{total:,}' if any(value is not None for value in tokens) else 'Not reported'
    if any(value is not None for value in tokens) and not usage.get('usage_complete'):
        token_label += ' (partial)'
    cost = usage.get('known_cost_usd')
    cost_label = 'Not reported' if cost is None else f'${cost:,.4f}' + ('' if usage.get('cost_complete') else ' (partial)')
    lines = ['NOTES', plain(task.get('notes')) or 'Add the intent, decisions, and next step with Edit.', '',
             'WORKSPACE', plain(task.get('workspace')) or 'No workspace selected',
             'Snapshot: ' + stamp(snapshot.get('captured_ns'))]
    if task.get('continuity_drafts'):
        latest=task['continuity_drafts'][-1]
        lines+=['','LATEST APPROVED CONTINUITY DRAFT',plain(latest['text'])]
    brief_lines = []
    for field, value in task.get('brief', {}).items():
        if value:
            brief_lines += [field.replace('_', ' ').title(), plain(value), '']
    decisions = task.get('decisions', [])
    replaced = {item.get('supersedes_id') for item in decisions}
    current = [item for item in decisions if item['id'] not in replaced]
    if brief_lines or current:
        insertion = lines.index('NOTES')
        summary = [f'{len(current)} current decisions · {len(decisions)} retained versions · open Decisions for rationale and history', ''] if decisions else []
        lines[insertion:insertion] = ['STRUCTURED BRIEF', *brief_lines, *summary]
    if snapshot:
        lines += ['Branch: ' + plain(snapshot.get('branch') or 'Not available'),
                  'Base: ' + str(snapshot.get('base_commit') or 'Not available')[:16]]
        if snapshot.get('error'):
            lines.append('Capture issue: ' + plain(snapshot['error']))
        files = snapshot.get('files') or []
        lines += [f'{len(files)} captured changed paths' + ('' if snapshot.get('complete') else ' · partial capture')]
        lines += ['  ' + plain(entry['status'] + ' ' + entry['path']) for entry in files[:8]]
        if len(files) > 8:
            lines.append(f'  + {len(files) - 8} more in Handoff')
        lines += ['Workspace changes may include pre-existing or concurrent work.',
                  'Capture again to update this saved view.']
    else:
        lines.append('Capture saves the current Git changes for your next handoff.')
    has_usage = (bool(task.get('links')) or usage.get('linked_runs') or usage.get('unscoped_links')
                 or any(value is not None for value in tokens) or cost is not None)
    if has_usage:
        lines += ['', 'USAGE DETAILS', f'Tokens {token_label} · Reported cost {cost_label}',
                  f"{usage.get('linked_runs', 0)} linked runs · {usage.get('unscoped_links', 0)} session-only links",
                  'Only explicitly linked runs are counted. Link again to refresh a run.']
        if usage.get('aggregation_ambiguous'):
            lines.append('Aggregate unavailable: linked scopes share a trace or a recorded parent relationship and may overlap. Handoff retains each scope separately.')
        cached = usage.get('cache_read_input_tokens')
        if cached is not None:
            lines.append('Cached input: ' + f'{cached:,}' + ('' if usage.get('cache_read_input_tokens_complete') else ' (partial)'))
    lines += ['', 'CONTINUE', 'Handoff previews your notes, saved changes, and recorded checks.',
              'Recorded checks do not verify the current workspace.']
    return '\n'.join(lines)


class TaskJobs:
    """Keep SQLite, Git, and document rendering off the input thread."""
    busy = False

    def status(self, message):
        self.query_one('#task-status', Static).update(plain(message))

    def job(self, operation, callback, message='Loading saved tasks…', *, needs_store=True):
        if self.busy:
            return
        self.busy = True
        self.status(message)
        for button in self.query('.task-actions Button'):
            button.disabled = True
        def perform():
            result = error = None
            store = None
            try:
                if needs_store:
                    store = TaskStore(self.owner.task_store_path)
                result = operation(store)
            except Exception as exc:
                error = str(exc) or type(exc).__name__
            finally:
                if store is not None:
                    store.close()
            try:
                self.app.call_from_thread(self.finish_job, result, error, callback)
            except RuntimeError:
                pass  # The application may have exited while a requested save finished.
        self.run_worker(perform, thread=True, exit_on_error=False)

    def finish_job(self, result, error, callback):
        self.busy = False
        if not self.is_mounted:
            return
        for button in self.query('.task-actions Button'):
            button.disabled = False
        if error:
            self.status('Could not finish: ' + error)
            return
        self.status('')
        callback(result)


class TaskTable(DataTable):
    """Fit a task title around fixed metadata columns, preserving selection."""
    def fit(self, labels, fixed):
        widths = (max(12, self.size.width - 2 - sum(fixed) - 6), *fixed)
        if getattr(self, 'fitted_widths', None) == widths:
            return
        self.fitted_widths = widths
        rows = [(key, self.get_row(key)) for key in self.rows]
        selected, scroll = self.cursor_row, self.scroll_offset
        self.clear(columns=True)
        for label, width in zip(labels, widths):
            self.add_column(label, width=width)
        for key, values in rows:
            self.add_row(*values, key=key.value)
        if rows:
            self.move_cursor(row=min(selected, len(rows) - 1), scroll=False)
        self.scroll_to(scroll.x, scroll.y, animate=False, force=True)


class TaskScreen(TaskJobs, ModalScreen):
    DEFAULT_CSS = '''
    TaskScreen { background: $agent-canvas; }
    #task-too-small { display: none; height: auto; padding: 1; }
    TaskScreen.too-small #saved-root { display: none; }
    TaskScreen.too-small #task-too-small { display: block; }
    #saved-root { width: 100%; height: 100%; padding: 0 1; }
    #task-heading { height: 1; text-style: bold; color: $agent-text; }
    #task-heading { text-wrap: nowrap; text-overflow: ellipsis; }
    #task-status { height: 2; color: $agent-secondary; }
    #task-hints { height: 1; color: $agent-muted; text-wrap: nowrap; }
    #task-search { height: 3; }
    #task-table { height: 1fr; }
    #task-body { height: 1fr; min-height: 1; }
    .task-actions { height: 3; }
    .task-actions Button { width: 1fr; min-width: 8; height: 3; padding: 0; }
    #handoff-path { height: 3; }
    '''
    BINDINGS = [Binding('escape', 'back', 'Back')]

    def __init__(self, owner):
        super().__init__()
        self.owner = owner

    def action_back(self):
        self.dismiss()

    def compose_floor(self):
        return Static('Saved tasks needs 60 columns × 20 rows. Enlarge this terminal or press Esc to return.',
                      id='task-too-small', markup=False)

    def on_resize(self):
        self.update_floor()

    def update_floor(self):
        self.set_class(self.size.width < 60 or self.size.height < 20, 'too-small')


class SavedTasksScreen(TaskScreen):
    BINDINGS = [Binding('n', 'new', 'New task'), Binding('a', 'archived', 'Include archived')]

    def __init__(self, owner, create=False, decision_excerpt=None, capture_context=None):
        super().__init__(owner)
        self.rows = []
        self.total = 0
        self.include_archived = False
        self.create_on_mount = create
        self.preferred = None
        self.decision_excerpt = deepcopy(decision_excerpt)
        self.capture_context = deepcopy(capture_context)

    def compose(self):
        yield self.compose_floor()
        with Vertical(id='saved-root'):
            yield Static('Saved tasks', id='task-heading')
            yield Input(placeholder='Find a saved task…', id='task-search')
            yield TaskTable(id='task-table', cursor_type='row', zebra_stripes=False)
            yield Static('Loading saved tasks…', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('New · n', id='task-new')
                yield Button('Open', id='task-open')
                yield Button('Archived · a', id='task-archived')
            yield Static('↑/↓ Move  Enter Open  n New  Esc Monitor', id='task-hints', markup=False)

    def on_mount(self):
        table = self.query_one(DataTable)
        table.add_column('Task', width=30)
        table.add_column('Links', width=5)
        table.add_column('Saved', width=12)
        self.call_after_refresh(self.resize_columns)
        table.focus()
        self.reload()

    def on_resize(self):
        self.update_floor()
        if self.is_mounted:
            self.call_after_refresh(self.resize_columns)

    def resize_columns(self):
        self.query_one(TaskTable).fit(('Task', 'Links', 'Saved'), (5, 12))

    def reload(self, preferred=None):
        self.preferred = preferred or self.selected_id()
        self.job(lambda store: (store.list(include_archived=self.include_archived),
                                store.count(include_archived=self.include_archived)), self.loaded)

    def loaded(self, result):
        rows, self.total = result
        self.rows = rows
        self.render_rows()
        if self.create_on_mount:
            self.create_on_mount = False
            self.action_new()

    def selected_id(self):
        table = self.query_one('#task-table', DataTable)
        keys = list(table.rows)
        return keys[table.cursor_row].value if 0 <= table.cursor_row < len(keys) else None

    def render_rows(self):
        query = self.query_one('#task-search', Input).value.casefold()
        matches = [row for row in self.rows if query in (row['title'] + ' ' + row['workspace']).casefold()]
        selected = self.preferred or self.selected_id()
        table = self.query_one(DataTable)
        table.clear()
        for row in matches:
            title = row['title'] + (' · archived' if row['archived'] else '')
            table.add_row(Text(title), str(row.get('link_count', 0)), stamp(row['updated_ns']), key=row['id'])
        identities = [row['id'] for row in matches]
        if selected in identities:
            table.move_cursor(row=identities.index(selected), animate=False)
        self.preferred = None
        self.query_one('#task-heading', Static).update(f'Saved tasks · {len(matches)}' + (' · including archived' if self.include_archived else ''))
        if not matches:
            self.status('No matching saved tasks.' if self.rows else 'No saved tasks yet. New saves the selected session or starts a blank task.')
        else:
            self.status('Saved locally. Open a task to capture changes or prepare a handoff.')
        if self.total > len(self.rows):
            self.status(f'Showing the newest {len(self.rows):,} of {self.total:,} tasks. Search covers these loaded tasks.')
        if not self.query_one('#task-search', Input).has_focus:
            table.focus()

    @on(Input.Changed, '#task-search')
    def filter_rows(self):
        if not self.busy:
            self.render_rows()

    @on(DataTable.RowSelected, '#task-table')
    def row_open(self):
        self.open_selected()

    def open_selected(self):
        identity = self.selected_id()
        if identity:
            def opened(task):
                if self.decision_excerpt:
                    self.app.push_screen(TaskDecisionEditor(self.owner, task,
                        source=self.decision_excerpt['source'], excerpt=self.decision_excerpt['text']),
                        lambda _: self.reload(identity))
                else:
                    self.app.push_screen(TaskDetailScreen(self.owner, task), lambda _: self.reload(identity))
            self.job(lambda store: store.get(identity), opened)

    def action_new(self):
        if not self.busy:
            def created(task):
                if not task:
                    return
                self.reload(task['id'])
                if self.decision_excerpt:
                    self.app.push_screen(TaskDecisionEditor(self.owner, task,
                        source=self.decision_excerpt['source'], excerpt=self.decision_excerpt['text']))
            context = deepcopy(self.capture_context) if self.capture_context is not None else capture_selected_run(self.owner)
            self.app.push_screen(TaskEditor(self.owner, context=context), created)

    def action_archived(self):
        if not self.busy:
            self.include_archived = not self.include_archived
            self.reload()

    @on(Button.Pressed)
    def buttons(self, event):
        if event.button.id == 'task-new': self.action_new()
        elif event.button.id == 'task-open': self.open_selected()
        elif event.button.id == 'task-archived': self.action_archived()


class TaskDetailScreen(TaskScreen):
    BINDINGS = [Binding('e', 'edit', 'Edit'), Binding('l', 'link', 'Link selected run'),
                Binding('ctrl+r', 'reload', 'Reload saved task'),
                Binding('r', 'capture', 'Capture workspace'), Binding('h', 'handoff', 'Handoff'),
                Binding('p', 'brief', 'Brief'), Binding('d', 'decisions', 'Decisions'),Binding('g','draft','Continuity draft')]
    DEFAULT_CSS = '''
    #task-title-row { height: 1; }
    #task-title-row #task-heading { width: 1fr; }
    #task-title-row Button { min-width: 8; width: auto; color: $agent-secondary; }
    '''

    def __init__(self, owner, task):
        super().__init__(owner)
        self.record = task

    def compose(self):
        yield self.compose_floor()
        with Vertical(id='saved-root'):
            with Horizontal(id='task-title-row'):
                yield Static('', id='task-heading', markup=False)
                yield Button('Draft · g',id='task-draft')
                yield Button('Brief', id='task-brief')
                yield Button('Decisions', id='task-decisions')
            yield RichLog(id='task-body', wrap=True, markup=False, min_width=1, auto_scroll=False)
            yield Static('', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                for name, label in (('edit', 'Edit'), ('runs', 'Runs'), ('link', 'Link'),
                                    ('capture', 'Capture'), ('handoff', 'Handoff'), ('archive', 'Archive')):
                    yield Button(label, id='task-' + name)
            yield Static('e Edit  r Capture  h Handoff  Esc Tasks', id='task-hints', markup=False)

    def on_mount(self):
        # Inline styles take precedence over the app's three-row Button rule.
        for button in self.query('#task-title-row Button'):
            button.set_styles('height: 1; padding: 0 1; border: none;')
        self.call_after_refresh(self.render_task)
        self.query_one('#task-body').focus()
        self.source_signature=task_policy_signature(self.owner,self.record)
        self.set_interval(.5,self.refresh_source_policy)

    def refresh_source_policy(self):
        signature=task_policy_signature(self.owner,self.record)
        if signature!=self.source_signature:
            self.source_signature=signature;self.render_task()

    def on_resize(self):
        self.update_floor()
        if self.is_mounted:
            self.call_after_refresh(self.render_task)

    def render_task(self):
        self.query_one('#task-heading', Static).update(plain(self.record['title']))
        body = self.query_one(RichLog)
        body.clear()
        title = plain(self.record['title'])
        content = (title + '\n\n' if cell_len(title) > body.scrollable_content_region.width else '') + task_overview(self.record,task_source_policy(self.owner))
        text = Text(content)
        text.highlight_regex(r'(?m)^(STRUCTURED BRIEF|NOTES|WORKSPACE|USAGE DETAILS|CONTINUE)$', 'bold')
        body.write(text)
        self.query_one('#task-archive', Button).label = 'Restore' if self.record['archived'] else 'Archive'
        self.query_one('#task-link', Button).tooltip = 'Link the session currently selected in the monitor. Re-linking an exact run updates its evidence.'

    def changed(self, task):
        if task:
            self.record = task
            self.render_task()
            self.status('Saved locally.')

    def reload(self, _=None):
        self.job(lambda store: store.get(self.record['id']), self.changed)

    def action_reload(self):
        self.reload()

    def action_edit(self):
        if not self.busy:
            self.app.push_screen(TaskEditor(self.owner, task=self.record), self.changed)

    def action_brief(self):
        if not self.busy:
            self.app.push_screen(TaskBriefEditor(self.owner, self.record), self.changed)

    def action_decisions(self):
        if not self.busy:
            self.app.push_screen(TaskDecisionsScreen(self.owner, self.record), self.changed)

    def action_draft(self):
        if self.busy:return
        from .review_continuity import editable_draft
        context=capture_selected_run(self.owner) or {}
        projection=context.get('review_snapshot')
        if not projection:
            projection=next((row['review_snapshot'] for row in reversed(self.record['links']) if row.get('review_snapshot')),None)
            if projection:
                scope=projection['scope']
                context=current_capture_context(self.owner,{**scope,'review_snapshot':projection})
        if not projection or context.get('capture_content') is False:
            self.status('Open a source-enabled Review for an exact recorded execution before drafting continuity.');return
        try:draft=editable_draft(projection,self.record)
        except ValueError as exc:self.status(str(exc));return
        self.app.push_screen(TaskEditor(self.owner,self.record,context=context,continuity_draft=draft),self.changed)

    def action_link(self):
        context = capture_selected_run(self.owner)
        if not context:
            self.status('Select a session in the monitor, then return to this task to link it.')
            return
        task_id, revision = self.record['id'], self.record['revision']
        self.job(lambda store: store.link(task_id, context, expected_revision=revision), self.changed,
                 'Linking selected run: ' + context['task'])

    def action_capture(self):
        task_id, revision = self.record['id'], self.record['revision']
        self.job(lambda store: store.snapshot(task_id, expected_revision=revision), self.changed,
                 'Capturing Git changes… You can keep using other terminals.')

    def action_handoff(self):
        if not self.busy:
            self.app.push_screen(HandoffScreen(self.owner, self.record))

    @on(Button.Pressed)
    def buttons(self, event):
        key = event.button.id
        if key == 'task-draft':self.action_draft()
        elif key == 'task-edit': self.action_edit()
        elif key == 'task-brief': self.action_brief()
        elif key == 'task-decisions': self.action_decisions()
        elif key == 'task-link': self.action_link()
        elif key == 'task-capture': self.action_capture()
        elif key == 'task-handoff': self.action_handoff()
        elif key == 'task-runs': self.app.push_screen(TaskRunsScreen(self.owner, self.record), self.reload)
        elif key == 'task-archive':
            task_id, revision, value = self.record['id'], self.record['revision'], not self.record['archived']
            self.job(lambda store: store.update(task_id, archived=value, expected_revision=revision), self.changed)


class TaskEditor(TaskJobs, ModalScreen):
    DEFAULT_CSS = '''
    TaskEditor { align: center middle; background: $background 75%; }
    #task-too-small { display: none; height: auto; padding: 1; }
    TaskEditor.too-small #task-editor { display: none; }
    TaskEditor.too-small #task-too-small { display: block; }
    #task-editor { width: 92%; max-width: 90; height: 94%; padding: 0 1; border: round $primary; background: $surface; }
    TaskEditor.compact #task-editor { width: 100%; height: 100%; border: none; }
    #task-editor Input, #task-editor TextArea { background: $agent-panel; color: $agent-text; border: solid $agent-border; border-title-color: $agent-secondary; }
    #task-editor Input:focus, #task-editor TextArea:focus { border: solid $agent-primary; border-title-color: $agent-primary; }
    #task-editor-heading { height: 1; text-style: bold; }
    #task-name, #task-workspace { height: 3; }
    #task-notes { height: 1fr; min-height: 3; }
    #task-status { height: 2; color: $secondary; }
    .task-actions { height: 3; }
    .task-actions Button { width: 1fr; height: 3; min-width: 8; padding: 0; }
    '''
    BINDINGS = [Binding('ctrl+s', 'save', 'Save', priority=True),
                Binding('escape', 'cancel', 'Cancel')]

    def __init__(self, owner, task=None, context=None,continuity_draft=None):
        super().__init__()
        self.owner, self.record, self.context = owner, deepcopy(task), context
        self.continuity_draft=deepcopy(continuity_draft or (context or {}).get('continuity_draft'))
        self.initial = (task['title'], task['workspace'], task['notes']) if task else (
            one_line((context or {}).get('task'), 200), (context or {}).get('local_workspace') or '', '')
        if self.continuity_draft:self.initial=(*self.initial[:2],self.continuity_draft['text'])
        self.discard_armed = False

    def compose(self):
        yield Static('Task editing needs 60 columns × 20 rows. Enlarge the terminal; your edits are preserved.',
                     id='task-too-small', markup=False)
        with Vertical(id='task-editor'):
            yield Static('Review continuity draft · nothing saved yet' if self.continuity_draft else 'Edit task' if self.record else 'Save a task', id='task-editor-heading')
            yield Input(self.initial[0], placeholder='Task title (required)', max_length=200, id='task-name')
            yield Input(self.initial[1], placeholder='Workspace directory (optional)', max_length=4096, id='task-workspace')
            yield TextArea(self.initial[2], id='task-notes', soft_wrap=True,
                           placeholder='Intent, acceptance checks, decisions, and the next step…')
            yield Static('Ctrl+S saves · Tab next field · Esc back' if self.record else
                         'Ctrl+S saves notes, selected run, and workspace.', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('Approve · Ctrl+S' if self.continuity_draft else 'Save · Ctrl+S', id='task-save')
                yield Button('Cancel', id='task-cancel')

    def on_mount(self):
        if not self.query('#task-name'):
            return  # Structured editors supply their own focused field.
        self.query_one('#task-name', Input).border_title = 'Task title'
        self.query_one('#task-workspace', Input).border_title = 'Local workspace'
        self.query_one(TextArea).border_title = 'Editable continuity draft · approval preserves authored notes' if self.continuity_draft else 'Your brief · decisions and next steps'
        if self.record and self.continuity_draft:
            self.query_one('#task-name',Input).disabled=True;self.query_one('#task-workspace',Input).disabled=True
            self.query_one(TextArea).focus()
        else:self.query_one('#task-name', Input).focus()
        if self.continuity_draft:self.status('Ctrl+S explicitly approves a versioned draft. Authored notes, brief and decisions are preserved.')

    def on_resize(self):
        self.set_class(self.size.width < 60 or self.size.height < 20, 'too-small')
        self.set_class(self.size.width < 80 or self.size.height <= 20, 'compact')

    def values(self):
        return (self.query_one('#task-name', Input).value,
                self.query_one('#task-workspace', Input).value,
                self.query_one(TextArea).text)

    def action_cancel(self):
        if self.busy:
            return
        if self.values() != self.initial and not self.discard_armed:
            self.discard_armed = True
            self.status('Unsaved edits. Save with Ctrl+S, or press Esc again to discard.')
        else:
            self.dismiss(None)

    def action_save(self):
        if self.busy:
            return
        title, workspace, notes = self.values()
        for field in self.query('Input, TextArea'):
            field.disabled = True
        task, context = self.record, self.context
        def save(store):
            if self.continuity_draft:
                safe_context=current_capture_context(self.owner,context)
                if not safe_context or safe_context.get('capture_content') is False:
                    from .saved_tasks import TaskError
                    raise TaskError('Source capture or receiver changed. Reopen current Review before approving this source-backed draft.')
                draft=deepcopy(self.continuity_draft);draft['text']=notes
                if task:return store.approve_continuity(task['id'],draft,expected_revision=task['revision'],source_allowed=True)
                return store.create(title,workspace,'',link=safe_context,continuity_draft=draft,source_allowed=True)
            if task:
                return store.update(task['id'], title=title, workspace=workspace, notes=notes,
                                    expected_revision=task['revision'])
            safe_context = current_capture_context(self.owner, context)
            revoked = bool(context and safe_context.get('capture_content') is False)
            safe_title = 'Metadata-only execution' if revoked and title == self.initial[0] else title
            safe_workspace = '' if revoked and workspace == self.initial[1] else workspace
            created = store.create(safe_title, safe_workspace, notes, link=safe_context)
            if safe_workspace:
                try:
                    created = store.snapshot(created['id'], expected_revision=created['revision'])
                except Exception as exc:
                    # Creation has committed; retain it rather than inviting a
                    # second creation when a concurrent capture fails.
                    created['_capture_warning'] = str(exc)
            return created
        def saved(task):
            if task.get('_capture_warning'):
                self.owner.notify('Task saved. Capture again from the task: ' + task['_capture_warning'], severity='warning')
            self.dismiss(task)
        self.job(save, saved, 'Saving task and workspace…' if not task and workspace else 'Saving task…')

    def finish_job(self, result, error, callback):
        if self.is_mounted:
            for field in self.query('Input, TextArea'):
                field.disabled = False
        super().finish_job(result, error, callback)

    @on(Input.Changed)
    @on(TextArea.Changed)
    def edit_changed(self):
        self.discard_armed = False

    @on(Button.Pressed)
    def buttons(self, event):
        if event.button.id == 'task-save': self.action_save()
        elif event.button.id == 'task-cancel': self.action_cancel()


class TaskBriefEditor(TaskEditor):
    """One field at a time keeps structured editing usable at 60×20."""
    fields = BRIEF_FIELDS
    heading = 'Structured brief · user-owned intent and next steps'

    def __init__(self, owner, task):
        super().__init__(owner, task=task)
        self.draft = {key: task.get('brief', {}).get(key, '') for key in self.fields}
        self.initial = deepcopy(self.draft)
        self.field = self.fields[0]

    def compose(self):
        yield Static('Task editing needs 60 columns × 20 rows. Enlarge the terminal; your edits are preserved.',
                     id='task-too-small', markup=False)
        with Vertical(id='task-editor'):
            yield Static(self.heading, id='task-editor-heading', markup=False)
            yield from self.extra_fields()
            yield Select([(key.replace('_', ' ').title(), key) for key in self.fields],
                         value=self.field, allow_blank=False, id='brief-field')
            yield TextArea(self.draft[self.field], id='task-notes', soft_wrap=True)
            yield Static('Ctrl+S saves all fields · Tab controls · Esc back', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('Save · Ctrl+S', id='task-save')
                yield Button('Copy draft', id='brief-copy')
                yield Button('Cancel', id='task-cancel')

    def extra_fields(self):
        return iter(())

    def on_mount(self):
        self.query_one(TextArea).border_title = self.field.replace('_', ' ').title()
        self.query_one(TextArea).focus()
        self.on_resize()

    def values(self):
        self.draft[self.field] = self.query_one(TextArea).text
        return deepcopy(self.draft)

    @on(Select.Changed, '#brief-field')
    def choose_field(self, event):
        if not self.is_mounted or event.value not in self.fields or event.value == self.field:
            return
        self.draft[self.field] = self.query_one(TextArea).text
        self.field = event.value
        self.query_one(TextArea).load_text(self.draft[self.field])
        self.query_one(TextArea).border_title = self.field.replace('_', ' ').title()

    def action_save(self):
        if self.busy:
            return
        values = self.values()
        task = self.record
        self.lock_fields()
        self.job(lambda store: store.update_brief(task['id'], values, expected_revision=task['revision']),
                 self.dismiss, 'Saving structured brief…')

    def lock_fields(self):
        for field in self.query('Select, TextArea'):
            field.disabled = True

    def finish_job(self, result, error, callback):
        if self.is_mounted:
            for field in self.query(Select):
                field.disabled = False
        super().finish_job(result, error, callback)

    @on(Button.Pressed, '#brief-copy')
    def copy_draft(self, event):
        import json
        self.owner.copy_to_clipboard(json.dumps(self.values(), ensure_ascii=False, indent=2))
        self.status('Copied the unsaved draft. On a conflict, cancel and reload the task before applying it.')
        event.stop()


class TaskDecisionEditor(TaskBriefEditor):
    fields = ('text', 'rationale', 'scope')
    heading = 'Decision · append an accepted or rejected approach'

    def __init__(self, owner, task, *, supersedes=None, source=None, excerpt=''):
        super().__init__(owner, task)
        self.supersedes = supersedes
        self.source = deepcopy(source)
        self.draft = {key: (supersedes or {}).get(key, '') for key in self.fields}
        if excerpt:
            self.draft['text'] = excerpt
        self.initial = {**deepcopy(self.draft), 'kind': (supersedes or {}).get('kind', 'accepted')}

    def extra_fields(self):
        yield Select([('Accepted approach', 'accepted'), ('Rejected approach', 'rejected')],
                     value=self.initial['kind'], allow_blank=False, id='decision-kind')
        if self.supersedes:
            yield Static('New version of ' + self.supersedes['id'][:12] + ' · previous version retained', markup=False)
        if self.source:
            yield Static('Explicit excerpt · review the text and its saved source before saving', markup=False)

    def values(self):
        return {**super().values(), 'kind': self.query_one('#decision-kind', Select).value}

    @on(Select.Changed, '#decision-kind')
    def kind_changed(self):
        self.discard_armed = False

    def action_save(self):
        if self.busy:
            return
        values = self.values()
        task = self.record
        source = self.source
        if source and current_capture_context(self.owner, source).get('capture_content') is False:
            source = None
            if values['text'] == self.initial['text']:
                self.status('Source capture was revoked. Enter your own decision text or cancel; the excerpt cannot be saved.')
                return
        self.lock_fields()
        def save(store):
            safe_source = source
            if safe_source and current_capture_context(self.owner, safe_source).get('capture_content') is False:
                from .saved_tasks import TaskError
                if values['text'] == self.initial['text']:
                    raise TaskError('Source capture was revoked. Enter your own decision text or cancel; the excerpt cannot be saved.')
                safe_source = None
            return store.add_decision(task['id'], **values, source=safe_source,
                supersedes_id=self.supersedes['id'] if self.supersedes else None,
                expected_revision=task['revision'])
        self.job(save, self.dismiss, 'Saving new decision version…')


class TaskDecisionsScreen(TaskScreen):
    BINDINGS = [Binding('n', 'new', 'New decision'), Binding('s', 'supersede', 'Supersede')]
    DEFAULT_CSS = '''
    #decision-version { height: 3; }
    TaskDecisionsScreen .task-actions Button { margin-right: 1; }
    '''

    def __init__(self, owner, task):
        super().__init__(owner)
        self.record = deepcopy(task)

    def compose(self):
        yield self.compose_floor()
        with Vertical(id='saved-root'):
            yield Static('Decisions · immutable versions and source references', id='task-heading', markup=False)
            yield Select([], id='decision-version', prompt='No decisions saved')
            yield RichLog(id='task-body', wrap=True, markup=False, min_width=1, auto_scroll=False)
            yield Static('', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('Add', id='decision-add')
                yield Button('Supersede', id='decision-supersede')
                yield Button('Copy version', id='decision-copy')
                yield Button('Back', id='decision-back')
            yield Static('n Add  s Supersede  Tab Controls  Esc Task', id='task-hints', markup=False)

    def on_mount(self):
        self.render_decisions()
        self.update_floor()
        self.query_one('#decision-version' if self.record.get('decisions') else '#decision-add').focus()

    def render_decisions(self):
        decisions = self.record.get('decisions', [])
        replaced = {item.get('supersedes_id') for item in decisions}
        selector = self.query_one('#decision-version', Select)
        selector.set_options([(str(i+1) + ' · ' + item['kind'] +
            (' · superseded' if item['id'] in replaced else ' · current') + ' · ' + one_line(item['text'], 60), item['id'])
            for i, item in enumerate(decisions)])
        if decisions:
            selector.value = decisions[-1]['id']
        self.show_decision()

    def selected(self):
        value = self.query_one('#decision-version', Select).value
        return next((item for item in self.record.get('decisions', []) if item['id'] == value), None)

    @on(Select.Changed, '#decision-version')
    def show_decision(self):
        body = self.query_one(RichLog)
        body.clear()
        selected = self.selected()
        if selected:
            import json
            body.write(Text(selected['text'] + '\n\nRationale\n' + selected['rationale'] +
                '\n\nScope\n' + (selected['scope'] or 'Not specified') +
                '\n\nVersion ' + selected['id'] + ' · ' + stamp(selected['created_ns']) +
                ('\nSupersedes ' + selected['supersedes_id'] if selected.get('supersedes_id') else '') +
                ('\n\nSaved source · availability not revalidated\n' + json.dumps(selected['source'], ensure_ascii=False, indent=2)
                 if selected.get('source') else '\n\nManual decision · no source reference')))
        else:
            body.write('No decisions saved. Add an accepted or rejected approach with its rationale.')
        replaced = {item.get('supersedes_id') for item in self.record.get('decisions', [])}
        self.query_one('#decision-supersede', Button).disabled = not selected or selected['id'] in replaced
        self.status(f"{len(self.record.get('decisions', []))} of 128 retained versions · no automatic extraction")

    def changed(self, task):
        if task:
            self.record = task
            self.render_decisions()

    def action_new(self):
        if not self.busy:
            self.app.push_screen(TaskDecisionEditor(self.owner, self.record), self.changed)

    def action_supersede(self):
        selected = self.selected()
        if selected and not self.busy:
            if any(item.get('supersedes_id') == selected['id'] for item in self.record.get('decisions', [])):
                self.status('This version was superseded. Choose its current version to append another change.')
                return
            self.app.push_screen(TaskDecisionEditor(self.owner, self.record, supersedes=selected), self.changed)

    def action_back(self):
        self.dismiss(self.record)

    @on(Button.Pressed)
    def buttons(self, event):
        if event.button.id == 'decision-add': self.action_new()
        elif event.button.id == 'decision-supersede': self.action_supersede()
        elif event.button.id == 'decision-back': self.action_back()
        elif event.button.id == 'decision-copy' and self.selected():
            import json
            self.owner.copy_to_clipboard(json.dumps(self.selected(), ensure_ascii=False, indent=2))
            self.status('Copied exactly the selected decision version and its saved source reference.')


class TaskRunsScreen(TaskScreen):
    def __init__(self, owner, task):
        super().__init__(owner)
        self.record = task

    def compose(self):
        yield self.compose_floor()
        with Vertical(id='saved-root'):
            yield Static('Linked runs and sessions', id='task-heading')
            yield TaskTable(id='task-table', cursor_type='row')
            yield Static('Session-only links are excluded from usage totals.', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('Copy resume', id='task-resume')
                yield Button('Unlink', id='task-unlink')
            yield Static('↑/↓ Select  Tab Actions  Esc Task', id='task-hints', markup=False)

    def on_mount(self):
        table = self.query_one(DataTable)
        table.add_column('Agent / request', width=25)
        table.add_column('Scope', width=12)
        table.add_column('Saved state', width=13)
        self.call_after_refresh(self.resize_columns)
        self.render_runs()

    def on_resize(self):
        self.update_floor()
        if self.is_mounted:
            self.call_after_refresh(self.resize_columns)

    def resize_columns(self):
        self.query_one(TaskTable).fit(('Agent / request', 'Scope', 'Saved state'), (12, 13))

    def render_runs(self):
        table = self.query_one(DataTable)
        table.clear()
        for link in self.record['links']:
            table.add_row(Text((link.get('runtime') or 'Agent') + ' · ' + (link.get('task') or link.get('session_id') or 'Recorded run')),
                          'Repair scope' if review_scope_issue(link) else 'Turn' if link.get('turn_id') else 'Run' if link.get('trace_id') or link.get('run_start_ns') else 'Session only', link.get('state') or 'Unknown', key=link['key'])
        if self.record.get('link_scope_warnings'):
            self.status('A saved review needs scope repair. Relink that Review or explicitly unlink it; aggregate usage is unavailable.')
        if not self.record['links']:
            self.status('No linked runs. Return to the task and Link a selected monitoring session.')
        table.focus()

    def selected(self):
        index = self.query_one(DataTable).cursor_row
        return self.record['links'][index] if 0 <= index < len(self.record['links']) else None

    @on(Button.Pressed)
    def buttons(self, event):
        link = self.selected()
        if not link:
            return
        if event.button.id == 'task-resume':
            if link.get('resume_command'):
                self.owner.copy_to_clipboard(link['resume_command'])
                self.status('Copied saved resume command. Check whether that session is already active before running it.')
            else:
                self.status('A native resume command was not recorded for this session.')
        elif event.button.id == 'task-unlink':
            task_id, revision, key = self.record['id'], self.record['revision'], link['key']
            def changed(task):
                self.record = task
                self.render_runs()
                self.status('Unlinked from this task. Source telemetry is unchanged.')
            self.job(lambda store: store.unlink(task_id, key, expected_revision=revision), changed)


def prepare_handoff_lines(markdown, width):
    """Format once in a worker; painting a frame only reads visible lines."""
    theme = Theme({key: 'bold' for key in ('markdown.h1', 'markdown.h2', 'markdown.h3')})
    console = Console(file=StringIO(), width=width, theme=theme)
    lines = console.render_lines(Markdown(markdown), console.options, pad=False)
    return [Strip(line) for line in lines]


class HandoffPreview(ScrollView, can_focus=True):
    DEFAULT_CSS = '''
    HandoffPreview { background: $agent-surface; color: $agent-text; overflow-x: hidden; overflow-y: scroll; scrollbar-size: 1 1; }
    '''

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.lines = []

    def show_lines(self, lines, width):
        scroll = self.scroll_offset.y
        self.lines = lines
        self.virtual_size = Size(width, len(lines))
        self.scroll_to(y=scroll, animate=False, immediate=True)
        self.refresh()

    def render_line(self, y):
        index = y + self.scroll_offset.y
        width = self.scrollable_content_region.width
        line = self.lines[index] if index < len(self.lines) else Strip.blank(width)
        return line.crop_extend(0, width, self.rich_style).apply_style(self.rich_style)


class HandoffScreen(TaskScreen):
    def __init__(self, owner, task):
        super().__init__(owner)
        self.record = deepcopy(task)
        self.markdown = ''
        self.preview_width = None
        self.preview_pending = False
        self.source_signature=task_policy_signature(owner,task);self.handoff_generation=0

    def compose(self):
        yield self.compose_floor()
        with Vertical(id='saved-root'):
            yield Static('Handoff preview · saved evidence', id='task-heading')
            yield HandoffPreview(id='task-body')
            yield Input(placeholder='Export path, e.g. ~/handoff.md', id='handoff-path')
            yield Static('Review before sharing. This includes saved notes, paths, commands, and changes.', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('Copy brief', id='handoff-copy')
                yield Button('Write MD + HTML', id='handoff-write')
            yield Static('PgUp/PgDn Read  Tab Actions  Esc Task', id='task-hints', markup=False)

    def on_mount(self):
        self.query_one(HandoffPreview).focus()
        self.refresh_handoff()
        self.set_interval(.5,self.check_source_policy)

    def refresh_handoff(self):
        self.handoff_generation+=1;self.preview_width=None
        self.source_signature=task_policy_signature(self.owner,self.record)
        policy=task_source_policy(self.owner)
        self.job(lambda _: render_handoff(self.record,source_policy=policy),self.loaded,'Preparing the saved handoff…',needs_store=False)

    def check_source_policy(self):
        if self.source_signature!=task_policy_signature(self.owner,self.record):
            self.markdown='';self.preview_width=None;self.handoff_generation+=1
            body=self.query_one(HandoffPreview);body.show_lines([],max(1,body.scrollable_content_region.width))
            self.status('Source policy changed. Refreshing derived sections; authored notes remain available.')
            if not self.busy:self.refresh_handoff()

    def loaded(self, markdown):
        if self.source_signature!=task_policy_signature(self.owner,self.record):
            self.markdown='';self.check_source_policy();return
        self.markdown = markdown;self.preview_width=None
        self.call_after_refresh(self.render_preview)
        self.status('Review before sharing. This includes saved notes, paths, commands, and changes.')

    def on_resize(self):
        self.update_floor()
        if self.is_mounted and self.markdown:
            self.call_after_refresh(self.render_preview)

    def render_preview(self):
        if self.has_class('too-small'):
            return
        if self.busy:
            self.preview_pending = True
            return
        body = self.query_one(HandoffPreview)
        width = max(1, body.scrollable_content_region.width)
        self.preview_pending = False
        if self.preview_width == width:
            return
        generation=self.handoff_generation
        def ready(lines):
            if generation!=self.handoff_generation or self.source_signature!=task_policy_signature(self.owner,self.record):return
            self.preview_width = width
            body.show_lines(lines, width)
            self.status('Review before sharing. This includes saved notes, paths, commands, and changes.')
        self.job(lambda _: prepare_handoff_lines(self.markdown, width), ready,
                 'Formatting the saved preview… Esc returns to the task.', needs_store=False)

    def finish_job(self, result, error, callback):
        super().finish_job(result, error, callback)
        if self.is_mounted and self.source_signature!=task_policy_signature(self.owner,self.record):
            self.call_after_refresh(self.check_source_policy)
        elif self.is_mounted and not error and self.preview_pending:
            self.call_after_refresh(self.render_preview)

    @on(Button.Pressed)
    def buttons(self, event):
        self.check_source_policy()
        if not self.markdown:return
        if event.button.id == 'handoff-copy':
            self.owner.copy_to_clipboard(self.markdown)
            self.status('Copied the previewed brief. Paste it into the agent you want to continue with.')
        elif event.button.id == 'handoff-write':
            path = self.query_one('#handoff-path', Input).value.strip()
            if not path:
                self.status('Choose a new .md or .html destination. Both files will be written.')
                return
            signature=self.source_signature;policy=task_source_policy(self.owner)
            def write(_):
                from .saved_tasks import TaskError
                if signature!=task_policy_signature(self.owner,self.record):raise TaskError('Source capture changed. Reopen the current handoff before exporting.')
                return export_handoff(self.record,path,source_policy=policy)
            self.job(write,lambda paths: self.status('Wrote ' + ' and '.join(paths)), 'Writing the previewed handoff…', needs_store=False)
