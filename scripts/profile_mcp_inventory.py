"""Diagnostic-only MCP SQL/CPU attribution on a new verified synthetic copy."""
from contextlib import closing, contextmanager
from datetime import datetime, timezone
import argparse
import cProfile
import io
import json
import os
from pathlib import Path
import pstats
import re
import sqlite3
import sys
import time
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.core import Store


class Cursor:
    def __init__(self, cursor, row):
        self.cursor, self.row = cursor, row
    def fetchall(self):
        started = time.perf_counter_ns()
        values = self.cursor.fetchall()
        self.row['fetch_ns'] += time.perf_counter_ns() - started
        self.row['rows'] += len(values)
        return values
    def fetchone(self):
        started = time.perf_counter_ns()
        value = self.cursor.fetchone()
        self.row['fetch_ns'] += time.perf_counter_ns() - started
        self.row['rows'] += value is not None
        return value
    def __iter__(self):
        return iter(self.fetchall())
    def __getattr__(self, name):
        return getattr(self.cursor, name)


class Connection:
    def __init__(self, db, totals):
        self.db, self.totals = db, totals
    def execute(self, sql, args=()):
        key = re.sub(r'\s+', ' ', sql).strip()
        row = self.totals.setdefault(key, dict(execute_ns=0, fetch_ns=0, calls=0, rows=0, parameters=len(args)))
        started = time.perf_counter_ns()
        cursor = self.db.execute(sql, args)
        row['execute_ns'] += time.perf_counter_ns() - started
        row['calls'] += 1
        return Cursor(cursor, row)
    def __getattr__(self, name):
        return getattr(self.db, name)


def prepare(source, target):
    # Explicit synthetic source only; native files and personal stores are never discovered.
    descriptor = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(descriptor)
    with closing(sqlite3.connect(source.resolve().as_uri() + '?mode=ro', uri=True)) as origin:
        with closing(sqlite3.connect(target)) as destination:
            origin.backup(destination)
    started = time.monotonic()
    store = Store(target, async_derived_refresh=True)
    try:
        last_note = started
        while not store.index_status()['ready']:
            if store.last_derived_error or time.monotonic() - started > 900:
                raise ValueError('Synthetic migration failed or timed out: ' + str(store.last_derived_error))
            if time.monotonic() - last_note > 30:
                print('Preparing current generated index:', store.migration_status.get('stage'),
                      round(time.monotonic() - started, 1), flush=True)
                last_note = time.monotonic()
            time.sleep(.2)
    finally:
        store.close()
    result = dict(synthetic=True, source=str(source), destination=str(target),
                  preparation_seconds=time.monotonic() - started,
                  note='SQLite read-only backup of operator-verified synthetic fixture; preparation is not a controlled cold-start result.')
    target.with_suffix('.fixture.json').write_text(json.dumps(result, indent=2))
    return result


def profile(path, calls):
    if json.loads(path.with_suffix('.fixture.json').read_text()).get('synthetic') is not True:
        raise ValueError('Use an explicitly synthetic prepared copy.')
    store = Store(path)
    try:
        totals = {}
        original = store._connect
        @contextmanager
        def connection():
            with original() as db:
                yield Connection(db, totals)
        store.mcp_page(limit=100)
        profiler = cProfile.Profile()
        started = time.perf_counter_ns()
        with patch.object(store, '_connect', connection):
            profiler.enable()
            for _ in range(calls):
                result = store.mcp_page(limit=100)
            profiler.disable()
        text = io.StringIO()
        pstats.Stats(profiler, stream=text).strip_dirs().sort_stats('cumulative').print_stats(35)
        queries = sorted([dict(sql=sql, ms=(row['execute_ns'] + row['fetch_ns']) / 1e6, **row)
                          for sql, row in totals.items()], key=lambda row: row['ms'], reverse=True)
        return dict(diagnostic_only=True, synthetic=True, fixture=str(path), calls=calls,
                    measured_utc=datetime.now(timezone.utc).isoformat(),
                    instrumented_wall_ms=(time.perf_counter_ns()-started)/1e6,
                    visible_calls=len(result['calls']), summary=result['summary'],
                    queries=queries, cpu_profile=text.getvalue())
    finally:
        store.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    setup = sub.add_parser('prepare')
    setup.add_argument('--source', type=Path, required=True)
    setup.add_argument('--verified-synthetic-source', action='store_true', required=True)
    setup.add_argument('--db', type=Path, required=True)
    diagnostic = sub.add_parser('profile')
    diagnostic.add_argument('--db', type=Path, required=True)
    diagnostic.add_argument('--calls', type=int, default=3)
    diagnostic.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'prepare':
        print(json.dumps(prepare(args.source, args.db), indent=2))
    else:
        if not 1 <= args.calls <= 10:
            parser.error('Use 1..10 diagnostic calls.')
        result = profile(args.db, args.calls)
        with args.output.open('x') as stream:
            json.dump(result, stream, indent=2)
        print(result['instrumented_wall_ms'], 'ms instrumented', result['visible_calls'], 'visible calls')
        for row in result['queries'][:8]:
            print(round(row['ms'], 2), 'ms', row['calls'], 'calls', row['rows'], 'rows', row['sql'][:160])
        print(result['cpu_profile'])
