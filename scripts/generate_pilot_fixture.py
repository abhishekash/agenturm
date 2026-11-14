"""Create an explicitly synthetic pilot database; no personal transcripts or discovery.

The destination must be new. This starts no receiver and launches no agent.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.agent_hierarchy import canonical_agent_id
from observatory.core import Store, normalize_native
from observatory.storage_investigation import initialize, replace_owner


def records(anchor):
    owners = {name: canonical_agent_id('Synthetic', 'pilot-v1', name) for name in ('review', 'worker', 'overlap')}
    trace = 'a' * 32
    cases = [
        ('codex', 'review', 'message', 'user', 'Keep the public API stable.', 'Keep the public API stable.', 'observed', {}),
        ('codex', 'review', 'shell', 'execute', 'pytest tests/retry.py', '', 'success',
            {'tool.name': 'Bash', 'process.command_line': 'pytest tests/retry.py', 'process.working_directory': '/synthetic/worktree-a', 'process.exit.code': 0}),
        ('codex', 'review', 'file', 'edit', 'src/retry.py', '', 'success', {}),
        ('claude', 'worker', 'shell', 'execute', 'python migrate.py', '', 'error',
            {'tool.name': 'Bash', 'process.command_line': 'python migrate.py', 'process.working_directory': '/synthetic/worktree-a', 'process.exit.code': 1, 'stderr': 'Synthetic migration rejected'}),
        ('opencode', 'overlap', 'file', 'rename', 'src/新しい.py', json.dumps({'old_path': 'src/古い.py', 'new_path': 'src/新しい.py'}, ensure_ascii=False), 'success', {}),
        ('opencode', 'overlap', 'file', 'edit', 'src/retry.py', '', 'unknown', {}),
        ('claude', 'worker', 'message', 'assistant', 'All checks passed.', 'All checks passed. This is a synthetic assistant claim.', 'observed', {}),
        ('codex', 'review', 'agent', 'waiting', 'Approval requested', 'Synthetic explicit request; no approval is performed.', 'observed', {'agent.waiting_for': 'Approval'}),
        ('sdk', None, 'message', 'user', 'Preserve the retry budget.', 'Preserve the retry budget.', 'observed', {}),
    ]
    for index, (service, owner, domain, action, subject, body, result, extra) in enumerate(cases):
        attrs = {'agent.activity.domain': domain, 'agent.activity.action': action,
                 'agent.activity.subject': subject, 'agent.activity.result': result, **extra}
        if owner:
            attrs.update({'agent.id': owners[owner], 'session.id': 'synthetic-' + owner})
        if service == 'opencode':
            attrs['agent.turn.id'] = 'b' * 32
        event_trace = ('c' if index == 4 else 'd') * 32 if service == 'opencode' else trace
        yield dict(trace_id=event_trace, span_id=None, time_unix_nano=anchor + index,
            service=service, event_name='agent.activity', body=body, attributes=attrs,
            resource_attributes={}, scope_name='synthetic.pilot', severity_number=17 if result == 'error' else 9,
            severity_text='ERROR' if result == 'error' else 'INFO')


def create(path):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(descriptor)
    store = Store(path)
    try:
        anchor = time.time_ns() - 1000000000
        owner = canonical_agent_id('Synthetic', 'pilot-v1', 'overlap')
        store.ingest([normalize_native(dict(trace_id=letter * 32, span_id=letter * 16,
            name='agent.run', start_unix_nano=anchor + offset, end_unix_nano=anchor + offset + 1,
            status='OK', attributes={'agent.id': owner, 'session.id': 'synthetic-overlap',
                'agent.turn.id': 'b' * 32, 'agent.runtime': 'OpenCode',
                'gen_ai.usage.input_tokens': 0, 'gen_ai.usage.output_tokens': 0}, events=[],
            scope_name='observatory.opencode'), 'opencode') for letter, offset in [('c', 4), ('d', 5)]])
        store.ingest_logs(list(records(anchor)))
        # This explicit fixture generator also works before receiver hook wiring.
        # No history is scanned: the generated corpus has exactly nine records.
        page = store.activity_page(limit=100)
        with store._connect() as db:
            initialize(db)
            for row in page['activity']:
                replace_owner(db, 'log', row['source_id'], [(row['id'], row)])
            db.execute('INSERT OR REPLACE INTO schema_meta VALUES(?,?)', ('investigation_terms_v1', '1'))
        return dict(synthetic=True, participant_count=0, records=len(page['activity']),
            database=str(path), note='No live Claude/Pi/native process evidence or participant results. Start a dedicated receiver with local discovery disabled if needed.')
    finally:
        store.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(create(args.output), ensure_ascii=False, indent=2))
