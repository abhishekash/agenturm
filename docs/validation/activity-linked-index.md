# Exact linked-log seeks during Activity replay

Status: a narrow declarative index removes a repeated agent-history scan while preserving the existing source query. Functional gates pass. All timings below are **diagnostics under development load**, not controlled performance or resource-budget results. Stage B, full cold startup, complete-monitor CPU/RSS and final combined G/F replay costs remain open.

## Bounded change

The only runtime change is one index in `storage_activity.initialize`:

```sql
CREATE INDEX IF NOT EXISTS logs_activity_linked_v1 ON logs(
    trace_id,span_id,service,agent_id,time_ns DESC,digest DESC)
WHERE trace_id IS NOT NULL AND span_id IS NOT NULL AND agent_id IS NOT NULL
  AND event_name='agent.activity'
  AND length(CAST(attributes AS BLOB))+length(CAST(resource_attributes AS BLOB))+
      length(CAST(body AS BLOB))<=65536;
```

The existing linked-log query already requires those exact trace/span/producer/actor equalities, the same stored-source byte cap, and newest timestamp/digest ordering with a 16-record limit. A `NULL` identity cannot satisfy its equality predicates. The extra null exclusions therefore avoid indexing unlinkable logs without removing a possible match. Only source identity metadata enters the index; captured prompts/output are not copied into it.

Drain order and limits, invalidation triggers, `_activity_for`, typed provenance/supersession checks, capture behavior, hydration and per-owner Python memory are unchanged. The source iterator still handles at most 16 qualifying linked logs, each with at most 65,536 combined stored attribute/resource/body bytes. No batch raw-payload prefetch, skipped semantic check, extra queue or result cache is added. Existing/older writers maintain the index automatically through SQLite; source updates still queue the existing repair jobs.

## Where SQL work was going

The baseline is `5e0c397`, including C3 and the Sessions improvement. The private database is a read-only SQLite backup of the verified generated retained corpus: 32,170 spans, 42,765 logs and 6,047 traces. Neither the original source nor personal telemetry was modified. The diagnostic requeues the latest 500 raw log identities and latest 500 span identities, then runs the real `drain(limit=1000)` in one write transaction.

**Pending Stage G investigation index/FTS replacement hooks and Stage F workflow dirty/projection hooks are absent from this baseline.** Their cost is not included in replay or raw-insert diagnostics. The selected recent records also include richer native/typed facts than the earlier simple generated review fixture. Its 2.612-second/4,931-call profile cannot be compared directly with this 2.943-second/4,741-call profile.

[Baseline statements, counts, rows and EXPLAIN](activity-drain-before.json), [indexed replay](activity-drain-indexed.json):

| Statement group | Calls | Returned rows | Baseline diagnostic SQL time | Indexed diagnostic SQL time |
| --- | ---: | ---: | ---: | ---: |
| Linked logs for one span | 500 | 148 | 603.63 ms | 12.54 ms |
| Delete existing Activity owner rows | 1,000 | 0 | 113.78 ms | 109.11 ms |
| Insert Activity rows | 740 | 0 | 75.17 ms | 43.54 ms |
| Read raw logs by digest | 500 | 500 | 28.67 ms | 18.18 ms |
| Read raw spans by primary key | 500 | 500 | 26.04 ms | 19.14 ms |
| Delete completed dirty jobs | 1,000 | 0 | 19.07 ms | 13.75 ms |
| Delete old log-usage entries | 500 | 0 | 5.08 ms | 3.84 ms |
| Select queued source jobs | 1 | 1,000 | 3.09 ms | 1.68 ms |

The baseline linked lookup chose `logs_agent_id (agent_id=?)`, then filtered trace/span/service and sorted the remaining timestamp tie field. It repeatedly visited an actor's retained history. The new plan is `logs_activity_linked_v1 (trace_id=? AND span_id=? AND service=? AND agent_id=?)`, with no temporary order B-tree. Other predicates remain in the same query and semantic checks remain after retrieval.

