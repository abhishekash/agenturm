# Native broker commands

The Linux bundle includes `agenturm-storage-service` and explicit broker clients. The broker owns the history database and authenticates the client UID and primary GID. The client verifies the configured broker UID and GID before sending a request. Configure each native source ID in the service allowlist.

Create the data directory as mode `0700`, owned by the broker identity. Create the transport directory as mode `2710`, owned by the broker identity with the client's primary group. Run the service as its dedicated non-root identity.

```sh
agenturm-storage-service --data-root /srv/agenturm/history \
  --transport-dir /run/agenturm --receiver-id receiver:local \
  --client-uid 1000 --client-gid 1000 --native-sources harness.pi,harness.codex
```

Use the actual broker and client identities for the installation. A separate
client identity must be able to traverse the transport directory and connect
to its socket. Its permissions must deny direct access to the data directory.
Source adapters also need the appropriate OS isolation before they can be
treated as untrusted executables.

Create an explicit client configuration.

```json
{
  "version": 1,
  "socket": "/run/agenturm/agenturm-native-storage-v1.sock",
  "receiver_id": "receiver:local",
  "service_uid": 45000,
  "service_gid": 45000
}
```

The source configuration uses the same adapter catalog and native format as
local collection. Its source IDs must match the broker's allowlist.

```sh
agenturm broker collect --broker-config broker.json --config sources.json
agenturm broker collect --broker-config broker.json --config sources.json --watch
agenturm broker query --broker-config broker.json --harness-id harness.pi
agenturm broker review --broker-config broker.json \
  --harness-id harness.codex --session-id session-1 --turn-id turn-1
agenturm broker cockpit --broker-config broker.json
agenturm broker task list --broker-config broker.json
```

These clients resolve history through the service. The service owns the
database path, checkpoints and durable receipts. Collection validates the
adapter facts, commits them through the broker, then sends the adapter ACK.
An exact retry recovers the committed result after a lost reply.

Queries return the existing JSON projection and stable cursor fields.
Continue a traversal with the same filters and watermark. The broker limits
source-free reads to its configured allowlist. The inspector uses the
background workspace request loop for session and turn navigation.

The broker cockpit supports saved tasks, handoffs, request seen state and
review decisions alongside remote history. These user-owned records live
under the client's state directory. Their namespace binds the authenticated
endpoint and the broker's persistent history identity. A fresh database at
the same socket opens a fresh workspace. A running cockpit pins its remote
reads and exact link validation to the selected history identity.

## Grant administration

The service can authorize separate reader, grant administrator and maintenance
identities using `--native-reader UID:GID`,
`--native-grant-admin UID:GID` and `--native-maintenance UID:GID`.
The configured client remains the collector and reader. The roles apply
the native source allowlist. Each role's Linux identity is checked before
a request body is consumed. Configure the transport directory and socket
group access for those identities as described in the protocol guide.

Derive a binding from the exact pinned source configuration. This operation
reads the configuration and source identity without opening history or
approving content.

```sh
agenturm content binding --config sources.json --source-id harness.pi > binding.json
agenturm broker grant check --broker-config reader.json --binding binding.json
agenturm broker grant approve --broker-config grant-admin.json --binding binding.json
agenturm broker grant revoke --broker-config grant-admin.json --binding binding.json
```

For Codex operation content, add `--kind codex.mcp_payload` or
`--kind codex.edit_detail` when deriving the binding. A different content
kind has its own approval. The JSON response reports the current epoch
when approved. It contains no disclosure token or source content.

## Maintenance

Configure an optional broker-owned backup directory with
`--native-backup-root`. The service chooses each backup name and returns
its receipt. Maintenance requests accept no filesystem destination.

```sh
agenturm broker integrity --broker-config maintenance.json
agenturm broker backup --broker-config maintenance.json
agenturm broker prune --broker-config maintenance.json --before-unix-ns 1790000000000000000
```

