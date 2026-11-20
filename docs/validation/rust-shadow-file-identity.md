# Shadow snapshot Store-file fence

Recorded 20 November 2026. `rust_shadow_snapshot.capture()` now binds its content-free, policy-current fixture to the regular SQLite Store file it read. `current()` checks that same file identity before and after its receiver, lease, policy and projection read. The two operations use the Store's read-only connection pool, so a missing-path race cannot create a new SQLite file. Python remains the live authority and sole writer; this is an opt-in diagnostic boundary.

## Why the extra check matters

An atomic Store-file replacement can carry the same receiver and source revision but a different projected row. Revision equality alone would still call the old snapshot current. The snapshot now includes the Store target's `(device, inode)` identity. A replacement or a `--db` symlink retarget to another file makes the old snapshot stale, even when the copied revision matches. Existing direct construction of `ShadowSnapshot` remains possible with an omitted identity, but such a snapshot cannot pass `current()`.

The check follows an explicit `--db` symlink and requires its target to be a regular file. The backup-query attester now reuses this same target-identity check. A symlink retarget changes the target identity and fails closed in both paths. The read pool also detects path replacement around its read transaction. These are point-in-time checks: a later write or another replacement requires a new check. They do not authenticate a copied database, prevent adversarial swap-and-restore between observations, or make a Rust result authoritative.

## Verification

`AGENTURM_RUST_BACKUP_QUERY_BIN=/tmp/agenturm-rust-opencode-target/debug/agenturm-backup-query python3 -m unittest tests.test_rust_shadow_snapshot tests.test_rust_backup_query tests.test_plugin_pi_content -q` passed 36 tests with one installed-reducer test skipped after rebasing on the Pi content gate and architecture ledger. New regressions prove a same-revision, different-projection atomic file replacement invalidates the old snapshot, and an explicit symlink works until retargeted. The installed attestation test now proves that a symlinked Store is accepted, then rejected when retargeted during the final check. The earlier backup-attestation replacement test was updated to expect `current()` itself to reject the swap. An older local reducer executable was excluded after it produced a known Activity schema mismatch; no reducer result is claimed for this change.

This does not change production query defaults, Rust storage authority, or any performance claim. Full storage cutover and installed-client parity remain separate gates.
