# Rust content grant fence

The standalone Rust store records host approvals for user text from pinned Pi, Codex, Claude Code, Kiro, and OpenCode sources. A grant identifies a configured source, a closed content kind, and a 32-byte fingerprint that the trusted host derives from the current source configuration and artifact. A grant begins denied and requires an explicit store API call. Repeating the same active authorization returns the same token. Revocation denies access immediately in the next policy read. Reauthorization and fingerprint changes issue a new epoch and nonce.

Grant validation reads the current SQLite state against the caller's freshly checked scope. Tests cover default denial, same-scope retries, changed fingerprints, revocation from another connection, reopen and reapproval, and tokens from two databases at the same epoch. The v8→v9 migration starts with an empty grant table and retains observations, cursors, and replay receipts. Existing v8 stores remain readable and can be backed up before migration.

`agenturm backup` snapshots the Rust database and disables private content approvals in the detached copy. The live store retains its current state. A restored database requires a fresh approval, and previously issued tokens fail validation. The backup also retains the version counter so a fresh approval advances the epoch.

The Linux CLI grants explicit access to one file in a version 2 collection
catalog. The adapter must be pinned by SHA-256. The collector keeps only
observation metadata in durable history. Run the CLI while the history
writer lease is available.
```sh
agenturm content grant --db agenturm.sqlite --config agenturm-sources.json --source-id harness.pi
agenturm content preview --db agenturm.sqlite --config agenturm-sources.json --source-id harness.pi --event-id '<event-id>' --revision 0
agenturm content revoke --db agenturm.sqlite --source-id harness.pi
```

For JSONL sources, the host verifies the no-follow file identity and current
committed prefix. Each retained receipt stores a proof of the exact admitted
prefix without the adapter's opaque checkpoint fields. A historical row
must fall inside its own verified receipt prefix. The host verifies native
row identity, session, worker, turn, role, revision and time. Legacy rows
without a stored receipt proof qualify only in the latest commit. A prefix
above 8 MiB or a row above 8 KiB is outside this preview limit.

For OpenCode, the host verifies the current SQLite sweep signature and opens
a bounded read-only transaction. A user row must match the exact admitted
event, native turn, revision, and timestamp. An earlier idle boundary
establishes turn membership. An updated source requires a fresh proof
from collection.

The host rechecks the source and current approval before delivery. It
holds the database writer fence through the CLI flush or cockpit redaction.
Preview output is one JSON string of at most 2 KiB of eligible text. The
host filters terminal and direction controls and withholds likely credential
material. The default cockpit can run the same check on its collector's
writer connection.

Run the store suite with `cargo test --locked -p agenturm-store --lib`.
The `prompt_content` executable tests verify approval, exact user identity,
credential filtering, retained JSONL proofs, source rewrites and replacement,
and revocation. The OpenCode `configured_checks` test verifies the current
SQLite sweep, turn identity, and denial after a database modification.
