#!/usr/bin/env python3
"""Synthetic multi-week history probe; never reads native/user telemetry.

Builds the named NEW database, waits for resumable migration, then measures
ordinary reads idle and under writes. Results describe this host, not a portable
latency promise. Retention is deliberately disabled for this explicit fixture.
"""

from __future__ import annotations
import argparse, json, math, os, platform, resource, statistics, threading, time
from pathlib import Path
from observatory.core import Store, normalize_native, _agent_id_from_attributes


def build(path, traces=5751, span_count=31000, log_count=41000):
    if path.exists():
        raise FileExistsError("Choose a new benchmark database")
    store = Store(path)
    now = time.time_ns()
    start = now - 35 * 86400 * 10**9
    with store._connect() as db:
        spans = []
        logs = []
        activity = []
        for index in range(span_count):
            run = index % traces
            trace = f"{run + 1:032x}"
            offset = index // traces
            attrs = {
                "session.id": f"session-{run % 36}",
                "agent.id": f"worker-{run % 36}",
                "agent.runtime": "Synthetic",
                "fixture.padding": "synthetic output " * 50,
            }
            if offset == 0:
                attrs.update(
                    {
                        "agent.run.state": "completed",
                        "agent.task": f"Implement synthetic task {run}",
                    }
                )
            else:
                attrs.update(
                    {
                        "tool.name": "pytest synthetic",
                        "gen_ai.tool.type": "shell",
                        "agent.activity.domain": "shell",
                        "agent.activity.subject": "pytest synthetic -q",
                        "agent.activity.action": "execute",
                        "agent.activity.result": "success",
                    }
                )
            if offset == 1:
                attrs.update(
                    {"gen_ai.usage.input_tokens": 100, "gen_ai.usage.output_tokens": 20}
                )
            if index % 29 == 0:
                attrs.update(
                    {
                        "mcp.method.name": "tools/call",
                        "mcp.server.name": "fixture",
                        "mcp.tool.name": "search",
                    }
                )
            stamp = start + run * (now - start) // traces + offset * 10**8
            spans.append(
                (
                    trace,
                    f"{index + 1:016x}",
                    None if offset == 0 else f"{run + 1:016x}",
                    "agent.run" if offset == 0 else "Shell command",
                    stamp,
                    stamp + 10**7,
                    "OK",
                    "synthetic",
                    json.dumps(attrs),
                    "[]",
                    "[]",
                    "{}",
                    "fixture",
                    _agent_id_from_attributes(attrs, {}, "synthetic"),
                )
            )
            activity.append((trace, stamp + 10**7))
            if len(spans) >= 1000:
                db.executemany(
                    "INSERT INTO spans VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)", spans
                )
                spans = []
        if spans:
            db.executemany(
                "INSERT INTO spans VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)", spans
            )
        db.executemany(
            "INSERT INTO trace_activity VALUES(?,?) ON CONFLICT(trace_id) DO UPDATE SET last_ns=MAX(last_ns,excluded.last_ns)",
            activity,
        )
        activity = []
        for index in range(log_count):
            run = index % traces
            stamp = start + run * (now - start) // traces + (index // traces) * 10**8
            attrs = {
                "session.id": f"session-{run % 36}",
                "agent.id": f"worker-{run % 36}",
                "agent.runtime": "Synthetic",
                "agent.activity.domain": "message" if index % 17 == 0 else "shell",
                "agent.activity.action": "user" if index % 17 == 0 else "execute",
                "agent.activity.subject": "Implement synthetic task"
                if index % 17 == 0
                else "pytest synthetic -q",
                "agent.activity.result": "success",
            }
            activity.append((f"{run + 1:032x}", stamp))
            logs.append(
                (
                    f"{index + 1:064x}",
                    f"{run + 1:032x}",
                    None,
                    stamp,
                    "synthetic",
                    9,
                    "INFO",
                    "agent.activity",
                    json.dumps("synthetic captured result " * 40),
                    json.dumps(attrs),
                    "{}",
                    "fixture",
                    _agent_id_from_attributes(attrs, {}, "synthetic"),
                )
            )
            if len(logs) >= 1000:
                db.executemany(
                    "INSERT INTO logs VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)", logs
                )
                logs = []
        if logs:
            db.executemany("INSERT INTO logs VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)", logs)
        db.executemany(
            "INSERT INTO trace_activity VALUES(?,?) ON CONFLICT(trace_id) DO UPDATE SET last_ns=MAX(last_ns,excluded.last_ns)",
            activity,
        )
        db.execute(
            "DELETE FROM schema_meta WHERE key IN ('trace_derived_v13','trace_derived_v13_cursor')"
        )
    store.close()


