# Rust exact domain query parity

Recorded 20 November 2026. `agenturm-domain-query` is a pure, read-only query over the version 3 domain reducer fixture. It is an offline parity gate, not a live Store reader, HTTP endpoint, database writer, or authority cutover. Python Store remains the only SQLite writer and the only component that captures a coherent policy-current snapshot.

## Contract

The version 1 request contains a reducer version 3 `snapshot` and one exact query:

- `kind = "session"` selects `source_id` and `native_session_id`.
- `kind = "activity"` selects `source_id` and an opaque `plugin-event:<64 lowercase hex>` Activity ID.

A source mismatch fails closed. The Rust crate depends on `agenturm-domain-reducer`, not on any harness. It validates the outer request, passes the snapshot through the reducer's strict evidence and gap validation, then selects exactly one reduced session or Activity fact. The reply carries the receiver, source, plugin, policy epoch and dirty sequence from the validated snapshot. The two revisions are decimal strings. `coverage = "partial"` is unconditional, including when `session` or `activity` is `null`; an absent row does not prove a complete source.

Before an exact Activity lookup, the version 3 reducer applies the Store's current content-free tool-call relation rule to latest admitted revisions. A result links to a turn only when exactly one call in the same source/session/operation has an earlier source timestamp and no later admission timestamp. Ambiguous, future, equal-time and conflicting calls do not supply a turn. Other adapters can retain their explicit uncorrelated producer turn; older bundled Pi process results are quarantined when no call survives because they could contain a guessed later turn. Coarse category conflict and relation markers match Python's policy-current Activity projection. Retention and source-gap invalidation suppress affected Activity rows. Version 2 fixture behavior remains unchanged.

The CLI accepts at most 16 MiB plus 1,024 bytes for the query envelope. It rejects duplicate fields, unknown query fields, unsupported versions, malformed native identity and oversized input. The underlying reducer bounds evidence, sessions, nesting and metadata. The query holds no SQLite connection, source file handle or network socket.

A live caller must first obtain `rust_shadow_snapshot.capture(store, source_id)`, pass its `fixture` to the query, and call `rust_shadow_snapshot.current(store, snapshot)` before using the result. That freshness check includes receiver identity, source lease, capture policy, dirty sequence and published projection revision. The CLI cannot perform this check itself and its output must not be published as policy-current without it.

## Verification

The [version 1 query corpus](../../tests/fixtures/rust-domain-query-v1.json) covers an exact completed session, an absent session with partial coverage, source-gap Unknown state, invalid query and snapshot versions, malformed identity and duplicate outer keys. The installed CLI test compares each valid result with a disposable Python Store projection. Live Store tests capture read-only shadow fixtures and compare exact Activity results with persisted policy-current rows for late Pi results, a corrected result, source-gap suppression, Kiro terminal tool facts without a separate started phase, and an old Pi result with no retained call. They prove stale snapshots fail the caller's freshness check. Rust unit tests cover opaque ID shape, source mismatch, unknown query fields and absent Activity. A frozen version 2 golden omits the later Python-only `tool_turn_relation` metadata field; version 3 exercises exact current Activity parity.

On an isolated no-debug Cargo target, the touched Rust crates passed two query and seven reducer tests, plus workspace Clippy with warnings denied and `cargo fmt --all -- --check`. The rebuilt isolated CLIs passed five installed query tests and 16 reducer/shadow Python tests. An earlier shared Cargo target mixed build artifacts from parallel worktrees and is not counted as validation. The full isolated workspace test matrix was not repeated under the remaining local disk budget; CI for this change remains pending.

This slice does not provide live Rust query/storage authority, multiple source pagination, full native Review/export, retained-history performance evidence, legacy writer exclusion or a migration drill. Those remain Phase 8 and 9 gates in the [execution plan](../rfcs/rust-plugin-kernel-plan.md). No performance gain is asserted.
