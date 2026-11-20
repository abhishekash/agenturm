# Codex directory member read gaps

Recorded 20 November 2026. The opt-in Codex directory process reader now quarantines a previously acknowledged file when it can no longer establish that file's continuity after successful directory enumeration. Python Store remains the sole writer, policy authority, and acknowledgment authority. No performance gain is claimed.

## Admission and recovery

The shared JSONL reader distinguishes source continuity failures from invalid cursors, parser or format errors, and batch budgets. A failed open, metadata or identity check, read I/O, incomplete previously observed header, or changed source during verification is a continuity failure. A new file that vanishes between directory enumeration and open is reported as unavailable, rather than as an idle successful poll. Explicit single-file readers retain their prior missing-new-file behavior.

For a discovered file with a **nonempty durable checkpoint**, the Codex directory reader emits one deterministic member-scoped `possible_source_gap` diagnostic and advances its global cursor to `d3`. It retains the old per-file checkpoint in the same Store transaction. Store ownership proof decides the invalidation scope: exact member ownership leaves healthy sibling sessions visible; legacy, missing, or ambiguous ownership conservatively invalidates the whole source. Prior invalidated session IDs remain Unknown with usage, relationships, and Activity suppressed. The diagnostic uses a cursor-derived logical sequence, not a wall-clock failure timestamp. Repeating the same pre-gap poll produces identical bytes and is accepted as an exact replay.

While `d3` is active, the reader retries that file first. Another read failure is a worker error, so source health degrades without producing repeated diagnostics or advancing the cursor. On a successful read it returns to `d1` and resumes the normal bounded scan. Because the old file checkpoint is retained, the shared JSONL reader compares the acknowledged prefix and creates a fresh event generation if the file was rewritten, even when device and inode stayed the same. If the file disappears from enumeration, a second deterministic member-removal gap clears its checkpoint and frees discovery capacity; repeating that removal poll is byte-identical. Deletion followed by a same-identity reappearance remains a separate source-history limit.

`d3` is a new durable cursor mode for this opt-in reader. An older Codex process binary does not understand it and will fail closed on restart. Keep the installed binary that admitted the outage until it recovers or upgrade to a compatible binary. Resetting capture is an explicit data-loss action that erases process evidence and checkpoints; it is not an automatic downgrade path. The default bundled native reader is unaffected.

An unreadable file with **no prior checkpoint** reports a worker failure without a gap: the receiver has no prior facts from that member to invalidate. The same rule applies to cursor, parser, format and budget errors, which are not silently recast as source gaps. A healthy sibling may remain last recorded while the source's availability overlay says unavailable; its current freshness is partial until polling resumes.

## Verification

Rust tests simulate a file vanishing between enumeration and open, byte-identical retry, one-shot `d3` behavior, a later root outage, a separate replayable member removal, recovery to `d1` with fair scanning, and rejection of parser failure as a continuity gap. Installed Codex tests exercise permission-denied read after two admitted files, retained member checkpoint, scoped Unknown state and Activity suppression, a second failure, restart, same-inode rewrite with generation change, and a new healthy session. An existing asynchronous root-outage fixture now waits for both files to be admitted before taking the root down; it had previously allowed a late first admission to appear after recovery.

On this branch, an isolated no-debug Cargo target passed workspace formatting, Clippy with warnings denied, and all workspace tests. The installed directory suite passed nine tests with the rebuilt Codex and Rust host binaries; its prior revision was rerun twice after fixing the fixture race. Focused Store admission, discovery, gap, projection and runtime tests passed 58 tests. GitHub CI is pending at this checkpoint.

This change does not establish full native-to-process Review/export parity, a file-system sandbox, or a default reader cutover.
