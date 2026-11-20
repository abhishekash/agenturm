# Rust scratch storage admission

This checkpoint adds a **feature-off experiment** for one host-prepared lifecycle batch. It is a small transaction shape toward the planned Rust storage service, not a live writer or a cutover. The Python Store remains the only production database writer.

## Boundary

- The standalone `agenturm-storage-admission` crate is absent from the receiver launcher, plugin host, wheel, and Python Store runtime.
- Its CLI requires an existing regular SQLite file and the explicit `schema_meta.storage_admission_scratch_v1=enabled` marker before a read/write open. The marker is rechecked under `BEGIN IMMEDIATE`. Tests create disposable Store fixtures and add it deliberately.
- The caller supplies **host-prepared** canonical metadata: event hash, payload JSON, record digest, batch digest, source lease and expected cursor. This crate does not parse plugin wire frames, compute canonical hashes, sanitize an arbitrary payload, or grant a child storage authority. A caller must not feed child-selected values directly to it.
- The supported input is at most 48 revision-zero lifecycle records whose canonical payload contains only an allowed `state` and optional bounded `reason_code`. A metadata-only source with member discovery or mapping, retained content, gaps, a retention watermark or invalidation, or non-lifecycle history fails closed. No source reads or content references occur.

## Transaction behavior

The scratch connection checks receiver identity, plugin/source ownership, executable generation, lease epoch, policy epoch and capture state under one immediate SQLite transaction. New lifecycle evidence, the source cursor, prior cursor, batch digest, dirty sequence, dirty marker and empty-content receipt/status commit together. An unchanged empty poll has no write. An exact replay returns without a write, but rejects a present empty-content receipt that disagrees with the batch; as in Python's metadata-only path, an absent receipt is permitted. A stale cursor, changed event digest, disabled capture, unsupported history or a late conflicting row aborts the entire transaction.

The supported path retains Python's 50,000-row ceiling by rejecting a new insertion at capacity; it does not implement Python retention or correction. Timestamps for `admitted_ns` and `updated_ns` are independently generated, so differential comparison excludes those fields.

## Verification

The installed Python differential suite `tests/test_rust_scratch_admission.py` creates separate disposable Python Store fixtures. One fixture receives a prepared batch through the Rust binary, the other through `plugin_admission.commit_batch`. It compares persisted evidence, cursor, dirty marker, empty-content receipt and status, then compares exact replay and idle behavior. It also tests rollback after a later conflicting row, stale cursor and lease, revoked capture, marker denial and unsupported correction. The Rust crate has in-memory unit tests for the same transaction fences, so its baseline suite does not depend on an installed binary.

This proves only the declared lifecycle subset on disposable fixtures. It leaves the full admission policy, content, source gaps, retention, corrections, ownership exclusion, legacy-writer fencing, physical backup/WAL handling and safe cutover gates open. No performance gain is asserted.
