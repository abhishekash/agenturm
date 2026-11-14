# Sessions covered-log lookup: remove redundant grouping

This narrow followup removes a grouping step from the hottest remaining Sessions query. Exact output and query-plan tests pass; a generated private-copy diagnostic records fewer SQLite VM steps. **No new controlled latency result is claimed. The under-write 100 ms p95 target remains open.**

## Change and preservation

`Store.sessions` excludes an Activity log from its preview when the same known actor's retained run summary covers that trace and timestamp. The covered relation previously grouped by trace and actor and computed `MAX(trace_activity.last_ns)`. Both `live_summaries_v1.trace_id` and `trace_activity.trace_id` are primary keys. Their inner join therefore has at most one row for each non-null trace; the maximum and grouping cannot combine distinct observations.

The replacement selects the same actor and timestamp directly. It changes neither the equality predicates nor the timestamp comparison. SQL null behavior remains unchanged: only non-null summary actors cover non-null matching Activity actors, and unknown actors remain unallocated. Falsy retained text identities are preserved exactly. A missing trace-activity row still cannot cover a log.

The candidate union, raw non-Activity fallback, per-session ranking, newest 50 candidates, digest tie-break, requested-session order and payload hydration remain unchanged. The cutoff still applies **before** the rank limit, so older uncovered errors remain visible behind hundreds of newer covered rows. The query remains inside the existing read transaction. No index, persisted table, migration, cache, writer path, native turn-proof path or MCP behavior changes.

## Correctness and query-work gates

The final 40-test Sessions/reminder/operation-correlation/allocation/unknown-usage gate passed in **12.937 seconds**:

```sh
python -m unittest tests.test_session_covered_lookup tests.test_session_projection_batch tests.test_error_reminders tests.test_operation_correlation tests.test_agent_usage_allocation tests.test_snapshot_unknown_usage
```

Four focused tests verify the primary-key premise in the actual schema; complete Sessions payload equality against the immediately preceding covered CTE; exact ordered log-row equality; and removal of the covered temporary grouping without an extra index. Cases include 250 newer covered logs, older unknown/foreign errors, null and falsy text actors, timestamps immediately before/at/after the cutoff, tied timestamps, multiple sessions, the 50-row cap and the non-Activity fallback. Existing deterministic WAL-writer tests retain the original snapshot and raw session identity. Existing allocation/unknown tests preserve token and missing-data semantics.

The old query plan materializes `covered`, scans the joined summary inventory and builds a temporary B-tree for its group. The new plan has no `USE TEMP B-TREE FOR GROUP BY` in this query, while retaining the existing Activity session index and trace primary-key lookup. This is a structural work reduction, not a portable timing guarantee.

## Diagnostic provenance and limits

The diagnostic source `/tmp/agenturm-stage-b-economics/profile.db` was explicitly authorized as generated data. It contains **32,079 spans, 42,765 logs and 6,046 traces**, with v13 derived markers. This is older than the final normalized corpora in the earlier Sessions/MCP reports. The source was opened using SQLite `mode=ro&immutable=1` after verifying an empty WAL. Its file SHA-256, size and modification time matched before/after. No Store initializer, migration, discovery, provider call or writer was run against it.

One private backup was made. Only the three previously documented experimental indexes were removed on that private copy during setup: `live_summary_owner_v1`, `trace_activity_cover_v1`, `activity_log_preview_v3`. No production index was added. The first setup invocation failed before Store method samples because a scratch filename shadowed Python's standard profiling module; the failure and prepared copy were preserved, the filename was corrected, and no bad sample was substituted or hidden.

A short three-call, development-load attribution then identified the uncovered-log query as about 47% of recorded Sessions thread CPU. Full MCP grouping remained the main MCP cost, and its execution-association path already used the expression index; no full-span-scan or MCP simplification is asserted. That attribution included instrumentation/import overhead and allowed main to advance without import-time product hashes. It is useful for finding SQL work, **not frozen-source latency or resource evidence**. Its complete raw output and setup failure remain under `/tmp/agenturm-api-tail-diagnostic-20261114/`.

After the approved change, a bounded query-work diagnostic ran the captured old and new covered-log query once each in the **same read transaction** on that private copy. It used a SQLite progress callback at 1,000-operation granularity, with no writes. Both returned 50 rows in identical order, with identical complete payload SHA-256:

`5680e26223169983932b6aecfba6fed143594e2d580ea419e70ad5591c351e21`

The old query recorded approximately **1,257,000 VM operations**; the new query recorded approximately **1,082,000**, about **13.9% fewer**. VM instructions have unequal costs, and callback instrumentation changes execution. These are query-work observations, not CPU percentages, API p95, monitor CPU/RSS, cold-readiness or whole-method speedup claims. Exact SQL, parameters, both query plans, counts and hashes are preserved in [the diagnostic artifact](session-covered-query-work.json).

The candidate is based on `1aa1948`; the artifact records the candidate runtime and regression-test file hashes. Frozen combined-build measurement remains the coordinator's next gate. This change does not close Stage B or any remaining resource target.


The coordinator independently reviewed the primary-key equivalence and ran a combined Sessions/native-inventory gate on main through PR82: **21 tests passed in 14.316 seconds**. This confirms the query change integrates with the shared native dirty-index path. It does not close endpoint latency or whole-monitor budgets.
