# Rust/plugin migration: first implementation checkpoint

Branch `architecture/rust-plugin-kernel`; baseline `ca202c7`; collection date 19 November 2026. This checkpoint starts the reviewed plan. It is not the completed microkernel or a Rust performance result.

## Implemented

- Extracted native file discovery specifications for Codex, Claude Code, Pi and Kiro into `observatory/native_sources.py`. The scanner accepts an injected specification list. Kiro directory and file rules now come from its specification instead of runtime branches in the scanner. Existing session parsing, policy and Store ownership remain intact. OpenCode discovery is separate and still needs its own adapter extraction.
- Added a synthetic registered-source test that filters directories/files without editing scanner dispatch, plus duplicate/invalid ID checks.
- Repaired `benchmark_app.py`: it had closed its Store before trying to read through the live receiver. It now waits for derivation and history readiness while the receiver stays open, with bounded failure.

## Synthetic diagnostic baseline

Command: `python scripts/benchmark_app.py --idle-seconds 10` using the project Python 3.12.3 environment. Each scenario starts a fresh process/private temporary database; all seven views are warmed. The benchmark excludes native discovery and real terminal rendering. Twelve HTTP refreshes and ten idle seconds are descriptive samples, not a reliable p95 or a pass/fail performance gate. The source-discovery extraction does not run in this benchmark (`local_agents=False`), so these numbers describe the unchanged receiver/feed/UI path plus the benchmark setup repair.

| Scenario | Spans | Ingest + derivation | First refresh | Warm median refresh | Idle CPU | Current RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| small | 200 | 0.387 s | 135.0 ms | 26.3 ms | 5.90% | 86.4 MiB |
| large | 20,000 | 14.448 s | 142.0 ms | 25.8 ms | 7.98% | 93.3 MiB |
| payloads | 2,000 | 3.367 s | 148.8 ms | 28.4 ms | 6.88% | 103.7 MiB |

The initial diagnostic attempt failed because the script closed read connections before feed refresh. A later payload run exposed a second setup issue: an empty derived queue did not yet mean history indexing was ready. Both failures were reproduced and the script was fixed before collecting the table above. Raw measurements are in [rust-plugin-phase0.json](rust-plugin-phase0.json).

## Validation and next gate

The final discovery, benchmark-statistics, Kiro and native-session suites passed 54 tests. `git diff --check` passed. These runs do not prove a new external harness can complete a source-to-export journey.

Before choosing the Rust accelerator, register a representative native-history workload and measure source scan/parse CPU, ingest/derivation queue wait, SQL/write time, WAL stalls, feed serialization and UI event latency separately. Run the predeclared request-count and duration gates from the [execution plan](../rfcs/rust-plugin-kernel-plan.html). A parser speedup alone cannot decide the migration.

The next implementation increment is the canonical ingestion/query/policy ports and one complete adapter including cursors, lifecycle and source-to-Review/Activity/export proof. Only after those contracts and the plugin host are sound should a production Rust worker enter the path.
