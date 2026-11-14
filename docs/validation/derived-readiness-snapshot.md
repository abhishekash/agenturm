# Derived reads certify their own source snapshot

A ready HTTP preflight could race an incoming source commit and return an old token total marked complete. Cached readers now check readiness in the **same SQLite transaction** as their source and derived data. A pending source change returns a typed, retryable 503; a write after the read snapshot is pinned may return a coherent older view. This fixes source fidelity, not incoming-write availability or latency.

## Reproduced problem

On parent main `55c6244`, an authenticated loopback HTTP fixture with two generated spans first reported 5 input tokens. The test captured the handler's real ready probe, committed an update to 999 while the existing derived lock held refresh, and released the request. Sessions returned **200, input 5, reported true, usage complete true**, despite the pending source invalidation. After normal drain it returned 999. The ordinary non-racing incoming request correctly returned 503. [The generated before-fix evidence](derived-readiness-before.json) retains both cases and the committed source hashes; no personal telemetry or provider was involved.

## Resulting contract

`IndexNotReady` is a distinct `RuntimeError`, so existing argument/selector `ValueError` handling is preserved. The small GET wrapper maps only this retryable condition to 503 with actual indexing state. It does not replace failures with empty or successful payloads, retain them in the response cache, remove them from a benchmark, or weaken authentication and receiver publication checks.

The shared read guard verifies the durable run-summary version, initial/migration-error state and Activity plus run/Findings invalidations inside the caller's `BEGIN`. These are the cached readers' dependencies. Unrelated typed-search/native-inventory queues retain their own readiness contracts; the global HTTP preflight and `index_status()` remain unchanged and continue to check all existing queues. An empty Python worker dictionary is not treated as completion.

The guarded transaction now contains:

- Sessions selection, scalar totals, labels, original title, raw-log fallback and nested whole-turn usage. The nested native membership/usage read borrows that same connection; standalone turn Review retains its existing pooled snapshot, receiver-file fence and scoped partial/stale-member proof.
- Exact `review_run` cached facts.
- Every trace-page batch, filter and exact hierarchy/subtree proof.
- Trace overview/default details, raw progress/logs/outcome and cached Findings. The HTTP overview-plus-span-page response also shares one snapshot.
- Metrics' derived totals, raw counts/services and log-only usage fallback.
- Usage-series summary and log-only contributions.

Missing trace-cache repair never acquires a writer while retaining the old read snapshot. A synchronous reader may repair through the existing derivation path outside the transaction and restart the entire read once. An asynchronous receiver schedules its existing worker and returns truthful 503 until repaired. Actual source-backed missing summaries are not silently skipped as empty data; a genuinely unknown trace keeps its existing empty/404 result. Repeated source changes cannot create an unbounded retry loop.

Raw MCP grouping/hydration and its existing `BEGIN` are byte-for-byte unchanged. No schema, index, retained cache, new worker, coalescing rule, waiting/grace policy, native approval or remediation is added.

## Verification

The focused 32-case source/session/turn-proof gate passed in **26.581 seconds**. The separate 96-case backend/API/storage/unknown/allocation/receiver/read-pool/compatibility/cache gate passed in **30.685 seconds**. Two further narrow cases passed in **1.755 seconds**, covering asynchronous missing-cache repair and nested native-turn/session snapshot consistency. An independent peer also passed the first six new cases in 8.946 seconds. The later dependency, async-repair and nested-turn cases are included in the local gates above; new-head integration CI remains separate. These are correctness gates, not controlled resource measurements, and overlapping gates are not summed as independent coverage.

The new authenticated HTTP fixture checks seven paths: Sessions, exact-run Review, trace listing, default trace details, overview-plus-spans, metrics and usage series. It tests both event orders:

1. Source commit after the ready preflight but before the data snapshot: every path returns 503, then 999 after drain. It never returns stale 5 as complete or a fabricated empty success.
2. Source commit after the transaction guard pins the snapshot: the response retains old derived 5 **and old raw 5**. Raw trace spans, log counts and service sets cannot come from a newer connection. The next settled response reports 999.

A separate native fixture changes a member from 12 to 999 while Sessions assembles its nested turn rollup: the response retains coherent session/turn 17; a later repaired read reports 1,004. Additional cases cover initial migration/errors, unauthorized requests, invalid selectors, external source writes, durable-marker invalidation, unrelated queue separation, synchronous repair after the read closes, async scheduled repair and raw MCP null/falsy payload semantics.

The existing interleaved-writer Sessions test now explicitly rejects pending source changes, runs the existing repair outside the read, then deliberately re-corrupts the scalar Activity session field. Its original raw-identity-versus-frozen-output assertion remains: the readiness change does not erase that adversarial ownership check. Existing tests retain retention, receiver replacement, serialized cache invalidation, ownership/unknown usage and exact ordering coverage.

```sh
python -m unittest tests.test_derived_readiness_snapshot tests.test_session_projection_batch tests.test_session_covered_lookup tests.test_history_read_economics tests.test_opencode_turn_proof
python -m unittest tests.test_observatory tests.test_storage_derived tests.test_snapshot_unknown_usage tests.test_agent_usage_allocation tests.test_storage_receiver_hardening tests.test_read_connections tests.test_idle_history_watch tests.test_response_cache
```

## Remaining acceptance

A busy asynchronous receiver can still return 503 while its normal 250 ms quiet-period / 1 second maximum-delay derivation is pending. This patch does not claim to meet the under-write HTTP availability or p95 target. Any bounded demand catch-up must be separately reviewed against the existing worker, wait outside a read lease, preserve a fresh guarded retry and count genuine unavailable responses.

Holding raw/cached facts in one transaction may affect read duration and WAL retention; those costs require the final frozen combined HTTP/monitor/soak/storage measurements. The prior candidate freeze was withdrawn for this required safety fix. Integration/full CI, resource targets, live runtime coverage and external outcomes remain separate gates. No long, quiet-window or controlled performance run was used to validate this change.
