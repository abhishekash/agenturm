# Feature-off v2 lifecycle mutation contract

20 November 2026. The Rust storage-service contract crate now has a **data-only v2 proposal** for one host-prepared lifecycle batch. The existing v1 socket API and probe-only installed service still parse only v1. No receiver, Store, listener, or canonical database calls v2. Python Store remains the production writer.

## Scope and wire shape

V2 uses a 64 KiB JSON ceiling. The top-level request requires `version: 2`, a 32-character lowercase hex `request_id`, a bounded `receiver_id`, and one `admit_lifecycle` operation. That operation carries the matching receiver, plugin and source identities; a 64-character lowercase hex source generation; positive lease and policy epochs; expected and next source cursors; a 64-character lowercase hex batch digest; and at most 48 prepared rows. Cursors are at most 512 UTF-8 bytes without control characters. The operation contains no database path, content reference, command reference, discovery member, root gap or arbitrary evidence kind.

Each row carries a source-scoped event ID hash, revision zero, the literal lifecycle kind, a nonnegative signed 64-bit nanosecond timestamp, a canonical metadata-only `payload_json`, a record digest, and bounded native session, worker, turn and operation identities. The payload is at most 4 KiB and contains only a lifecycle state from the Python admission set and an optional bounded reason code. Duplicate event hashes, unknown or duplicate JSON fields, unsupported evidence, malformed canonical payloads and excessive frame bytes are rejected. These bounds narrow the 1 MiB scratch admission experiment; a host must split or reject a prepared batch that does not fit the local-service frame.

The typed success reply echoes the protocol, request and receiver identities, SHA-256 of canonical request JSON, next cursor and batch digest, plus nonnegative dirty sequence and one of `committed`, `replay` or `idle`. A parser binds every reply field to the original request. This is a wire shape only: an untrusted process could fabricate a successful reply without the future authenticated service and durable transaction. No rejection reply semantics have been defined.

## Replay and policy boundary

V2 computes a deterministic replay identity from canonical, lexically sorted JSON fields. Equivalent JSON whitespace and key ordering yield the same digest. Reusing a request ID with changed content is a conflict; a new request ID is distinct. This is pure comparison material, not a persisted receipt. The supplied batch and record digests are **untrusted assertions** derived by Python host admission from its validated original plugin batch. They cannot be recomputed from the prepared rows alone. A future service writer must bind the source lease, capture policy, current and prior cursors, existing evidence and content status, durable request receipt and commit in one transaction before sending a success reply. The request's receiver string does not replace kernel peer authorization or service-owned storage.

The current scratch writer only accepts ordinary metadata-only lifecycle sources and rejects discovery, content, retention, gaps and non-lifecycle history. V2 intentionally carries the same narrow subset and does not expose the scratch writer's `sqlite_path`. The future writer must preserve its explicit unsupported-history checks; this module makes no claim of permission to mutate an arbitrary source.

## Verification

The shared fixture is generated from Python `plugin_admission._validated` and fixes the source-scoped event hash, record digest, batch digest and canonical request digest. Rust tests parse that fixture, compare the replay digest and JSON reordering, reject conflicting replay and unsupported evidence, validate reply binding and exercise unknown, duplicate and oversized input. A Python test recomputes the prepared row and digests independently. Focused Cargo tests, Clippy, formatting and the Python fixture test are the checkpoint gates. None exercises a production socket mutation or database write.
