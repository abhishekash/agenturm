"""Short synthetic Store-route diagnostic, not an HTTP/resource acceptance run."""
from contextlib import closing
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
from tempfile import TemporaryDirectory
import time


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


if len(sys.argv) > 1 and sys.argv[1] == 'child':
    root, source, target = map(Path, sys.argv[2:5])
    sys.path.insert(0, str(root))
    from observatory.core import Store
    # A just-created owned fixture can still have committed WAL pages. Read
    # through SQLite backup, including its WAL; immutable would silently omit
    # those pages and turn the positive fixture into an empty database.
    with closing(sqlite3.connect(source.as_uri() + '?mode=ro', uri=True)) as origin:
        with closing(sqlite3.connect(target)) as db:
            origin.backup(db)
    store = Store(target)
    try:
        sql = '''SELECT 1 FROM spans WHERE lower(json_extract(attributes,'$."gen_ai.tool.type"'))='mcptool' LIMIT 1'''
        with store._connect() as db:
            present = db.execute(sql).fetchone() is not None
        assert present == (sys.argv[5] == 'positive'), 'Fixture execution evidence changed before measurement'
        assert store.mcp_page(limit=100)['calls'], 'Fixture call inventory is empty'
        times, hashes = [], []
        for index in range(19):
            started = time.perf_counter_ns()
            page = store.mcp_page(limit=100)
            elapsed = (time.perf_counter_ns() - started) / 1e6
            hashes.append(digest(page))
            if index >= 3:
                times.append(elapsed)
        with store._connect() as db:
            sql = '''SELECT 1 FROM spans WHERE lower(json_extract(attributes,'$."gen_ai.tool.type"'))='mcptool' LIMIT 1'''
            plan = [row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + sql)]
            present = db.execute(sql).fetchone() is not None
        assert len(set(hashes)) == 1
        print(json.dumps(dict(root=str(root), head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
                              samples_ms=times, complete_output_sha256=hashes[0], calls=len(page['calls']),
                              executions_present=present, probe_plan=plan)))
    finally:
        store.close()
    raise SystemExit


before = Path('/tmp/agenturm-derived-economics')
after = Path('/tmp/agenturm-mcp-negative-association')
negative = Path('/tmp/agenturm-mcp-negative-before.db')
output = Path(sys.argv[1])
assert not output.exists()
results = dict(diagnostic_only=True, acceptance=False, protocol='Separate process ABBA; identical private SQLite backup per arm; 3 warmups,16 timed Store.mcp_page reads; no HTTP, concurrent writer, UI, cold-readiness or monitor-resource claim.', cases={})
with TemporaryDirectory(prefix='agenturm-mcp-absence-diagnostic-') as directory:
    directory = Path(directory)
    sys.path.insert(0, str(after))
    from observatory.core import Store
    from tests.test_mcp_source_ownership import record
    positive = directory / 'positive.db'
    store = Store(positive)
    try:
        records = [record(1, 'parent', 'one', tool=True),
                   record(2, 'parent', 'one', execution=True, parent=f'{1:016x}')]
        for event in records:
            event['start_unix_nano'] = time.time_ns()
            event['end_unix_nano'] = event['start_unix_nano'] + 5
        store.ingest(records)
    finally:
        store.close()
    for case, source in [('negative', negative), ('positive', positive)]:
        arms = []
        for index, root in enumerate((before, after, after, before)):
            target = directory / f'{case}-{index}.db'
            raw = subprocess.check_output([sys.executable, __file__, 'child', str(root), str(source), str(target), case], text=True)
            arms.append(json.loads(raw))
        assert len({arm['complete_output_sha256'] for arm in arms}) == 1
        results['cases'][case] = arms
    source = Path('/tmp/agenturm-final-common-seed/raw-seed.db')
    with closing(sqlite3.connect(source.as_uri() + '?mode=ro&immutable=1', uri=True)) as db:
        sql = '''SELECT 1 FROM spans WHERE lower(json_extract(attributes,'$."gen_ai.tool.type"'))='mcptool' LIMIT 1'''
        samples = []
        for _ in range(12):
            started = time.perf_counter_ns()
            present = db.execute(sql).fetchone() is not None
            samples.append((time.perf_counter_ns() - started) / 1e6)
        results['historical_probe_only'] = dict(source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
            spans=db.execute('SELECT count(*) FROM spans').fetchone()[0], executions_present=present,
            samples_ms=samples, query_plan=[row[3] for row in db.execute('EXPLAIN QUERY PLAN ' + sql)],
            qualification='Immutable read-only existing synthetic raw fixture; this is only the added absence probe, not a complete historical Store-page benchmark.')
output.write_text(json.dumps(results, indent=2) + '\n')
print(output)
