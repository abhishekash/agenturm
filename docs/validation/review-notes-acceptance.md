# Review-notes acceptance audit

Current snapshot: **2026-11-14T11:51:14.142427+00:00**, main **`e434fcf54b83a19ecdbb94a1e1712f4a43772ec6`**. Core E/F/G, replay and the acceptance harness are merged. **The complete review-notes plan is not yet accepted:** combined CI, current native-version qualification, final resource/soak/storage acceptance and external pilot/live-runtime evidence remain open.

## Current decision

- **Delivered:** source-backed Overview; exact operation/Findings evidence; frozen Git and approved drafts; evidence-qualified MCP/Usage/progression; typed investigations; persistent shared headless attention; isolated acceptance harness.
- **CI:** [combined main run 43591334638](https://github.com/abhishekash/agenturm/actions/runs/43591334638) is `in_progress` at this snapshot (8 success, 4 running). Earlier E `928cae0` and F `1c0b795` each passed 12 checks. Six superseded duplicate runs were intentionally cancelled. A cancellation or an earlier green head does not establish all-green CI for the rebased merge heads.
- **Next delivery:** OpenCode 2.2.5 adapter/provenance/cache/reasoning positive qualification and Sessions covered-lookup `79ac99d` are pending integration. Do not attribute either to this frozen main yet.
- **Acceptance still open:** matched cold/idle/API targets, final one-hour soak, sustained RSS/modal retention, index/WAL growth and drains, live Claude/Pi, and consented reconstruction/resumption/repeat-use outcomes. Pilot denominator remains **0 participants / 0 sessions / 0 repeat-use observations**.

## Audit basis

The user-owned `agenturm-review-notes.md` is read-only and unchanged: SHA-256 `8352ce8a78edaea075083e59850ddd96bb70868164e3c146b11cd97b0a617ce3`. Original review: main `d36446c`, 13 November 2026, version 0.2.3. All **27 findings and 11 recommendations** are accounted for below, with implementation, regression evidence, delivery and limits separated. [The global plan](../rfcs/review-notes-completion-plan.html) and [machine-readable ledger](review-notes-acceptance-data.json) use the same snapshot. Historical code/test links remain pinned to the exact inspected/tested heads; they are evidence, not a claim that every test ran on final main.

## Integration and CI ledger

| PR | Delivery | Exact PR head | CI qualification | Merge commit |
| --- | --- | --- | --- | --- |
| [#79](https://github.com/abhishekash/agenturm/pull/79) | MERGED | `920f44e517bc5f8583521dbae5664fe3fff37d1e` | 12 success | `5e0c39786066fb88c0e270418573e361aeb6cf9a` |
| [#80](https://github.com/abhishekash/agenturm/pull/80) | MERGED | `0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7` | 12 success | `1e8bdcbe5337ca52e7fdfe5b30c793c1e48764c8` |
| [#81](https://github.com/abhishekash/agenturm/pull/81) | MERGED | `c84e9bcb506fa498bfbcb4badc03c6cfb12b21e6` | 4 cancelled, 8 success | `093bf87866f92c0af1c23522082b1f7c8d48cae7` |
| [#82](https://github.com/abhishekash/agenturm/pull/82) | MERGED | `e9280e622e7874327d4ea357369f8d0912121a74` | 12 cancelled | `e434fcf54b83a19ecdbb94a1e1712f4a43772ec6` |
| [#83](https://github.com/abhishekash/agenturm/pull/83) | MERGED | `ca9b51918a7932b492a1f67ccb2d066dde34ca95` | 12 success | `728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c` |
| [#84](https://github.com/abhishekash/agenturm/pull/84) | MERGED | `3368e09cdf6cb7a4cb582e588e0959fc76ef5f40` | 12 success | `11a3b7b72e0e18f8d92650d6e14f57f3a8a72d3d` |
| [#85](https://github.com/abhishekash/agenturm/pull/85) | MERGED | `b56a353654696941489a1d0853d1c68f1fc94c4c` | 4 cancelled, 8 success | `1aa1948f9723ac4da88a0b5e0ea93322bb8f0f39` |

Earlier corrected E/F heads had complete green matrices before rebasing. Current merged PR81/82/85 duplicate runs were cancelled intentionally; main's active matrix is the combined gate. PR84 and the earlier PR79/80/83 head matrices remain historical successes. No new merge-head or combined all-green claim is made.

The original E failures exposed positive fixtures missing required receiver/single-owner/typed-command proof. The corrected contract retained strict negative safeguards. The original F matrix exposed a real lightweight headless Review crash and doubled Activity traversal; the fix restored exact once-per-refresh visits and Review/native parity. These failure histories are retained in the JSON ledger and linked implementation reports. Focused gates overlap and are not summed as independent coverage.

## Functional preflights

| Generated preflight | Actual result | Limit |
| --- | --- | --- |
| Native concurrency, main e434fcf | Two real tmux cockpits plus one headless watcher; quiet baseline; exactly one fixture notification; 210 newer unrelated records per transition; acknowledgement/restart and exact remote resolution; all owned exits clean, no errors. | One generated generic native-contract scenario. Notification sink is mocked; no native request approved. Not alert precision or live OS/runtime compatibility. |
| Baseline keyboard smoke 4 | 24 checks passed: prompt arrows/Home/End/read/section/help, 120×36/80×24/60×18, 50×14 too-small, normal quit and terminal restoration. | Functional baseline only; final candidate signal/modal/keyboard matrix remains open. |
| Tiny baseline idle preflight 2 | No errors; cockpit/receiver clean exit; `smoke=true`, `publishable=false`. | Harness sanity only. No CPU/readiness/RSS target can be closed from this short run. |

Raw local paths, exact source/harness heads and result hashes are recorded in the JSON ledger. Raw directories contain generated receiver credentials; publish only sanitized reports, not whole artifact directories.

## Resource acceptance ledger


The targets remain **cold readiness <30 s**, **complete-monitor idle CPU <5% of one core**, and **Sessions/MCP p95 <100 ms** on a matched corpus/reference host. Different corpora and workloads are kept separate.

| Experiment | What is supported | What remains open |
| --- | --- | --- |
| [PR72 retained-history pair](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/history-read-economics.md) | Cold 184.38→112.96 s; idle CPU 23.63→15.90%; under-write Sessions p95 373.58→284.90 ms and MCP 246.09→142.60 ms on that pair. | All stated targets remain missed. Earlier lock-failure comparison was rejected. |
| [PR73 idle watch](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/idle-history-watch.md) | Matched idle CPU 17.80→14.90%. | No new cold/read or final combined memory claim. |
| [PR74 source preparation](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/source-preparation.md) | Three measured consumers: median 21% canonical / 34% fallback preparation reduction; individual regressions are retained. | This isolated work is not whole-monitor CPU or a general speedup. |
| [PR79 normalized Sessions pair](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/session-projection-batch.md) | Separate 6,047-trace synthetic corpus: idle p95 312.93→220.50 ms; under-write p95 441.02→286.34 ms; 91 accepted writes, 89 during reads, zero errors. | Still >100 ms. Excludes receiver HTTP/UI/native discovery/cold/RSS. |
| [PR83 shared MCP grouping](https://github.com/abhishekash/agenturm/blob/ca9b51918a7932b492a1f67ccb2d066dde34ca95/docs/validation/mcp-inventory-grouping.md) | Same 6,047-trace corpus: both pairs lower median; first under-write p95 230.88→239.96 ms, unchanged-source repeat 343.53→307.98 ms. | Mixed tails; no general p95 improvement, both >100 ms. Retained inventory remains proportional to source population. |
| [G isolated index cost](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/docs/validation/typed-investigations.md) | 6,000 synthetic events: median build 608.53→4,017.29 ms; DB +19.4%; typed reads measured in isolation. | Predates final identity/dirty-queue/rename integration. No combined monitor resource acceptance. |
| Merged PR84 source qualification / linked-log seeks | Frozen serialized output equivalence across 14 fixture groups; raw operation-fact projections 237→134; 46 focused tests pass. A narrow linked-log index has independent output/query gates; its timing is diagnostic. | Diagnostic profiler samples only; no controlled whole-cold or monitor CPU claim. |

The earlier one-hour generated active/probed run reported DB 427.82 MiB and max DB+WAL 433.56 MiB, but it predates the final E/F/G/MCP combination. It does **not** establish a final normal-load memory plateau or WAL-growth bound. Final combined cold readiness, CPU, sustained RSS and DB/WAL acceptance have not been measured.

## User-reachable paths and conservative boundaries


- Review: open the selected execution; `g`/`m` explicitly map/capture Git, `d` prepares the continuity draft and explicit approval appends an immutable version. CLI Review JSON composes with saved-task links. Frozen evidence must recheck every contributing source and receiver at copy/export/save.
- Investigations: Activity History, `Ctrl+F`, command palette or `agenturm investigate`; typed query, bounded paging, exact pinned source. No match is not proof of no activity.
- MCP/Usage: existing inspectors, not extra dashboards. Loaded-window reliability describes recorded protocol/tool outcomes, never response correctness. Optional estimates require explicit exact-model rates and proven allocation; absence never recommends deleting a tool.
- Repetition/progress: `AGENTURM_WORKFLOW_PATTERNS=1` is opt-in. Later matching success retains both source records; reported usage contribution context must be source-complete, exact-owner/execution and strictly timed. It is neither waste attribution, a repaired-task claim, cumulative-counter delta nor context occupancy.
- Persistent attention: `agenturm attention watch --endpoint … [--desktop]`; shared durable claims, quiet baseline and optional native metadata. Native approval remains in the actual agent terminal. Unsupported runtime approval mapping, ambiguous clocks or capacity defer rather than invent authority.

## Findings F1–F27

### F1 · CPU/RSS economics

**Open resource acceptance**

Read/source increments merged through #84; validation harness merged #85. Sessions covered-lookup 79ac99d is delivered, integration pending.

Code: [storage_activity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_activity.py), [storage_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_sessions.py), [storage_mcp.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_mcp.py), [read_connections.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/read_connections.py).

Contract tests: [test_history_read_economics.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_history_read_economics.py), [test_idle_history_watch.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_idle_history_watch.py), [test_session_projection_batch.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_projection_batch.py).

Cold <30 s, complete-monitor idle <5%, endpoint p95 <100 ms and final combined memory/WAL acceptance remain open. Endpoint/call-count results do not prove whole-monitor improvement.

Controlled historical results stay workload-labelled. New tiny idle smoke is error-free but non-publishable; final frozen combined resource runs remain open.

### F2 · No default retention

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [server.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/server.py), [core.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/core.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_storage_receiver_hardening.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_storage_receiver_hardening.py).

New managed stores use a 14-day age policy; existing stores require explicit opt-in. Age retention is not a byte cap, immediate file shrink or proof of a memory/WAL plateau.

### F3 · Telemetry in the launch directory

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/__main__.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_storage_receiver_hardening.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_storage_receiver_hardening.py).

Private XDG state and explicit non-destructive legacy migration are provided. Explicit --db remains an intentional user override; captured content is still sensitive.

### F4 · Unreliable one-shot scripting

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) and follow-up [#70](https://github.com/abhishekash/agenturm/pull/70).

Code: [terminal.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/terminal.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/__main__.py).

Contract tests: [test_recovery_once.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_once.py), [test_snapshot_unknown_usage.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_snapshot_unknown_usage.py).

Readiness/error/partial/interrupt are distinguished; missing data is not a zero-success snapshot. Discovery/readiness can exceed the deadline and return partial rather than pretend success.

### F5 · OpenCode step mistaken for turn

**Legacy contract implemented; current-version qualification pending**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#71](https://github.com/abhishekash/agenturm/pull/71).

Code: [opencode.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/opencode.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [review_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_facts.py).

Contract tests: [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py).

Legacy recorded user-parent membership joins prompt/assistant steps. OpenCode 2.2.5 uses a different persisted shape; adapter/idle-boundary proof and nonzero cache/reasoning accounting qualification are pending. Missing initiating text or membership stays unavailable; no proximity join.

### F6 · Codex Review omits real evidence

**Implemented for tested contracts**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_operation_hydration.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_hydration.py).

Native-shaped ingestion→Store→authenticated API→Review/export assertions cover edits/checks/text/usage. This is not universal producer-version compatibility; F26 provenance remains qualified.

### F7 · Noisy attention signal

**Implemented, qualified**

Merged #67/#70; persistent semantics merged #82 at e434fcf.

Code: [attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/attention.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py), [ui_workflow.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_workflow.py).

Contract tests: [test_attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_attention.py), [test_session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_status.py), [test_today_outcomes.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_today_outcomes.py).

Explicit request/unknown separation and incident identity are present. Advisory stalled/failed states are not native approval authority; unsupported clocks/coverage defer reliable delivery.

Generated real-process preflight: two cockpits plus watcher, one fixture notification, acknowledgement/restart and exact remote resolution pass; no live desktop or alert-precision claim.

### F8 · Low roster/Overview information density

**Implemented, qualified**

Merged [#72](https://github.com/abhishekash/agenturm/pull/72) / [#74](https://github.com/abhishekash/agenturm/pull/74) / [#78](https://github.com/abhishekash/agenturm/pull/78).

Code: [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/terminal_ui.py), [overview.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/overview.py), [source_projection.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/source_projection.py).

Contract tests: [test_agents_readability.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_agents_readability.py), [test_overview_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_overview_facts.py), [test_overview_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_overview_ui.py).

Compact/expanded inspectors show recorded paths/checks/issues before assistant claims; hierarchy/shading/keyboard behavior retained. Loaded bounded evidence is not a full-history summary.

### F9 · Old completion backlog

**Implemented, qualified**

Merged #67/#70; the same durable receipts are reused by merged #82.

Code: [ui_workflow.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_workflow.py), [workflow_store.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_store.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py).

Contract tests: [test_today_outcomes.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_today_outcomes.py), [test_workflow_store.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_workflow_store.py), [test_keyboard_navigation.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_keyboard_navigation.py).

Today/history, human-readable ages and frozen-ID bulk review exist. Marking reviewed is neither native approval nor proof of resolution; old archived records remain inspectable.

### F10 · No observed continuity adoption

**External outcome open**

Technical drafts merged [#77](https://github.com/abhishekash/agenturm/pull/77); pilot materials in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [review_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_continuity.py), [saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/saved_tasks.py), [ui_saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_saved_tasks.py).

Contract tests: [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_ui.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py).

No new participant, author-use or voluntary repeat-use observation is established. Pilot denominator is 0 participants / 0 sessions / 0 repeat-use observations; do not infer adoption from tests.

### F11 · Shared daemon falsely proves progress

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#70](https://github.com/abhishekash/agenturm/pull/70).

Code: [local_runtime.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_runtime.py), [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py).

Contract tests: [test_local_runtime.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_local_runtime.py), [test_session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_status.py), [test_soak_monitor.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_soak_monitor.py).

Multiplexed/app-server activity is not per-session progress; unconfirmed status remains possible. Linux process evidence and the generated soak do not cover every runtime/platform.

### F12 · Unknown children counted as actionable

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/attention.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py), [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/terminal_ui.py).

Contract tests: [test_attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_attention.py), [test_today_outcomes.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_today_outcomes.py), [test_session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_status.py).

Unknown/unlinked coverage is separated from explicit requests. New collision/loop/native heuristics must not silently turn it into an approval request.

### F13 · Strong MCP screen lacks qualified recommendations

**Implemented and merged; combined CI pending**

Merged #81 at 093bf878; shared-query economics merged #83 at 728e0ba.

Code: [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/terminal_ui.py), [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py).

Contract tests: [test_explanation_integration.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_integration.py), [test_evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_evidence_explanations.py).

Selected-tool loaded-page outcome/duration denominators, small samples, actual configured-server unobserved context and keyboard flagged-call navigation are visible and tested. No response-correctness, all-history reliability or tool-deletion conclusion follows. Earlier corrected E head passed 12 checks; rebased/main combined CI is a separate unfinished gate.

### F14 · Generic failures without useful evidence

**Implemented, qualified**

Merged #75/#76; replay qualification/reference preservation merged #84 at 11a3b7b.

Code: [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/findings.py), [finding_sources.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/finding_sources.py), [ui_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_findings.py).

Contract tests: [test_operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_facts.py), [test_owned_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_owned_findings.py), [test_finding_sources_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_finding_sources_ui.py).

Commands/exits/output roles, occurrence ownership, safe fingerprints and exact sources are implemented. Unknown/redacted/clipped inputs cannot prove equivalence; a later success does not prove complete repair.

### F15 · Historical JSON expansion/read cost

**Partial resource acceptance**

Merged indexed/scalar/source/Sessions/MCP increments through #84; validation harness merged #85. Covered-log followup 79ac99d is delivered, integration pending.

Code: [storage_activity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_activity.py), [storage_derived.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_derived.py), [storage_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_sessions.py), [storage_mcp.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_mcp.py).

Contract tests: [test_cockpit_read_paths.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_cockpit_read_paths.py), [test_session_projection_batch.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_projection_batch.py), [test_history_read_economics.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_history_read_economics.py).

Indexed Activity/scalar rollups and bounded source reads replace repeated full-history Activity expansion. Remaining retained MCP grouping, cold migration and combined monitor cost are material and not closed.

### F16 · Local API/privacy/redaction defaults

**Implemented safeguards; continuous invariant**

Merged safeguards through #77; merged E/F/G paths inherit the same invariants.

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [server.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/server.py), [http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/http_transport.py), [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [ui_export.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_export.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_http_transport.py), [test_receipt_export_policy.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_receipt_export_policy.py).

Private token/Host/Origin/redirect checks, expanded best-effort redaction and late-egress capture fences exist. No universal secret-detection or security guarantee follows; new paths still need combined-source gates.

### F17 · Native add/update edits discarded

**Implemented, qualified**

Native actions merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75); typed rename integration in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [command_evidence.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/command_evidence.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/workflow_evidence.py), [recorded_paths.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/recorded_paths.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_native_review_contract.py), [test_investigation_store.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_store.py).

Failed/unknown file attempts do not prove a persisted change. Both explicit rename paths survive typed argument projection in G; no arrow/path equivalence guessing.

### F18 · Wrapped native checks not recognized

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75).

Code: [command_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/command_evidence.py), [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_facts.py).

Recognized wrappers/qualified compound success are supported; no-op, masked, unknown or ambiguous failure remains unverified. Recorded success does not certify later worktree changes.

### F19 · Exact usage rejected or misallocated

**Implemented and merged; current native qualification pending**

Allocation/continuity merged #67/#71/#75; usage explanation wiring merged #81 at 093bf878.

Code: [review_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [core.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/core.py).

Contract tests: [test_agent_usage_allocation.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_agent_usage_allocation.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py), [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py).

Single-owner/turn usage, unknown fields and partial costs are preserved. R8 label/selection work is separate from accounting; no shared-trace allocation or zero from missing data. OpenCode 2.2.5 adapter and source-membership/cache/reasoning qualification remain pending; missing or stale proof must withhold totals.

### F20 · Assistant-message placeholders

**Implemented for tested contracts**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [session_text.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_text.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_native_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_sessions.py), [test_session_text.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_text.py).

Supported native text blocks reach Review and generic placeholders are suppressed. New upstream schemas and live Claude/Pi compatibility are not proven by these synthetic contracts.

### F21 · Initiating request crowded out

**Implemented, bounded**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75); instruction investigation in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [core.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/core.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [review_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_facts.py).

Contract tests: [test_workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_workflow_evidence.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py).

Independent request/update context and paging expose intent within explicit source limits. Pruned, uncaptured or clipped intent remains unavailable, never reconstructed by guesswork.

### F22 · Complete captured run shows zero work

**Implemented for tested contracts**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#70](https://github.com/abhishekash/agenturm/pull/70) / [#71](https://github.com/abhishekash/agenturm/pull/71) / [#75](https://github.com/abhishekash/agenturm/pull/75).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [work_receipts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/work_receipts.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_workflow_evidence.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py).

End-to-end native-shaped checks/edits/text/usage and generated reconstructions are verified. Coverage limits and unsupported producer variants remain explicit; tests are not current live participant evidence.

### F23 · Review JSON/task-link mismatch

**Implemented, qualified**

Merged [#71](https://github.com/abhishekash/agenturm/pull/71) / [#77](https://github.com/abhishekash/agenturm/pull/77).

Code: [saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/saved_tasks.py), [task_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/task_cli.py), [ui_saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_saved_tasks.py).

Contract tests: [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py), [test_saved_task_scope_links.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_saved_task_scope_links.py), [test_task_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_task_cli.py).

Raw Review JSON composes without retyping IDs; turn members/check source traces/accounting survive. Contradictory links reject; legacy ambiguous data stays readable without trusted totals until explicit repair.

### F24 · Passing suite hid fixture mismatch

**Strength preserved; final matrix pending**

Implementation merges #79–#85 are recorded separately from their exact tested heads and the combined-main matrix.

Code: [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py), [test_owned_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_owned_findings.py).

Earlier corrected E head `928cae0`/F head `1c0b795` each passed 12 checks. Rebased PR81/82/85 heads include intentionally cancelled duplicate runs; these are not all-green matrices. Main e434fcf run 43591334638 is still active at this snapshot. Final native-version, packaging/security/ownership/capture/terminal and resource gates remain.

### F25 · OpenCode prompts excluded by trace

**Legacy prompt membership implemented; current-version qualification pending**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#71](https://github.com/abhishekash/agenturm/pull/71).

Code: [opencode.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/opencode.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [review_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_cli.py).

Contract tests: [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py).

Legacy whole-turn mode includes a recorded initiating prompt when present. Current OpenCode 2.2.5 may lack that persisted legacy user-parent record; absent text must stay unavailable. Adapter/idle-boundary qualification is pending; exact-step mode remains separate.

### F26 · Idealized rather than version-labelled goldens

**Partial provenance; current-version integration pending**

Native-shaped tests merged [#67](https://github.com/abhishekash/agenturm/pull/67); G pilot/search in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [opencode.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/opencode.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py), [test_native_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_sessions.py).

Historical synthetic repository fixtures have unknown originating upstream versions. Current version-labelled manifest and OpenCode 2.2.5 positive/cache/reasoning gates are adapter-owned and pending integration; this audit does not overwrite that manifest. Live Claude/Pi and future-version compatibility remain open.

### F27 · Cockpit-bound delivery, native events and Git

**Implemented and merged; runtime/external gates open**

Git/drafts merged #77; persistent/native delivery merged #82 at e434fcf.

Code: [attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_runner.py), [attention_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_cli.py), [native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/native_lifecycle.py), [storage_native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/storage_native_lifecycle.py), [review_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/review_cli.py), [source_projection.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/source_projection.py).

Contract tests: [test_native_inventory.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_inventory.py), [test_native_activity_integration.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_activity_integration.py), [test_attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_attention_runner.py), [test_native_review_projection_fix.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_review_projection_fix.py).

Headless watcher, shared claims, receiver-owned native inventory and shared Review parity are merged. Generated two-cockpit+watcher preflight passed with one fixture notification and 210 unrelated records per transition, ack/restart and exact resolution. Combined CI and live request-contract/clock/capacity/desktop precision remain qualified; native approval stays manual.

## Recommendations R1–R11

### R1 · Reliable cross-runtime attention

**Implemented and merged; external precision open**

Shared classifier, headless delivery and native inventory merged #82 at e434fcf.

Code: [attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_runner.py), [attention.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention.py), [native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/native_lifecycle.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/__main__.py), [review_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/review_cli.py), [source_projection.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/source_projection.py).

Contract tests: [test_attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_attention_runner.py), [test_native_inventory.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_inventory.py), [test_native_activity_integration.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_activity_integration.py), [test_native_review_projection_fix.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_review_projection_fix.py).

Quiet inventory, durable dedup, remote pending/resolution catch-up and acknowledgement/restart passed a generated real two-cockpit+watcher preflight. This is one functional scenario with a fixture notification sink, not measured alert precision, OS desktop delivery or live cross-runtime approval support.

### R2 · Truthful per-turn receipts with frozen Git

**Implemented, qualified**

Merged [#71](https://github.com/abhishekash/agenturm/pull/71) / [#75](https://github.com/abhishekash/agenturm/pull/75) / [#77](https://github.com/abhishekash/agenturm/pull/77).

Code: [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [review_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_continuity.py), [work_receipts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/work_receipts.py).

Contract tests: [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py), [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_ui.py).

Review g/m explicitly maps/captures read-only Git; earlier checks do not certify the current checkout. Recorded edits and all worktree changes/pre-existing/concurrent work stay distinct.

### R3 · Actionable failure diagnosis

**Implemented and merged; diagnosis remains qualified**

Operation/Findings facts merged #75/#76; opt-in later matching-success observations merged #81.

Code: [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/findings.py), [finding_sources.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/finding_sources.py), [ui_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_findings.py).

Contract tests: [test_operation_correlation.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_correlation.py), [test_owned_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_owned_findings.py), [test_finding_sources_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_finding_sources_ui.py).

Current/previous unseen reminders remain separate from archival Findings. Exact source s/container trace t, expected nonzero and later-success evidence retain source scope; no repaired-task conclusion.

### R4 · Approved automatic continuity

**Implemented; adoption external**

Merged [#77](https://github.com/abhishekash/agenturm/pull/77).

Code: [review_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_continuity.py), [saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/saved_tasks.py), [ui_saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_saved_tasks.py), [task_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/task_cli.py).

Contract tests: [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_ui.py), [test_review_continuity_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_cli.py).

Review d prepares source-backed editable text, then explicit approval appends an immutable version. Human title/notes/brief/decisions are not overwritten; no human decision or intent is invented.

### R5 · Parallel-work overlap awareness

**Implemented, qualified**

Merged [#77](https://github.com/abhishekash/agenturm/pull/77); typed rename pair fidelity in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [review_continuity.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/review_continuity.py), [ui_review.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/ui_review.py), [recorded_paths.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/recorded_paths.py).

Contract tests: [test_review_continuity.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_review_continuity.py), [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_review_continuity_ui.py), [test_investigation_store.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_store.py).

Explicit mapped checkouts/recorded paths and strict check-before-other-edit chronology are shown. Historical branch identity often remains unverified; unknown overlap is not collision proof. Different worktrees, failed attempts and ambiguous paths do not become confirmed collisions.

### R6 · Progression and repeated-work evidence

**Implemented opt-in and merged; qualified observations**

Merged #81 at 093bf878 includes progression, source-complete Store publication and versioned legacy-cache repair.

Code: [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py), [workflow_patterns.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/workflow_patterns.py).

Contract tests: [test_evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_evidence_explanations.py), [test_workflow_patterns.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_workflow_patterns.py), [test_workflow_backend.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_workflow_backend.py), [test_explanation_cache_upgrade.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_cache_upgrade.py), [test_reported_usage_progress.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_reported_usage_progress.py).

Opt in via AGENTURM_WORKFLOW_PATTERNS=1. Later matching success includes failed and successful sources. Repetition observations may attach strictly timed leaf-model reported contributions only with complete exact owner/execution/source proof. Actual Store publication and resumable workflow_patterns_v2 repair are tested; unknown/partial input remains unavailable. No new always-on usage finding, waste/retry-cost attribution, cumulative delta or context-occupancy inference.

### R7 · Qualified MCP recommendations

**Implemented and merged; window-qualified**

Merged #81/#83; loaded-page MCP explanations and shared grouped inventory remain separate from response-quality evaluation.

Code: [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py), [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/terminal_ui.py).

Contract tests: [test_explanation_integration.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_integration.py), [test_evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_evidence_explanations.py).

Current loaded-page outcome/duration denominators, small samples, configured-unobserved context and keyboard flagged-call navigation are wired in the existing MCP inspector. Exact receiver/capture checks remain. An observed protocol/tool success does not evaluate response correctness; absence never recommends deleting configuration.

### R8 · Usage scope, cache and optional estimates

**Implemented and merged; unsupported windows remain unknown**

Allocation plus merged #81 explanations distinguish turn/step/session scope and optional exact-model estimates.

Code: [ui_monitoring.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/ui_monitoring.py), [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/terminal_ui.py), [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py).

Contract tests: [test_task_usage.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_task_usage.py), [test_explanation_integration.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_integration.py), [test_agent_usage_allocation.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_agent_usage_allocation.py).

Whole-turn ID/member-step labels are distinct from one trace. Exact-owner captured session lifetime is separate; exact-agent calendar-day data is explicitly unavailable. Retry cost and current occupancy remain unavailable without authoritative evidence. Exact-model explicit-rate estimates stay separate from recorded cost and require complete unambiguous allocation.

### R9 · Global retrospective investigations

**Implemented and merged; resource limits remain**

Merged [#80](https://github.com/abhishekash/agenturm/pull/80), all 12 final checks green.

Code: [investigation.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/investigation.py), [storage_investigation.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/storage_investigation.py), [investigation_cli.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/investigation_cli.py), [ui_investigations.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/ui_investigations.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/__main__.py).

Contract tests: [test_investigation_store.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_store.py), [test_investigation_cli.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_cli.py), [test_ui_investigations.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_ui_investigations.py).

Activity History / Ctrl+F / palette and `agenturm investigate` are wired. Typed retained path/command/instruction queries, bounded paging, FTS availability and pinned exact source reader are tested. No match proves no activity; old lookup/index-cost data excludes final combined-source overhead.

### R10 · Safe local collection defaults

**Implemented safeguards; combined gates open**

Merged defaults #67/#70 and merged E/F/G paths inherit invariants; combined-source acceptance remains separate.

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [server.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/server.py), [native_policy.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/native_policy.py), [http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/http_transport.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_storage_receiver_hardening.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_storage_receiver_hardening.py), [test_http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_http_transport.py).

Private state/token, Host/Origin, best-effort redaction, capture fences and age retention exist. No all-secrets guarantee, hard disk cap or automatic purge of independently authored task content.

### R11 · Optional native lifecycle integration

**Implemented supported contract; coverage qualified**

Optional bounded lifecycle metadata, watcher/native CLI and receiver inventory merged #82.

Code: [native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/native_lifecycle.py), [attention_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_cli.py), [storage_native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/storage_native_lifecycle.py), [server.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/server.py).

Contract tests: [test_native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_lifecycle.py), [test_native_inventory.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_inventory.py), [test_native_activity_integration.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_activity_integration.py).

Generic-v1 is explicit exact metadata; Codex notify adapter is completion-only with independently checked turns. Claude/Pi/OpenCode approval contracts without authoritative request/order mapping remain unsupported/uncertain. Installed version inspection is not live compatibility testing.

Installed/runtime research and generated exact-contract tests are separately recorded. The successful native-race preflight is generic metadata, not a live Claude/Pi/OpenCode approval-hook qualification.

## F26: historical fixture provenance and current qualification


The historical hash table below fingerprints the earlier synthetic repository contracts at published main `728e0ba`. It is not the current adapter-owned producer manifest; no old manifest is imported or overwritten by this audit. Codex JSONL and generated OpenCode SQLite fixtures are synthetic/schema-derived. Their originating upstream producer build and schema revision were **not recorded**. Inspected installed CLI versions used for hook research cannot be retroactively assigned to these fixtures.

The readers and listed fixtures at main `728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c` are byte-identical to the head `920f44e517bc5f8583521dbae5664fe3fff37d1e` tested by PR79 with all 12 checks green. This establishes which repository contract was tested; it does not establish which upstream release emitted a live record.

| File | SHA-256 |
| --- | --- |
| `observatory/local_agents.py` | `12602f8c1e9c1c4d5c47c616aad7f8eb08bf271a4704394a5e49f62908cd4c8c` |
| `observatory/opencode.py` | `cc444be5c2521c0e59089d55930c249e65b1d8e78ea27191a8e24e4bfa5e16bd` |
| `tests/fixtures/native_review/codex.jsonl` | `cfb73258d9824e36b7d9240d3bc2d49b2c7f001dbc12f338e79db182aee682ab` |
| `tests/test_opencode.py` | `bc065e066131d8e89c04da2ad1690389c0b256f0d5d2cdf9f8e927ebfdc8a186` |
| `tests/test_opencode_turn_review.py` | `892bc2a828c96f12b6d4f25ada2b603eec50ccb3d79479724d01fae1039cc62b` |

The legacy hashes above must not be relabelled as OpenCode 2.2.5 evidence. The current producer/schema-labelled adapter and nonzero cache/reasoning assertions are pending peer/integration gates; this audit neither imports nor overwrites its manifest. F26 remains **partial** until consent-safe, producer/schema-version-labelled goldens cover the claimed supported versions through ingestion → Store → authenticated API → Review/export. Live Claude Code and Pi validation remains unavailable at this snapshot.

## Remaining acceptance and external outcomes

The approved [final validation protocol](final-monitor-protocol.html) uses frozen B=`5e0c397` and a still-to-be-frozen combined C, one committed harness/common raw seed and balanced 60-second warmup plus 300-second idle pairs. The harness/preflights are delivered; the final matched resource runs have not been accepted. Final one-hour active soak, modal retained-memory series, complete storage/index/WAL/drain accounting and candidate signal/navigation checks remain open. Existing historical resource and profiler results retain their own source/workload boundaries.

The pilot kit is a protocol and generated scenario, not a study. No willingness-to-pay, measured reconstruction/resumption improvement, alert precision, author repeat use or voluntary adoption result is inferred from tests. Actual OS desktop delivery remains mocked. Live Claude Code and Pi remain untested; unsupported authoritative request/order mappings remain unsupported.

The next update must record the actual native adapter and Sessions-followup merge commits, their exact CI results, then the frozen candidate and measured acceptance outcomes. No additional dashboard or speculative automation is required to finish this scope.

## Verification of this audit

The ledger enumerates F1–F27 and R1–R11 exactly once, retains original note SHA and historical evidence, records actual current merge/check states, and separates functional preflights from non-publishable resource smokes. Referenced frozen code/test paths and the regenerated self-contained responsive light/dark HTML are checked independently. Document checks are not product or performance tests. Earlier browser QA remains historical; this current document's check status is recorded in the delivery message.
