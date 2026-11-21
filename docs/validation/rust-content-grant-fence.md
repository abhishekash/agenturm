# Rust content grant fence

The standalone Rust store now records host approvals for Pi user text. A grant identifies a configured source, a closed content kind, and a 32-byte fingerprint that the trusted host derives from the current source configuration and artifact. A grant begins denied and requires an explicit store API call. Repeating the same active authorization returns the same token. Revocation denies access immediately in the next policy read. Reauthorization and fingerprint changes issue a new epoch and nonce.

Grant validation reads the current SQLite state against the caller's freshly checked scope. Tests cover default denial, same-scope retries, changed fingerprints, revocation from another connection, reopen and reapproval, and tokens from two databases at the same epoch. The v8→v9 migration starts with an empty grant table and retains observations, cursors, and replay receipts. Existing v8 stores remain readable and can be backed up before migration.

`agenturm backup` snapshots the Rust database and disables private content approvals in the detached copy. The live store retains its current state. A restored database requires a fresh approval, and previously issued tokens fail validation. The backup also retains the version counter so a fresh approval advances the epoch.

The Linux CLI provides an explicit Pi user-text preview for one JSONL file configured with a SHA-256-pinned Pi adapter in a version 2 catalog. The operator approves that source, selects its admitted user message and revision from `agenturm query`, then requests the preview. The collector and the database continue to retain metadata. The commands use the cooperative writer lease, so stop any running collector for this database before using them.

```sh
agenturm content grant --db agenturm.sqlite --config agenturm-sources.json --source-id harness.pi
agenturm content preview --db agenturm.sqlite --config agenturm-sources.json --source-id harness.pi --event-id '<event-id>' --revision 0
agenturm content revoke --db agenturm.sqlite --source-id harness.pi
```

The host selects a retained user-message revision from the commit that supplied the current durable source cursor. It verifies the no-follow file identity, header hash, canonical initial generation and SHA-256 over the entire committed prefix, which covers the selected row's bytes. It requires a unique native row identity and exact session, worker, turn, role and time. Files with an unproven generation or a prefix above 8 MiB are ineligible, as are rows above 8 KiB. The host rechecks the configured source, retained evidence and approval before delivery and holds the database writer lease through the output flush.

Preview output is one JSON string containing at most 2 KiB of eligible text. The host filters terminal and directional control characters and withholds text that matches known credential markers. This filtering is best effort. Older receipts lack their own cursor proof, so only the current source commit can supply a preview. Previously collected history remains available through normal metadata reads. Directory sources and gaps require their own admission proof. The [Pi reference gate](../rfcs/pi-content-reference-gate.md) describes the separate directory evidence contract.

The store tests exercise the grant lifecycle, restored backup, and v8 read-only and migration paths. Run them with `cargo test --locked -p agenturm-store --lib`. The CLI process test exercises collection and ACK, approval and preview, wrong event IDs, credential withholding, source rewrite and file replacement, stale events after a later commit, concurrent writer rejection, and revoke/reapprove. Run `cargo test --locked -p agenturm-app --test pi_content` with `AGENTURM_RUNTIME_PLUGIN_HOST` and `AGENTURM_RUNTIME_PI_ADAPTER` set to the built binaries.
