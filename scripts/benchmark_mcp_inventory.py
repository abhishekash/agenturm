"""Paired complete MCP endpoint reads on an explicitly synthetic private copy.

Prepare outside the quiet interval, commit source, then coordinate all local
implementation agents before measure. No native discovery, personal paths or
provider calls are used. Endpoint reads exclude receiver HTTP and UI work.
"""
from contextlib import closing
from datetime import datetime, timezone
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import sqlite3
import statistics
import subprocess
import sys
from tempfile import TemporaryDirectory
import threading
import time
from types import MethodType
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.core import Store, normalize_native
from scripts.profile_mcp_inventory import prepare as prepare_copy
from tests.fixtures.mcp_page_before import mcp_page as baseline

current = Store._mcp_page
SOURCES = ('observatory/core.py', 'observatory/storage_mcp.py', 'observatory/storage_activity.py',
           'scripts/benchmark_mcp_inventory.py', 'scripts/profile_mcp_inventory.py',
           'tests/fixtures/mcp_page_before.py')


def file_digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def revision():
    dirty = subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal',
                                     '--', 'observatory', 'scripts', 'tests/fixtures/mcp_page_before.py'],
                                    cwd=ROOT, text=True)
    return dict(head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                dirty=dirty, hashes={name: file_digest(ROOT / name) for name in SOURCES})


def inventory(db):
    counts = {table: db.execute('SELECT COUNT(*) FROM ' + table).fetchone()[0]
              for table in ('spans', 'logs', 'trace_activity', 'live_run_stats_v1', 'live_summaries_v1')}
    bounds = db.execute('SELECT MIN(last_ns),MAX(last_ns) FROM trace_activity').fetchone()
    return dict(counts=counts, first_ns=bounds[0], last_ns=bounds[1],
                duration_days=(bounds[1]-bounds[0])/86400e9 if bounds[0] is not None else 0)


def named_indexes(db):
    return dict(db.execute("SELECT name,sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL"))


def prepare(source, destination):
    result = prepare_copy(source, destination)
    with TemporaryDirectory(prefix='agenturm-mcp-production-indexes-') as directory:
        fresh = Store(Path(directory) / 'schema.db')
        try:
            with fresh._connect() as db:
                expected = named_indexes(db)
        finally:
            fresh.close()
    changes = []
    with closing(sqlite3.connect(destination)) as db:
        actual = named_indexes(db)
        with db:
            for name, sql in actual.items():
                if expected.get(name) != sql:
                    db.execute('DROP INDEX "' + name.replace('"', '""') + '"')
                    changes.append(dict(action='drop', name=name, sql=sql))
            for name, sql in expected.items():
                if actual.get(name) != sql:
                    db.execute(sql)
                    changes.append(dict(action='create', name=name, sql=sql))
        if named_indexes(db) != expected:
            raise ValueError('Private synthetic copy does not match production named index manifest.')
        result.update(inventory(db))
    result.update(database_sha256=file_digest(destination),
                  index_normalization=dict(exact_production_manifest=True, changes=changes, manifest=expected))
    destination.with_suffix('.fixture.json').write_text(json.dumps(result, indent=2))
    return result


def read(store, function):
    original = store._mcp_page
    store._mcp_page = MethodType(function, store)
    try:
        return store.mcp_page(limit=100)
    finally:
        store._mcp_page = original


def summary(values):
    return dict(samples_ms=values, median_ms=statistics.median(values),
                p95_ms=sorted(values)[math.ceil(len(values)*.95)-1], maximum_ms=max(values))


def paired(store, samples):
    results = dict(baseline=[], current=[])
    for index in range(samples):
        order = [('baseline', baseline), ('current', current)]
        if index % 2:
            order.reverse()
        for name, function in order:
            started = time.perf_counter_ns()
            value = read(store, function)
            results[name].append((time.perf_counter_ns()-started)/1e6)
            if not value['calls'] or value['summary']['calls'] < len(value['calls']):
                raise ValueError('Synthetic MCP inventory unexpectedly empty or incoherent.')
    return {name: summary(values) for name, values in results.items()}


