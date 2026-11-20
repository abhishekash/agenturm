# Rust migration candidate verifier: offline checkpoint

20 November 2026. This is a feature-off inspection slice. Python `Store` remains the production writer. The verifier has no service dispatch, production caller, target creation, marker flip, publish, archive, or recovery action.

## Contract

`agenturm-migration-verify MANIFEST.json` accepts one trusted `migration_manifest_v1` JSON document with three required detached SQLite backups: telemetry, workflow, and saved tasks. Unknown JSON fields and manifest versions fail. Each backup descriptor fixes its absolute pathname, device, inode, byte length, SHA-256, and schema contract version. Every file must be a single-link regular file with owner-only permissions in a private real directory; SQLite sidecars are forbidden. The verifier reads immutable, query-only SQLite snapshots, checks `quick_check(1)`, and hashes the bytes again after inspection. On Linux, SQLite opens the checked file through `/proc/self/fd` to bind its read to that inode. File size is capped at 4 GiB; the manifest at 1 MiB.

The telemetry schema contract is version `1`, proved by `schema_meta.plugin_sessions_read_schema=1` and required plugin cursor/receipt tables. There is no single global telemetry schema version in the current Python Store. The workflow schema must be `workflow_meta.schema_version=2`; saved tasks must have `task_schema_v1.version=1`. The telemetry backup's `schema_meta.receiver_id` must equal the manifest receiver ID.

Snapshot claims must exactly match all source cursors (up to 4,096), including lease/policy/dirty sequence, last batch digest, and optional Pi content/command replay receipts. Workflow notification claim and native event counts, saved task count, and maximum task revision must match and stay within 1,000,000. These bounded claims are a consistency check against the detached files. **Counts alone do not attest every durable receipt**; the full file digests bind the remaining bytes only if a trusted producer made the manifest.

## Evidence and limits

Focused Rust tests include a committed WAL row included by SQLite backup, tampered digest, same-byte new inode, receiver and snapshot mismatch, missing auxiliary backup, symlink/nonprivate file, unknown field/version, and interrupted preparation leaving the live source unchanged. No test enables Rust writer authority.

The manifest is **trusted input**. This verifier does not create it or prove that the three backups were taken during a single stopped-write epoch. It does not exclude a held-open old writer. The surrounding cutover must still stop old processes, drain all Store/Workflow/Task/outbound mutation paths, prepare and authenticate backups atomically, enforce a distinct service principal and protected new path, then handle crash recovery and the first Rust durable write. See [Rust writer cutover gate](rust-writer-cutover-gate.md). On non-Linux Unix, the verifier checks path identity before and after opening SQLite but does not yet have the Linux descriptor-bound open guarantee; platform qualification remains required.
