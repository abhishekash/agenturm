# Rust v2 scratch source activation

20 November 2026. Implementation code is in commit `8c5fd44`; the open PR also includes a later documentation-only ledger/status refresh. Local checks below ran against the implementation commit. This checkpoint covers the PR-only `ActivateSource` slice. It is available only through the explicitly marked Rust scratch path; it does not route production callers, replace Python `Store`, or claim a performance gain.

## Contract and boundary

The typed v2 operation takes a plugin ID, source ID, and generation. Under `BEGIN IMMEDIATE`, it verifies the scratch marker, receiver identity, and the cooperating writer lease when the CLI provides one. A new source receives a server-generated positive lease epoch and explicit Python-compatible defaults: empty cursor/prior cursor/digest/fingerprint, dirty sequence 0, policy epoch 1, and capture enabled.

For an existing source, activation requires the exact receiver/plugin/generation, enabled capture, an empty private-content fingerprint, and supported metadata-only lifecycle history. It increments only the lease epoch, preserving cursor, policy epoch, and dirty sequence. A nonempty cursor requires matching content-batch and content-status receipts; incomplete history fails closed. Revoked rows, private owners/content, unsupported member/gap/retention state, malformed projection history, and absent-row orphans (including source-linked lifecycle, revoke, and activation receipts) are refused.

Activation, lifecycle admission, and revocation share one request-ID namespace enforced by bidirectional checks across their typed receipt tables. Activation checks write row counts, rereads the source and typed receipt before commit, and repeats the cross-operation request-ID check after receipt insertion. Trigger-suppressed writes, trigger-mutated source state, and trigger-created cross-operation receipt collisions roll back. Activation stores its typed reply atomically. Exact retries are checked before active-source validation and return the original lease after restart, later activation, or revocation without changing current source state. Changed payloads and IDs used by another operation fail closed. Duplicate activation requests on independent SQLite connections serialize under the immediate transaction and return one original lease.

This operation does not clean private providers and does not add a provider cleanup SPI. The scratch lifecycle subset remains deliberately narrow. Python `Store` remains production authority.

## Validation

Rust 1.100.0 local checks on the PR head:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p agenturm-storage-admission -p agenturm-storage-service-contract -p agenturm-storage-service
cargo build --locked --bin agenturm-storage-admission
cargo build --locked --bin agenturm-storage-service
AGENTURM_RUST_SCRATCH_ADMISSION_BIN="$PWD/target/debug/agenturm-storage-admission" python3 -m unittest tests.test_rust_scratch_admission -v
AGENTURM_RUST_STORAGE_SERVICE_BIN="$PWD/target/debug/agenturm-storage-service" python3 -m unittest tests.test_rust_storage_service -v
```

Results: workspace formatting and Clippy passed; workspace Rust tests passed with 150 passed, 0 failed, 0 ignored across 44 test-result groups. The focused three-crate run passed 54 tests (27 admission, 25 contract, 2 service). The installed scratch admission suite passed all 15 tests; installed storage service integration passed all 10 tests. The scratch integration exercises first activation defaults, persisted replay, changed-request conflict, real `Store` lifecycle admission/projection, reactivation lease fencing and cursor/policy/dirty preservation, stale-lease rejection, retry of the earlier activation after reactivation and after revoke, and refusal to re-enable after revocation. Rust trigger tests also cover ignored update/insert/receipt writes and source/request-ID mutation by post-write triggers.

The CI workflow already runs the installed scratch admission module after building the Rust binary; no production dispatch or CI authority change is part of this slice. GitHub CI is reported separately from local evidence.

## Remaining boundary

The Rust operation is an experimental marked-scratch writer only. It is not exposed to canonical/production callers; production capture policy and Store authority remain in Python. No private-provider cleanup, general source lifecycle service, cutover safety, or speed claim follows from this checkpoint.
