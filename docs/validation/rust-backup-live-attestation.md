# Exact Rust backup query attestation

Recorded 20 November 2026. This opt-in Python diagnostic boundary checks one result from `agenturm-backup-query` against the policy-current Python Store projection. It does not start Rust, create a backup, serve an API or change the production query path. Python remains the sole live authority and writer. This slice makes no performance claim.

## Contract

`observatory.rust_backup_attestation.attest_exact(store, query, rust_result)` accepts the Rust process's raw JSON text and one exact, source-scoped session or Activity query. It rejects duplicate JSON keys, non-finite numbers, oversized output, extra or missing fields, malformed identities, a different query or source, a missing exact result field, and any state other than `ready` with `coverage: "partial"`. The live `capture()` must be ready. The attester binds the Store's resolved regular target file identity, including an explicit `--db` symlink, matches the full receiver and revision certificate, compares the selected row **or exact absence** with `capture(...).expected`, then calls `current()` and rechecks file identity before returning. A symlink retarget fails closed. It returns the row from Python's snapshot with the checked revision and `coverage: "partial"`; it never returns a row from the backup.

The returned result was valid **at the final check instant**. A later Store write can stale it. A caller publishing it later must check freshness again. An exact miss means only “absent from this policy-current partial projection”; it is not evidence that a session never existed or that the source is complete. A source gap may yield a matching `Unknown/source_gap` session row. Pending projection, disabled capture and unknown source fail closed. This helper makes no authenticity claim for the supplied backup; exact row comparison prevents a structurally valid altered backup from being treated as the current projection.

## Evidence and limits

The installed Rust backup-query test corpus checks matching session and Activity rows, exact absence, a valid-looking altered backup with the same revision, stale live evidence, capture revocation, pending projection, conservative source-gap state, malformed or duplicate output, a revocation between snapshot capture and final freshness check, and an atomic same-revision Store file replacement. The test uses small completed SQLite backups. No production backup copy is made by this helper.

This is a point-in-time compatibility gate for one exact query. It does not certify a Rust storage cutover, full-source completeness, crash recovery, a production query default, or a performance improvement. The full writer and client migration gates remain open.
