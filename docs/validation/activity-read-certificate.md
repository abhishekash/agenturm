# Native monitor updates and dependency certificates

**Validated for review and merge.** The retained-history three-worker/restart functional gate now passes on the exact frozen candidate, including strict post-restart coverage. One-hour, startup/resource and live-runtime targets remain open.

Small prepared native updates now publish their source, canonical summary and usage projections in one transaction. Agents/Sessions, Activity and Metrics certify the data each view actually needs. Large or unprepared updates retain the existing background path and truthful loading states.

Unscoped Activity HTTP reads can now return a complete page while unrelated Findings, search or native-inventory work is pending. They still return HTTP 503 whenever Activity itself is dirty or its initial source certificate cannot be proved. No failed retained run is reclassified by this change.

## Evidence before changing the gate

Base: `52fe6715496d3a3a8ce2828b939e0ff377296cf5`. One prepared generated Store contains three explicit event sources and nine distinct Activity rows. For each independently inserted Findings, search and native dirty queue, its complete Store page sequences remain identical across pagination, service/session/index filters and scoped queries: full rows, row counts/IDs, cursors, context, receiver and agent-scope metadata are compared. The existing authenticated HTTP endpoint nevertheless returns 503 before reading that coherent page.

The desired-200 regression was loaded against the unchanged frozen 9bee source after verifying its core/server hashes equal the base. It fails on all three unrelated-queue subcases with actual 503, not a fixture exception. The candidate passes the same regression. This proves one excessive dependency; it does **not** identify which queue caused the historical 9bee Activity 503.

Baseline core SHA: `b1f5b304d2f20e63f593e4e0651fb16533ebee71a48b5e23dab069a11e920866`. Baseline server SHA: `05eb08754faa8dcc4301f6ce833dbf4875bb875de3b3e0be8d1983222d533e42`. Candidate hashes and selected test commands are recorded in the JSON companion.

## Certificate and scope

Every eligible request certifies initial readiness, absence of migration/derived errors and stopping, exact receiver/file identity, durable `trace_derived_v15`, current `activity_sources_v2` and `activity_operation_facts_v2`, and a globally empty Activity dirty queue. It does this before cache lookup, then again inside the same read-only BEGIN used for indexed pagination and raw source hydration/context. Certified HTTP reads never enter the legacy 1,000-owner write-repair path. Direct public Store reads retain that existing bounded repair behavior.

Only unscoped reads and service/session/index-local filters may ignore unrelated queues. Presence of `agent_id`, `subtree`, `scope_revision`, `trace_id`, `turn_id` or `review` retains the full strict gate, including blank, false and duplicate values. Requests with globally pending unrelated work skip response-cache reads and writes. Healthy cache hits must first pass the certificate; final response checks reject receiver or database-file replacement, including replacement preserving the same receiver UUID.

The reviewed MCP initial-source read-only/file/preparation mechanics are shared through private helpers; MCP wrappers and error wording are preserved. No schema, stored content copy, new cache, timeout, catch-up budget or retained-workload cadence changes were introduced.

## Mutation, capture and snapshot controls

A real raw event UPDATE must remain unavailable until Activity is repaired. A write after the old HTTP readiness probe is caught by the certificate rather than silently repaired by the HTTP request. Initial history, fatal state, stopping and missing semantic markers cannot be hidden by warmed caches. Owner changes invalidate old scopes; stale hierarchy revisions are rejected. Actual source redaction before certification yields 503 while Activity is dirty; after explicit normal repair, the full scrubbed page equals Store output and another source's full rows remain unchanged.

A source write committed after the certified BEGIN may still produce the entire older coherent page, as in the existing MCP contract. A deterministic test changes both subject and detail during hydration and verifies complete old-page equality, then a subsequent request returns 503 for the committed dirty state. This is not a promise of latest live policy after BEGIN. Existing client capture-generation, namespace and late-egress fences are unchanged.

## Earlier certificate validation and limits

- **62 focused tests passed in 43.297 seconds**, Python 3.13.5 with ResourceWarnings treated as errors. The complete merged tool stream was inspected without warning markers, but no original standalone stdout/stderr file or hash was retained; this is not a file-backed cleanliness certificate. Modules cover the new Activity contract, MCP initial certificates, Activity ownership/linking, index/query plans, guarded source snapshots, response identity and caching.
- Independent peer source review found no blocker. Its 31-case Activity/MCP/cache gate passed in 14.701 seconds on Python 3.12.3 with strict ResourceWarnings; these tests overlap the focused gate.
- After adding explicit nine-row/nine-distinct-ID assertions, the affected case passed in 1.770 seconds. This overlaps the focused gate; counts are not added.
- A separate independent mixed-source redaction/stale-replay test, commit `bc318bdbe55a564fc1c09c9b266ac29fa67cd07d`, passed on the candidate and failed at its desired 200 on the baseline. It checks exact surviving sibling rows, full-page equality, strict selectors and zero accepted/retained effects from an older collector batch. That test-only commit is now integrated as `62a8000` and included in the current focused gate.
- Initial development fixture errors changed an off-page event, used the wrong revision key, and failed to rewarm after checkpointing a replacement fixture. They were corrected in tests; no runtime fallback or assertion relaxation was added. The first expanded capture expectation was corrected from 200 to the approved stronger 503-until-normal-repair contract.

