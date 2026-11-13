# Retained-history reads and idle-monitor economics

13 November 2026. This is the first Stage B increment of the [full review-notes plan](../rfcs/review-notes-completion-plan.md). It does not close the resource budgets or the remaining work in the full plan.

## What changed

MCP page hydration previously computed call identity expressions over all retained span/log rows to locate the selected calls. Matching expression indexes now perform those lookups. Trace-scoped partial indexes also keep conservative MCP execution association from degenerating into service-wide nested scans. Connection counts likewise use the exact indexed text expression, retaining legacy numeric-source comparison semantics.

Session original-task lookup now selects the newest nonempty original title per actor inside the same SQLite read snapshot as session metadata. Only the winning title is transferred into Python. Repeated original titles do not expand into historical payloads on every poll.

Derived history replacement uses atomic chunks of at most 16 traces, with a 25 ms acquisition window for waiting writers between chunks. Each chunk holds SQLite's writer lock while reading and replacing derived facts, so ingestion/pruning from other processes cannot split an evidence snapshot. A failure rolls back its chunk; previously committed chunks remain resumable.

Workflow reuses one installed widget tree through public Textual APIs. Reopening restores the recent inbox and exits transient settings/detail states; a drill-down cannot push the same instance twice. This removes repeated Workflow widget allocation. It does not establish that every screen is free of retained render caches or that whole-process RSS cannot grow.

Wide Agents rows use two content lines and one separator instead of two separators. Short terminals retain two-line rows, hierarchy guides, alternating backgrounds and keyboard navigation. Unchanged usage, scope labels and inspector headings avoid redundant presentation updates.

MCP inventory, candidate selection, scope resolution, hydration and execution enrichment share one read snapshot. Call identity includes producer, worker and session; producer-local call IDs do not merge other traces or servers. Execution enrichment requires explicit or unambiguous compatible linkage. Time proximity and summary ownership do not prove call ownership. Ambiguous links remain unallocated. Directly owned log arguments/results preserve JSON types, including false, zero, empty containers and null; directed request bodies cannot supply result/error evidence. The inspector labels recorded null/empty text separately from unavailable values. Scoped call cursors handle the JSON escaping of long accepted identifiers without truncating identity.

Native ingestion retains a failed parsed batch until both span and log writes complete. Another collector's newer source position cannot be overwritten by a stale first attempt or retry. File identity is checked under the writer transaction, including replacement races. Rejected native writes cannot change raw rows, activity timestamps, endpoints or derived facts. Record positions govern storage ordering only; they never establish liveness or turn identity. Pending sources share the existing 50-source bound, and omitted current files are counted separately from retired pending files.

Internal approval-review classification persists as a sticky source policy. In-memory status clears captured content immediately; raw storage and derived-cache redaction commit atomically. A failed write leaves that durable transition pending for retry. New collectors scrub under the writer transaction; SQLite fences reject content-bearing writes from older collectors. An interrupted async refresh cannot keep an old private prompt visible. Existing internal captures are scrubbed during a one-time upgrade. Recorded numeric usage and known metadata remain available. This boundary is source-specific; separately authored task content retains its own consent boundary.

Cheap receiver metadata reads now lease exclusive read-only connections, retaining at most four idle handles. Every cursor closes on return, including unfinished cursors, and each transaction ends before reuse. File identity is checked at checkout/creation/return, including symlink retargets; an in-flight replaced-file read fails rather than publishing the previous receiver's facts. Cross-process updates remain visible. Writers keep their existing close-on-context lifecycle and caller-supplied evidence snapshots are unchanged. This is connection reuse, not a query-result cache.

Workflow review lookup computes its immutable scope hash once per lookup rather than once per saved receipt. A diagnostic profile exposed that repeated calculation; no receipt identity or acknowledgement rule changed.

## Measurement protocol

The comparison uses separate frozen checkouts and fresh generated 35-day corpora: 5,751 traces, 31,000 spans and 41,000 logs, plus generated native session inventory. The pre-change application is merged PR #71 (`ae9fa9c`), with only the identical measurement harness added in local validation commits. `git diff ae9fa9c <baseline> -- observatory` is empty. The current application source is recorded in the attached results.

`benchmark_monitor_idle.py` starts the real receiver, native collectors and a 120×36 terminal cockpit through the soak's isolated child environment. It never reads personal transcript/configuration roots or calls a model. After initial indexing/discovery readiness and a 15-second warmup, it samples `/proc` for 60 seconds. The timed interval contains no driver API probes, keyboard input or generated workload updates. The child heartbeat is present in both runs.

Healthy timed heartbeats must cover the interval with the expected UI PID, receiver namespace, fresh session snapshots and all four generated native sessions. Retained session data is required. MCP history is verified before timing because Agents deliberately does not poll hidden MCP data. Transport, session, JSONL and OpenCode read/schema errors fail validation. Normal cockpit exit, receipt drain and receiver derived-work drain are required. Dirty or profiled diagnostic runs are marked non-publishable.

After the idle monitor closes, `benchmark_history.py` measures 20 samples per read both without writes and under controlled synthetic writes. Its five-second CPU pause describes the Store process only; the separate monitor result describes receiver plus cockpit CPU. These measurements must not be interchanged.

Runs are sequential on one reference host, not randomized. Cold readiness includes background migration and native discovery but excludes corpus generation. The p95 is an empirical order statistic from 20 samples, not a portable latency guarantee. Fixture inventory is idle; it does not establish active-run approval or tool latency. The previous complete-monitor hour remains the active-workload validation.

## Results