SQL call count remains 4,741; this change removes the repeated history scan rather than batching source payloads. The whole profiled replay was 2.943 seconds before and 1.796 seconds with the index, but host load, cache state and semantic CPU also varied. That is **not** a controlled end-to-end improvement claim. All sampled Activity rows and serialized payloads were exactly equal before/after indexed replay. No full 75,000-source replay was run.

## Existing-index alternative and footprint

Private-copy comparisons execute the same 500 lookups, returning exactly the same 148 records. Three samples per variant include progress callbacks for approximate SQLite VM counts. [Narrow-index diagnostics](activity-linked-index-narrow.json):

| Lookup variant | Median wall | Approximate VM steps | Plan |
| --- | ---: | ---: | --- |
| Current planner | 422.80 ms | 1,312,000 | Agent index plus partial-order sort |
| Force existing `logs_trace` | 9.94 ms | 29,000 | Trace seek plus partial-order sort |
| Narrow composite index | 5.06 ms | 11,000 | Exact four-key seek; no sort |

Forcing the existing trace index adds zero schema/storage cost and captures most of this fixture's benefit. The additional composite benefit is only about 4.9 ms across 500 lookups here. The declarative index is chosen for its exact bounded seek across all four keys and complete order, rather than coupling runtime SQL to a forced implementation index. A large shared trace can still make a trace-only seek broad; no large-shared-trace performance bound was measured here.

The initial broad composite candidate indexed unlinked logs too: 7,303,168 bytes (6.96 MiB). It also increased the diagnostic 500-log insert median from 59.24 to 69.82 ms. That candidate was rejected; [its measurements are retained](activity-linked-index-broad.json).

The selected null-excluding index covers **392** eligible rows in this corpus and occupies **73,728 bytes (72 KiB)** according to `dbstat`. Diagnostic creation took 139.05 ms wall / 88.91 ms thread CPU. Eligibility population and index growth depend on actual linked-source retention; these numbers are not a universal size or cold-start limit.

Five generated committed 500-log batches per case, with alternating baseline/indexed first order, include current raw-source triggers/indexes and commit. They exclude Activity drain, semantic derivation, pending G/F hooks and cleanup. Cache, host scheduling and physical file layout affect the times:

| Raw insert case | Baseline median wall | Indexed median wall | Baseline median thread CPU | Indexed median thread CPU |
| --- | ---: | ---: | ---: | ---: |
| Unlinked trace/span | 63.01 ms | 60.62 ms | 59.48 ms | 54.93 ms |
| Known trace/span/actor | 67.53 ms | 70.93 ms | 60.78 ms | 66.41 ms |

No speedup is claimed for the noisy unlinked case. The known-link case shows about **5.0% wall / 9.3% thread-CPU overhead** in this diagnostic. The write fixture supplies nonnull source identities without running a complete parent span/turn pipeline; it measures index maintenance, not real agent ingestion throughput. The read benefit is therefore weighed against explicit write and storage costs.

## Correctness gates and remaining work

The 61-test focused Activity/recovery/hydration/native/MCP/economics/storage-budget gate passed in 42.737 seconds. After strengthening the legacy-writer owner-reassignment case, the four new tests passed again in 3.088 seconds. These gates overlap; combined-branch CI remains required.

Tests verify exact Activity outputs with the index absent/present, typed operation presence, newest-16 and timestamp ties, exact 65,536/65,537 stored-byte boundaries, foreign actor/producer/trace/span and unknown ownership, null exclusions, old-writer body/owner updates and deletes, dirty-job invalidation, prune removal and idempotent initialization. EXPLAIN requires all four identity keys and no order sort. Root will combine this index with the separately reviewed queue-hook work and measure the resulting application; this increment does not change hydration or queue fairness.

```sh
python -m unittest tests.test_activity_linked_index tests.test_recovery_index tests.test_operation_hydration tests.test_native_storage_retry tests.test_mcp_source_ownership tests.test_history_read_economics tests.test_statistics_storage_budget
```
