# Python Store authority preflight

Recorded 20 November 2026. This is a guarded startup prerequisite for a future one-writer migration. Python `Store` remains the sole production writer. No Rust writer, process lock, cutover, default change, or performance result is introduced.

## Startup contract

Before `Store.__init__` creates a directory or file, changes mode, enters WAL, installs schema, or starts a worker, it opens an **existing** regular SQLite database through a short `mode=ro`, `query_only` read. A missing database is still created by Python. A valid legacy SQLite database with no `schema_meta` or no authority marker remains accepted. A marked database must have the exact `schema_meta.storage_authority_v1 = python-v1` value. `rust-v1`, an unknown value, duplicate values, malformed metadata, a corrupt database, nonregular file, or a broken symlink fails closed.

The marker query reads at most two rows and asks SQLite for only the value's byte length and a 32-character prefix. It does not load an arbitrary-size marker into Python. The constructor binds a relative path to one lexical absolute path, checks the target device/inode before its first path mutation, refuses a file that appeared after an absent-path check, changes permissions only through a verified open descriptor, and checks the connected database's marker again before `PRAGMA journal_mode` or schema SQL. Python records `python-v1` after creating `schema_meta`; it does not overwrite another marker. `Store._connect()` now uses SQLite `mode=rw`, so a disappeared path cannot be recreated by a later connection. Explicit `--db` symlinks remain supported, and retargeting detected at these checks fails closed.

## Evidence and limits

The focused corpus covers new, reopened, empty and legacy databases; foreign, unknown, oversized and live uncheckpointed WAL markers; corrupt and nonregular paths; broken symlinks; a retarget during read preflight; concurrent creation after an absent preflight; retargeting immediately before chmod; disappearance immediately before the first SQLite connection; and a later cwd change after a relative-path Store startup. Refusal leaves the tested main database bytes, mode and modification time unchanged and starts no migration worker.

This is a **point-in-time guard**, not writer exclusion. A path can change after the final check, an ABA swap may escape the observations, and a process already using an open SQLite connection is not fenced by this marker. Older Python binaries ignore it. SQLite may touch WAL sidecars during a read-only preflight; the guard does not claim literal zero filesystem writes. A Rust authority cutover still requires stopping legacy writers, a cross-process ownership protocol, schema and replay parity, and a reviewed rollback drill.
