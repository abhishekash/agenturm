"""Saved local tasks, deliberate workspace capture, and reviewed handoffs."""
from __future__ import annotations

from copy import deepcopy
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
from textual.widgets import Button, DataTable, Input, RichLog, Static, TextArea

from .saved_tasks import TaskStore, export_handoff, render_handoff
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


def capture_selected_run(owner):
    """Freeze only the chosen identity and exact-run evidence already loaded."""
    item = next((item for item in owner.workflow_items()
                 if item['id'] == owner.selected_source), None)
    if not item:
        return None
    data = owner.feed.snapshot
    usage = task_run_usage(data.sessions, item, data.traces)
    trace = str(usage.get('trace_id') or item.get('latest_trace_id') or '')
    trace = trace.lower() if re.fullmatch(r'[0-9a-fA-F]{32}', trace) else ''
    checks = []
    if trace:
        for event in events_for(item, data.activity):
            command = str(event.get('subject') or '')
            if (event.get('trace_id') == trace and event.get('domain') == 'shell'
                    and re.search(r'\b(pytest|unittest|test|check|lint|tsc)\b', command)):
                checks.append({'command': plain(command)[:2000],
                    'result': one_line(event.get('result') or event.get('status') or 'not reported', 200),
                    'time_ns': event.get('time_unix_nano') or 0, 'trace_id': trace})
    endpoint = urlsplit(getattr(owner.feed.api, 'endpoint', '')).hostname
    workspace = local_workspace(item.get('working_directory'))
    return {'session_id': str(item.get('session_id') or ''),
            'agent_id': str(item.get('agent_id') or ''),
            'runtime': one_line(item.get('runtime'), 120), 'trace_id': trace,
            'task': one_line(item.get('task')), 'state': one_line(item.get('state'), 120),
            'working_directory': workspace,
            'local_workspace': workspace if endpoint in {'localhost', '127.0.0.1', '::1'} else '',
            'branch': one_line(item.get('branch'), 512),
            'captured_ns': time.time_ns(), 'usage': deepcopy(usage) if trace else {},
            'checks': checks[-8:], 'resume_command': resume_command({**item, 'working_directory': workspace})}


def stamp(value):
    return datetime.fromtimestamp(value / 1e9).strftime('%b %d %H:%M') if value else 'Not captured'


