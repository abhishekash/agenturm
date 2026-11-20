# Feature-off Rust storage-service contract

20 November 2026. `agenturm-storage-service-contract` is an internal transport and peer-admission experiment. It has no listener, production caller, database handle or mutation operation. Python Store remains the receiver database writer; WorkflowStore and TaskStore are separate Python mutation paths.

## V1 boundary

One local Unix socket connection carries one request and one response. Each is a four-byte big-endian length followed by 1–65,536 JSON bytes; the sender half-closes its write side. Each whole frame has one two-second monotonic deadline; every socket read/write uses only the remaining time, so drip-fed bytes cannot renew it. The strict v1 request accepts a 32-character lowercase hex request ID, bounded receiver identity and only a `probe` operation with `protocol` or `receiver_identity` topic. Unknown operations, versions, fields, duplicate fields, malformed JSON, incomplete frames, excessive lengths and trailing bytes fail closed. The reply echoes version, request ID and receiver ID; the client rejects any mismatch. `protocol_ready` certifies only this contract, not a storage service or writable database.

On Linux, each side reads `SO_PEERCRED` **before** parsing peer bytes. The server policy binds its current effective UID and PID, requires a distinct non-root client UID and exact GID, and rejects identity changes after construction. The client policy similarly pins its own identity and checks the expected distinct service UID/GID before accepting a reply. Other Unix targets return unavailable for the kernel peer check. This authenticates local process identities only. It does not establish a protected socket path, service-owned database/ACL, client authorization to mutate a source, or legacy writer exclusion.

The pure replay comparison computes a digest of the validated canonical request: the same request ID and bytes are exact, a reused ID with changed topic or receiver is a conflict, and a different ID is unrelated. Nothing persists this token. A future writer must commit its durable receipt with the data transaction before acknowledging a mutation.

## Verification and remaining gates

Eleven focused Rust tests exercise a kernel-backed socket round trip, client/server peer denial before byte parsing, receiver/reply mismatch, malformed/oversized/incomplete/trailing frames, drip-fed request and response frames past one monotonic deadline, unsupported mutation and duplicate-key denial, invalid IDs, policy identity checks and replay comparison. The round trip uses a test-only synthetic policy because both ends of the socket pair run under one UID; no installed distinct-principal service is tested. Cargo formatting, focused tests and Clippy were run on the isolated worktree. No performance gain or production cutover is claimed.

The next service increment needs a separately reviewed typed mutation API, host-owned canonical path, durable replay and policy checks, a distinct installed service identity with protected directory/socket, routing of every Python mutation path, old process/descriptor quiescence, adversarial legacy-binary fresh-open tests and recovery drills. See the [writer cutover gate](rust-writer-cutover-gate.md) and [execution plan](../rfcs/rust-plugin-kernel-plan.md).
