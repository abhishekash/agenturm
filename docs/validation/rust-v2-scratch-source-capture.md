# Rust v2 scratch source capture policy

20 November 2026. This PR #297 checkpoint is based on merged main `c841568` (PRs #262–#296). The implementation commit after rebase is `0600dec`; local checks below were run against the rebased code. The operation is available only through the explicitly marked Rust scratch path. It does not route production callers, replace Python `Store`, or claim a performance gain.

## Contract and boundary

The typed v2 `SetSourceCapture` request carries the target capture state, exact `SourceLease`, expected cursor, expected dirty sequence, and expected current capture state. The operation uses `BEGIN IMMEDIATE`, checks the scratch marker, receiver identity, and cooperating writer lease when the CLI supplies one, then validates source identity and all supplied fences.

A same-state request is a durable, stable no-op: it writes only its typed receipt and preserves the source row, markers, and their exact shape. This includes a valid fresh active source with no evidence-dirty or projection marker yet. Exact retries return the original reply before checking current source state, including after a later transition or process restart. Changed payloads, mismatched receipts, and request IDs already used by lifecycle admission, revocation, or activation fail closed. All four operation types check the shared request-ID namespace in both directions.

A real state transition preserves the cursor, increments policy epoch and dirty sequence once, rotates the lease epoch, and stores the resulting typed reply atomically. Disabling deletes only supported metadata-only lifecycle evidence, projections, Activity, and investigation dirty rows. It verifies cleanup and marker writes before commit; trigger-suppressed deletes or writes, trigger-mutated state, schema identity changes, and cross-operation receipt collisions roll back. Enabling does not restore deleted evidence. An explicit enable may re-enable a clean disabled row after `RevokeSource`, using its current lease and fences; the old lease remains stale.

The operation refuses private provider ownership, content payloads/fingerprints, member/gap/retention state, malformed or unsupported history, and any state requiring provider cleanup. It does not add a provider cleanup SPI. No production Store path calls this API; Python `Store` remains the canonical writer.

## Validation

Rust 1.100 local checks on the rebased implementation commit `0600dec`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p agenturm-storage-admission
AGENTURM_RUST_SCRATCH_ADMISSION_BIN="$PWD/target/debug/agenturm-storage-admission" \
  python3 -m unittest tests.test_rust_scratch_admission -v
```

Results: workspace formatting and Clippy passed; 163 Rust workspace tests passed with no failures. The focused admission and service-contract crates passed 39 and 26 tests. The installed Python scratch-admission suite passed all 17 tests. It exercises same-state no-op and retry with absent markers, disable→enable, revoke→explicit enable, stale lease rejection, no evidence restoration, retries after a later transition, cleanup rollback, and changed-request conflict. Rust regressions cover independent-connection duplicate requests, cross-operation request-ID reuse in both directions, malformed flags, stale cursor/dirty/state/lease fences, private and unsupported state, overflow, ignored or mutated writes, receipt-trigger injection, and trigger-created namespace collisions.

The existing CI workflow builds the scratch binary and runs `tests.test_rust_scratch_admission`; no production dispatch or CI authority change is part of this slice. GitHub checks for PR #297 are pending and are not recorded as passing.

## Remaining boundary

This is an experimental marked-scratch lifecycle policy operation only. It is not exposed to canonical or production callers. It does not establish Python writer quiescence, Rust Store authority, cutover safety, general provider cleanup, or any whole-app performance result.
