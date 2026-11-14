"""Non-interactive saved-task commands; no receiver or agent process required."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sqlite3
import sys

from .saved_tasks import (BRIEF_FIELDS, MAX_NOTES, TaskConflict, TaskError, TaskStore,
    backup_task_database, default_path, export_handoff, render_handoff)


def _source_api(args):
    from .terminal import Api,CockpitError
    try:return Api(args.endpoint,args.token)
    except CockpitError as exc:raise TaskError(str(exc)) from exc


def add_task_parser(subparsers):
    parser = subparsers.add_parser('task', help='save tasks and prepare local handoffs',
        description='Saved tasks work offline. Source-backed drafting/approval revalidates the exact receiver.',
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

    draft=action('draft','prepare an editable source-backed draft without saving a task')
    draft.add_argument('--review-file',type=Path,required=True,help='frozen exact Review JSON')
    draft.add_argument('--endpoint',required=True,help='revalidate this exact receiver and source capture policy')
    draft.add_argument('--token',help='receiver token; normal local token discovery also applies')
    draft.add_argument('--workspace',help='explicit user mapping to a local checkout; receiver cwd is not local proof')
    draft.add_argument('--task-id',help='include an existing task’s current human decisions and revision')
    draft.add_argument('--format',choices=('json','markdown','html'),default='json')
    draft.add_argument('-o','--output',type=Path,help='new .json/.md/.html artifact; Markdown/HTML companions are written')
    approve=action('approve-draft','explicitly save reviewed draft text, preserving authored fields')
    approve.add_argument('draft_file',type=Path)
    approve.add_argument('--endpoint',required=True)
    approve.add_argument('--token')
    approve.add_argument('--title',help='title for a new task; an existing task title is never changed')
    approve.add_argument('--workspace',default='',help='local workspace for a new task only')
    guard(approve)
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
    link.add_argument('--run-start-ns', type=int, help='positive recorded native lifecycle start; never use latest-update time')
    link.add_argument('--run-end-ns', type=int, help='recorded lifecycle end needed to assign native check observations')
    link.add_argument('--runtime', default='', help='agent name, such as Codex or Claude Code')
    link.add_argument('--receiver', default='', help='exact receiver namespace from review')
    link.add_argument('--producer', default='', help='exact producer namespace from review')
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
    handoff.add_argument('--endpoint',help='revalidate source-backed continuity/Git sections; omitted sections stay withheld offline')
    handoff.add_argument('--token')
    handoff.add_argument('--include-decision-history', action='store_true', help='include superseded decisions in stdout handoff')
    brief = action('brief', 'show or update structured task fields', True)
    brief.add_argument('--file', type=Path, help='bounded UTF-8 JSON object of structured brief fields')
    for field in BRIEF_FIELDS:
        brief.add_argument('--' + field.replace('_', '-'), help='user-owned text, up to 4,000 characters')
    guard(brief)
    decision = action('decide', 'append an accepted or rejected decision version', True)
    decision.add_argument('--kind', choices=('accepted', 'rejected'), required=True)
    decision_text = decision.add_mutually_exclusive_group(required=True)
    decision_text.add_argument('--text')
    decision_text.add_argument('--text-file', type=Path, help='UTF-8 decision or explicit excerpt, at most 4,000 characters')
    decision.add_argument('--rationale', default='')
    decision.add_argument('--scope', default='')
    decision.add_argument('--supersedes', help='existing decision ID in this task')
    decision.add_argument('--source-file', type=Path, help='explicit bounded JSON reference for an excerpt; metadata-only sources are refused')
    guard(decision)
    action('decisions', 'show all immutable decision versions', True)
    backup = action('backup', 'create a consistent private SQLite backup without overwriting')
    backup.add_argument('destination', type=Path)
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
        if args.task_action == 'backup':
            path = backup_task_database(args.tasks_db or default_path(), args.destination)
            print(json.dumps({'format': 'agenturm-task-backup-v1', 'path': path}) if structured else path, flush=True)
            return 0
        if args.task_action=='draft':
            from .review_continuity import source_scope,bind_workspace,capture_review_workspace,freeze_workspace,editable_draft,policy_checker,draft_formats
            from .terminal import Api
            from .work_receipts import export_receipt
            try:projection=json.loads(_read(args.review_file,3*1024*1024))
            except (ValueError,RecursionError) as exc:raise TaskError('Review file must contain one frozen Review object.') from exc
            source_scope(projection);check=policy_checker(_source_api(args),projection);check()
            if args.workspace:
                binding=bind_workspace(projection,args.workspace)
                projection=freeze_workspace(projection,capture_review_workspace(projection,binding))
                check()
            task=TaskStore(args.tasks_db).get(args.task_id) if args.task_id else None
            draft=editable_draft(projection,task);formats=draft_formats(draft);check()
            if args.output:
                files=export_receipt(args.output,formats)
                print(json.dumps({'format':'agenturm-continuity-draft-export-v1','files':files}) if structured else '\n'.join(files),flush=True)
            else:print(formats['json' if structured else args.format],end='',flush=True)
            return 0
        store = TaskStore(args.tasks_db)
        action = args.task_action
        revision = getattr(args, 'expected_revision', None)
        approved_policy=None
        if action=='approve-draft':
            from .review_continuity import validate_draft,policy_checker
            from .terminal import Api
            try:draft=validate_draft(json.loads(_read(args.draft_file,3*1024*1024)))
            except (ValueError,RecursionError) as exc:raise TaskError(str(exc)) from exc
            check=policy_checker(_source_api(args),draft['review']);check()
            if draft.get('task_id'):
                expected=revision if revision is not None else draft.get('task_revision')
                result=store.approve_continuity(draft['task_id'],draft,expected_revision=expected,source_allowed=check())
            else:
                result=store.create(args.title or draft['title'],args.workspace,'',continuity_draft=draft,source_allowed=check())
            from .review_continuity import content_scopes
            allowed_scopes=content_scopes(draft['review'])
            approved_policy=lambda scope:scope in allowed_scopes and check()
            text=result['id']
        elif action == 'list':
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
                if args.session_id or args.agent_id or args.trace_id or args.run_start_ns is not None or args.run_end_ns is not None or args.runtime or args.receiver or args.producer or args.state != 'Unknown':
                    raise TaskError('Use either --evidence-file or individual link fields.')
                try:
                    evidence = json.loads(_read(args.evidence_file, 3 * 1024 * 1024))
                except (json.JSONDecodeError, RecursionError) as exc:
                    raise TaskError('Evidence file must contain one JSON object.') from exc
            else:
                evidence = {'session_id': args.session_id, 'agent_id': args.agent_id,
                            'trace_id': args.trace_id, 'runtime': args.runtime, 'state': args.state,
                            'receiver': args.receiver, 'producer': args.producer,
                            'run_start_ns': args.run_start_ns, 'run_end_ns': args.run_end_ns}
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
        elif action == 'brief':
            values = {field: getattr(args, field) for field in BRIEF_FIELDS if getattr(args, field) is not None}
            if args.file:
                if values:
                    raise TaskError('Use --file or individual brief fields.')
                try:
                    values = json.loads(_read(args.file, 128 * 1024))
                except (json.JSONDecodeError, RecursionError) as exc:
                    raise TaskError('Brief file must contain one JSON object.') from exc
                if not isinstance(values, dict):
                    raise TaskError('Brief file must contain one JSON object.')
            result = store.update_brief(args.task_id, values, expected_revision=revision) if values else store.get(args.task_id)
            text = '\n\n'.join(field.replace('_', ' ').title() + ':\n' + value for field, value in result['brief'].items())
        elif action == 'decide':
            source = None
            if args.source_file:
                try:
                    source = json.loads(_read(args.source_file, 32 * 1024))
                except (json.JSONDecodeError, RecursionError) as exc:
                    raise TaskError('Source file must contain one JSON reference object.') from exc
            decision_text = _read(args.text_file, 16000) if args.text_file else args.text
            result = store.add_decision(args.task_id, kind=args.kind, text=decision_text, rationale=args.rationale,
                scope=args.scope, source=source, supersedes_id=args.supersedes, expected_revision=revision)
            text = result['decisions'][-1]['id']
        elif action == 'decisions':
            task = store.get(args.task_id)
            result = {'format': 'agenturm-task-decisions-v1', 'task_id': task['id'],
                      'revision': task['revision'], 'decisions': task['decisions']}
            replaced = {item.get('supersedes_id') for item in task['decisions']}
            text = '\n\n'.join(item['id'] + ' · ' + item['kind'] +
                (' · superseded' if item['id'] in replaced else ' · current') + '\n' + item['text'] +
                '\nRationale: ' + item['rationale'] for item in task['decisions']) or 'No decisions saved.'
        elif action == 'backup':
            result = {'format': 'agenturm-task-backup-v1', 'path': store.backup(args.destination)}
            text = result['path']
        else:
            task = store.get(args.task_id)
            source_policy=None
            if args.endpoint:
                from .terminal import Api
                from .review_continuity import policy_checker
                api=_source_api(args)
                def source_policy(scope):
                    # Only scopes already validated and persisted by approval/linking
                    # are used here; the checker does not infer a new source owner.
                    from .workflow_evidence import ExecutionScope
                    from .review_facts import review_capture_policy
                    from types import SimpleNamespace
                    health=api.get('/api/health')
                    if health.get('receiver_id')!=scope.get('receiver'):return False
                    return review_capture_policy({},SimpleNamespace(receiver_id=health['receiver_id'],local_status=health.get('local_agents') or {}),ExecutionScope(**scope)).get('capture_content') is not False
            if args.output:
                result = {'format': 'agenturm-task-export-v1', 'files': export_handoff(task, args.output,
                    include_decision_history=args.include_decision_history,source_policy=source_policy)}
                text = '\n'.join(result['files'])
            else:
                # Streaming output is the primary result; nothing else goes to stdout.
                print(render_handoff(task, 'json' if structured else args.format,
                    include_decision_history=args.include_decision_history,source_policy=source_policy), end='', flush=True)
                return 0
        if structured and isinstance(result,dict) and 'links' in result and 'workspace_snapshot' in result:
            from .saved_tasks import continuity_view
            result=continuity_view(result,approved_policy)
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
