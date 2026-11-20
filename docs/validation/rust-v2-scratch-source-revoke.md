# Rust v2 scratch source revocation

20 November 2026. This is an experimental, scratch-only policy operation. It does not change production capture policy, the canonical receiver database, or Python `Store` authority.

## Contract and transaction

The v2 protocol adds a typed `RevokeSource` request carrying the current source lease, acknowledged cursor, and expected dirty sequence. The service checks the receiver identity, explicit scratch marker, pinned scratch path, writer lease, and exact source lease under `BEGIN IMMEDIATE`. Revocation advances the lease and policy epochs, disables capture, clears the content fingerprint, advances the dirty sequence, and keeps the acknowledged cursor. It erases lifecycle evidence and the corresponding projection, Activity, content-status, and dirty rows in that same transaction.

Requests with any private-reference owner, stored content, directory/member state, continuity gap, retention state, or non-lifecycle evidence fail before mutation. Rust has no trusted reference-provider cleanup SPI, so this operation never claims to clean private providers. The request receipt is committed with revocation. Exact retries return the stored typed result, including after reopening the database; a reused request ID with changed bytes or the other v2 operation fails closed. Both operations check the other's receipt table so one request ID cannot commit once as admission and again as revocation.

## Validation

Local checks passed:

```sh
cargo fmt --all -- --check
cargo test -p agenturm-storage-service -p agenturm-storage-service-contract -p agenturm-storage-admission
cargo clippy -p agenturm-storage-service -p agenturm-storage-service-contract -p agenturm-storage-admission --all-targets -- -D warnings
AGENTURM_RUST_SCRATCH_ADMISSION_BIN=/path/to/agenturm-storage-admission python3 -m unittest tests.test_rust_scratch_admission -v
```

The Rust command passed 42 tests: 16 admission, 24 contract, and 2 Linux service. The installed Python module passed all 14 tests. Its new real-`Store` scratch case covers admission and projection, revoke cleanup and epoch fencing, stale admission rejection, exact retry through a second process, changed-request rejection, and cross-operation request-ID conflict. GitHub CI status is reported separately from local results.

This is not a production writer or a general source lifecycle service. It remains unavailable to canonical database callers. Python `Store` remains the production writer and capture-policy authority; all other writer-exclusion, migration, recovery, compatibility, and platform gates remain open. No performance gain is claimed.
