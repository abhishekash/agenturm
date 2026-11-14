# Skip impossible MCP execution joins

MCP inventory and call details require a recorded execution span whose `gen_ai.tool.type`, lowercased by SQLite, is exactly `mcptool`. Inside the existing read transaction, an exact absence probe now uses the existing `spans_mcp_execution_v1` partial index. If no such execution exists anywhere in that snapshot, the inventory's execution-failure branch is false and the hydration association join cannot match. Both can be omitted. A positive probe runs the existing ownership, session, server, uniqueness and explicit-linkage queries unchanged. The probe never establishes an association, and nothing is cached across requests. Direct MCP transport/log failure evidence remains independent.

## Contract and review

The strict ResourceWarning gate passed **34 tests in 17.042 seconds**. Full frozen-method payloads match across filters, cursors, query misses, agent scopes, mixed-case and nontext execution types. Explicit and ambiguous ownership/error semantics remain covered. Real external connections insert or delete execution rows after the probe: the current response retains its original snapshot, and the next request observes the change. The query-plan test now supplies a real execution candidate and still requires the authoritative joins to use existing trace indexes. Independent read-only peer review found no blocker.

## Short development diagnostic

Frozen runtime before `029719b024` and candidate `bb81cc7ec4` ran in separate processes in B → C → C → B order. Each arm backed up the same generated fixture with SQLite, performed three warmups, then timed 16 complete `Store.mcp_page(limit=100)` reads. All complete-output hashes matched within each fixture. Ordinary concurrent development continued; these are diagnostic observations, not controlled HTTP, under-write, cold-readiness, monitor CPU/RSS or final resource acceptance.

| Fixture | Before arm medians | Candidate arm medians | Result |
| --- | --- | --- | --- |
| Negative: one MCP call, no execution candidates | 59.108 / 50.471 ms | 13.868 / 13.735 ms | Same full page; impossible SQL omitted. |
| Positive: one MCP call and its explicitly linked execution | 54.291 / 51.629 ms | 52.868 / 47.954 ms | Same full page, error and execution evidence; no general improvement or regression established. |

An added-probe-only check against the separate 31,000-span common raw fixture used the existing partial index. It does not measure a complete historical MCP page. The [raw observations](mcp-execution-absence-diagnostic.json) retain every timing, source revision, output hash and query plan. The [exact executed diagnostic driver](mcp-execution-absence-driver.py) records the local synthetic paths and protocol; those paths are not product defaults.

## Invalid fixture attempts retained

The [first diagnostic](mcp-execution-absence-invalid.json) is invalid for its positive fixture: it read zero calls and no executions. Its immutable read-only backup omitted committed pages from a freshly generated fixture's WAL. Its ancient unit-test timestamps were independently unsuitable for a reopened retention-managed fixture; they are not the proven cause of the empty main-file copy. A second attempt updated timestamps and correctly aborted its new positive-inventory preflight before timing. The final diagnostic uses normal SQLite read-only backup including WAL, recent timestamps, and requires both expected execution presence and nonempty calls **before** timing. No runtime defect or positive-case speed claim is inferred from either invalid attempt. No source seed was mutated or checkpoint forced to conceal WAL.

Full-history authenticated HTTP cadence, cold readiness, idle resources, storage and sustained stability remain open. This change is unreleased.