def measure(store, count=10):
    result = {}
    for name, call in [
        ("connections", store.connections),
        ("activity", lambda: store.activity_page(limit=200)),
        ("sessions", lambda: store.sessions(limit=500)),
        ("metrics", store.metrics),
        ("mcp", lambda: store.mcp_page(limit=100)),
    ]:
        values = []
        for _ in range(count):
            started = time.perf_counter()
            call()
            values.append((time.perf_counter() - started) * 1000)
        result[name] = {
            "samples": len(values),
            "median_ms": round(statistics.median(values), 2),
            "p95_ms": round(sorted(values)[max(0,math.ceil(len(values)*.95)-1)], 2),
            "max_ms": round(max(values), 2),
        }
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=20)
    args = parser.parse_args()
    if args.samples<1:parser.error('Samples must be positive')
    if args.build:
        build(args.db)
    started = time.monotonic()
    store = Store(args.db, async_derived_refresh=True)
    startup_ms = round((time.monotonic() - started) * 1000, 2)
    deadline = time.monotonic() + 600
    while not store.index_status()["ready"]:
        if time.monotonic() > deadline:
            raise RuntimeError("Migration deadline exceeded")
        time.sleep(0.2)
    migration_seconds = round(time.monotonic() - started, 2)
    report = {
        "host": platform.platform(),
        "python": platform.python_version(),
        "cpu_count": os.cpu_count(),
        "startup_ms": startup_ms,
        "migration_seconds": migration_seconds,
        "idle_reads": measure(store, args.samples),
    }
    cpu = time.process_time()
    wall = time.monotonic()
    time.sleep(5)
    report["idle_cpu_one_core_percent"] = round(
        (time.process_time() - cpu) / (time.monotonic() - wall) * 100, 2
    )
    stop = threading.Event()
    writes = []
    errors = []

    def writer():
        index = 0
        while not stop.is_set():
            index += 1
            stamp = time.time_ns()
            record = normalize_native(
                dict(
                    trace_id="f" * 32,
                    span_id=f"{index:016x}",
                    name="fixture live tool",
                    start_unix_nano=stamp,
                    end_unix_nano=stamp + 1,
                    attributes={"tool.name": "fixture", "session.id": "live"},
                ),
                "synthetic",
            )
            try:
                store.ingest([record])
                writes.append(index)
            except Exception as exc:
                errors.append(type(exc).__name__)
            stop.wait(0.1)

    thread = threading.Thread(target=writer)
    thread.start()
    try:
        report["under_writes"] = measure(store, args.samples)
    finally:
        stop.set()
        thread.join()
    report.update(
        write_batches=len(writes),
        write_errors=errors,
        max_rss_mib=round(
            int(
                next(
                    line.split()[1]
                    for line in Path("/proc/self/status").read_text().splitlines()
                    if line.startswith("VmHWM:")
                )
            )
            / 1024,
            2,
        )
        if Path("/proc/self/status").exists()
        else round(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024, 2),
        derived_pending=len(store._derived_pending),
    )
    with store._connect() as db:
        report["counts"] = {
            table: db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
            for table in ("spans", "logs", "trace_summaries", "activity_index_v1")
        }
        report["database_mib"] = round(args.db.stat().st_size / 1024**2, 2)
        try:
            report["table_mib"] = {
                row[0]: round(row[1] / 1024**2, 2)
                for row in db.execute(
                    "SELECT name,SUM(pgsize) FROM dbstat GROUP BY name"
                )
            }
        except Exception:
            report["table_mib"] = {"available": False}
    store.close()
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {key: value for key, value in report.items() if key != "table_mib"},
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
