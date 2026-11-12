# Rust port: trace ingestion

This is an experimental native ingestion component for Agenturm. It is a Rust
JSONL importer and loopback HTTP trace receiver that writes the existing SQLite
schema. The Python cockpit can read the resulting database. The normal
`agenturm` command remains the supported full application.

## Build and use

Install a Rust toolchain, then from the repository root:

```bash
cargo build --release --manifest-path rust/Cargo.toml
rust/target/release/agent-observatory-rs --db runs.db import-jsonl traces.jsonl
rust/target/release/agent-observatory-rs --db runs.db serve --port 4318
```

The receiver accepts `POST /api/spans` with native JSON spans and
`POST /v1/traces` with OTLP JSON. It accepts gzip request bodies, bounds the
compressed and decompressed body to 8 MiB, and supports `--token` or
`OBSERVATORY_TOKEN` for bearer authentication. It listens on `127.0.0.1` by
default. `GET /api/health` reports receiver health.

Stop any Python receiver using the database before importing with Rust. After
the import, stop the Rust receiver and open the database with the Python cockpit:

```bash
uv run agenturm --db runs.db cockpit --port 14319 --no-local-agents
```

Concurrent Rust/Python writers are not a supported workflow. The Rust process
does not implement local session discovery, native hierarchy indexing, OTLP
protobuf, logs, metrics, progress, outcomes, or the query API. It is not a
drop-in replacement for `agenturm serve` or a cockpit endpoint.

When writing a current Python database, Rust updates an explicitly supplied
canonical `ag1_…` span owner and clears an old indexed owner when the replacement
has no canonical owner. Unknown ownership stays unknown. Use the Python receiver
for native worker identity and hierarchy support.

## Validation and performance

```bash
cargo test --manifest-path rust/Cargo.toml
python rust/compat.py
python rust/bench.py
```

The compatibility check verifies that Python reads Rust-imported traces and
endpoint inventory, reimporting a span invalidates the cached summary, and
replacing Python-written spans updates or clears their indexed canonical owner.
The benchmark includes parsing, storage, derived summaries, and a query of
all traces. On an original prototype run against an older Python store with
20,000 synthetic spans across 100 traces,
Python took 0.78 seconds; Rust import followed by Python summary backfill took
0.62 seconds. This is a modest improvement for that workload, not evidence of
a generally faster current app. The Rust import itself took 0.18 seconds. Run repeated
benchmarks with representative traces and live dashboard traffic before
making a performance claim.

The next porting steps are derived summaries and findings, protobuf plus
logs/metrics, the query API, local session discovery, and terminal UI. Keep
cross-language database checks as each component moves.
