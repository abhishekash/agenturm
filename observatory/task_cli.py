"""Non-interactive saved-task commands; no receiver or agent process required."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sqlite3
import sys

from .saved_tasks import MAX_NOTES, TaskConflict, TaskError, TaskStore, export_handoff, render_handoff


def add_task_parser(subparsers):
    parser = subparsers.add_parser('task', help='save tasks and prepare local handoffs',
        description='Save work across agent sessions. These commands work without a receiver.',
        epilog='Example: agenturm task create --title "Fix refresh expiry" --workspace . '
               'Exit codes: 0 success, 2 invalid input/storage error, 3 revision conflict.')
    parser.add_argument('--tasks-db', help='saved-task database (or AGENTURM_TASKS_DB)')
    actions = parser.add_subparsers(dest='task_action')

    def action(name, help, identity=False):
        item = actions.add_parser(name, help=help)
        item.add_argument('--tasks-db', default=argparse.SUPPRESS, help=argparse.SUPPRESS)
        item.add_argument('--json', action='store_true', help='write one JSON result; errors go to stderr')
        if identity:
            item.add_argument('task_id', help='saved task UUID from task list')
        return item

    def notes(item):
        group = item.add_mutually_exclusive_group()
        group.add_argument('--notes', help='intent, decisions, and next steps')
        group.add_argument('--notes-file', type=Path, help='read editable notes from a UTF-8 file (up to 32,000 characters)')

    def guard(item):
        item.add_argument('--expected-revision', type=int, help='refuse changes if the task has changed since this revision')

    listing = action('list', 'list the 1,000 most recently updated tasks')
    listing.add_argument('--all', action='store_true', help='include archived tasks')
    listing.add_argument('--offset', type=int, default=0, help='skip this many tasks to read the next page (default: 0)')
    create = action('create', 'save a new task and print its ID')
    create.add_argument('--title', required=True)
    create.add_argument('--workspace', default='', help='local workspace directory')
    create.add_argument('--snapshot', action='store_true', help='capture Git changes after saving the task')
    notes(create)
    action('show', 'show a saved task', True)
    update = action('update', 'edit a saved task', True)
    update.add_argument('--title')
    update.add_argument('--workspace', help='change workspace and clear the old Git snapshot')
    notes(update)
    guard(update)
    link = action('link', 'associate a specific run or session with a task', True)
    link.add_argument('--evidence-file', type=Path, help='read a bounded JSON evidence object')
    link.add_argument('--session-id')
    link.add_argument('--agent-id')
    link.add_argument('--trace-id', help='exact 32-character trace ID; session-only links do not contribute usage totals')
    link.add_argument('--runtime', default='', help='agent name, such as Codex or Claude Code')
    link.add_argument('--state', default='Unknown', help='recorded state of this run')
    guard(link)
    unlink = action('unlink', 'remove an association without deleting its source evidence', True)
    unlink.add_argument('link_key', help='key from task show --json')
    guard(unlink)
    snapshot = action('snapshot', 'capture bounded read-only Git changes against the saved base', True)
    guard(snapshot)
    handoff = action('handoff', 'print a brief or export new Markdown and HTML files', True)
    handoff.add_argument('--format', choices=('markdown', 'html', 'json'), default='markdown', help='stdout format (default: markdown)')
    handoff.add_argument('-o', '--output', type=Path, help='new .md/.html/.json path; Markdown and HTML companions are always created')
    archive = action('archive', 'archive a task, or restore it to the list', True)
    archive.add_argument('--restore', action='store_true')
    guard(archive)
    return parser


def _read(path, limit):
    with path.open('rb') as stream:
        value = stream.read(limit + 1)
    if len(value) > limit:
        raise TaskError(f'{path.name} exceeds the input size limit ({limit:,} bytes).')
    try:
        return value.decode('utf-8')
    except UnicodeDecodeError as exc:
        raise TaskError(f'{path.name} must be UTF-8 text.') from exc


def _notes(args):
    if getattr(args, 'notes_file', None):
        return _read(args.notes_file, MAX_NOTES * 4)
    return getattr(args, 'notes', None)


def _show(task):
    lines = [task['title'], f"ID: {task['id']}", f"Workspace: {task['workspace'] or 'Not selected'}",
             f"Revision: {task['revision']}" + (' · archived' if task['archived'] else ''), '',
             task['notes'] or 'No notes saved.', '', f"Linked runs/sessions: {len(task['links'])}"]
    for link in task['links']:
        lines.append(f"  {link['key']}  {link.get('runtime') or 'Unknown agent'}  {link.get('state') or 'Unknown'}")
    snapshot = task['workspace_snapshot']
    lines.append('Git snapshot: ' + ('not captured' if not snapshot else
                 snapshot.get('error') or f"{snapshot.get('file_count', 0)} changed paths · " +
                 ('within output limits' if snapshot.get('complete') else 'partial')))
    return '\n'.join(lines)


def run_task_command(args, parser):
    if not args.task_action:
        parser.print_help()
        return 0
    structured = getattr(args, 'json', False) or getattr(args, 'format', '') == 'json'
    try:
        store = TaskStore(args.tasks_db)
        action = args.task_action
        revision = getattr(args, 'expected_revision', None)
        if action == 'list':
            entries = store.list(args.all, offset=args.offset)
            total = store.count(args.all)
            next_offset = args.offset + len(entries) if args.offset + len(entries) < total else None
            result = {'format': 'agenturm-task-list-v1', 'tasks': entries, 'limit': 1000,
                      'offset': args.offset, 'total': total, 'next_offset': next_offset, 'truncated': next_offset is not None}
            text = '\n'.join(f"{task['id']}\t{'archived' if task['archived'] else 'saved'}\t{task['title']}" for task in result['tasks'])
            if not text:
                text = ('No tasks at this offset. Use `agenturm task list --offset 0` to return to the first page.' if total else
                        'No saved tasks. Create one with `agenturm task create --title TITLE --workspace PATH`.')
            if next_offset is not None and not structured:
                print(f'Showing {len(entries):,} of {total:,} tasks. Read more with --offset {next_offset}.', file=sys.stderr)
        elif action == 'create':
            result = store.create(args.title, args.workspace, _notes(args) or '')
            if args.snapshot:
                result = store.snapshot(result['id'], expected_revision=result['revision'])
                issue = result['workspace_snapshot'].get('error')
                if issue and not structured:
                    print('Task saved. Workspace capture: ' + issue, file=sys.stderr)
            text = result['id']
        elif action == 'show':
            result = store.get(args.task_id)
            text = _show(result)
        elif action == 'update':
            note = _notes(args)
            if args.title is None and args.workspace is None and note is None:
                raise TaskError('Choose --title, --workspace, --notes, or --notes-file to update.')
            result = store.update(args.task_id, title=args.title, workspace=args.workspace, notes=note, expected_revision=revision)
            text = result['id']
        elif action == 'link':
            if args.evidence_file:
                if args.session_id or args.agent_id or args.trace_id or args.runtime or args.state != 'Unknown':
                    raise TaskError('Use either --evidence-file or individual link fields.')
                try:
                    evidence = json.loads(_read(args.evidence_file, 128 * 1024))
                except (json.JSONDecodeError, RecursionError) as exc:
                    raise TaskError('Evidence file must contain one JSON object.') from exc
            else:
                evidence = {'session_id': args.session_id, 'agent_id': args.agent_id,
                            'trace_id': args.trace_id, 'runtime': args.runtime, 'state': args.state}
            result = store.link(args.task_id, evidence, expected_revision=revision)
            text = result['id']
        elif action == 'unlink':
            result = store.unlink(args.task_id, args.link_key, expected_revision=revision)
            text = result['id']
        elif action == 'snapshot':
            result = store.snapshot(args.task_id, expected_revision=revision)
            text = _show(result)
        elif action == 'archive':
            result = store.update(args.task_id, archived=not args.restore, expected_revision=revision)
            text = result['id']
        else:
            task = store.get(args.task_id)
            if args.output:
                result = {'format': 'agenturm-task-export-v1', 'files': export_handoff(task, args.output)}
                text = '\n'.join(result['files'])
            else:
                # Streaming output is the primary result; nothing else goes to stdout.
                print(render_handoff(task, 'json' if structured else args.format), end='', flush=True)
                return 0
        print(json.dumps(result, ensure_ascii=False, indent=2) if structured else text, flush=True)
        return 0
    except BrokenPipeError:
        # Consumers such as head may stop early; suppress Python's shutdown flush.
        import os
        with open(os.devnull, 'w') as sink:
            os.dup2(sink.fileno(), sys.stdout.fileno())
        return 0
    except (TaskError, OSError, sqlite3.Error) as exc:
        code = 'conflict' if isinstance(exc, TaskConflict) else 'already_exists' if isinstance(exc, FileExistsError) else 'invalid_input' if isinstance(exc, TaskError) else 'storage_error'
        message = str(exc)
        if structured:
            print(json.dumps({'format': 'agenturm-task-error-v1', 'error': {'code': code, 'message': message}}), file=sys.stderr)
        else:
            print('Agenturm: ' + message, file=sys.stderr)
        return 3 if isinstance(exc, TaskConflict) else 2
