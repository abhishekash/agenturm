# Shared Rust JSONL source runtime

Recorded 20 November 2026. This refactor extracts the opt-in Pi reader's bounded file tail, durable cursor, gap generation, complete-line batching, framed handshake, poll/ack sequencing and source-root validation into `agenturm-jsonl-source`. The Pi-specific parser remains in `agenturm-pi-harness` behind the `SourceFormat` trait. The Python receiver still owns policy, admission, acknowledgment and every database write.

The trait supplies a pinned plugin ID, gap event prefix, initial context from the configured path and first complete line, compact checkpoint and line translation. The first line may be a format header or an ordinary event; it is still translated as a source line. The shared runtime admits all facts from a complete JSONL line together or leaves that line unread. Its cursor checks device/inode, offset, preceding bytes and the full first-line digest. Detected replacement, truncation or checked-region rewrite advances the source generation so replayed native row IDs cannot collide with old revisions. A malformed checkpoint fails before cursor acknowledgment.

This is a behavior-preserving extraction for the installed Pi reader. It does not enable another harness, change the default native Pi reader, widen the content-free evidence contract or assert a speed gain. Existing limits remain: one configured file, 1 MiB line and scan caps, 256 records per batch, the receiver's 8,192 structural-token budget and a 512-character cursor. Source edits outside the checked region remain a documented append-only assumption.

## Verification

| Check | Result |
| --- | --- |
| Rust 1.100.0 workspace formatting, Clippy with warnings denied, and tests | Passed. |
| Rebuilt `agenturm-pi-harness` installed through a SHA-pinned manifest into a disposable receiver | Six Pi conformance tests passed, including same-inode rewrite, restart, revocation and private-content exclusion. |

This extraction is a prerequisite for a Codex format translator that can reuse the same process and replay machinery without duplicating it.
