# Rust shadow Store revision read

Recorded 20 November 2026. This checkpoint adds a small read-only Rust SQLite inspector for a caller-supplied, consistent **backup** of Agenturm's Python Store. It is a diagnostic storage-compatibility gate toward the planned Rust kernel. Python remains the sole production database writer, policy authority and live freshness authority. No performance gain or Rust cutover is claimed.

## Boundary

`agenturm-storage-shadow` accepts one bounded request containing an absolute path to an isolated SQLite backup and one exact source ID. It requires a regular file with no `-wal`, `-shm` or `-journal` sidecar. It opens the backup with SQLite's read-only and URI flags, sets `immutable=1` and `query_only`, and reads the receiver identity, source lease, policy, dirty sequence, capture flag and projection cursor in one short read transaction. Immutable mode prevents SQLite from creating read sidecars; it is safe here only because the caller supplies a finished, isolated backup that will not change during inspection. The inspector neither creates schema nor writes a database or sidecar. The output is a content-free revision certificate with a `ready`, `pending`, `disabled` or `unknown` state. Integer revisions are decimal strings so JSON consumers do not lose values above 2^53.

`ready` uses the same condition as Python's shadow snapshot: capture is enabled, the source has a positive dirty sequence, and the projected sequence and policy epoch match it. A missing source is `unknown`; malformed tables, invalid types, out-of-range fields and a receiver mismatch fail closed. The certificate describes the backup at the instant it was made. It **cannot** authorize a live query. The Python caller must still run `rust_shadow_snapshot.current()` against the live Store after any separate Rust reduction or query before publishing a result. A later commit, lease/policy change, revocation or projection rebuild makes the old certificate stale even if it still says `ready`.

The tests make each backup through Python SQLite's backup API into a private 0600 file and normalize it to a single main file. The Rust CLI is not wired into the receiver or HTTP API; a caller must supply the backup explicitly. A path to a live WAL database is outside this contract. SQLite's read-only flag is a database access boundary, not a process sandbox or proof that the supplied file is a consistent backup.

## Verification

An installed binary test compares Rust and Python revision/readiness results for unknown, newly activated, dirty pending, projected ready and capture-disabled sources. It checks that an old backup remains historically ready after Python's live `current()` fence rejects it. Malformed schema, bad lease/policy/dirty/receiver types, invalid capture flag, partial projection cursor, receiver mismatch, existing sidecar and missing file fail without creating a database. File hashes and directory entries remain unchanged after inspection, including no new `-wal` or `-shm` sidecars for the isolated backup. Rust unit tests reject malformed request envelopes before opening a file.

```sh
RUSTUP_HOME=/tmp/agenturm-rustup-home CARGO_HOME=/tmp/agenturm-cargo-home \
  PATH=/tmp/agenturm-cargo-home/bin:$PATH cargo test -p agenturm-storage-shadow --locked
AGENTURM_RUST_STORAGE_SHADOW_BIN=/path/to/agenturm-storage-shadow \
  uv run python -m unittest tests.test_rust_storage_shadow -q
```

This proves one narrow direct Store-schema read. Full evidence/projection query compatibility, durable user data, Rust writer exclusion, migration, post-write downgrade rules and crash/rollback drills remain open. A future writer cutover must stop the Python writer, take and verify a consistent backup, establish exactly one writer, and prove recovery without losing post-cutover writes. Restoring an old backup after new durable writes would discard user data; forward recovery is the default unless a separately validated lossless journal/export exists.
