"""Paired production Store.sessions reads on an explicitly synthetic new copy.

Prepare outside the quiet timing interval, then measure after coordinating other
local workloads. Never points a Store at the source or imports personal native files.
"""
from __future__ import annotations

import argparse
from contextlib import closing, contextmanager
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import sqlite3
import statistics
import sys
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from observatory.core import Store, normalize_native
import observatory.storage_sessions as current_module
from tests.fixtures.session_projection_before import session_projection as baseline

current = current_module.session_projection


def file_digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def inventory(db):
    counts = {table: db.execute('SELECT COUNT(*) FROM ' + table).fetchone()[0]
              for table in ('spans', 'logs', 'trace_activity', 'live_run_stats_v1', 'live_summaries_v1')}
    bounds = db.execute('SELECT MIN(last_ns),MAX(last_ns) FROM trace_activity').fetchone()
    return dict(counts=counts, first_ns=bounds[0], last_ns=bounds[1],
        duration_days=(bounds[1] - bounds[0]) / 86400e9 if bounds[0] is not None else 0,
        session_indexes=[row[1] for row in db.execute('PRAGMA index_list(live_run_stats_v1)')])


def prepare(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(destination, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(descriptor)
    with closing(sqlite3.connect(source.resolve().as_uri() + '?mode=ro', uri=True)) as origin:
        with closing(sqlite3.connect(destination)) as target:
            origin.backup(target)
    started = time.monotonic()
    store = Store(destination, async_derived_refresh=True)
    try:
        last_note = started
        while not store.index_status()['ready']:
            if store.last_derived_error:
                raise ValueError('Synthetic fixture migration failed: ' + str(store.last_derived_error))
            if time.monotonic() - started > 900:
                raise ValueError('Synthetic fixture migration did not complete in 900 seconds.')
            if time.monotonic() - last_note > 30:
                print(json.dumps({'preparation_stage': store.migration_status.get('stage'),
                                  'elapsed_seconds': time.monotonic() - started}), flush=True)
                last_note = time.monotonic()
            time.sleep(.2)
        with store._connect() as db:
            facts = inventory(db)
        manifest = dict(synthetic=True, source=str(source), destination=str(destination),
            preparation_seconds=time.monotonic() - started, **facts,
            note='Source explicitly verified synthetic by operator; SQLite read-only backup, no source Store or native discovery. Preparation excluded from paired read timings.')
    finally:
        store.close()
    manifest['database_sha256'] = file_digest(destination)
    with destination.with_suffix('.fixture.json').open('x') as stream:
        json.dump(manifest, stream, indent=2)
    return manifest


@contextmanager
def helper(function):
    original = current_module.session_projection
    current_module.session_projection = function
    try:
        yield
    finally:
        current_module.session_projection = original


def summary(values):
    return dict(samples_ms=values, median_ms=statistics.median(values),
                p95_ms=sorted(values)[math.ceil(len(values) * .95) - 1], maximum_ms=max(values))


def paired(store, samples):
    result = dict(baseline=[], current=[])
    for index in range(samples):
        order = [('baseline', baseline), ('current', current)]
        if index % 2:
            order.reverse()
        for name, function in order:
            with helper(function):
                start = time.perf_counter_ns()
                rows = store.sessions(limit=500)
                result[name].append((time.perf_counter_ns() - start) / 1e6)
                if not rows:
                    raise ValueError('Synthetic session corpus unexpectedly became empty.')
    return {key: summary(values) for key, values in result.items()}


def measure(path, samples):
    manifest = json.loads(path.with_suffix('.fixture.json').read_text())
    if manifest.get('synthetic') is not True or manifest.get('destination') != str(path):
        raise ValueError('Use a newly prepared, explicitly synthetic fixture.')
    if file_digest(path) != manifest.get('database_sha256'):
        raise ValueError('Fixture bytes changed after preparation; make a fresh copy for measurement.')
    store = Store(path, async_derived_refresh=True)
    try:
        deadline = time.monotonic() + 15
        while not store.index_status()['ready']:
            if store.last_derived_error or time.monotonic() > deadline:
                raise ValueError('Fixture is not ready; finish preparation before the quiet interval.')
            time.sleep(.02)
        with store._connect() as db:
            if inventory(db)['counts'] != manifest['counts']:
                raise ValueError('Fixture has changed since preparation; make a fresh copy for measurement.')
        for _ in range(3):
            for function in (baseline, current):
                with helper(function):
                    store.sessions(limit=500)
        with store._connect() as db:
            db.execute('BEGIN')
            old = baseline(db, 500)
            new = current(db, 500)
            equivalent = ([dict(row) for row in old[0]], old[1:]) == ([dict(row) for row in new[0]], new[1:])
        if not equivalent:
            raise ValueError('Projection comparison failed on the fixed synthetic snapshot.')
        started = time.monotonic()
        interval = dict(started_utc=datetime.now(timezone.utc).isoformat(), started_monotonic=started)
        idle = paired(store, samples)
        stop = threading.Event()
        writes = []
        errors = []
        live_trace = uuid.uuid4().hex
        first_write = threading.Event()
        def writer():
            index = 0
            while not stop.is_set():
                index += 1
                stamp = time.time_ns()
                span = normalize_native(dict(trace_id=live_trace, span_id=f'{index:016x}', name='fixture live tool',
                    start_unix_nano=stamp, end_unix_nano=stamp + 1, attributes={
                        'tool.name': 'fixture', 'agent.id': 'synthetic-live-worker', 'session.id': 'synthetic-live'}, events=[]), 'synthetic')
                try:
                    store.ingest([span])
                    writes.append(index)
                    first_write.set()
                except Exception as exc:
                    errors.append(type(exc).__name__)
                stop.wait(.1)
        thread = threading.Thread(target=writer, name='synthetic-session-writer')
        thread.start()
        try:
            if not first_write.wait(10):
                raise ValueError('Synthetic writer did not accept its first batch.')
            writes_before_reads = len(writes)
            under_writes = paired(store, samples)
            writes_during_reads = len(writes) - writes_before_reads
        finally:
            stop.set()
            thread.join(timeout=15)
        if thread.is_alive() or errors or not writes_during_reads:
            raise ValueError('Synthetic write workload did not drain cleanly: ' + repr(errors))
        interval.update(finished_utc=datetime.now(timezone.utc).isoformat(), finished_monotonic=time.monotonic(),
                        elapsed_seconds=time.monotonic() - started)
        files = ['observatory/storage_sessions.py', 'observatory/core.py', 'observatory/storage_activity.py',
                 'tests/fixtures/session_projection_before.py', 'scripts/benchmark_session_projection.py']
        return dict(format='agenturm-session-projection-cost-v1', synthetic=True, fixture=manifest,
            python=platform.python_version(), sqlite_version=sqlite3.sqlite_version, same_snapshot_equivalent=equivalent,
            protocol='Alternating old/new production Store.sessions(limit=500) calls; 3 warmups each; same prepared file and process; paced synthetic ingestion every100ms under writes. No receiver/UI/native-discovery work measured.',
            measurement=interval, idle=idle, under_writes=under_writes, write_batches=len(writes),
            writes_during_read_phase=writes_during_reads, write_errors=errors,
            source_hashes={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in files},
            resource_target=dict(session_under_writes_p95_under100ms=under_writes['current']['p95_ms'] < 100,
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
    read = commands.add_parser('measure')
    read.add_argument('--db', type=Path, required=True)
    read.add_argument('--samples', type=int, default=20)
    read.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'prepare':
        print(json.dumps(prepare(args.source, args.db), indent=2))
    else:
        if not 5 <= args.samples <= 40:
            parser.error('Use 5..40 paired samples per condition.')
        result = measure(args.db, args.samples)
        with args.output.open('x') as stream:
            json.dump(result, stream, indent=2)
            stream.write('\n')
        print(json.dumps({key: {name: data['p95_ms'] for name, data in result[key].items()}
                          for key in ('idle', 'under_writes')}))
