# Shared MCP grouping: less query work, mixed latency tails

Status: the bounded implementation passes functional and snapshot gates and reduces repeated SQL work. Both controlled pairs show lower median reads, but their under-write p95 directions disagree. The 100 ms under-write target remains open. This increment does not close Stage B, whole-monitor CPU/RSS, cold readiness or MCP scalability targets.

## What changes

Previously each MCP page grouped retained scoped source records twice: once for complete inventory and once for the page's call keys. The replacement groups calls once in a request-local SQL CTE, then emits tagged inventory and candidate rows. Inventory still includes every retained call in the requested scope; only candidates apply search, cursor and page limits. The existing final call-key/timestamp ordering and server ordering are explicit.

The current ownership-aware call keys, qualified actor/session filters, ambiguity rules, source/error expressions, hydration scope checks and transport/execution association remain unchanged. `null`, false, zero and empty payload presence retain their existing semantics. Unknown-owned sources remain unallocated under an exact known-owner filter. Cursor metric counters remain separate from event calls and retain their existing scope, temporality and reset handling.

The helper executes inside `mcp_page`'s existing `BEGIN` snapshot. No result/namespace cache, persisted table, index or migration is added. External raw updates are visible on the next request. When search is absent, the shared grouping carries no captured search body; when search is present it uses the same bound substring expression as before. Captured result bodies are hydrated through the existing selected-key path.

Production changes are limited to `storage_mcp.inventory_page` and the inventory/candidate regions of `Store._mcp_page`. Runtime and harness are frozen at `1ef63709` on parent `449f64f`; the frozen baseline method comes from `da90e73`, with identical MCP code on `449f64f`.

## Functional gates

The 91-test focused gate passed in 38.339 seconds: new whole-method/snapshot/materialization tests, all MCP source ownership/economics tests, Observatory/API tests and operation hydration. The five new tests independently passed in 6.559 seconds. These gates overlap. Combined-branch CI is still required.

The frozen entire old method is compared with the replacement across every page of global, producer, session, known agent, confirmed subtree, review exclusion and search scopes. Fixtures contain tied timestamps, source-key collisions, foreign and unknown owners, direct log payloads and metric-only counters. Inventory remains unchanged when search finds nothing or the candidate cursor advances. Deterministic external-writer tests replace ownership, change server data or delete logs between grouped selection and hydration; the current page stays in its original WAL snapshot, while the next request sees the update.

On the deployed SQLite version, EXPLAIN shows exactly one `MATERIALIZE calls`; source scans retain the MCP partial indexes and execution/candidate-execution searches retain `trace_id` index predicates. SQLite also materializes its `evidence` CTE. A diagnostic inline alternative removed that extra materialization but showed no CPU or block-I/O benefit, so the runtime was left unchanged. No SQLite-version-specific materialization hint was introduced.

```sh
python -m unittest tests.test_mcp_inventory_batch tests.test_mcp_source_ownership tests.test_history_read_economics tests.test_observatory tests.test_operation_hydration
```

## Fixture and controlled protocol

Both accepted comparisons use fresh independent private copies of the same verified synthetic corpus: **32,170 spans, 42,765 logs, 6,047 traces, about 37.76 days**. This is not the earlier 5,751-trace corpus. The original source is opened only through SQLite's read-only backup API and is not modified. Current derived-index preparation is excluded from timing; the initial C2 rebuild took 291.20 seconds under development load and is not a controlled cold-readiness result.

Each private copy's named indexes exactly match a fresh production Store manifest. No normalization changes were needed. The script refuses changed fixture bytes/inventory, a dirty runtime/harness, nonready data or failed fixed-snapshot entire-method equivalence. It records source HEAD/hashes before and after, requiring equality. Each method receives three warmups. Reads alternate original/current with balanced first-read order. The writer waits 100 ms **after each completed ingest**; actual acceptance frequency also depends on write cost. It does not promise a constant 10 Hz workload. Write acceptance, in-read batches, errors, thread drain and post-timing derived catch-up are checked.

Root and all implementation agents held local heavy tests, browser and Git activity during both intervals. Existing user/background Agenturm processes on separate databases were preserved. Host background noise was not eliminated. These are two short paired experiments on one host. Longer runs also accumulate more synthetic MCP source spans within the writer trace; their absolute timings are not interchangeable.

