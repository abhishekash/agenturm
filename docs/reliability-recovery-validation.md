# Reliability recovery validation

13 November 2026. Baseline: `d36446c`; implementation branch: `fix/native-evidence-and-monitoring-reliability`. See the [peer-reviewed implementation plan](rfcs/reliability-recovery-plan.md). No release is published by this work.

## Correctness and trust gates

- Current Codex-shaped synthetic records pass Session ingestion → SQLite → authenticated HTTP → the real headless CLI → frozen JSON → saved-task link. Checks, native file operations, assistant content and scoped usage are asserted together. Failed operations remain attempts; shell no-op and masked-result cases remain unverified.
- OpenCode tests preserve original step traces while joining same-session user-parent relationships. They cover two disjoint turns, late/missing/contradictory parents, streamed text-only updates, oversized parts, plan progress without accounting proof and missing step token/cost fields.
- Concurrent log and span mutation tests reproduce the pointer race and assert one snapshot binds ownership to content. Source indexing tests cover history beyond summary-preview limits, deletion, interrupted migration/restart and cursor uniqueness.
- Local defaults tests cover concurrent token creation, file mode/shape, Host/Origin rejection, credential-bearing redirect refusal, safe non-strict SDK failures, retention enrolment, WAL-aware copy and overwrite/sidecar refusal.
- A real Textual shutdown test pauses publication, accepts an acknowledgement and viewed-error receipt, exits the app and verifies both durable records. The worker drains with the event loop available. Snapshot tests distinguish ready-empty, read-failed and readiness-expired output. Bulk review freezes completion IDs and excludes requests/later arrivals. A 100-completion persistence test verifies repeat bulk actions do not undo review or consume later arrivals, and turn anchors survive SQLite storage/restart across assistant-step changes.

The independent reviewer approved the final snapshot/provenance fixes after a 28-test targeted run and approved implementation commit `ecdf027eca91a0782590e8644e5364142fe49f0d` after a further 38-test run and an authenticated older-run usage probe. Additional focused batches passed 33 security/identity tests, 82 storage/native/local-monitoring tests and 27 recovery-contract tests. These batches overlap and must not be added as a unique test count.

## Full integration record

The full integration run discovered **698 tests**, with **14 optional SDK skips**. Four failures exposed remaining integration contracts: the transport mock target, the new Unknown label, the original task-field size budget and the Workflow reading floor. All four were corrected; the six directly affected tests passed on rerun. The 512 KiB user-field bound is preserved separately from 4 MiB total receipt capacity. The first immutable-code matrix then ran 703 tests: Python 3.12–3.14 passed, while 3.11 exposed a test fixture racing the intentionally asynchronous startup projection. The fixture now waits on advertised health readiness before asserting history results; it does not assume a scheduler delay. Authoritative current-commit results and merge status are recorded in [PR #67 checks](https://github.com/abhishekash/agenturm/pull/67/checks).

## Representative retained-history probe

Reproduce with a new synthetic database; the script refuses an existing build target:

```bash
PYTHONPATH=. .venv/bin/python scripts/benchmark_history.py \
  --db /tmp/agenturm-history-new.db --build \
  --output /tmp/agenturm-history-results.json --samples 10
```

The input contains 5,751 short traces, 31,000 spans and 41,000 logs over 35 days, across 36 sessions. It uses producer-normalized owners and trace timestamps, command/output-sized payloads and recorded MCP calls. The writer adds a separate live trace during measurement. No native transcript, personal database, provider or existing local receiver is read. The explicit fixture has no age retention, so the full multi-week dataset remains available.

This is an in-process **Store read** probe, not an end-to-end network/TUI or native-discovery benchmark. It ran on Linux x86-64, Python 3.12.3, with 8 logical CPUs; another full test process ran concurrently. Host load, storage and payload distribution affect results. The original reviewer’s live API/CPU observations are not a controlled before/after comparison.

| Read | Idle median / max | Under writes median / max |
| --- | --- | --- |
| Activity | 12.20 / 14.72 ms | 14.21 / 25.28 ms |
| Sessions | 340.04 / 368.71 ms | 375.52 / 417.37 ms |
| Metrics | 36.21 / 38.72 ms | 51.66 / 63.92 ms |
| Mcp | 237.10 / 252.98 ms | 249.98 / 266.03 ms |

The measured process reached **41.07 MiB peak RSS** (Linux VmHWM). With no read workload, its background maintenance used **1.12% of one core** over five seconds. This excludes the TUI and real native-reader discovery. The write probe completed 62 batches with no write errors; one derived trace was pending at the pre-close sample and graceful close drains it.

The prepared-schema Store reopen returned in 8.64 ms, while the one-time source/ownership backfill took **212.81 seconds** to become ready. That cost is visible through health stage/progress; API reads return 503 while initial indexing is incomplete, and --once returns partial status 3 after its 15-second deadline. Interrupted batches resume. Existing legacy schema/index creation can add startup work; this is not a portable startup guarantee.

The retained fixture occupied **413.85 MiB** after the write probe. Activity metadata/pointers occupied 60.12 MiB, in addition to source tables and indexes. Indexing trades disk for predictable reads; it is not free storage. New managed stores have 14-day age retention, while explicit existing stores require opt-in. Age deletion is not a hard byte cap and SQLite may retain allocated free pages. The [machine-readable result](validation/recovery-history.json) records per-table sizes and exact final counts.

## Further quality work

1. Expand deidentified schema fixtures across producer versions, keeping the same ownership, clipping and command-result contracts. Existing Claude/Pi regression coverage is not a new claim of live testing those runtimes.
2. Run a longer native-ingestion plus interactive-cockpit soak and the existing reconstruction pilot; measure missed requests, false attention, reconstruction mistakes and repeat use. The Store probe alone does not establish those outcomes.
3. Reduce one-time rebuild latency, remaining session-history scalar lookups and index disk amplification without weakening readiness or source provenance. These refine the existing features.

## Packaged and terminal checks

The built wheel was installed into an isolated environment outside the source checkout. Imports, CLI help, migration/review selectors, automatic token discovery, authenticated cold `--once` and managed retention passed. An actual private tmux journey exercised two recorded prompts and the expanded inspector, followed by Workflow/history; all data and configuration paths were synthetic/isolated. HTML companions were rendered at 1280px light and 390px dark: 16 states, no page overflow or external assets.
