# Feature-off v2 scratch admission bridge

20 November 2026. The `agenturm-storage-admission` CLI accepts a strict v2 lifecycle request only with `--v2-scratch-path PATH`. This is a disposable, explicitly marked scratch database. The Rust storage service continues to serve v1 probes only; Python `Store` remains the production writer. No receiver or plugin host calls this bridge.

## Boundary and transaction

The v2 frame has no database path. The explicit CLI argument supplies a local scratch path; the bridge checks that the existing regular database has `storage_admission_scratch_v1=enabled` before creating a writer lease. The same `WriterLease` and guarded SQLite transaction used by the older scratch CLI perform source lease, capture policy, receiver identity, expected cursor, unsupported history, evidence conflict, empty-content receipt and dirty-sequence checks. The typed v2 parser adds a 64 KiB frame ceiling, closed lifecycle shape and canonical request digest. A request cannot select a canonical or unmarked database through a frame field.

For v2 requests only, the scratch transaction creates `storage_admission_request_v2` and records the request ID, receiver/source, canonical request digest, cursor, batch digest, first disposition and dirty sequence. The receipt commits atomically with evidence and cursor changes. Reusing the same ID with different content fails; an exact retry can use the existing last-batch replay path while its source lease and receipts remain current. An older retry after another batch advances is rejected. The reply binds the full request digest and the scratch transaction result. The v1 scratch CLI and schema remain compatible.

This receipt is a **scratch proof**, not a production replay journal. Host-prepared record and batch digests remain untrusted assertions. The service has no v2 dispatch, canonical database, recovery or migration path. A future service writer must independently own and validate the canonical store, route every Python mutation path, exclude held-open old writers and provide durable recovery across all databases before cutover.

## Verification

The installed process tests exercise commit, exact retry across CLI restarts, conflicting request ID, stale retry, malformed or unknown fields, unmarked path, receipt tamper and capture revocation. They compare the scratch evidence/cursor projection with existing Python admission tests. The test suite is wired to CI after building the Rust CLI. Focused Rust tests, Clippy, formatting and installed Python tests are the local gates; GitHub CI status is reported separately at merge.