def task_overview(task):
    usage = task.get('usage_summary') or {}
    snapshot = task.get('workspace_snapshot') or {}
    tokens = [usage.get(field) for field in ('input_tokens', 'output_tokens')]
    total = sum(value for value in tokens if value is not None)
    token_label = f'{total:,}' if any(value is not None for value in tokens) else 'Not reported'
    if any(value is not None for value in tokens) and not usage.get('usage_complete'):
        token_label += ' (partial)'
    cost = usage.get('known_cost_usd')
    cost_label = 'Not reported' if cost is None else f'${cost:,.4f}' + ('' if usage.get('cost_complete') else ' (partial)')
    lines = [f'Tokens {token_label} · Reported cost {cost_label}',
             f"{usage.get('linked_runs', 0)} linked runs · {usage.get('unscoped_links', 0)} session-only links", '',
             'NOTES', plain(task.get('notes')) or 'Add the intent, decisions, and next step with Edit.', '',
             'WORKSPACE', plain(task.get('workspace')) or 'No workspace selected',
             'Snapshot: ' + stamp(snapshot.get('captured_ns'))]
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
    lines += ['', 'USAGE DETAILS',
              'Only explicitly linked runs are counted. Link again to refresh a run.']
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
    BINDINGS = [Binding('escape', 'back', 'Back', priority=True)]

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

    def __init__(self, owner, create=False):
        super().__init__(owner)
        self.rows = []
        self.total = 0
        self.include_archived = False
        self.create_on_mount = create
        self.preferred = None

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
            self.job(lambda store: store.get(identity),
                     lambda task: self.app.push_screen(TaskDetailScreen(self.owner, task), lambda _: self.reload(identity)))

    def action_new(self):
        if not self.busy:
            self.app.push_screen(TaskEditor(self.owner, context=capture_selected_run(self.owner)),
                                 lambda task: self.reload(task['id']) if task else None)

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
                Binding('r', 'capture', 'Capture workspace'), Binding('h', 'handoff', 'Handoff')]

    def __init__(self, owner, task):
        super().__init__(owner)
        self.record = task

    def compose(self):
        yield self.compose_floor()
        with Vertical(id='saved-root'):
            yield Static('', id='task-heading', markup=False)
            yield RichLog(id='task-body', wrap=True, markup=False, min_width=1, auto_scroll=False)
            yield Static('', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                for name, label in (('edit', 'Edit'), ('runs', 'Runs'), ('link', 'Link'),
                                    ('capture', 'Capture'), ('handoff', 'Handoff'), ('archive', 'Archive')):
                    yield Button(label, id='task-' + name)
            yield Static('e Edit  r Capture  h Handoff  Esc Tasks', id='task-hints', markup=False)

    def on_mount(self):
        self.call_after_refresh(self.render_task)

    def on_resize(self):
        self.update_floor()
        if self.is_mounted:
            self.call_after_refresh(self.render_task)

    def render_task(self):
        self.query_one('#task-heading', Static).update(plain(self.record['title']))
        body = self.query_one(RichLog)
        body.clear()
        title = plain(self.record['title'])
        content = (title + '\n\n' if cell_len(title) > body.scrollable_content_region.width else '') + task_overview(self.record)
        text = Text(content)
        text.highlight_regex(r'(?m)^(NOTES|WORKSPACE|USAGE DETAILS|CONTINUE)$', 'bold')
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
        if key == 'task-edit': self.action_edit()
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
                Binding('escape', 'cancel', 'Cancel', priority=True)]

    def __init__(self, owner, task=None, context=None):
        super().__init__()
        self.owner, self.record, self.context = owner, deepcopy(task), context
        self.initial = (task['title'], task['workspace'], task['notes']) if task else (
            one_line((context or {}).get('task'), 200), (context or {}).get('local_workspace') or '', '')
        self.discard_armed = False

    def compose(self):
        yield Static('Task editing needs 60 columns × 20 rows. Enlarge the terminal; your edits are preserved.',
                     id='task-too-small', markup=False)
        with Vertical(id='task-editor'):
            yield Static('Edit task' if self.record else 'Save a task', id='task-editor-heading')
            yield Input(self.initial[0], placeholder='Task title (required)', max_length=200, id='task-name')
            yield Input(self.initial[1], placeholder='Workspace directory (optional)', max_length=4096, id='task-workspace')
            yield TextArea(self.initial[2], id='task-notes', soft_wrap=True,
                           placeholder='Intent, acceptance checks, decisions, and the next step…')
            yield Static('Ctrl+S saves · Tab next field · Esc back' if self.record else
                         'Ctrl+S saves notes, selected run, and workspace.', id='task-status', markup=False)
            with Horizontal(classes='task-actions'):
                yield Button('Save · Ctrl+S', id='task-save')
                yield Button('Cancel', id='task-cancel')

    def on_mount(self):
        self.query_one('#task-name', Input).border_title = 'Task title'
        self.query_one('#task-workspace', Input).border_title = 'Local workspace'
        self.query_one(TextArea).border_title = 'Your brief · decisions and next steps'
        self.query_one('#task-name', Input).focus()

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
        title, workspace, notes = self.values()
        task, context = self.record, self.context
        def save(store):
            if task:
                return store.update(task['id'], title=title, workspace=workspace, notes=notes,
                                    expected_revision=task['revision'])
            created = store.create(title, workspace, notes, link=context)
            if workspace:
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

    @on(Input.Changed)
    @on(TextArea.Changed)
    def edit_changed(self):
        self.discard_armed = False

    @on(Button.Pressed)
    def buttons(self, event):
        if event.button.id == 'task-save': self.action_save()
        elif event.button.id == 'task-cancel': self.action_cancel()


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
                          'Run' if link.get('trace_id') else 'Session only', link.get('state') or 'Unknown', key=link['key'])
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
        self.job(lambda _: render_handoff(self.record), self.loaded, 'Preparing the saved handoff…', needs_store=False)

    def loaded(self, markdown):
        self.markdown = markdown
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
        def ready(lines):
            self.preview_width = width
            body.show_lines(lines, width)
            self.status('Review before sharing. This includes saved notes, paths, commands, and changes.')
        self.job(lambda _: prepare_handoff_lines(self.markdown, width), ready,
                 'Formatting the saved preview… Esc returns to the task.', needs_store=False)

    def finish_job(self, result, error, callback):
        super().finish_job(result, error, callback)
        if self.is_mounted and not error and self.preview_pending:
            self.call_after_refresh(self.render_preview)

    @on(Button.Pressed)
    def buttons(self, event):
        if not self.markdown:
            return
        if event.button.id == 'handoff-copy':
            self.owner.copy_to_clipboard(self.markdown)
            self.status('Copied the previewed brief. Paste it into the agent you want to continue with.')
        elif event.button.id == 'handoff-write':
            path = self.query_one('#handoff-path', Input).value.strip()
            if not path:
                self.status('Choose a new .md or .html destination. Both files will be written.')
                return
            self.job(lambda _: export_handoff(self.record, path),
                     lambda paths: self.status('Wrote ' + ' and '.join(paths)), 'Writing the previewed handoff…', needs_store=False)
