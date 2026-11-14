# Activity reads certify their own dependencies

**Work in progress.** Preserved for review; not merged or accepted as a completed reliability fix. The retained-history run, hour soak and resource targets remain open.

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

## Validation and limits

- **62 focused tests passed in 43.297 seconds**, Python 3.13.5 with ResourceWarnings treated as errors. The complete merged tool stream was inspected without warning markers, but no original standalone stdout/stderr file or hash was retained; this is not a file-backed cleanliness certificate. Modules cover the new Activity contract, MCP initial certificates, Activity ownership/linking, index/query plans, guarded source snapshots, response identity and caching.
- Independent peer source review found no blocker. Its 31-case Activity/MCP/cache gate passed in 14.701 seconds on Python 3.12.3 with strict ResourceWarnings; these tests overlap the focused gate.
- After adding explicit nine-row/nine-distinct-ID assertions, the affected case passed in 1.770 seconds. This overlaps the focused gate; counts are not added.
- A separate independent mixed-source redaction/stale-replay test, commit `bc318bdbe55a564fc1c09c9b266ac29fa67cd07d`, passed on the candidate and failed at its desired 200 on the baseline. It checks exact surviving sibling rows, full-page equality, strict selectors and zero accepted/retained effects from an older collector batch. This test-only commit remains a separate integration input.
- Initial development fixture errors changed an off-page event, used the wrong revision key, and failed to rewarm after checkpointing a replacement fixture. They were corrected in tests; no runtime fallback or assertion relaxation was added. The first expanded capture expectation was corrected from 200 to the approved stronger 503-until-normal-repair contract.

All fixtures are generated, temporary and use an authenticated loopback server; no provider, personal transcript, retained-corpus clone, profiling run or new full workload was used. Test durations describe correctness gates only. Genuine dirty Activity can still produce 503. A fresh exact candidate's registered functional run, one-hour validation and all resource targets remain separate open gates. Frozen 512 stays incomplete; 9bee stays failed.

The HTML companion was generated but has not had browser layout QA. This checkpoint pushes existing work only; no new validation run was performed.
