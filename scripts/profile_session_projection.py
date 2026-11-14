"""Diagnostic SQL/CPU attribution on a verified synthetic session fixture.

Instrumentation changes timings; this is not a performance comparison or budget gate.
"""
import argparse
from contextlib import contextmanager
import cProfile
import io
import json
from pathlib import Path
import pstats
import re
import sys
import time
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.core import Store
from observatory.storage_sessions import session_projection
from tests.fixtures.session_projection_before import session_projection as baseline
from tests.fixtures.store_sessions_before import sessions as baseline_sessions


class Cursor:
    def __init__(self, cursor, row):
        self.cursor, self.row = cursor, row
    def fetchall(self):
        started = time.perf_counter_ns()
        result = self.cursor.fetchall()
        self.row['fetch_ns'] += time.perf_counter_ns() - started
        self.row['rows'] += len(result)
        return result
    def fetchone(self):
        started = time.perf_counter_ns()
        result = self.cursor.fetchone()
        self.row['fetch_ns'] += time.perf_counter_ns() - started
        self.row['rows'] += result is not None
        return result
    def __iter__(self):
        return iter(self.fetchall())
    def __getattr__(self, name):
        return getattr(self.cursor, name)


class Connection:
    def __init__(self, db, totals):
        self.db, self.totals = db, totals
    def execute(self, sql, args=()):
        key = re.sub(r'\s+', ' ', sql).strip()
        row = self.totals.setdefault(key, dict(execute_ns=0, fetch_ns=0, calls=0, rows=0))
        started = time.perf_counter_ns()
        cursor = self.db.execute(sql, args)
        row['execute_ns'] += time.perf_counter_ns() - started
        row['calls'] += 1
        return Cursor(cursor, row)
    def __getattr__(self, name):
        return getattr(self.db, name)


def profile(path, calls=3):
    if json.loads(path.with_suffix('.fixture.json').read_text()).get('synthetic') is not True:
        raise ValueError('Use an explicitly synthetic prepared copy.')
    store = Store(path)
    report = {}
    try:
        for name, function in [('baseline', baseline), ('current', session_projection)]:
            totals = {}
            original = store._connect
            @contextmanager
            def connection():
                with original() as db:
                    yield Connection(db, totals)
            with patch('observatory.storage_sessions.session_projection', function):
                read = lambda: baseline_sessions(store, limit=500) if function is baseline else store.sessions(limit=500)
                read()
                profiler = cProfile.Profile()
                started = time.perf_counter_ns()
                with patch.object(store, '_connect', connection):
                    profiler.enable()
                    for _ in range(calls):
                        read()
                    profiler.disable()
                wall_ms = (time.perf_counter_ns() - started) / 1e6
            text = io.StringIO()
            pstats.Stats(profiler, stream=text).strip_dirs().sort_stats('cumulative').print_stats(30)
            queries = sorted([dict(sql=sql, ms=(row['execute_ns'] + row['fetch_ns']) / 1e6,
                                   **row) for sql, row in totals.items()], key=lambda row: row['ms'], reverse=True)
            report[name] = dict(calls=calls, instrumented_wall_ms=wall_ms,
                               queries=queries, cpu_profile=text.getvalue())
        return dict(diagnostic_only=True, synthetic=True, fixture=str(path), **report)
    finally:
        store.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--db', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = profile(args.db)
    with args.output.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    for name in ('baseline', 'current'):
        print(name, result[name]['instrumented_wall_ms'])
        for row in result[name]['queries'][:8]:
            print(round(row['ms'], 2), 'ms', row['calls'], 'calls', row['rows'], 'rows', row['sql'][:140])
        print(result[name]['cpu_profile'])