Both frozen monitor runs are healthy and publishable under the stated protocol: no native/transport/schema errors, normal terminal/receiver exits, fresh correctly scoped snapshots and drained derived work. Final measured application source: `7c569c13f59b84bf79e973a34200e085fe6d549c`. Subsequent test-expectation/report commits do not change application or harness code. Baseline application is PR #71, with measurement-only commits; [source manifest](history-read-sources.json).

| Measurement | Before | After | Target / qualification |
| --- | --- | --- | --- |
| Cold readiness | 184.38 s | 112.96 s | <30 s |
| Complete monitor idle CPU, one core | 23.63% | 15.90% | <5% |
| Receiver idle CPU, one core | 14.35% | 7.60% | Included above |
| Receiver peak sampled RSS | 58.86 MiB | 67.12 MiB | No plateau claim |
| Cockpit idle CPU, one core | 9.28% | 8.30% | Included above |
| Cockpit peak sampled RSS | 66.66 MiB | 71.09 MiB | No plateau claim |

| Read | Before idle p95 | After idle p95 | Before under writes p95 | After under writes p95 |
| --- | --- | --- | --- | --- |
| Connections | 57.11 ms | 0.12 ms | 71.30 ms | 0.27 ms |
| Activity | 11.65 ms | 17.65 ms | 21.22 ms | 22.26 ms |
| Sessions | 348.41 ms | 264.62 ms | 373.58 ms | 284.90 ms |
| Metrics | 37.48 ms | 74.68 ms | 51.54 ms | 68.54 ms |
| Mcp | 239.08 ms | 140.35 ms | 246.09 ms | 142.60 ms |

Cold readiness improved by about 39%, and complete-monitor idle CPU by about 33%. Connection and MCP reads improved substantially. Sessions remain above the 100 ms p95 target; MCP remains above it both idle and under writes. Activity and metrics p95 rose, although both remain below 100 ms in this fixture. This is a measured increment, not a completed performance repair.

The first healthy correctness increment (`d3f816a`) reduced read costs but regressed whole-monitor idle CPU to 27.20%. A diagnostic profile identified repeated metadata connection setup and repeated scope hashing; its cross-thread/async call-stack attribution is unsuitable for assigning CPU cost to individual UI/CSS stacks. The final measured connection reuse reduced receiver CPU from that increment's 18.02% to 7.60%. The intermediate pool run measured whole-monitor idle CPU at 15.88%; the final payload-compatible source repeats it at 15.90%. The earlier `ca93` run recorded native SQLite lock errors and is rejected for resource claims. These results remain visible alongside the final measurements.

The read probes use the same paced synthetic writer; write-batch counts vary with read duration. They report Store-process idle CPU separately and can capture a pending derived item before the subsequent close/drain. Those fields are not the complete-monitor CPU or its shutdown gate. The generated history deliberately disables retention; its database size is not a 14-day managed-store estimate. The read-probe database measured 414.08 MiB before and 425.89 MiB after; lookup indexes consume additional disk space.

Raw generated measurements: [before monitor](history-monitor-before.json), [final monitor](history-monitor-after.json), [before reads](history-reads-before.json), [final reads](history-reads-after.json), [first healthy monitor](history-monitor-first.json), [first healthy reads](history-reads-first.json), [intermediate pool monitor](history-monitor-pooled.json), [intermediate pool reads](history-reads-pooled.json), and [rejected diagnostic](history-monitor-rejected.json). Private health snapshots, databases and host process inventories are excluded from published artifacts.

## Correctness and usability gates

The final combined backend/native/continuity/ownership/idle-validator gate passed 219 tests in 156.29 seconds. The connection-reuse/controller/source/receiver gate then passed 72 tests in 24.73 seconds; independent final lease review passed 24 tests in 15.19 seconds. These include eight concurrent leases, unfinished-cursor WAL checkpointing, external-process commits, shutdown and idle/in-flight symlink replacement. The final native cleanup change then passed 66 targeted tests in 58.13 seconds. Independent peer review passed its final 24 retry/native/runtime/discovery tests in 15.88 seconds. Earlier storage/native/cache/receiver identity regressions passed 67 tests. New cases verify exact index query plans, legacy numeric-source counts, one-snapshot original titles during a concurrent writer, bounded repeated-title transfer, and chunk rollback/resumption. Workflow tests exercise repeated reopen/resize and reentry beneath a real Review screen. Idle-gate tests reject stale, partial, wrong-namespace and reader-error intervals.

The final 171-test remaining-module gate passed in 372.96 seconds, including compact layouts, keyboard navigation, turn/task continuity, repeated Workflow reuse and delivery claims. A monolithic local run was terminated by SIGTERM after 603 test starts; it is not counted as a full-suite pass. Final CI runs the whole suite across four supported Python versions. Actual offline Google ADK 2.14.1 interoperability passed on Python 3.13; 31 final MCP/paging/type/inspector gates passed in 25.28 seconds and independent MCP review passed 26 tests in 18.92 seconds.

An earlier UI batch passed 62 of 63 cases; the sole failure expected the superseded four-line layout. Its three-line assertions were updated and the affected journey passed again. Rendered synthetic rosters were inspected at 120×36, 80×24 and 60×18. The floor keeps the agent list as the primary pane, while expanded inspection remains a drill-down.

## Work still open

Keep the full plan's cold-readiness, steady idle CPU and session/MCP latency targets explicit. An index improvement alone cannot establish every budget. Remaining work includes source-backed Overview/draft content and reaching the resource budgets. The new ownership and retry gates cover reproduced interleavings, not every runtime or arbitrary external writer. Do not interpret the widget reuse test as a long-run leak-free claim or a human usability/adoption result.
