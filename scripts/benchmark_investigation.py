"""Synthetic typed-search build/read cost; never reads personal telemetry.

Run from a clean checkout: python scripts/benchmark_investigation.py --output result.json
This measures this index only, not whole-monitor CPU, startup or user outcomes.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import statistics
import sys
from tempfile import TemporaryDirectory
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.storage_investigation import initialize, read_page, replace_owner

SCHEMA = '''CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT);
CREATE TABLE activity_index_v1(event_key TEXT PRIMARY KEY,kind TEXT,owner_key TEXT,stamp INTEGER,
trace_id TEXT,session_id TEXT,agent_id TEXT,service TEXT,domain TEXT,action TEXT,result TEXT,turn_id TEXT,payload TEXT);'''


def fixture(count):
    for index in range(count):
        source = f'{index:064x}'
        row = dict(id='a' * 32 + ':' + source, source='log', source_id=source,
            trace_id='a' * 32, span_id=None, turn_id=None,
            service=('codex', 'claude', 'opencode')[index % 3],
            agent_id=None if index % 17 == 0 else 'synthetic-worker-' + str(index % 12),
            session_id='synthetic-session-' + str(index % 12), time_unix_nano=index + 1,
            result='success', domain='file', action='edit', subject=f'src/module_{index % 90}.py', detail='')
        if index % 3 == 1:
            row.update(domain='shell', action='execute', subject='Bash', result='error' if index % 5 == 0 else 'success',
                operation_evidence=dict(version=1, commands=[('command', f'pytest tests/module_{index % 90}.py')],
                    inputs=[], outputs=[], exits=[('exit', 1 if index % 5 == 0 else 0)], cwd='/synthetic', tool='Bash'))
        elif index % 3 == 2:
            row.update(domain='message', action='user', subject='User instruction',
                detail=f'Keep the public API stable while updating synthetic module {index % 90}.')
        yield row


def build(path, rows, indexed):
    db = sqlite3.connect(path)
    db.row_factory = sqlite3.Row
    db.execute('PRAGMA journal_mode=WAL')
    db.executescript(SCHEMA)
    if indexed:
        initialize(db)
    start = time.perf_counter_ns()
    for offset in range(0, len(rows), 100):
        with db:
            for row in rows[offset:offset + 100]:
                key = row['id']
                # Same compact Activity payload in both cases; only index work differs.
                payload = {key: value for key, value in row.items() if key != 'operation_evidence'}
                payload['detail'] = ''
                db.execute('INSERT INTO activity_index_v1 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
                    (key, 'log', key, row['time_unix_nano'], row['trace_id'], row['session_id'], row['agent_id'],
                     row['service'], row['domain'], row['action'], row['result'], row['turn_id'], json.dumps(payload)))
                if indexed:
                    replace_owner(db, 'log', key, [(key, row)])
    elapsed = (time.perf_counter_ns() - start) / 1e6
    db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    return db, elapsed, path.stat().st_size


def benchmark(count=6000, samples=3):
    rows = list(fixture(count))
    results = []
    with TemporaryDirectory(prefix='agenturm-investigation-bench-') as directory:
        for sample in range(samples):
            result = {}
            # Alternate build order; this is still a local index microbenchmark.
            for indexed in ((False, True) if sample % 2 == 0 else (True, False)):
                path = Path(directory) / f'{sample}-{indexed}.db'
                db, elapsed, size = build(path, rows, indexed)
                result['indexed' if indexed else 'baseline'] = dict(build_ms=elapsed, database_bytes=size)
                if indexed:
                    reads = {}
                    for kind, query in [('path', 'src/module_0.py'), ('command', 'pytest tests'), ('instruction', 'public API')]:
                        times = []
                        for _ in range(20):
                            start = time.perf_counter_ns()
                            with db:
                                db.execute('BEGIN')
                                response = read_page(db, kind, query, receiver='synthetic-receiver')
                            times.append((time.perf_counter_ns() - start) / 1e6)
                        reads[kind] = dict(median_ms=statistics.median(times),
                            p95_ms=sorted(times)[18], samples_ms=times, returned=len(response['matches']),
                            has_more=response['next_cursor'] is not None)
                    result['reads'] = reads
                    result['terms'] = db.execute('SELECT COUNT(*) FROM investigation_terms_v1').fetchone()[0]
                db.close()
            results.append(result)
    scripts = [Path(__file__), ROOT / 'observatory/investigation.py', ROOT / 'observatory/storage_investigation.py', ROOT / 'observatory/recorded_paths.py']
    return dict(format='agenturm-investigation-benchmark-v1', synthetic=True, events=count,
        samples=samples, sqlite_version=sqlite3.sqlite_version,
        source_hashes={str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in scripts},
        scope='Isolated SQLite Activity write/index/query microbenchmark; excludes receiver, UI, native discovery and raw-source hydration.',
        runs=results, medians=dict(
            baseline_build_ms=statistics.median(row['baseline']['build_ms'] for row in results),
            indexed_build_ms=statistics.median(row['indexed']['build_ms'] for row in results),
            baseline_database_bytes=statistics.median(row['baseline']['database_bytes'] for row in results),
            indexed_database_bytes=statistics.median(row['indexed']['database_bytes'] for row in results)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--events', type=int, default=6000)
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not 30 <= args.events <= 30000 or not 1 <= args.samples <= 7:
        parser.error('Use 30..30000 events and 1..7 samples.')
    result = benchmark(args.events, args.samples)
    with args.output.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps(result['medians']))
