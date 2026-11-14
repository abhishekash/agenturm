# Review-notes acceptance audit

Current checkpoint: **2026-11-14T17:43:27.111051+00:00**, verified merged main **`1ca4fafe71aed559d381b7e613efb748b9ae9878`**, after [PR #104](https://github.com/abhishekash/agenturm/pull/104). Startup writer admission, readiness diagnostics and the receiver/CI fixture corrections are merged. **Full CI and final runtime/resource acceptance remain open.** The active MCP correction is under independent review and is not delivered.

## Current decision

- **Merged:** A–G increments, writer ownership #100, canonical success projection #101, metadata-only native bootstrap #102 README pip/uv instructions #103 and startup repair/diagnostics #104. Bootstrap keeps `updated=0`, history/usage unknown and content/workflow authority unavailable until ordinary coherent history is ready.
- **Current CI is pending:** [main104 run 43622391716](https://github.com/abhishekash/agenturm/actions/runs/43622391716) has eight successes and four Python jobs in progress at this fetch. PR102/main103 failures are retained separately; earlier green matrices do not certify this head.
- **Distinct startup outcomes:** frozen 473 exceeded 600 seconds before timed work, with zero UI/heartbeat/turn samples. Candidate 3fb plus harness 883 reached retained-history readiness (`initial_readiness_seconds=221.505`, 52,002 Activity jobs / 5,752 summaries), then the configured 12-second functional smoke **failed** on `/api/mcp?limit=20` HTTP 503 after 7.305 active seconds, with zero completed turns and no restart. Neither is a one-hour pass or resource acceptance. The readiness observation retained 20 connection errors; eventual readiness is not uninterrupted HTTP availability.
- **Strict cadence remains invalid:** 315/420 scheduled writes, with genuine 503, missed-slot and progress failures. Earlier 44/35, 5/114, 145/420 and 25/30 attempts remain preserved. There is no publishable historical loaded HTTP p95 or comparison.
- **Open:** current full CI, active MCP availability, full one-hour stability, cold/useful-frame/idle/API/RSS/storage/WAL/drain targets and human pilot. Denominators remain **0 participants / 0 sessions / 0 repeat-use observations**. Live Claude/Pi remains unqualified.

## Audit basis and historical preservation

User-owned `agenturm-review-notes.md` remains read-only and unchanged at SHA-256 `8352ce8a78edaea075083e59850ddd96bb70868164e3c146b11cd97b0a617ce3`. All **27 findings and 11 recommendations** are accounted for below. [The implementation plan](../rfcs/review-notes-completion-plan.html) and [machine ledger](review-notes-acceptance-data.json) share this checkpoint. The complete previous main97 ledger is retained through the immutable commit/path/SHA reference under `previous_main97_snapshot`; older main91/main55 records, CI states, frozen links and failed attempts remain preserved without recursively copying the entire ledger. The [immutable previous audit](https://github.com/abhishekash/agenturm/blob/a241b149b7afa8661f99fb65871e4e45bcd1a7c0/docs/validation/review-notes-acceptance.html) and [previous plan](https://github.com/abhishekash/agenturm/blob/a241b149b7afa8661f99fb65871e4e45bcd1a7c0/docs/rfcs/review-notes-completion-plan.html) preserve their original wording. Historical pending states below are not current CI claims.

## Historical PR98–103 merge and CI fetch

| PR | Scope | Merge | Exact fetched head checks |
| --- | --- | --- | --- |
| [#98](https://github.com/abhishekash/agenturm/pull/98) | CI readiness/policy fixtures | `7983661bd27478f9f35af70ef46cc45332ed90ee` | head `ce312db6829d740b7ff5f9b7d5e0bf7806e9a174`: 12 success, 0 failure, 0 pending |
| [#99](https://github.com/abhishekash/agenturm/pull/99) | main97 audit checkpoint | `a241b149b7afa8661f99fb65871e4e45bcd1a7c0` | head `efe23ad599a7af57553991428fc91b79d30cc857`: 12 success, 0 failure, 0 pending |
| [#100](https://github.com/abhishekash/agenturm/pull/100) | writer ownership/admission | `bd04f205a97291fd9b8b23fc952854c2a72c6e36` | head `52392147f6ded3d02e5094741ab7f65295c6c222`: 12 success, 0 failure, 0 pending |
| [#101](https://github.com/abhishekash/agenturm/pull/101) | canonical success projection | `250030bd82aa143e72860f4a392504133bac4c88` | head `9f3d1d76282328905f4427e57be81276a366d955`: 12 success, 0 failure, 0 pending |
| [#102](https://github.com/abhishekash/agenturm/pull/102) | native bootstrap | `abf1627950aa53032e97dea2136fa25e3748a7d4` | head `766f1e982e803636699b6d50f78522510e92caf6`: 8 success, 4 failure, 0 pending |
| [#103](https://github.com/abhishekash/agenturm/pull/103) | README pip alongside uv | `7c80dc3946f7df7f88f8b3c541f0fc08a4bfa9fb` | head `6049dd870d681c19b0a25e85481d339b6073aa77`: 8 success, 1 failure, 3 pending |

Main102 run [43619493189](https://github.com/abhishekash/agenturm/actions/runs/43619493189) recorded 8 successes / 3 failures / 1 pending; main103 run [43619971350](https://github.com/abhishekash/agenturm/actions/runs/43619971350) recorded 8 successes / 4 pending. These are the exact timestamped fetches, not predictions of completion. Individual checks and URLs remain in JSON.

## Merged startup repair and distinct failed validation

[PR #104](https://github.com/abhishekash/agenturm/pull/104) merged at `1ca4fafe71aed559d381b7e613efb748b9ae9878` after a warning-free 60-case combined gate and a separate overlapping 22-case harness gate. The [integration report](startup-integration.html) records exact reviewed source boundaries, the proved connection closure, failure-safe diagnostics and later reversible artifact cleanup. Full CI is still pending.

The following authored components are now delivered by #104:

- **Startup writer admission** (`3fb82549d08730c753bd76553b19b41e48545bf8`): Core fix and 13 local tests; merged #104. No cold or HTTP target acceptance.
- **Readiness diagnostics/fail-fast harness** (`883958949c024784c5b01c4afa35bc65dc2359a8`): Harness correction merged #104 after focused validation; not sustained runtime acceptance.
- **Independent receiver startup integration** (`adb13508d6ed1a84a8b485852c2807a0aea84d6a`): Two real generated native/authenticated HTTP tests passed in 2.616 seconds. Independent two-case pass: 2.536 seconds before final shutdown-exception assertion strengthening. Merged #104 after the combined gate; functional evidence only.
- **CI102 missing-method test double** (`2354ecf2f7f0d1dc2bbc0d2c312e4b3f7dcecced`): Fixture-only correction committed; 28 local integration tests passed in 34.654 seconds and seven Python 3.13 monitoring models passed in 2.022 seconds. Merged #104 after the warning-free combined gate; remote CI remains pending. No runtime fallback or green CI claim.

The frozen 473 failure report was delivered at `129b12d7d1712d4736b441f60a86d578fd5d8b7e`. Its sanitized artifacts preserve 13 total errors (the primary readiness deadline plus 12 consequential unmet checks), receiver exit 0 and no timed samples. Health-side hierarchy INSERT lock failure was observed; its blocking transaction and extended code remain unresolved. Later BUSY/517 reproductions are not assigned as its unique cause.

The separate [3fb smoke and failure report](https://github.com/abhishekash/agenturm/blob/eace36b6f824904e07fc80896749690e27142d07/docs/validation/full-corpus-startup-3fb.html) ended with receiver exit 0 and pending work 2→0, but no graceful cockpit exit record and no completed restart/review cycle. Its raw results SHA-256 is `093927512e524be8f791f01b7849042ba2880deba48a13d220c16f02b71d390b`; registered children were verified absent. Full result directories contain generated credentials and must not be published.

## Preserved main97 evidence narrative

The following at-time CI/readiness/preflight/resource narrative is retained verbatim; current decisions above supersede its pending/merged labels. Its original integration table and complete38row snapshot are retained in JSON and the immutable audit link.

Earlier corrected E/F heads had complete green matrices before rebasing. Merged PR81/82/85 duplicate runs were cancelled intentionally; completed full main e434 and main `55c6244` matrices remain historical combined evidence. PR84, PR86 and the earlier PR79/80/83 head matrices remain historical successes. PR87 head `e2c6a4d` retains one [Python 3.14 failure](https://github.com/abhishekash/agenturm/actions/runs/43592510668/job/132440055787); the later full main `55c6244` matrix passed independently. The earlier failure is neither removed nor assigned an unverified cause. The all-green claim applies exactly to those completed main matrices, not cancelled PR-head runs or unexecuted resource tests.

The original E failures exposed positive fixtures missing required receiver/single-owner/typed-command proof. The corrected contract retained strict negative safeguards. The original F matrix exposed a real lightweight headless Review crash and doubled Activity traversal; the fix restored exact once-per-refresh visits and Review/native parity. These failure histories are retained in the JSON ledger and linked implementation reports. Focused gates overlap and are not summed as independent coverage.

## Readiness, availability and CI follow-ups

PR88 prevents an authenticated ready preflight from racing a source write and returning stale cached usage as complete. Sessions, cached run/trace/metrics/usage reads certify source and derived facts in one transaction; missing or dirty dependencies return typed 503. Raw MCP query semantics remain unchanged. [The frozen report](https://github.com/abhishekash/agenturm/blob/ebcd03390bbbe57d18470165a4dbdb919de39357/docs/validation/derived-readiness-snapshot.html) retains the before-fix race and both commit orders.

PR90 requests at most 16 oldest existing worker jobs under one 75 ms catch-up allowance for incoming work, outside any retained database read lease. Initial migration/error/probe semantics and the final guarded read remain. [The report](https://github.com/abhishekash/agenturm/blob/e594a2a83c7d9d6da63e63aa497d88a8ff27b73b/docs/validation/incoming-catchup.html) retains real 503s and explicitly leaves active CPU/write-coalescing/WAL cost and p95 unknown. This is not a whole-request 75 ms guarantee.

CI exposed three fixtures relying on a cached read to repair asynchronous work, or calling public Sessions during deliberately pending redaction/legacy edits. PR91 changes fixtures only: immediate raw/derived-text scrubbing, zero derived caches and public `IndexNotReady` are checked before normal refresh/drain; final public privacy assertions remain. The affected 24-test gate passed 38.882 s and read-only peer review found no blocker. [The report](https://github.com/abhishekash/agenturm/blob/e389889ff7ff7ad92716fa5fb889a0a8ea8760a8/docs/validation/readiness-ci-fixtures.html) is local evidence, not a completed new-head matrix.

The earlier tiny B 25 / C 30 run remains invalid with missed slot 25, unequal source inputs and real 503 responses. PR93 commits the raw-clone/schema and scheduled-slot classification; PR95 permits shallow CI checkout while preserving exact candidate registration. A successful classification test reports a failed experiment correctly; it does not turn that experiment into a performance pass.

PR94 merged the capture follow-up. Its [frozen report](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/docs/validation/feed-capture-safety.html) records the 102-test integrated gate in 106.887 s. Same-known-receiver negative policy propagates before ordinary readiness; actor-wide denials, retained per-scope generations, source-owned bindings and final copy/export/save checks fence stale content across rapid restoration and restart. Fresh explicit recapture and independently authored material remain usable. This is a tested client contract, with qualified legacy-unbound material and **no durable offline revocation ledger**.

PR95/main95 failures exposed ResourceWarning stderr contamination and transient policy-message assumptions. PR96 isolates actual CLI call/post-call warning collection, preserves failure on current leaks and closes four independently proven owned soak connections. The original CI warning allocation remains unknown. PR96 then failed a different policy-message assertion and one headless CLI journey. The local follow-up records actual HTTP status, explicitly awaits asynchronous readiness, and retains blocked approval/editor/task-count assertions; a controlled real pending migration returned health 200 / Sessions 503 / CLI 2, then the same command succeeded after the worker became ready. This is a reproduced fixture race, **not a proved unique cause of the original uninstrumented failure**. Root's integrated two-test strict-warning gate passed in 5.362 s after retaining an earlier 5.716 s assertion failure; PR98 correction was subsequently verified merged as `7983661`; remote CI remains open. The fixed main97 CI snapshot below remains unchanged.

### Preserved checkpoint91

The earlier snapshot at `e389889`, 2026-11-14T13:32:52.180353+00:00, recorded main90/main91 as eight successes/four pending and privacy/harness follow-ups as open. Those were accurate fetched states at that checkpoint. They are superseded here by merged #93/#94 and completed failed #90/#91 head matrices. The original checkpoint CI, pending delivery, PR rollups and document validation remain intact under `previous_main91_snapshot` in JSON. Historical main55's twelve successes are likewise retained; no later failure is erased.

## Functional preflights

| Generated preflight | Actual result | Limit |
| --- | --- | --- |
| Native concurrency, main e434fcf | Two real tmux cockpits plus one headless watcher; quiet baseline; exactly one fixture notification; 210 newer unrelated records per transition; acknowledgement/restart and exact remote resolution; all owned exits clean, no errors. | One generated generic native-contract scenario. Notification sink is mocked; no native request approved. Not alert precision or live OS/runtime compatibility. |
| Baseline keyboard smoke 4 | 24 checks passed: prompt arrows/Home/End/read/section/help, 120×36/80×24/60×18, 50×14 too-small, normal quit and terminal restoration. | Functional baseline only; final candidate signal/modal/keyboard matrix remains open. |
| Tiny baseline idle preflight 2 | No errors; cockpit/receiver clean exit; `smoke=true`, `publishable=false`. | Harness sanity only. No CPU/readiness/RSS target can be closed from this short run. |
| Main e434 two-cycle modal/Investigate preflight | Two distinct Review scopes, Usage and Investigate source/back paths; no errors and clean owned exits. | Two cycles with no warmup are functional sanity only, not an RSS plateau or the planned 100-cycle retained-memory series. |
| Tiny main `55c6244` candidate idle preflight | No errors, clean cockpit/receiver exit; `smoke=true`, `publishable=false`. | Tiny fixture and short interval; no accepted CPU/readiness/RSS comparison. |

Raw local paths, exact source/harness heads and result hashes are recorded in the JSON ledger. Raw directories contain generated receiver credentials; publish only sanitized reports, not whole artifact directories.

## Resource acceptance ledger


The targets remain **cold readiness <30 s**, **complete-monitor idle CPU <5% of one core**, and **Sessions/MCP p95 <100 ms** on a matched corpus/reference host. Different corpora and workloads are kept separate. The PR79 Sessions and PR83 MCP numbers below are direct Store-method comparisons, **not actual HTTP**. Separate-process authenticated HTTP and normalized clone/registration harness changes are merged #89/#93/#95. Actual full-corpus and tiny full-cadence attempts below are invalid. Their retained raw results are evidence of failure, not accepted loaded latency.

| Experiment | What is supported | What remains open |
| --- | --- | --- |
| [PR72 retained-history pair](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/history-read-economics.md) | Cold 184.38→112.96 s; idle CPU 23.63→15.90%; under-write Sessions p95 373.58→284.90 ms and MCP 246.09→142.60 ms on that pair. | All stated targets remain missed. Earlier lock-failure comparison was rejected. |
| [PR73 idle watch](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/idle-history-watch.md) | Matched idle CPU 17.80→14.90%. | No new cold/read or final combined memory claim. |
| [PR74 source preparation](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/source-preparation.md) | Three measured consumers: median 21% canonical / 34% fallback preparation reduction; individual regressions are retained. | This isolated work is not whole-monitor CPU or a general speedup. |
| [PR79 normalized Sessions pair](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/docs/validation/session-projection-batch.md) | Separate 6,047-trace synthetic corpus: idle p95 312.93→220.50 ms; under-write p95 441.02→286.34 ms; 91 accepted writes, 89 during reads, zero errors. | Still >100 ms. Excludes receiver HTTP/UI/native discovery/cold/RSS. |
| [PR83 shared MCP grouping](https://github.com/abhishekash/agenturm/blob/ca9b51918a7932b492a1f67ccb2d066dde34ca95/docs/validation/mcp-inventory-grouping.md) | Same 6,047-trace corpus: both pairs lower median; first under-write p95 230.88→239.96 ms, unchanged-source repeat 343.53→307.98 ms. | Mixed tails; no general p95 improvement, both >100 ms. Retained inventory remains proportional to source population. |
| [G isolated index cost](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/docs/validation/typed-investigations.md) | 6,000 synthetic events: median build 608.53→4,017.29 ms; DB +19.4%; typed reads measured in isolation. | Predates final identity/dirty-queue/rename integration. No combined monitor resource acceptance. |
| Merged PR84 source qualification / linked-log seeks | Frozen serialized output equivalence across 14 fixture groups; raw operation-fact projections 237→134; 46 focused tests pass. A narrow linked-log index has independent output/query gates; its timing is diagnostic. | Diagnostic profiler samples only; no controlled whole-cold or monitor CPU claim. |
| [PR86 covered-log query](session-covered-lookup.html) | Exact frozen payload/order parity; 21 root integration tests in 14.316 seconds. Approximately 1,257,000 → 1,082,000 VM operations (~14% fewer) for one private-copy query. | Diagnostic query work only; not HTTP/API p95, complete-monitor CPU or target acceptance. |

### Registered HTTP attempts: invalid, with writer chronology retained

The full common raw corpus contains 31,000 spans / 41,000 logs / 5,751 traces. Both arms used 420 fixed two-record writer batches at 100 ms cadence and 80 loaded reads. Auth/import/schema/drain/cleanup checks passed; the writer schedule and matched accepted inputs did not.

| Arm | Accepted batches /420 | GET 503 /80 | Successful reads after writer stopped | Interpretation |
| --- | ---: | ---: | ---: | --- |
| B `5e0c397` |44 |10 |70 | Last accepted POST 377 ms; next slot 278 ms late, dispatch refused. No successful GET before the writer stopped. |
| C `e795f06` |35 |4 |72 | Last accepted POST 360 ms; next slot 261 ms late. Only four successful GETs during actual writes. |
| Tiny D0 `e795f06` |5 | Retained in raw |78 of all 80 reads occurred after stop | Last accepted POST 221 ms; next slot 122 ms late. Not the full retained corpus. |
| Tiny D1 `029719b` |114 | Retained in raw |56 of all 80 reads occurred after stop | Last two POSTs 182/192 ms; next slot175 ms late. Unequal inputs prevent comparison. |

**No loaded p95 may be published from these attempts.** Later valid-looking 200 samples predominantly measure a stopped writer, and unavailable samples must not be discarded. Raw initial-readiness observations around 430 s / 354 s include raw preparation and are not the primary cold-readiness metric. Component diagnostics do not repair this protocol failure. Local raw paths and exact hashes are in JSON; credentials-bearing artifact directories are not publication material.

Post-snapshot, the strict tiny ownership/MCP integration `8abdbf8` accepted 145/420 scheduled writes and retained all 80 reads, genuine 503s, a missed writer slot and post-abort progress failures. It is also invalid; no loaded p95 is accepted. This unmerged diagnostic does not change the frozen main97 CI or merged-delivery ledger.

### Merged economics PR97: bounded component evidence

The [ingest writer](ingest-write-contention.html), [scratch/UDF reuse](derived-preparation.html), [ASCII sanitation](fact-sanitization.html), [terminal-only projection](terminal-result-projection.html) and [trace-local sanitized context](trace-operation-preparation.html) reports retain exact-output/ownership/redaction invariants and null/invalid experiments. Root's integrated97-test gate passed in 41.067 s. The trace-local context is capped at 512 canonical object identities / 2 MiB sanitized data and clears per trace; it is not a global content cache. These isolated improvements are **not** an additive speedup or acceptance of full-corpus cold/idle/HTTP/RSS/WAL targets. Pending compatibility ownership and metadata-only bootstrap are separate unmerged work.

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

Read/source increments and same-snapshot/catch-up through #90, normalized harness #93/#95, privacy #94 and bounded performance components #97 are merged. Historical CI failures remain retained; current main104 combined CI is pending. Writer ownership #100, canonical success projection #101 and metadata-only bootstrap #102 are merged. Startup repair, diagnostics and receiver/fixture tests merged #104; full CI remains pending.

Code: [storage_activity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_activity.py), [storage_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_sessions.py), [storage_mcp.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_mcp.py), [read_connections.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/read_connections.py), [operation_preparation.py](https://github.com/abhishekash/agenturm/blob/d5538e6af2bcc7961ddc7af48fb4c79b978c32fd/observatory/operation_preparation.py).

Contract tests: [test_history_read_economics.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_history_read_economics.py), [test_idle_history_watch.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_idle_history_watch.py), [test_session_projection_batch.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_projection_batch.py), [test_derived_readiness_snapshot.py](https://github.com/abhishekash/agenturm/blob/ebcd03390bbbe57d18470165a4dbdb919de39357/tests/test_derived_readiness_snapshot.py), [test_incoming_catchup.py](https://github.com/abhishekash/agenturm/blob/e594a2a83c7d9d6da63e63aa497d88a8ff27b73b/tests/test_incoming_catchup.py), [test_operation_preparation.py](https://github.com/abhishekash/agenturm/blob/d5538e6af2bcc7961ddc7af48fb4c79b978c32fd/tests/test_operation_preparation.py).

Cold readiness <30 s, complete-monitor idle CPU <5%, Sessions/MCP actual HTTP under-write p95 <100 ms and final combined memory/WAL acceptance remain open. Direct Store/query-work results do not prove those HTTP or whole-monitor targets.

Full common-corpus HTTP B 44 / C 35 and tiny e795/029719b 5/114 accepted batches are invalid. Most 200 reads occurred after writer abort; no loaded p95 may be published. Isolated sanitized-facts/scratch/terminal/context improvements preserve outputs but do not close whole-monitor budgets. Strict cadence 315/420 remains invalid. Frozen 473 failed its 600-second readiness deadline before timed work. The separate 3fb smoke reached readiness in a recorded 221.505 seconds, then failed active MCP HTTP 503 at 7.305 seconds, with zero turns; neither closes performance acceptance.

Acceptance: OPEN: resource targets.

### F2 · No default retention

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [server.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/server.py), [core.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/core.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_storage_receiver_hardening.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_storage_receiver_hardening.py).

New managed stores use a 14-day age policy; existing stores require explicit opt-in. Age retention is not a byte cap, immediate file shrink or proof of a memory/WAL plateau.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F3 · Telemetry in the launch directory

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/__main__.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_storage_receiver_hardening.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_storage_receiver_hardening.py).

Private XDG state and explicit non-destructive legacy migration are provided. Explicit --db remains an intentional user override; captured content is still sensitive.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F4 · Unreliable one-shot scripting

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) and follow-up [#70](https://github.com/abhishekash/agenturm/pull/70).

Code: [terminal.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/terminal.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/__main__.py).

Contract tests: [test_recovery_once.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_once.py), [test_snapshot_unknown_usage.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_snapshot_unknown_usage.py).

Readiness/error/partial/interrupt are distinguished; missing data is not a zero-success snapshot. Discovery/readiness can exceed the deadline and return partial rather than pretend success.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F5 · OpenCode step mistaken for turn

**Implemented for source-derived current and legacy contracts**

Legacy turn repairs merged #67/#71; current split-part/idle-boundary adapter and bounded membership proof merged #87 at 55c6244.

Code: [opencode.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode.py), [opencode_turns.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turns.py), [opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turn_proof.py), [core.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/core.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/workflow_evidence.py).

Contract tests: [test_current_native_schema.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_current_native_schema.py), [test_opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_proof.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_review.py), [test_native_fixture_provenance.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_native_fixture_provenance.py).

Source-derived OpenCode 2.2.5 intervals use recorded preceding/closing idle boundaries; steered prompts remain in the same interval. Unknown prefix stays exact-step, partial/clipped/epoch-conflicting membership and stale summaries withhold totals. Legacy IDs/parent behavior remain. Missing captured prompt or idle evidence is not invented. This is selected-format synthetic qualification, not a live-agent certification.



Acceptance: Qualified implemented source contract; live/universal coverage remains unknown.

### F6 · Codex Review omits real evidence

**Implemented for tested contracts**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_operation_hydration.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_hydration.py).

Native-shaped ingestion→Store→authenticated API→Review/export assertions cover edits/checks/text/usage. This is not universal producer-version compatibility; F26 provenance remains qualified.

Source-derived Codex 0.167.1 fixture compatibility through Review/export is merged #87; it is synthetic, not a live transcript.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F7 · Noisy attention signal

**Implemented, qualified**

Merged #67/#70; persistent semantics merged #82 at e434fcf.

Code: [attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/attention.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py), [ui_workflow.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_workflow.py).

Contract tests: [test_attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_attention.py), [test_session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_status.py), [test_today_outcomes.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_today_outcomes.py).

Explicit request/unknown separation and incident identity are present. Advisory stalled/failed states are not native approval authority; unsupported clocks/coverage defer reliable delivery.

Generated real-process preflight: two cockpits plus watcher, one fixture notification, acknowledgement/restart and exact remote resolution pass; no live desktop or alert-precision claim.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F8 · Low roster/Overview information density

**Implemented, qualified**

Merged [#72](https://github.com/abhishekash/agenturm/pull/72) / [#74](https://github.com/abhishekash/agenturm/pull/74) / [#78](https://github.com/abhishekash/agenturm/pull/78). Metadata-only initial native observations merged #102 at abf1627; task/path/prompt content is withheld and the coherent snapshot clock stays zero.

Code: [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/terminal_ui.py), [overview.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/overview.py), [source_projection.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/source_projection.py), [native_bootstrap.py](https://github.com/abhishekash/agenturm/blob/abf1627950aa53032e97dea2136fa25e3748a7d4/observatory/native_bootstrap.py).

Contract tests: [test_agents_readability.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_agents_readability.py), [test_overview_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_overview_facts.py), [test_overview_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_overview_ui.py), [test_native_bootstrap.py](https://github.com/abhishekash/agenturm/blob/abf1627950aa53032e97dea2136fa25e3748a7d4/tests/test_native_bootstrap.py).

Compact/expanded inspectors show recorded paths/checks/issues before assistant claims; hierarchy/shading/keyboard behavior retained. Loaded bounded evidence is not a full-history summary. A useful first-native-frame target on retained history remains unproved. Frozen 473 Feed correctly refused fatal health. Partial metadata is not full history readiness.

Metadata-only native bootstrap is merged in PR #102. The partial frame preserves unknown history/usage and withholds source content and lifecycle authority. A useful first-native frame and resource acceptance on the retained corpus remain open; the original frozen-473 fatal-health refusal is not relabelled as a successful first frame.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F9 · Old completion backlog

**Implemented, qualified**

Merged #67/#70; the same durable receipts are reused by merged #82.

Code: [ui_workflow.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_workflow.py), [workflow_store.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_store.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py).

Contract tests: [test_today_outcomes.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_today_outcomes.py), [test_workflow_store.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_workflow_store.py), [test_keyboard_navigation.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_keyboard_navigation.py).

Today/history, human-readable ages and frozen-ID bulk review exist. Marking reviewed is neither native approval nor proof of resolution; old archived records remain inspectable.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F10 · No observed continuity adoption

**External outcome open**

Technical drafts merged [#77](https://github.com/abhishekash/agenturm/pull/77); pilot materials in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [review_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_continuity.py), [saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/saved_tasks.py), [ui_saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_saved_tasks.py).

Contract tests: [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_ui.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py).

No new participant, author-use or voluntary repeat-use observation is established. Pilot denominator is 0 participants / 0 sessions / 0 repeat-use observations; do not infer adoption from tests.



Acceptance: OPEN: external adoption.

### F11 · Shared daemon falsely proves progress

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#70](https://github.com/abhishekash/agenturm/pull/70).

Code: [local_runtime.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_runtime.py), [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py).

Contract tests: [test_local_runtime.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_local_runtime.py), [test_session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_status.py), [test_soak_monitor.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_soak_monitor.py).

Multiplexed/app-server activity is not per-session progress; unconfirmed status remains possible. Linux process evidence and the generated soak do not cover every runtime/platform.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F12 · Unknown children counted as actionable

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/attention.py), [session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_status.py), [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/terminal_ui.py).

Contract tests: [test_attention.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_attention.py), [test_today_outcomes.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_today_outcomes.py), [test_session_status.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_status.py).

Unknown/unlinked coverage is separated from explicit requests. New collision/loop/native heuristics must not silently turn it into an approval request.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F13 · Strong MCP screen lacks qualified recommendations

**Implemented and merged; current combined CI pending/failed**

Merged #81 at 093bf878; shared-query economics merged #83 at 728e0ba.

Code: [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/terminal_ui.py), [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py).

Contract tests: [test_explanation_integration.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_integration.py), [test_evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_evidence_explanations.py).

Selected-tool loaded-page outcome/duration denominators, small samples, actual configured-server unobserved context and keyboard flagged-call navigation are visible and tested. No response-correctness, all-history reliability or tool-deletion conclusion follows. Earlier corrected E head and previous main `55c6244` each passed all 12 checks; final resources/live outcomes remain separate.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F14 · Generic failures without useful evidence

**Implemented, qualified**

Merged #75/#76; replay qualification/reference preservation merged #84 at 11a3b7b.

Code: [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/findings.py), [finding_sources.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/finding_sources.py), [ui_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_findings.py).

Contract tests: [test_operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_facts.py), [test_owned_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_owned_findings.py), [test_finding_sources_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_finding_sources_ui.py).

Commands/exits/output roles, occurrence ownership, safe fingerprints and exact sources are implemented. Unknown/redacted/clipped inputs cannot prove equivalence; a later success does not prove complete repair.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F15 · Historical JSON expansion/read cost

**Partial resource acceptance**

Indexed/scalar/source/Sessions/MCP/replay increments are merged through #86; same-source snapshot certification #88 and bounded incoming availability #90 are merged.

Code: [storage_activity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_activity.py), [storage_derived.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_derived.py), [storage_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_sessions.py), [storage_mcp.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/storage_mcp.py).

Contract tests: [test_cockpit_read_paths.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_cockpit_read_paths.py), [test_session_projection_batch.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_projection_batch.py), [test_history_read_economics.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_history_read_economics.py), [test_derived_readiness_snapshot.py](https://github.com/abhishekash/agenturm/blob/ebcd03390bbbe57d18470165a4dbdb919de39357/tests/test_derived_readiness_snapshot.py), [test_incoming_catchup.py](https://github.com/abhishekash/agenturm/blob/e594a2a83c7d9d6da63e63aa497d88a8ff27b73b/tests/test_incoming_catchup.py).

Indexed Activity/scalar rollups and bounded source reads replace repeated full-history Activity expansion. Remaining retained MCP grouping, cold migration and combined monitor cost are material and not closed.

Normalized short soak/modal preflights are functional only. No final combined one-hour RSS/WAL plateau or storage-growth acceptance; full HTTP writer-deadline failures remain invalid. Startup writer admission and readiness diagnostics merged #104 after focused validation; full CI and sustained acceptance remain open. The newer retained smoke reached initial readiness, then failed active MCP HTTP 503. The original 473 timeout and every failed cadence attempt remain retained.

Acceptance: OPEN: combined resource acceptance.

### F16 · Local API/privacy/redaction defaults

**Merged tested privacy contract; explicit limits**

Shared actor-wide capture denial, retained client generations, own-window source bindings and UI/CLI late-egress checks merged #94 at `07e9e1a`; 102-test integrated gate passed in 106.887 s.

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [server.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/server.py), [http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/http_transport.py), [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [ui_export.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_export.py), [terminal.py](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/observatory/terminal.py), [ui_review.py](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/observatory/ui_review.py), [review_continuity.py](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/observatory/review_continuity.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_http_transport.py), [test_receipt_export_policy.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_receipt_export_policy.py), [test_native_storage_retry.py](https://github.com/abhishekash/agenturm/blob/e389889ff7ff7ad92716fa5fb889a0a8ea8760a8/tests/test_native_storage_retry.py), [test_feed_capture_safety.py](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/tests/test_feed_capture_safety.py), [test_capture_reference_bindings.py](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/tests/test_capture_reference_bindings.py), [test_capture_cli_bindings.py](https://github.com/abhishekash/agenturm/blob/07e9e1a6cccdcea29d4954405e1c2bb4caa84954/tests/test_capture_cli_bindings.py).

Private token/Host/Origin/redirect checks, best-effort redaction and exact receiver/capture fences remain. New bound material denies stale, missing or different client generations; separately authored material survives. Legacy unbound material cannot establish an unseen denial while offline. No durable offline revocation ledger or universal secret/security guarantee. Merged #102 partial frames never grant positive capture or content authority; denials stay sticky.

Synthetic actual Feed/UI/CLI coverage includes actor-wide missing-session denial, unrelated siblings, rapid deny/reallow with no UI tick, paused/late callbacks, replacement receiver, restart generation, mixed old/new approved drafts and fresh explicit recapture. Combined full CI is separate.

Acceptance: Tested contract94 merged; ongoing privacy invariants and combined CI remain; qualified legacy unbound/offline limits.

### F17 · Native add/update edits discarded

**Implemented, qualified**

Native actions merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75); typed rename integration in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [command_evidence.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/command_evidence.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/workflow_evidence.py), [recorded_paths.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/recorded_paths.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_native_review_contract.py), [test_investigation_store.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_store.py).

Failed/unknown file attempts do not prove a persisted change. Both explicit rename paths survive typed argument projection in G; no arrow/path equivalence guessing.

Source-derived Codex 0.167.1 fixture compatibility through Review/export is merged #87; it is synthetic, not a live transcript.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F18 · Wrapped native checks not recognized

**Implemented, qualified**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75).

Code: [command_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/command_evidence.py), [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_facts.py).

Recognized wrappers/qualified compound success are supported; no-op, masked, unknown or ambiguous failure remains unverified. Recorded success does not certify later worktree changes.

Source-derived Codex 0.167.1 fixture compatibility through Review/export is merged #87; it is synthetic, not a live transcript.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F19 · Exact usage rejected or misallocated

**Implemented and qualified for tested current/legacy source contracts**

Allocation/continuity #67/#71/#75, explanations #81 and current native membership/accounting #87 are merged.

Code: [review_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [core.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/core.py), [opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turn_proof.py).

Contract tests: [test_agent_usage_allocation.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_agent_usage_allocation.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py), [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_current_native_schema.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_current_native_schema.py), [test_opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_proof.py).

Exact single-owner allocations preserve unknowns and partial costs. Current OpenCode 2.2.5 source-derived fixtures verify inclusive 30 input / 13 output with cache-read 9/cache-write 4/reasoning 3; breakdowns are not added again. Missing, unsafe or stale source/member proof withholds totals. Legacy interpretation remains unchanged. These are synthetic accounting assertions, not observed live provider billing.



Acceptance: Qualified tested accounting; unsupported/live allocation stays unknown.

### F20 · Assistant-message placeholders

**Implemented for tested contracts**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [session_text.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/session_text.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_native_sessions.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_sessions.py), [test_session_text.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_session_text.py).

Supported native text blocks reach Review and generic placeholders are suppressed. New upstream schemas and live Claude/Pi compatibility are not proven by these synthetic contracts.

Source-derived Codex 0.167.1 fixture compatibility through Review/export is merged #87; it is synthetic, not a live transcript.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F21 · Initiating request crowded out

**Implemented, bounded**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#75](https://github.com/abhishekash/agenturm/pull/75); instruction investigation in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [core.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/core.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [review_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_facts.py).

Contract tests: [test_workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_workflow_evidence.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py).

Independent request/update context and paging expose intent within explicit source limits. Pruned, uncaptured or clipped intent remains unavailable, never reconstructed by guesswork.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F22 · Complete captured run shows zero work

**Implemented for tested contracts**

Merged [#67](https://github.com/abhishekash/agenturm/pull/67) / [#70](https://github.com/abhishekash/agenturm/pull/70) / [#71](https://github.com/abhishekash/agenturm/pull/71) / [#75](https://github.com/abhishekash/agenturm/pull/75).

Code: [local_agents.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/local_agents.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [work_receipts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/work_receipts.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_workflow_evidence.py), [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py).

End-to-end native-shaped checks/edits/text/usage and generated reconstructions are verified. Coverage limits and unsupported producer variants remain explicit; tests are not current live participant evidence.

Source-derived Codex 0.167.1 fixture compatibility through Review/export is merged #87; it is synthetic, not a live transcript.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F23 · Review JSON/task-link mismatch

**Implemented, qualified**

Merged [#71](https://github.com/abhishekash/agenturm/pull/71) / [#77](https://github.com/abhishekash/agenturm/pull/77).

Code: [saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/saved_tasks.py), [task_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/task_cli.py), [ui_saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_saved_tasks.py).

Contract tests: [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py), [test_saved_task_scope_links.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_saved_task_scope_links.py), [test_task_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_task_cli.py).

Raw Review JSON composes without retyping IDs; turn members/check source traces/accounting survive. Contradictory links reject; legacy ambiguous data stays readable without trusted totals until explicit repair.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### F24 · Passing suite hid fixture mismatch

**Current combined CI pending; historical failures retained**

Startup writer admission, actual MCP readiness fixture and deterministic native fixture connection closure merged #104. Its focused gates passed; main104 full CI is pending. PR102/main103 failures and earlier exact-head successes remain retained.

Code: [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py).

Contract tests: [test_native_review_contract.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_native_review_contract.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_opencode_turn_review.py), [test_owned_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_owned_findings.py), [test_derived_readiness_snapshot.py](https://github.com/abhishekash/agenturm/blob/ebcd03390bbbe57d18470165a4dbdb919de39357/tests/test_derived_readiness_snapshot.py), [test_incoming_catchup.py](https://github.com/abhishekash/agenturm/blob/e594a2a83c7d9d6da63e63aa497d88a8ff27b73b/tests/test_incoming_catchup.py).

ResourceWarning boundaries and owned soak connections corrected in #96 without suppression. New #96 wording/readiness failures have a local correction subsequently merged #98, whose exact head matrix later passed all 12 checks; exact original CLI HTTP cause is unknown. Source contracts, remote CI, invalid performance runs and live/pilot outcomes are separate.

PR95/main95 and PR96/main96 run IDs and actual failure details are retained above and in JSON. Controlled pending-migration HTTP 503→ready200 proves a fixture readiness race, not a unique diagnosis of the earlier CI failure. Local passing tests are not remote all-green. Root reproduced four MCP null/empty inspector subtest errors on main103: the test double lacks native_page_pending. The fixture-only correction merged #104 after 28 UI/model cases and an overlapping seven-model Python 3.13 gate passed; a separate 60-case combined gate passed without warnings. Earlier failures and unknown wire causes remain retained.

Acceptance: OPEN: current full CI; final runtime, resource and external gates.

### F25 · OpenCode prompts excluded by trace

**Implemented for captured current/legacy membership**

Legacy prompt membership #67/#71 and source-derived current-layout turn/parts handling #87 are merged.

Code: [opencode.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode.py), [opencode_turns.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turns.py), [opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turn_proof.py), [core.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/core.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/workflow_evidence.py).

Contract tests: [test_current_native_schema.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_current_native_schema.py), [test_opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_proof.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_review.py), [test_native_fixture_provenance.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_native_fixture_provenance.py).

Current split-part text/user records are collected in their same SQLite snapshot and attached only to proved idle-boundary intervals. Unknown prefix remains exact-step; missing initiating text stays unavailable. Legacy whole-turn mode retains parent behavior. Exact-step mode intentionally omits other steps; no proximity reconstruction.



Acceptance: Qualified implemented captured membership; uncaptured/live intent unknown.

### F26 · Idealized rather than version-labelled goldens

**Implemented source-derived provenance; live coverage open**

Version-labelled selected-format fixtures, immutable primary-source hashes, reader hashes and compatibility proof merged #87.

Code: [opencode.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode.py), [opencode_turns.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turns.py), [opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/opencode_turn_proof.py), [core.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/core.py), [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/observatory/workflow_evidence.py).

Contract tests: [test_current_native_schema.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_current_native_schema.py), [test_opencode_turn_proof.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_proof.py), [test_opencode_turn_review.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_opencode_turn_review.py), [test_native_fixture_provenance.py](https://github.com/abhishekash/agenturm/blob/55c6244bf0c1caad2a22cd5c826020f50a677e54/tests/test_native_fixture_provenance.py).

Current synthetic schema targets are OpenCode 2.2.5 and Codex 0.167.1, derived from immutable primary source contracts; all observed_producer_version fields remain null and captured_from_live_agent false. Legacy observed versions remain null. Full upstream decoding/migrations, live Claude/Pi, universal/future-version compatibility and participant capture are not established.

See [current native source compatibility report](native-fixture-provenance.html) and [authoritative fixture manifest](../../tests/fixtures/native_review/provenance.json). Runtime/provenance peers reviewed the scope; local 78-test gate in 60.817 seconds and root 33-test gate in 23.697 seconds are functional, not resource measurements.

Acceptance: Qualified implemented source-derived provenance; live/universal coverage open.

### F27 · Cockpit-bound delivery, native events and Git

**Implemented and merged; runtime/external gates open**

Git/drafts merged #77; persistent/native delivery merged #82 at e434fcf.

Code: [attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_runner.py), [attention_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_cli.py), [native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/native_lifecycle.py), [storage_native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/storage_native_lifecycle.py), [review_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/review_cli.py), [source_projection.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/source_projection.py).

Contract tests: [test_native_inventory.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_inventory.py), [test_native_activity_integration.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_activity_integration.py), [test_attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_attention_runner.py), [test_native_review_projection_fix.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_review_projection_fix.py).

Headless watcher, shared claims, receiver-owned native inventory and shared Review parity are merged. Generated two-cockpit+watcher preflight passed with one fixture notification and 210 unrelated records per transition, ack/restart and exact resolution. Historical main `55c6244` CI passed; current main104 CI is pending; live request-contract/clock/capacity/desktop precision remain qualified; native approval stays manual.

 Bootstrap #102 is display-only; it does not observe/finalize attention or acknowledge native work.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

## Recommendations R1–R11

### R1 · Reliable cross-runtime attention

**Implemented and merged; external precision open**

Shared classifier, headless delivery and native inventory merged #82 at e434fcf.

Code: [attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_runner.py), [attention.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention.py), [native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/native_lifecycle.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/__main__.py), [review_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/review_cli.py), [source_projection.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/source_projection.py).

Contract tests: [test_attention_runner.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_attention_runner.py), [test_native_inventory.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_inventory.py), [test_native_activity_integration.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_activity_integration.py), [test_native_review_projection_fix.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_review_projection_fix.py).

Quiet inventory, durable dedup, remote pending/resolution catch-up and acknowledgement/restart passed a generated real two-cockpit+watcher preflight. This is one functional scenario with a fixture notification sink, not measured alert precision, OS desktop delivery or live cross-runtime approval support.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R2 · Truthful per-turn receipts with frozen Git

**Implemented, qualified**

Merged [#71](https://github.com/abhishekash/agenturm/pull/71) / [#75](https://github.com/abhishekash/agenturm/pull/75) / [#77](https://github.com/abhishekash/agenturm/pull/77). Current split-part/idle-boundary proof and persisted links are also covered by merged #87.

Code: [workflow_evidence.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/workflow_evidence.py), [review_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_continuity.py), [work_receipts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/work_receipts.py).

Contract tests: [test_turn_task_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_turn_task_continuity.py), [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_ui.py).

Review g/m explicitly maps/captures read-only Git; earlier checks do not certify the current checkout. Recorded edits and all worktree changes/pre-existing/concurrent work stay distinct.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R3 · Actionable failure diagnosis

**Implemented and merged; diagnosis remains qualified**

Operation/Findings facts merged #75/#76; opt-in later matching-success observations merged #81.

Code: [operation_facts.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/operation_facts.py), [findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/findings.py), [finding_sources.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/finding_sources.py), [ui_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_findings.py).

Contract tests: [test_operation_correlation.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_operation_correlation.py), [test_owned_findings.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_owned_findings.py), [test_finding_sources_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_finding_sources_ui.py).

Current/previous unseen reminders remain separate from archival Findings. Exact source s/container trace t, expected nonzero and later-success evidence retain source scope; no repaired-task conclusion.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R4 · Approved automatic continuity

**Implemented; adoption external**

Merged [#77](https://github.com/abhishekash/agenturm/pull/77).

Code: [review_continuity.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/review_continuity.py), [saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/saved_tasks.py), [ui_saved_tasks.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/ui_saved_tasks.py), [task_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/task_cli.py).

Contract tests: [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_ui.py), [test_review_continuity_cli.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_review_continuity_cli.py).

Review d prepares source-backed editable text, then explicit approval appends an immutable version. Human title/notes/brief/decisions are not overwritten; no human decision or intent is invented.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R5 · Parallel-work overlap awareness

**Implemented, qualified**

Merged [#77](https://github.com/abhishekash/agenturm/pull/77); typed rename pair fidelity in merged [#80](https://github.com/abhishekash/agenturm/pull/80).

Code: [review_continuity.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/review_continuity.py), [ui_review.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/ui_review.py), [recorded_paths.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/recorded_paths.py).

Contract tests: [test_review_continuity.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_review_continuity.py), [test_review_continuity_ui.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_review_continuity_ui.py), [test_investigation_store.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_store.py).

Explicit mapped checkouts/recorded paths and strict check-before-other-edit chronology are shown. Historical branch identity often remains unverified; unknown overlap is not collision proof. Different worktrees, failed attempts and ambiguous paths do not become confirmed collisions.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R6 · Progression and repeated-work evidence

**Implemented opt-in and merged; qualified observations**

Merged #81 at 093bf878 includes progression, source-complete Store publication and versioned legacy-cache repair.

Code: [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py), [workflow_patterns.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/workflow_patterns.py).

Contract tests: [test_evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_evidence_explanations.py), [test_workflow_patterns.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_workflow_patterns.py), [test_workflow_backend.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_workflow_backend.py), [test_explanation_cache_upgrade.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_cache_upgrade.py), [test_reported_usage_progress.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_reported_usage_progress.py).

Opt in via AGENTURM_WORKFLOW_PATTERNS=1. Later matching success includes failed and successful sources. Repetition observations may attach strictly timed leaf-model reported contributions only with complete exact owner/execution/source proof. Actual Store publication and resumable workflow_patterns_v2 repair are tested; unknown/partial input remains unavailable. No new always-on usage finding, waste/retry-cost attribution, cumulative delta or context-occupancy inference.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R7 · Qualified MCP recommendations

**Implemented and merged; window-qualified**

Merged #81/#83; loaded-page MCP explanations and shared grouped inventory remain separate from response-quality evaluation.

Code: [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py), [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/terminal_ui.py).

Contract tests: [test_explanation_integration.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_integration.py), [test_evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_evidence_explanations.py).

Current loaded-page outcome/duration denominators, small samples, configured-unobserved context and keyboard flagged-call navigation are wired in the existing MCP inspector. Exact receiver/capture checks remain. An observed protocol/tool success does not evaluate response correctness; absence never recommends deleting configuration.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R8 · Usage scope, cache and optional estimates

**Implemented and merged; unsupported windows remain unknown**

Allocation plus merged #81 explanations distinguish turn/step/session scope and optional exact-model estimates.

Code: [ui_monitoring.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/ui_monitoring.py), [terminal_ui.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/terminal_ui.py), [evidence_explanations.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/observatory/evidence_explanations.py).

Contract tests: [test_task_usage.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_task_usage.py), [test_explanation_integration.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_explanation_integration.py), [test_agent_usage_allocation.py](https://github.com/abhishekash/agenturm/blob/928cae06ea9f8703966c105aa02d42b2dd179f12/tests/test_agent_usage_allocation.py).

Whole-turn ID/member-step labels are distinct from one trace. Exact-owner captured session lifetime is separate; exact-agent calendar-day data is explicitly unavailable. Retry cost and current occupancy remain unavailable without authoritative evidence. Exact-model explicit-rate estimates stay separate from recorded cost and require complete unambiguous allocation.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R9 · Global retrospective investigations

**Implemented and merged; resource limits remain**

Merged [#80](https://github.com/abhishekash/agenturm/pull/80), all 12 final checks green.

Code: [investigation.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/investigation.py), [storage_investigation.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/storage_investigation.py), [investigation_cli.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/investigation_cli.py), [ui_investigations.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/ui_investigations.py), [__main__.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/observatory/__main__.py).

Contract tests: [test_investigation_store.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_store.py), [test_investigation_cli.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_investigation_cli.py), [test_ui_investigations.py](https://github.com/abhishekash/agenturm/blob/0ac4c7560e3395985e8ec79d9f6d32ba5a8374f7/tests/test_ui_investigations.py).

Activity History / Ctrl+F / palette and `agenturm investigate` are wired. Typed retained path/command/instruction queries, bounded paging, FTS availability and pinned exact source reader are tested. No match proves no activity; old lookup/index-cost data excludes final combined-source overhead.



Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R10 · Safe local collection defaults

**Implemented safeguards; combined gates open**

Merged defaults #67/#70 and merged E/F/G paths inherit invariants; combined-source acceptance remains separate. Shared Feed/source/UI/CLI generation fixes merged #94 with a 102-test local integration gate; latest remote CI remains pending/failed as recorded.

Code: [receiver_state.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/receiver_state.py), [server.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/server.py), [native_policy.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/native_policy.py), [http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/observatory/http_transport.py).

Contract tests: [test_recovery_defaults.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_recovery_defaults.py), [test_storage_receiver_hardening.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_storage_receiver_hardening.py), [test_http_transport.py](https://github.com/abhishekash/agenturm/blob/728e0bad85a3e0cde3e62c5599f7a4b6e7ff995c/tests/test_http_transport.py).

Private state/token, Host/Origin, best-effort redaction, capture fences and age retention exist. No all-secrets guarantee, hard disk cap or automatic purge of independently authored task content. Main55 full CI passed; final sustained resource/terminal and external evidence remain separate. Legacy unbound/offline policy history is not manufactured; no durable offline revocation ledger.

 Bootstrap #102 withholds content and lifecycle authority during initial history; startup repair merged #104 after focused validation, with full CI and sustained acceptance still open.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

### R11 · Optional native lifecycle integration

**Implemented supported contract; coverage qualified**

Optional bounded lifecycle metadata, watcher/native CLI and receiver inventory merged #82.

Code: [native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/native_lifecycle.py), [attention_cli.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/attention_cli.py), [storage_native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/storage_native_lifecycle.py), [server.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/observatory/server.py).

Contract tests: [test_native_lifecycle.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_lifecycle.py), [test_native_inventory.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_inventory.py), [test_native_activity_integration.py](https://github.com/abhishekash/agenturm/blob/1c0b795c842a7f41a12f2b562a558bc44e3b5610/tests/test_native_activity_integration.py).

Generic-v1 is explicit exact metadata; Codex notify adapter is completion-only with independently checked turns. Claude/Pi/OpenCode approval contracts without authoritative request/order mapping remain unsupported/uncertain. Installed version inspection is not live compatibility testing.

Optional lifecycle approval authority is distinct from transcript format parsing. #87 qualifies selected source-derived OpenCode 2.2.5/Codex 0.167.1 records, not a live native approval-hook mapping. Generic native-race preflight and installed/runtime research retain their separate limits.

Acceptance: qualified implementation; current main104 combined CI pending; historical failures retained; listed final/live/resource limits remain.

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

The legacy hashes above must not be relabelled as OpenCode 2.2.5 evidence. The new [source compatibility/provenance report](native-fixture-provenance.html) and [adapter-owned fixture manifest](../../tests/fixtures/native_review/provenance.json) are merged #87 and independently peer reviewed. They bind immutable upstream source contracts, synthetic fixture hashes and reader hashes for selected OpenCode 2.2.5 and Codex 0.167.1 formats. All observed producer versions remain null; none is a captured live transcript, and the complete upstream decoder/migration is not claimed.

The current-format fixture verifies inclusive 30 input / 13 output, cache-read 9/cache-write 4 and reasoning 3 without adding breakdowns twice. Unknown prefix does not join a whole interval. Epoch changes, partial/count/digest/clipped proof, queued stale summaries and open-versus-closed/as-of boundaries remain conservative. The local 78-test gate passed in 60.817 seconds, root 33 cases in 23.697 seconds; these are correctness/provenance gates, not resource economics. Live Claude Code/Pi, universal/future-version compatibility and external outcomes remain open. This audit only crosslinks the manifest and never rewrites it.

## Remaining acceptance and external outcomes

Verify full CI for the merged #104 startup and warning/fixture repairs, then prove/fix the active MCP readiness dependency without weakening owner, capture or source guarantees. Complete the retained-corpus workflow/restart/drain smoke before attempting actual one-hour stability. Then refreeze the exact runtime/harness/common corpus for resource acceptance. Preserve both the cold 473 timeout and new 3fb active HTTP 503 failure. The invalid 315/420 schedule and earlier writer-abort attempts supply no loaded p95. Historical direct Store comparisons are not actual HTTP.

Full one-hour stability/restarts/reviews, cold readiness <30 seconds, a useful native frame on the full corpus, idle CPU <5% of one core, actual Sessions/MCP HTTP p95 <100 ms, sustained RSS/modal memory, index/WAL growth and drains remain open. Independent keyboard/signal checks on 473 remain qualified functional evidence; they cannot replace the failed history workload.

The pilot kit is a protocol/generated scenario, not a study. Human reconstruction/resumption, repeat use, alert precision, willingness to pay and adoption remain unmeasured: 0 participants / 0 sessions / 0 repeat-use observations. Desktop delivery is mocked; live Claude/Pi and unsupported runtime coverage remain unqualified.

## Verification of this audit

All 38 IDs, unchanged notes/adapter manifest, preserved historical snapshots, frozen Git paths, local references and standalone responsive light/dark HTML are document-validation gates. They are not product tests or resource measurements. Exact current checks are recorded in JSON and the delivery.
