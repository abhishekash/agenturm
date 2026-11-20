# Deterministic JSONL source-gap replay

Recorded 20 November 2026. The shared Rust JSONL reader serves the opt-in Pi, Codex, Claude Code and Kiro process adapters. A source replacement, header rewrite, truncation, or verified-prefix mismatch emits a `possible_source_gap` diagnostic before reading the new generation. The Python Store commits that diagnostic, the new cursor and prior-session invalidation before acknowledging the batch.

Previously, the gap event ID and cursor were derived from the prior cursor and observed file identity, but its `time_ns` came from file modification time. An identical retry after an acknowledgment failure could therefore carry the same event ID and cursor with a different record digest if only mtime changed. The Store correctly rejected the inconsistent replay, leaving an opt-in reader unable to recover.

The shared reader now emits `time_ns="0"` for its gap diagnostic. Zero means the gap's wall time is unknown; it does not claim that the source changed at the Unix epoch. The batch stays identical when file identity and bytes stay identical while mtime changes. OpenCode already uses an unknown timestamp for its analogous gap. No cursor or wire version changes are needed.

Rust tests pin both rotated-file and same-inode header-rewrite retries. Each sets a different file mtime between two polls from the same acknowledged cursor and compares the complete batches, including the diagnostic and next cursor. `cargo fmt --all -- --check`, workspace Clippy with warnings denied, and workspace tests passed on this branch.

This fixes mtime-only replay drift. If the replacement file's **bytes or identity** change between retries, the process has genuinely different source evidence; this change does not provide a durable raw-input spool or promise recovery from an arbitrarily mutating source. The process readers remain opt-in, Python Store remains the sole database writer, and no performance gain is asserted.
