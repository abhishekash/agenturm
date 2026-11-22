# Rust storage broker contract

This contract is for a fresh standalone Rust history database under a dedicated Linux service identity. The current storage service provides local socket and peer-identity checks to reuse. Its existing probe and experimental Python-era storage operations stay on their existing endpoints.

## Native history endpoint

Run `agenturm-storage-service` with the existing data root, transport directory,
receiver and client identity options, plus `--native-sources ID[,ID]`. The
allowlist grants 1–64 distinct source IDs to the collector and any configured
reader. The collector can also read. Reads without an explicit source
select only those IDs.
This mode uses `agenturm-native-storage-v1.sock`, its own listener lock and a
broker-owned `history.sqlite`. It accepts an empty private data root or an
existing native history database with its expected sidecars. The service holds
the native writer lease for the lifetime of the listener. On restart after a
crash, it can reclaim a stale socket while holding the instance lock. It
rechecks transport ownership and requires the old socket to have the expected
owner and mode and refuse a nonblocking connection before removing it.

Native requests use a big-endian u32 length followed by UTF-8 JSON. The
envelope contains version `1`, a 32-digit lowercase hexadecimal
`request_id`, the configured `receiver_id` and an `operation`. The
available operations are `source_cursor` with a `source_id`, `admit`
with a typed `ObservationBatch`, `query_sessions` with a `SessionQuery`,
and `query_history` with an `ObservationQuery` and a selection. Selections
cover observations, all workers on an exact turn, one exact worker or
workerless turn, and linked traces. The linked trace importer and native
session source must both be within the grant. The reply binds the version, request ID,
receiver ID and SHA-256 of the exact request frame body. Both peers
authenticate kernel credentials before exchanging bodies. Each write side
half-closes after one frame. Frames have a 32 MiB limit and a two-second
I/O deadline. The client connects without blocking on a full listener
backlog. Admission retries use the same deterministic batch ID.

Observation pages return at most 2,048 rows, with `has_more` and the original
snapshot watermark for keyset continuation. Session queries have the Store's
256-row limit and return their typed continuation. Source filters are applied
before ordering and paging. An expired snapshot returns the minimum live
watermark in a typed error. The Rust `NativeStore` client takes the absolute
socket path, receiver ID and broker UID/GID. It provides the kernel's
`ObservationStore` port and typed session and linked trace reads. The
broker assigns each history a persistent 128-bit `history_identity`.
When a cockpit binds its workspace to that identity, the broker verifies
it within each history read and exact anchor or event check. A replacement
history at the same socket cannot satisfy the old binding.

The local listener test verifies socket and database ownership and modes,
denial of same-UID peers, clean socket removal and restart after SIGKILL.
Unexpected paths and active sockets are preserved. The contract tests
exercise peer-first checks, malformed frames, a partial-frame timeout and
request/reply correlation. The admission tests exercise replay after a later
commit and reopen. Read tests verify allowlisted cross-source pagination and
exact worker selection.

## Private evidence delivery

Private delivery requires a separately configured `--native-reader` and a
broker-owned `0600` file supplied as `--native-content-config PATH`. The
file contains version `1` and a `sources` array of resolved source bindings.
Each entry defines its source and plugin IDs, absolute source, host and
adapter paths, a SHA-256 adapter pin and the declared adapter features. The
broker reopens and validates the configuration and adapter pin when proving
an occurrence. The reader supplies only the pinned history identity and
the exact source, session, event and revision. Codex operation detail also
requires the turn and operation IDs.

The broker uses the same source-byte and current-grant proof as local
Agenturm. It holds the Store writer fence while sending the bounded private
response. The reader ACKs the exact request and a random reply challenge
after the display sink flushes. The cockpit waits for redaction before
sending the ACK. The broker then releases the fence and confirms completion.
An invalid, absent or late ACK prevents completion. The acknowledgement
deadline is 32 seconds. Other operations on this single broker listener
wait while a private display is outstanding.

The Rust client can also consume a typed, verified user prompt for a task
draft, approval, decision, or explicit excerpt read. The reply includes the
proven text and occurrence metadata plus a fingerprint of the current grant.
The client runs a bounded local callback before acknowledging the reply.
A failed callback closes the connection and releases the broker fence. Each
later use must prove the exact occurrence and current approval again.

## Principals and deployment

The non-root broker owns a `0700` data directory and every database, WAL, shared-memory, lease and maintenance file inside it. Its listener verifies the filesystem owner, mode and identity of the socket directory and lock before use. The collector and cockpit use Linux UIDs distinct from the broker. Each configured caller is identified by `SO_PEERCRED` UID and primary GID before the service reads its request body. The client also verifies the broker UID and GID before sending requests. Receiver identity and protocol version bind each exchange to its configured endpoint.

The optional `--native-reader UID:GID`, `--native-grant-admin UID:GID` and
`--native-maintenance UID:GID` options assign separate roles. Each must
use a distinct non-root UID. The socket group is `--client-gid`. Additional
roles whose primary GID differs require supplementary membership in that
group to reach the socket. Kernel peer checks still use their configured
primary GIDs. The collector and reader may query effective approval for
an exact source, kind and fingerprint. The grant administrator may also
approve or revoke that binding. Responses report the active epoch or
revocation status and omit grant nonces.

