# Agenturm Rust 0.3.3

The published Linux release adds broker-backed workspaces, private evidence
and source-backed task continuity, alongside local decoder installation and
JavaScript OTLP traces.

## Broker workflows

A distinct Linux storage principal owns observation history. Separate reader,
grant administration and maintenance identities can list approved content,
change grants, prune history, create backups and restore into a fresh data
root. The cockpit and task CLI use a workspace bound to the broker endpoint
and persistent history identity. Replacing history at the same socket opens
a separate workspace.

Private prompts and Codex operation details retain their exact source proof
and current approval through terminal redaction. Escape, quit and Ctrl-C
replace the private view before releasing its acknowledgement. Client roles
can be denied direct access to both broker history and source transcripts.

Source-backed task drafts revalidate the exact occurrence and grant before
approval. The task stores source references and authored continuation;
source excerpts require a fresh proof on reveal. Task updates, structured
briefs, decisions, archives and handoffs support revision checks. A stale
cockpit form cannot overwrite an intervening CLI edit.

[Broker setup and commands](../rfcs/rust-broker-commands.md) ·
[Task commands](../rfcs/rust-linux-bundle.md)

## Producers, adapters and connectors

The shipped JavaScript client can emit OTLP traces through `agenturm run`
or `serve-traces`, with explicit source and session identity. The trusted
local adapter installer verifies an operator-supplied SHA-256 pin and
publishes a private catalog atomically. A native Pi fork can retain its
parent session when both native headers and the inherited first row agree.

The Linux isolated connector can forward typed status operations through a
grant-bound local Unix endpoint. The host checks its peer before sending a
credential, and derives receipts without reflecting endpoint content.
Production connector qualification remains open.

[JavaScript producer](../javascript.md) ·
[Adapter installation](../rfcs/rust-decoder-installation.md) ·
[Connector transport](../rfcs/rust-connector-api.md)

## Validation and scope

Development checks include real adapter process tests, Node-to-Rust OTLP,
separate-principal broker task and private-display journeys, role restrictions,
retention, backup and fresh-grant restore. The supported release platform is
Ubuntu 24.04 x86-64. A real Kiro transcript, production connector endpoint,
additional platforms and the remaining full-distribution cutover journeys
still need their own qualification. The default Python distribution has its
separate release and entry point.

The release tag `rust-v0.3.3` points to
`5fbe503a229594dba990d91da4c8acf26d30a54f`. The published x86-64 archive
has SHA-256 `54176000e12f3a5a30f5f46a385247a5c0fd3900b987f7f8dee77929f3491733`.
That exact archive passed managed installation, restart, upgrade from Rust
0.3.2, Python-free startup, separate-principal private-content checks,
blocked-output expiry and terminal redaction. The GitHub asset digest
matched the qualified archive.

[Published release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.3) ·
[Tagged build](https://github.com/abhishekash/agenturm/actions/runs/45080059783)