The initial certificate fixtures were generated and temporary, using an authenticated loopback server. That checkpoint used no provider, personal transcript, retained-corpus clone, profiling run or new full workload; the later retained run is recorded below. Test durations describe correctness gates only. Genuine dirty Activity can still produce 503. The fresh exact candidate's registered functional run is recorded below; one-hour validation and all resource targets remain open. Frozen 512 stays incomplete; 9bee stays failed.

The HTML companion is self-contained and generated from this Markdown. Browser layout inspection is not part of the functional proof.

## Completed validation after the WIP checkpoint

The gates overlap; counts are not added. The 181-case gate includes the harness and nine new atomic-publication cases.

The candidate includes main's reviewed phase-5 observation fix, endpoint certificates and bounded atomic native publication. Application and harness were frozen at `346fd39ac7a6140789ac39ce432065b0225ad87c`. The immutable registration was created before launch; all 86 application modules, ten harness helpers, exact source certificates and the common seed are checked. The same 60-second configured duration, two-second phases and 4 GiB reserve were retained. No provider or personal transcript was used.

| Gate | Observed result |
| --- | --- |
| Expanded affected tests | 181 passed, 78.889 s (includes harness cases); Python 3.12.3, strict ResourceWarnings |
| Harness readiness/diagnostics/tail tests | 17 passed, 1.818 s |
| Independent peer source review and overlapping tests | No blocker; final independent gate 50 passed, 40.339 s |
| Retained-history driver | Exit 0, no errors; 20 samples, 71 UI heartbeats |
| Reconstructed parent/child/OpenCode turns | 9/9 verified |
| Expected/accepted/persisted review receipts | 9/9/9; none missing |
| Coverage after both real process restarts | Strict analyzer complete; cycle(s) 2 observed from phase 0 through phase 5 |
| Shutdown | Both receivers drained to zero and exited 0; both cockpits exited 0 without warnings |
| Final readiness and owned-process cleanup | Fresh coherent ready proof; all registered lifetimes stopped |

Cold readiness was **240.286 seconds**; this remains far above the separate 30-second target. Configured active duration is not an absolute-cadence performance benchmark. This run closes this monitor fix's synthetic functional/restart gate, not the hour soak, live-agent pilot or resource targets. Earlier 512/9bee outcomes remain unchanged.

The JSON companion contains an explicit allowlisted result and strict checks, plus hashes for the original private result, immutable registration, analyzer and retained test logs. Raw health, native text, configuration, credentials, host inventory and logs remain local. The stopped historical 9bee and failed b9e2 database/WAL/SHM were archived and verified byte-for-byte before guarded clone cleanup; the failed result was preserved.

## Metrics correction discovered by the retained run

The first candidate b9e2 completed and verified three turns, then failed on `/api/metrics` HTTP503 at phase 9 after 28.667 seconds of active work. All ten original errors and the failed result remain preserved. The reported incoming-index state does not identify which queue caused the rejection.

Metrics reads derived run and log-usage totals as well as raw counts. Its new certificate therefore retains **both Activity and Findings** readiness plus `live_stats_v3`, `activity_sources_v2` and `activity_operation_facts_v2`. Only unrelated native-inventory and search queues are independent. The same initial/fatal/stopping, read-only snapshot and receiver/file fences apply before caching and inside aggregation. A desired200/full-aggregate regression fails on the prior candidate with503 for both unrelated queues and passes with this correction. Real token changes still503 until normal repair; old derived totals and raw counts/services remain one coherent snapshot after BEGIN. Missing schema is a typed503, not a dropped connection. The existing snapshot test now places its writer barrier at hydration rather than the newly added pre-cache probe; every original value assertion remains.

The eight final Metrics tests passed in 2.685 seconds and overlap the 88-test gate. No HTTP/client retry, timeout extension, cadence change or pass-predicate relaxation was introduced.

## Session-list correction and final review

The `cf7f` candidate then failed at `/api/sessions` HTTP503 during phase 9, after 28.915 active seconds and three verified turns. Its ten errors remain preserved; the safe diagnostic did not retain a recognized stage, so no dirty-queue cause is inferred.

The session list and nested OpenCode turn projection read the same Activity, summary and live-usage tables as Metrics, plus raw source facts, in one inherited snapshot. Neither search nor native-inventory indexes supply this projection. Sessions now shares the rollup dependency check, while retaining both Activity and Findings readiness, all preparation/runtime checks, read-only hydration and receiver/file fences. Four generated regressions cover exact full session rows under both unrelated queues, no cache or repair under unrelated dirty work, real dependency/marker/fatal/stopping failures, raw token changes and schema errors. The same full-payload desired200 regression fails with503 before this correction.

