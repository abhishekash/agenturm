# Rust exact-session query parity

Recorded 20 November 2026. `agenturm-domain-query` is a pure, read-only query over the existing version 3 domain reducer fixture. It is an offline parity gate, not a live Store reader, HTTP endpoint, database writer, or authority cutover. Python Store remains the only SQLite writer and the only component that captures a coherent policy-current snapshot.

## Contract

The version 1 request contains a reducer version 3 `snapshot` and a `query` with `kind = "session"`, the exact `source_id` and `native_session_id`. A source mismatch fails closed. The Rust crate depends on `agenturm-domain-reducer`, not on any harness. It validates the outer request, passes the snapshot through the reducer's strict evidence and gap validation, then selects exactly one reduced session. The reply carries the receiver, source, plugin, policy epoch and dirty sequence from the validated snapshot. The two revisions are decimal strings. `coverage = "partial"` is unconditional, including when `session` is `null`; an absent row does not prove a complete source.

The CLI accepts at most 16 MiB plus 1,024 bytes for the query envelope. It rejects duplicate fields, unknown query fields, unsupported versions, malformed native identity and oversized input. The underlying reducer bounds evidence, sessions, nesting and metadata. The query holds no SQLite connection, source file handle or network socket.

A live caller must first obtain `rust_shadow_snapshot.capture(store, source_id)`, pass its `fixture` to the query, and call `rust_shadow_snapshot.current(store, snapshot)` before using the result. That freshness check includes receiver identity, source lease, capture policy, dirty sequence and published projection revision. The CLI cannot perform this check itself and its output must not be published as policy-current without it.

## Verification

The [version 1 query corpus](../../tests/fixtures/rust-domain-query-v1.json) covers an exact completed session, an absent session with partial coverage, source-gap Unknown state, invalid query and snapshot versions, malformed identity and duplicate outer keys. The installed CLI test compares each valid result with a disposable Python Store projection. A live Store test captures a read-only shadow fixture, compares the exact result with its persisted projection, then proves that a later commit and capture revocation invalidate the snapshot. Rust unit, Clippy and formatting gates run through the workspace; the installed binary test runs in CI.

This slice does not provide live Rust query/storage authority, multiple source pagination, retained-history performance evidence, legacy writer exclusion or a migration drill. Those remain Phase 8 and 9 gates in the [execution plan](../rfcs/rust-plugin-kernel-plan.md).
