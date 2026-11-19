# Native reader registry and phase 0 profiling checkpoint

This change moves OpenCode's reader construction, polling, MCP server configuration and status aggregation behind a trusted in-process reader registry. `LocalAgents` accepts a list of reader specifications; a disabled reader is not constructed or polled. IDs are validated and cannot conflict with JSONL source IDs. The existing OpenCode status keys remain available during migration. A synthetic injected reader exercises the host loop and contributes a status record without adding a reader-specific branch.

The registry is an incremental composition seam. It is **not** a process plugin host, a stable third-party SDK, or a complete adapter extraction. JSONL parsing and Kiro hierarchy rules still live in `local_agents.py`; OpenCode still owns its SQLite source read. The Python Store remains the sole Agenturm database writer.

## Synthetic profiling evidence

- On this host, `scripts/benchmark_resources.py` used 12 synthetic OpenCode sessions, 12 messages per session and six 8 KiB tool parts per message. After initial import, eight idle polls averaged 0.036 ms CPU and emitted no spans. Eight polls following writes to an unrelated table averaged 37.104 ms CPU and emitted no spans. Eight polls following one tool-part update averaged 38.227 ms CPU and emitted eight span updates. The previous helper was unable to run against the current `ingest(..., skip_unchanged_native=...)` signature; this change updates its counting stub.
- `scripts/profile_ingest_writes.py --batches 20 --readers` used a generated database and HTTP ingestion. Twenty writes took 14.07–48.45 ms each. Across all 20, instrumented span `executemany` calls totaled 194.17 ms CPU; activity `drain` totaled 57.21 ms CPU; commit totaled 10.69 ms CPU and 41.14 ms wall time. The generated diagnostic JSON is kept outside the repository at `/tmp/agenturm-ingest-diag-phase0.json`; it contains no personal source files.

These are short diagnostics, not a reproducible latency acceptance result or proof of the user's live slowdown. OpenCode's file fingerprint invalidates on any database write. Skipping the unrelated-table case without a reliable per-table change marker could lose session evidence, so that behavior is unchanged. The profile suggests focusing next on span write preparation and source-specific invalidation before choosing a Rust worker target. UI render cost and larger sustained workloads are not yet measured.

## Validation

The focused suite covers injected/disabled/invalid readers, native discovery, native sessions, OpenCode, startup and bootstrap: **82 tests passed** using the repository virtual environment. `scripts/benchmark_resources.py` and the 20-batch ingest diagnostic completed. `git diff --check` passed.

No Rust executable or production plugin process is enabled by this change. The reviewed [migration plan](../rfcs/rust-plugin-kernel-plan.html) remains the source of later gates.