Final independent source review and 34 overlapping tests passed in 23.203 seconds, including authenticated OpenCode `collect_review` saved-handoff and disjoint-turn cases. The 92-test parent gate passed in 43.893 seconds. The retained workflow harness, phase sampling, two-second cadence, timeouts, disk reserve and pass predicates were unchanged throughout all three attempts. Both failed working databases and the older `9bee` database were preserved by exact archive roundtrips before guarded cleanup; a compression-format addendum records their current private archive locations.

## Preserve the existing bounded incoming demand

The f411 run failed at Metrics503 in phase3 after 8.933 active seconds, before any completed turn; its twelve errors remain preserved. The certified paths had skipped the existing 75 ms worker-demand window when a real dependent projection was still dirty. The existing `test_slow_worker_is_503_and_wait_retains_no_read_lease` independently reproduces that regression at f411: no wait was armed.

The server now tries its own certificate first. Only a nonfatal incoming-dependency failure on an independent path arms the existing single 75 ms catch-up budget. The failed certificate has released its read lease before waiting. A fresh certificate is mandatory afterward; cache use stays disabled for that request. Unrelated queues arm no wait; initial/fatal/stopping/schema failures and strict paths gain no extra budget. HTTP still never performs index repair.

A new generated async test holds the real worker until the HTTP demand waits, then releases normal accepted-ingest summary jobs. The same single Metrics/Sessions request returns the exact current full projection, with one demand and no HTTP retry. External-only dirty work has no promise of finishing within this budget; its existing 503 test is retained. A stalled worker still returns 503, with no held read lease and each wait at most 75 ms. At that checkpoint, the 101-case gate includes these checks plus all prior dependency, source/capture, cache, identity, snapshot and harness gates. No workload, timeout, reserve, cadence or pass-predicate change was made.

## Atomic publication closes the small-native commit gap

The `9f9f` candidate, despite the restored 75 ms demand, failed in phase 4 at Metrics503 after 11.560 active seconds: three generated turns had completed and none had been verified; all 14 errors remain preserved. The own-dependency certificate was still correctly rejecting incomplete projections.

Warm diagnostics used a private copy of the stopped prepared generated database, with no collector/discovery/provider. Native replay reproduced required Findings dirtiness during asynchronous summary work; SDK writes differed. Profiling overhead and the different replay workload make those timings diagnostic only, not a comparison or accepted performance result. The same native replay after atomic publication returned 200 for all 12 sampled endpoint reads. Original logs and byte-exact database archives remain private.

Prepared small native span/log updates now publish source, Activity, canonical summary, error references, Findings and live usage projections in the **same existing transaction**. The canonical per-trace publication helper is shared with the worker. No derived lock is acquired below the source writer; readers observe the entire old snapshot before commit and the entire new snapshot after acceptance. Existing older pending/inflight jobs recheck dirty state after writer admission and skip already-published work. Progress notification follows source commit and writer release.

Admission is restricted to the existing native scopes, async stores with durable preparation markers and no fatal/stopping state, at most 64 accepted records and four incoming traces, at most 256 retained raw rows and 512 KiB per trace, and 1 MiB total payload. The bound includes spans, logs, progress and outcomes. Large, mixed, non-native, cold, failed and workflow-pattern-enabled cases preserve the existing background path and truthful 503 behavior. Unrelated dirty traces remain dirty.

A savepoint covers all admitted trace publications. Injected failure on the **second** trace rolls back both partially updated projections while keeping both accepted raw changes and dirty rows; the error stays visible and the ordinary worker recovers. Actual redaction, a rejected stale retry with zero retained side effects and a newer still-scrubbed accepted update remain covered. A negative control runs the same accepted-native snapshot test against `9f9f` and fails its desired ready assertion. The final 181-test gate and independent 50-test review passed with strict ResourceWarnings.

**Tradeoff:** canonical work moves into bounded native ingestion, increasing writer occupancy; separate span/log ingestion for the same trace may derive twice rather than coalescing once. This correctness fix makes no CPU, API-latency, cold-start or hour-soak claim. Measuring and then combining native span/log batch publication is a focused follow-up. Retained-history startup and true long-duration/live-runtime validation remain open.

## Independent acceptance and remaining quality work

The independent analyzer confirmed **all 22 strict checks**, exact source/seed identities and unchanged artifacts, cycle 2 after both replacements, every receipt identity, final coherent readiness and no remaining owned process. The configured 60-second workload took **72.758 seconds** to finish its entered cycles and cleanup; phase timing and restart thresholds were unchanged. The peer proof hash is recorded in the JSON companion.

Further work stays within the existing features:

- Bring retained-history cold readiness down from the observed 240.286 seconds toward the existing 30-second target.
- Measure native ingestion CPU/writer occupancy, then combine span/log publication where evidence justifies it; preserve atomic source/capture semantics.
- Qualify this implementation with a genuine hour soak, real native runtimes and human reconstruction checks. This synthetic functional pass does not close those gates.
