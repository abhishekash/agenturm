# Rust storage broker contract

This contract is for a fresh standalone Rust history database under a dedicated Linux service identity. The current storage service provides local socket and peer-identity checks to reuse. Its existing probe and experimental Python-era storage operations stay on their existing endpoints.

## Native history endpoint

Run `agenturm-storage-service` with the existing data root, transport directory,
receiver and client identity options, plus `--native-sources ID[,ID]`. The
allowlist grants 1–64 distinct source IDs to that single collector and reader
principal. Reads without an explicit source select only those IDs.
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
`ObservationStore` port and typed session and linked trace reads.

The local listener test verifies socket and database ownership and modes,
denial of same-UID peers, clean socket removal and restart after SIGKILL.
Unexpected paths and active sockets are preserved. The contract tests
exercise peer-first checks, malformed frames, a partial-frame timeout and
request/reply correlation. The admission tests exercise replay after a later
commit and reopen. Read tests verify allowlisted cross-source pagination and
exact worker selection. Separate reader roles, grant and retention authority,
and private evidence delivery remain open.

## Principals and deployment

The non-root broker owns a `0700` data directory and every database, WAL, shared-memory, lease and maintenance file inside it. Its listener verifies the filesystem owner, mode and identity of the socket directory and lock before use. The collector and cockpit use a different Linux UID. Each configured caller is identified by `SO_PEERCRED` UID and primary GID on every connection. The client also verifies the broker UID and GID before sending requests. Receiver identity and protocol version bind each exchange to its configured endpoint. Each role has a configured UID/GID and may share a UID only if that principal is deliberately authorized for the union of those roles.

The collector owns native discovery, source supervision, format decoding and ACK delivery. The cockpit owns terminal state. Children must be denied the broker socket as well as the database. Children running as the client UID can impersonate that client unless their OS boundary denies access. Process isolation and deployment proof remain separate gates. The broker refuses startup on insecure ownership or unexpected database identity. Clients configured for broker access fail closed if the broker is unavailable.

## Operations and authority

| Operation | Authorized role | Native implementation and result |
| --- | --- | --- |
| Read current source cursor | Collector, restricted to configured source IDs | `ObservationStore::source_cursor`. Return the opaque cursor or an explicit unset result. |
| Admit an observation batch | Collector, restricted to configured source IDs | `Store::commit_batch`. Accept typed stored observations, expected and next cursor and deterministic request ID. Derive and verify the ID from the exact validated batch. Return committed or replay only after the transaction. The collector sends ACK after that result. |
| Read history and linked traces | Cockpit and authorized reader | `Store::query_observations`, exact turn/worker queries and linked trace query. Return a bounded page, the durable snapshot watermark and the next keyset cursor. Turn reads require an exact source and session. |
| List sessions | Cockpit and authorized reader | Store session-catalog query with its native filter, position and snapshot semantics. A partial result includes a continuation. |
| Grant, revoke, list approval | Grant administrator. A scoped reader may check the effective grant for its evidence. | Store content grant operations bind source, kind, source fingerprint and epoch. Approval mutations remain effective across restart. The grant administrator must be authorized independently of a reader. |
| Reveal private evidence | Cockpit, after exact current-grant verification | Resolve the exact retained event and revision, apply the source proof and serialize delivery with revocation. A check followed by a separate reveal on the client cannot enforce this ordering. Production private reveal stays disabled until the broker can enforce the full fence. |
| Prune | Maintenance operator | `Store::prune_observations_before`. Preserve checkpoints and replay receipts, advance the retention watermark and reject invalidated traversals. |
| Back up, restore, integrity check | Maintenance operator | Reuse the native history/workspace backup and restore contracts. The broker takes maintenance destinations from its own configuration. Restore runs with exclusive access, revalidates ownership and starts with private content approval disabled. Workspace sidecars stay client-owned until the workspace authority has its own contract. |

The caller provides bounded identifiers and approved operation fields. The broker owns source authorization, database paths and maintenance destinations. No caller supplies arbitrary SQL, a SQLite path or a filesystem path to reveal. The collector may express a source change only through the configured source policy.

## Wire and failure rules

- Use a new versioned local socket protocol for standalone history. Each connection carries one size-bounded request and one response. The response binds version, receiver, request identity and result. Verify authenticated peers before reading or writing body bytes. Limit frame bytes before allocation, reject unknown fields, bound connection count and make read/write deadlines finite.
- Use the existing typed observation and cursor limits. The admission frame must accept the largest batch the collector can validly submit. If encoding overhead would exceed the frame, document and enforce a shared producer limit before polling. Keep each source checkpoint and its complete batch together. Page retrieval must cap response bytes as well as row count. Return the last emitted ordering key if a size cap shortens the page, with the original watermark preserved.
- Persist canonical request identity, observation revisions and cursor atomically. An exact retry after a dropped reply returns the existing receipt even if a later cursor committed. An ID with different content returns a conflict. A stale expected cursor, source-policy mismatch or transaction failure produces no ACK. Commit interruption before the SQLite commit leaves the previous state intact. Interruption after commit permits exact replay.
- Return stable categories for unauthorized scope, invalid request, conflict, stale cursor, expired snapshot and temporary unavailability. Protocol errors and unauthenticated peers receive no privileged response. Keep paths, private payloads and database internals out of diagnostics. Treat transport failures as ambiguous and retry the exact admission within the existing supervised retry policy.
- Use a shared ownership lease for the service's write lifecycle and serialize admission, retention and approval mutation. Grant reads cannot replace the delivery fence. Backups preserve the committed watermark and receipts. Any raw proof material needed for an authorized capability stays on the broker side of the socket.

C3 owns admission and the collector port. C4 owns read/query, grant and retention ports over the same broker identity and store. C5 joins them on a distinct-principal host, including verified client filesystem denial, commit crash recovery, retention, backup and restore. [Current storage identity probe](../validation/rust-storage-distinct-principals.md) · [Standalone Rust history](rust-collection-config.md)
