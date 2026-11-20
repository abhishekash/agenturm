# Rust JSONL cursor prefix integrity

Recorded 20 November 2026. The opt-in Rust Pi, Codex, Claude Code, and Kiro readers share a JSONL cursor. Its earlier digest covered the first complete line and the 256 bytes immediately before the acknowledged offset. A same-length rewrite elsewhere in the acknowledged prefix could leave previously admitted evidence looking current. The cursor now records SHA-256 of the entire acknowledged prefix.

Each poll streams that prefix through a fixed 64 KiB buffer and compares it with the committed version 2 cursor before translating new lines. It updates the digest only after a complete line fits the batch. A second prefix check before returning the batch detects a rewrite during the poll. A mismatch emits `possible_source_gap`, starts a new event generation, and replays from the beginning; the Python Store applies source-gap invalidation to old evidence. Device/inode, truncation, first-line identity, frame budgets, and complete-line acknowledgment remain fenced. Existing unversioned cursors are treated as version 1 and conservatively cause one source gap before replay.

The verified prefix is capped at 512 MiB. A source reaching the cap stops with an explicit error before cursor acknowledgment. Each idle poll reads the acknowledged prefix twice, so a 512 MiB prefix can cause up to 1 GiB of source reads per poll. This is an opt-in single-file path, and its read cost needs measurement before wider use. It does not provide an atomic filesystem snapshot: a source may change immediately after the final check and be detected on the next poll. The Python Store remains the only writer and policy authority; native bundled readers remain the default.

## Verification

- Rust unit cases cover a distant same-length body rewrite, migration from a genuine legacy tail-digest cursor, a sparse-file cursor above the 512 MiB ceiling, header replacement, rotation, partial and oversized lines, and turn restoration across acknowledgment.
- An installed Pi binary in a disposable Python receiver verifies that a distant same-length rewrite emits a source gap, makes the old session Unknown, and clears its prior Activity.
- Workspace formatting, Clippy, Rust tests, and installed Pi/Codex/Claude/Kiro receiver tests are the release gates for this change.

This does not establish full Review/export parity or multi-session discovery for any Rust harness.