The collector owns native discovery, source supervision, format decoding and ACK delivery. The cockpit owns terminal state. Children must be denied the broker socket as well as the database. Children running as the client UID can impersonate that client unless their OS boundary denies access. Process isolation and deployment proof remain separate gates. The broker refuses startup on insecure ownership or unexpected database identity. Clients configured for broker access fail closed if the broker is unavailable.

## Operations and authority

| Operation | Authorized role | Native implementation and result |
| --- | --- | --- |
| Read current source cursor | Collector, restricted to configured source IDs | `ObservationStore::source_cursor`. Return the opaque cursor or an explicit unset result. |
| Admit an observation batch | Collector, restricted to configured source IDs | `Store::commit_batch`. Accept typed stored observations, expected and next cursor and deterministic request ID. Derive and verify the ID from the exact validated batch. Return committed or replay only after the transaction. The collector sends ACK after that result. |
| Read history and linked traces | Cockpit and authorized reader | `Store::query_observations`, exact turn/worker queries and linked trace query. Return a bounded page, the durable snapshot watermark and the next keyset cursor. Turn reads require an exact source and session. |
| List sessions | Cockpit and authorized reader | Store session-catalog query with its native filter, position and snapshot semantics. A partial result includes a continuation. |
| Grant, revoke, check approval | Grant administrator. Collector and reader can check approval. | Store content grant operations bind source, kind, source fingerprint and epoch. A revoke only applies to the exact fingerprint, and approval mutations remain effective across restart. |
| Reveal private evidence | Separate configured reader, with an active grant for the exact source binding and content kind | The broker proves the exact retained occurrence and holds the disclosure fence through the display ACK. |
| Verify a prompt for task continuity | Same reader, with an active user-text grant for the exact source binding | The broker holds the source grant and retention fence while the client consumes the typed prompt, through the same challenge ACK. |
| Prune | Maintenance operator | `Store::prune_observations_before`. Preserve checkpoints and replay receipts, advance the retention watermark and reject invalidated traversals. |
| Back up and check integrity | Maintenance operator | The broker verifies the snapshot, publishes it to its configured private backup root and returns its assigned filename. Integrity returns a status without diagnostic rows or paths. |
| Offline restore | Broker filesystem owner | Restore one named broker backup into a fresh private data root while holding the native instance lock. The restored history preserves its identity and starts with private content approval disabled. |

The caller provides bounded identifiers and approved operation fields. The broker owns source authorization, database paths and maintenance destinations. No caller supplies arbitrary SQL, a SQLite path or a filesystem path to reveal. The collector may express a source change only through the configured source policy.

Set `--native-backup-root` to an existing broker-owned `0700` directory
separate from the history and transport roots. With the broker shut down,
the broker account can supply `--native-restore NAME` with the assigned
backup filename and change `--data-root` to a fresh broker-owned `0700`
directory. Restore accepts only an owner-private, detached native SQLite
backup from the configured backup root. It verifies and atomically
publishes the snapshot without overwriting existing history. Remove
`--native-restore` when starting the listener on the restored data root.

## Wire and failure rules

- Use a new versioned local socket protocol for standalone history. Each connection carries one size-bounded request and one response. The response binds version, receiver, request identity and result. Verify authenticated peers before reading or writing body bytes. Limit frame bytes before allocation, reject unknown fields, bound connection count and make read/write deadlines finite.
- Use the existing typed observation and cursor limits. The admission frame must accept the largest batch the collector can validly submit. If encoding overhead would exceed the frame, document and enforce a shared producer limit before polling. Keep each source checkpoint and its complete batch together. Page retrieval must cap response bytes as well as row count. Return the last emitted ordering key if a size cap shortens the page, with the original watermark preserved.
- Persist canonical request identity, observation revisions and cursor atomically. An exact retry after a dropped reply returns the existing receipt even if a later cursor committed. An ID with different content returns a conflict. A stale expected cursor, source-policy mismatch or transaction failure produces no ACK. Commit interruption before the SQLite commit leaves the previous state intact. Interruption after commit permits exact replay.
- Return stable categories for unauthorized scope, invalid request, conflict, stale cursor, expired snapshot and temporary unavailability. Protocol errors and unauthenticated peers receive no privileged response. Keep paths, private payloads and database internals out of diagnostics. Treat transport failures as ambiguous and retry the exact admission within the existing supervised retry policy.
- Use a shared ownership lease for the service's write lifecycle and serialize admission, retention and approval mutation. Grant reads cannot replace the delivery fence. Backups preserve the committed watermark and receipts. Any raw proof material needed for an authorized capability stays on the broker side of the socket.

C3 owns admission and the collector port. C4 owns read/query, grant and retention ports over the same broker identity and store. C5 joins them on a distinct-principal host, including verified client filesystem denial, commit crash recovery, retention, backup and restore. [Current storage identity probe](../validation/rust-storage-distinct-principals.md) · [Standalone Rust history](rust-collection-config.md)