Pruning preserves durable source checkpoints and replay receipts. Older
query snapshots become explicitly expired. Backups disable private-content
approvals, so restored history requires fresh approval.

## Private evidence

Enable private delivery with a separate `--native-reader UID:GID` and
`--native-content-config /srv/agenturm/content.json` on the service. The
configuration must be broker-owned, mode `0600`, singly linked and under
secure absolute ancestors. It contains version `1` and a `sources` array of
resolved entries. Each entry contains `source_id`, `plugin_id`, `plugin_host`,
`adapter`, `artifact_sha256`, `features` and `source_root`, matching the exact
pinned collection configuration. The broker must have read access to the
source transcript and pinned adapter. It reopens this configuration and
verifies the adapter hash during proof checks. No client request chooses a
source filesystem path.

The separate reader can list and reveal approved occurrences. Collection
and grant administration identities cannot disclose private evidence.

```sh
agenturm broker content list --broker-config reader.json \
  --source-id harness.pi --session-id session-1
agenturm broker content preview --broker-config reader.json --selection selection.json
agenturm broker cockpit --broker-config reader.json
```

A user-text selection identifies the exact retained occurrence.

```json
{
  "source_id": "harness.pi",
  "session_id": "session-1",
  "event_id": "exact-event-id-from-the-prompt-list",
  "revision": 1,
  "kind": { "kind": "user_text" }
}
```

For Codex operation details, use `codex_mcp_payload` or `codex_edit_detail`
as the nested kind and include its exact `turn_id` and `operation_id` there.
Each operation kind requires its own approval. The cockpit builds these
selections from retained operation metadata.

The broker holds the proof and grant fence while the final sink flushes.
The cockpit acknowledges after its private display is redacted. CLI
previews acknowledge after stdout flushes. A random challenge binds this
acknowledgement to one bounded delivery; missing or incorrect
acknowledgements fail within the display deadline. Every private operation
pins the broker history identity, preventing stale selections from silently
crossing a replaced database. Ordinary queries retain metadata only.

## Source-backed task continuity

The reader can prepare an exact prompt draft using broker-owned sources.
The draft file contains a bounded approved excerpt and editable continuation.
Approval rechecks the source occurrence and current grant, then commits the
local task, source link and approval together while the broker fence is held.

```sh
agenturm broker task draft --broker-config reader.json \
  --source-id harness.pi --session-id session-1 --turn-id turn-1 \
  --event-id exact-event-id --revision 1 --output private-draft.json
agenturm broker task approve-draft private-draft.json --broker-config reader.json
agenturm broker task continuity 1 --broker-config reader.json
agenturm broker task continuity 1 --approval-id 1 --broker-config reader.json
```

Edit the draft's `title` and `continuation` before approval. The stored task
keeps the exact source proof and authored continuation. Revealing the source
excerpt checks its current grant and bytes again. Broker task proofs accept
no local `--config`; only the service chooses source paths. A source-backed
decision needs a draft bound to that task revision or an explicit
`--expected-revision`. Stale task edits return exit code `3`.

The client callback uses a verifier limited to the exact fenced occurrence,
so committing a task makes no nested broker request. The two databases do
not share a transaction. If a final acknowledgement is lost after the local
commit, inspect continuity metadata before retrying. The draft digest prevents
a second approval. Broker backups cover service-owned history; retain the
client state directory separately to preserve its workspace.

For a disposable root-capable Linux VM or container, the repeatable private
reader qualification is available alongside the protocol tests.

```sh
python3 scripts/validate_rust_native_private_content.py --bin-dir /opt/agenturm/bin
```

It uses unused numeric identities without creating system accounts. It
checks direct transcript and database access denial, exact source proof,
grant revocation, broker-backed task approval, and private modal redaction
before Escape, quit and Ctrl-C return control to the terminal.

[Broker protocol](rust-storage-broker.md)