## First controlled pair: tail regression retained

[All first-pair samples and provenance](mcp-inventory-first-paired.json). UTC **2026-11-14 10:20:24.106667–10:20:35.142782**, 11.036 seconds; 20 observations per method/condition. Entire-method equivalence passed, 35 writes were accepted, 34 during reads, zero write errors, derived data caught up, source fences matched.

| First pair | Original median | Candidate median | Original p95 | Candidate p95 |
| --- | ---: | ---: | ---: | ---: |
| Idle | 106.43 ms | 96.66 ms | 122.86 ms | 109.06 ms |
| Under paced writes | 178.02 ms | 141.42 ms | 230.88 ms | **239.96 ms** |

The first under-write p95 regressed about 3.9%; its maximum also rose, 235.36→283.62 ms. This result is retained unchanged and is not discarded because a later repeat looks better.

## Preregistered unchanged-source repeat

After bounded diagnostics, one repeat was agreed in advance: unchanged frozen baseline/current/harness, a fresh normalized copy, 40 observations per method/condition, the same balanced alternation, warmups and writer. No additional SQL variant or further controlled rerun was made. [All repeat samples and provenance](mcp-inventory-repeat-paired.json). UTC **2026-11-14 10:27:43.490808–10:28:08.259521**, 24.769 seconds. Entire-method equivalence passed, 58 writes were accepted, 56 during reads, zero write errors, derived data caught up, source fences matched.

| Repeat | Original median | Candidate median | Original p95 | Candidate p95 |
| --- | ---: | ---: | ---: | ---: |
| Idle | 118.57 ms | 105.84 ms | 151.54 ms | 134.30 ms |
| Under paced writes | 190.45 ms | 166.59 ms | 343.53 ms | 307.98 ms |

The repeat under-write p95 is lower, but remains 307.98 ms. Its maximum fell, 415.11→362.91 ms. p95 uses nearest rank: observation 19 of 20 or 38 of 40. Both artifacts retain all observations and maxima. The runs are reported separately without pooling or selecting a preferred tail result. They establish lower medians within both pairs and mixed tail evidence; they do not establish a general under-write p95 improvement. Both miss the 100 ms target.

## Work and tail diagnostics

[Diagnostic observations](mcp-inventory-diagnostics.json) are explicitly separate from controlled resource results. They include instrumentation and concurrent development load:

- The original inventory/candidate queries execute about 358,000 SQLite VM steps versus 266,000 for the shared query, with a 1,000-step progress-callback granularity: about 25.7% fewer query operations on this scope. VM steps do not have equal cost and are not a monitor CPU budget.
- Sixteen alternating full reads per method during paced writes recorded median main-thread CPU of 211.74→187.75 ms. This is a diagnostic work signal, about 11.3% lower, not a controlled CPU percentage or whole-monitor claim.
- Slow reads in both versions show wall time above main-thread CPU across grouping, hydration and execution phases. The measurements do not distinguish scheduling/GIL delays, SQLite waiting or other host contention. No specific cause is assigned to the first p95 regression.
- Seven alternating fresh-connection query-only samples showed no useful benefit from inlining evidence: median thread CPU 64.13→65.38 ms, approximately 266,000→265,000 VM steps; measured block I/O was unchanged. Those blocks cannot be assigned to temp spill versus WAL shared-memory lifecycle. The existing shared-CTE runtime was retained.

The optimization removes duplicated query work while preserving the existing evidence contract. Full retained inventory remains proportional to the retained MCP source population, and the materialized grouping still has a cost. Receiver HTTP serialization, TUI rendering, native discovery, cold startup, sustained memory/WAL growth and complete-monitor CPU are excluded. The remaining resource gates require the combined application and receiver/UI workload; no broader target or stage is marked complete here.

Root independently reviewed the query split, exact ordering and unchanged scope/error/hydration contract, and accepted this increment as reduced repeated query work with explicitly mixed tail evidence. This does not close a p95 or whole-monitor budget. Integration on the current source tree retains the same method/helper behavior; final combined CI remains pending.