def measure(path, samples):
    source_before = revision()
    if source_before['dirty']:
        raise ValueError('Commit runtime/harness source before controlled measurement.')
    fixture = json.loads(path.with_suffix('.fixture.json').read_text())
    if (fixture.get('synthetic') is not True or fixture.get('destination') != str(path)
            or not fixture.get('index_normalization', {}).get('exact_production_manifest')
            or file_digest(path) != fixture.get('database_sha256')):
        raise ValueError('Use a newly prepared unchanged synthetic production-index copy.')
    store = Store(path, async_derived_refresh=True)
    try:
        deadline = time.monotonic()+15
        while not store.index_status()['ready']:
            if store.last_derived_error or time.monotonic()>deadline:
                raise ValueError('Finish synthetic index preparation before measurement.')
            time.sleep(.02)
        with store._connect() as db:
            if inventory(db)['counts'] != fixture['counts']:
                raise ValueError('Synthetic fixture inventory changed since preparation.')
        equivalent = read(store, baseline) == read(store, current)
        if not equivalent:
            raise ValueError('Frozen entire-method MCP result comparison failed.')
        for _ in range(3):
            read(store, baseline)
            read(store, current)
        started = time.monotonic()
        interval = dict(started_utc=datetime.now(timezone.utc).isoformat(), started_monotonic=started)
        idle = paired(store, samples)
        stop = threading.Event()
        accepted = threading.Event()
        writes, errors = [], []
        trace = uuid.uuid4().hex
        def writer():
            index = 0
            while not stop.is_set():
                index += 1
                stamp = time.time_ns()
                span = normalize_native(dict(trace_id=trace, span_id=f'{index:016x}',
                    name='MCP synthetic search', start_unix_nano=stamp, end_unix_nano=stamp+1,
                    attributes={'session.id': 'mcp-synthetic-live', 'agent.id': 'synthetic-live-worker',
                        'mcp.method.name': 'tools/call', 'mcp.server.name': 'synthetic-live',
                        'mcp.tool.name': 'search', 'input.value': {'query': 'synthetic'},
                        'output.value': {'count': 2}}, events=[]), 'synthetic')
                try:
                    result = store.ingest([span])
                    if result != 1:
                        raise ValueError('Synthetic MCP write was not accepted.')
                    writes.append(index)
                    accepted.set()
                except Exception as exc:
                    errors.append(type(exc).__name__ + ': ' + str(exc))
                stop.wait(.1)
        thread = threading.Thread(target=writer, name='synthetic-mcp-writer')
        thread.start()
        try:
            if not accepted.wait(10):
                raise ValueError('Synthetic MCP writer did not accept its first record.')
            prior = len(writes)
            under_writes = paired(store, samples)
            writes_during = len(writes)-prior
        finally:
            stop.set()
            thread.join(15)
        if thread.is_alive() or errors or not writes_during:
            raise ValueError('Synthetic MCP workload did not drain cleanly: ' + repr(errors))
        interval.update(finished_utc=datetime.now(timezone.utc).isoformat(),
                        finished_monotonic=time.monotonic(), elapsed_seconds=time.monotonic()-started)
        # Derived repair can lag ingestion; require it to finish outside timing.
        deadline = time.monotonic()+15
        while not store.index_status()['ready']:
            if store.last_derived_error or time.monotonic()>deadline:
                raise ValueError('Derived MCP workload did not catch up after timing.')
            time.sleep(.02)
        source_after = revision()
        if source_before != source_after or source_after['dirty']:
            raise ValueError('Runtime or harness changed during measurement.')
        return dict(format='agenturm-mcp-inventory-cost-v1', synthetic=True, fixture=fixture,
            python=platform.python_version(), sqlite_version=sqlite3.sqlite_version,
            entire_method_equivalent=equivalent, source_before=source_before, source_after=source_after,
            measurement=interval, idle=idle, under_writes=under_writes,
            write_batches=len(writes), writes_during_read_phase=writes_during, write_errors=errors,
            derived_caught_up=True,
            protocol='Alternating frozen original/current full Store.mcp_page(limit=100), same synthetic production-index file/process; 3warmups each, paced accepted MCP writes every100ms. No HTTP/UI/native discovery measured.',
            resource_target=dict(mcp_under_writes_p95_under100ms=under_writes['current']['p95_ms']<100,
                cold_readiness='not measured', complete_monitor_cpu='not measured', complete_monitor_memory='not measured'))
    finally:
        store.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    setup = commands.add_parser('prepare')
    setup.add_argument('--source', type=Path, required=True)
    setup.add_argument('--verified-synthetic-source', action='store_true', required=True)
    setup.add_argument('--db', type=Path, required=True)
    measurement = commands.add_parser('measure')
    measurement.add_argument('--db', type=Path, required=True)
    measurement.add_argument('--samples', type=int, default=20)
    measurement.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'prepare':
        print(json.dumps(prepare(args.source, args.db), indent=2))
    else:
        if not 5 <= args.samples <= 40:
            parser.error('Use5..40 paired samples per condition.')
        result = measure(args.db, args.samples)
        with args.output.open('x') as stream:
            json.dump(result, stream, indent=2)
        print(json.dumps({mode: {name: value['p95_ms'] for name, value in result[mode].items()}
                          for mode in ('idle', 'under_writes')}))
