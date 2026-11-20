# Rust OpenCode SQLite reader checkpoint

Recorded 20 November 2026. The opt-in `agenturm-opencode-harness` reads one explicitly configured OpenCode SQLite database using the current `session_v2`, `session_message` and `session_part` column contract. It shares the format-independent framed hello/poll/ack runner with the existing JSONL adapters. Python Store admission, cursor, lease, capture policy and projection remain authoritative. The bundled OpenCode reader remains the default.

## Evidence and semantics

The adapter sends only content-free user/assistant roles, tool category and reported result, usage counters, lifecycle state and coarse reason, plus opaque session and tool-operation identities. The process facts omit turn identity because a later idle backfill can change the boundary for an already admitted row; Store revisions cannot safely rewrite that identity. Current-schema idle outcomes `succeeded`, `failed` and `interrupted` map to completed, failed and interrupted. Assistant `finish: stop` does not establish a terminal outcome. An unknown idle outcome is paused with an explicit coarse reason. Tool `completed` maps to success unless native shell metadata reports a nonzero exit, timeout, signal or still-running background command. Unknown tool status becomes a diagnostic, not success. Current OpenCode token components are disjoint: input includes input plus cache read/write; output includes output plus reasoning. Derived totals require every component.

The installed receiver test loads the committed OpenCode 2.2.5 schema fixture, compares final lifecycle with the bundled native reader, checks usage and tool Activity with turn identity absent, and searches the admitted payloads for private prompt, result, command and path examples. Other tests cover assistant output before idle, a changed tool revision, a deleted terminal idle that moves prior Completed to Unknown/source_gap and hides old Activity, a same-count delete-and-insert with the same invalidation, an equal-size rewrite with mtime restored, a conservative legacy cursor upgrade, an in-place idle outcome correction, backfilled idle without admission conflict, huge identifier preflight, durable restart, capture revocation, unsupported schema failure before ack, installer symlink rejection, path retarget rejection, and a live WAL writer appending between bounded pages. Pi and Codex tests exercise the extracted shared process runner.

## Source and snapshot boundary

Each poll opens the database with SQLite read-only flags and `PRAGMA query_only=ON`, then reads a single transaction snapshot. The main DB and WAL files each have a 512 MiB admission ceiling. The idle fast path checks device, inode, size, mtime and ctime for both files; ordinary same-size rewrites that restore mtime still change ctime and start a new sweep. At a changed-sweep boundary, three `SELECT 1 … LIMIT` walks count at most 50,001 rows combined and fail before ack if the ceiling is exceeded. The reader also hashes bounded row identities from the prior pinned rowid range and the new range without materializing oversized IDs. A missing or replaced identity, even when row counts remain unchanged, emits `possible_source_gap` before new facts. A v1 metadata-only cursor also causes one conservative gap when upgraded to v2. Python Store durably moves affected prior sessions to Unknown/source_gap, clears usage and relationships, and suppresses Activity. Ordinary in-place status corrections with stable row identity remain revisions rather than gaps.

Rows with payloads over 64 KiB or oversized identity fields are reported as coarse diagnostics without materializing those fields. A sweep pins message, part and session rowid high watermarks, materializes at most 256 source rows per poll and stops before exceeding the receiver's record, byte or structural-token budget. A later main/WAL file metadata change starts a new sweep after the current sweep finishes. Idle polls avoid a full byte hash; changed sweeps scan up to 50,000 identities twice in addition to the bounded row count walks. This has not been measured as a performance improvement. A same-user or filesystem actor that forges device/inode/size/mtime/ctime can still evade the metadata fast path. In-place changes that remove an emitted fact while preserving row identity also need general retraction parity before production replacement.

SQLite's [WAL read-only rules](https://www.sqlite.org/wal.html#readonly) can create sidecars when the source directory is writable. The adapter checks the database header first and refuses WAL mode unless both `-wal` and `-shm` already exist. Adapter SQL never updates the source database; SQLite can still coordinate through an existing SHM file. The adapter does not delete or checkpoint source files.

## Coverage still required for replacement

| Area | Current coverage | Remaining gate |
| --- | --- | --- |
| Discovery | One configured database | Versioned database discovery and multi-source lifecycle |
| Schema | Current split message/part layout | Older layouts and future migrations |
| Activity | Content-free role and tool metadata | Native action, trace and Review/export parity |
| Turn proof | Idle outcome reason only; no process turn ID | Revision-safe mutable boundary and native membership proof |
| Source changes | Bounded revision sweeps, ctime-aware change detection, row-identity gap and session status correction | Forged all-stat rewrites and full retraction parity |
| Packaging | POSIX implementation, Linux test | Platform and release matrix |

This checkpoint is not a production replacement or a performance result. The native Python reader remains the source of richer OpenCode evidence.
