# Rust lifecycle and usage reducer parity

Recorded 20 November 2026. This checkpoint adds `agenturm-domain-reducer`, a pure Rust library and stdin/stdout CLI for version 1 admitted plugin fixtures. It reads lifecycle and usage revisions and emits deterministic session state, reason, timestamps, and metadata usage. The receiver's Python Store remains the only database writer and the live projection path stays in Python. This is an architectural parity step without a performance claim.

## Fixture and reduction contract

Each fixture has `schema_version: 1`, receiver and source IDs, policy and dirty sequence, admitted rows, and optional retention invalidations. A row names a 64-character event hash, revision, kind, event and admission nanoseconds, optional native session ID, and a metadata-only payload. The CLI reads at most 16 MiB from stdin; the reducer accepts at most 50,000 rows and 2,000 projected sessions. It rejects duplicate JSON keys, nesting beyond 20 levels, invalid Unicode, malformed timestamps, counters, revisions, payload fields, identity changes between revisions, duplicate admitted revisions, and populated snapshots with dirty sequence zero. Null and empty native session IDs both mean absent identity, matching Python admission.

For each event hash, the highest revision participates in projection. Lifecycle state comes from the maximum event timestamp. Conflicting state or reason facts at that timestamp become `unknown` with `ambiguous_lifecycle`; a session with usage but no lifecycle has `unknown` with an empty reason. Usage fields sum across distinct latest events, while absent counters stay absent. Values above JavaScript's exact integer limit (`2^53 - 1`) are decimal strings in output. Retention invalidation forces `unknown` and `retention_gap` and clears usage; an invalidated session absent from evidence is still represented with its recorded last-seen timestamp.

Run the CLI against one fixture object:

```sh
cargo build --locked -p agenturm-domain-reducer
agenturm-domain-reducer < fixture.json > projection.json
```

The shared [golden corpus](../../tests/fixtures/rust-domain-reducer-v1.json) contains six valid and seventeen invalid cases. Rust unit tests compare to its expected JSON. The installed CLI differential test builds the same valid snapshots in disposable Python Stores and runs the production `plugin_projection.project_source` function. It reads every persisted projection row and cross-checks Python's `sessions` rendering wherever that API's 1,000-row limit can display the full result. Generated 201-session, 2,000-session, and 2,001-session tests check parity beyond the default display limit, the full accepted capacity, and Python's capacity rejection without enlarging the checked-in fixture. The Python oracle is unchanged.

## Verification

| Check | Result |
| --- | --- |
| Rust 1.100.0 `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` | Passed. |
| Built `agenturm-domain-reducer`; `AGENTURM_RUST_REDUCER_BIN=... python3 -m unittest tests.test_rust_domain_reducer -v` | Passed the valid fixture parity and invalid input tests against the installed executable. |

The current reducer covers lifecycle and usage only. Relationship and Activity projection, live Store snapshot fencing, native harness readers, and policy authority remain in their existing components. No runtime cutover is part of this checkpoint.
