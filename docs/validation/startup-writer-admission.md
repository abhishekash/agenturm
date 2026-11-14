# Startup writer admission and bounded backfills

Cold startup now coordinates initial history work and standalone metadata writers before they enter SQLite. It also removes the endpoint backfill's read-to-write snapshot upgrade. Retention stays atomic; borrowed transactions keep their caller's ownership. The frozen 473 full-corpus attempt remains a failed readiness attempt, not a one-hour result.

## Demonstrated defects

In a disposable generated Store on source 473d338, a real source ingestion held the existing local writer gate and SQLite writer at a controlled barrier. Concurrent initial migration and health metadata writes bypassed that gate and both failed at the unchanged ten-second SQLite timeout: SQLITE_BUSY 5 after 10.013/10.018 seconds. Migration retained a fatal not-ready state; the source later committed. This is fault injection of the demonstrated admission gap, not a cold-start performance measurement.

A separate 501-group endpoint fixture retained its first 500-row SELECT cursor. An independent committed writer before the first endpoint update caused SQLITE_BUSY_SNAPSHOT 517 immediately (0.023 seconds). Increasing a timeout cannot repair that snapshot upgrade. Both original reproductions, source commit and scripts are retained in the JSON companion.

The observed frozen run reported processed 9,600 and a database-locked migration error; HTTP logs also show LocalAgents.status→upsert_agent_nodes INSERT failures. Those logs do not retain every failed migration statement or SQLite extended error code. The bounded reproductions establish both code paths; full-corpus readiness must be rerun after integration.

## Lock and transaction contract

The private writer helper acquires the local gate, opens a connection, begins IMMEDIATE and commits/rolls back and closes before releasing the gate. A borrowed `_db` is yielded unchanged: no gate, BEGIN, commit or rollback. This avoids inverting a caller's already-held SQLite writer against the local gate. Metadata read-modify-write now reads only after writer admission, preserving competing parent claims across another Store/process's committed changes.

| Writer | Preserved scope and coordination |
| --- | --- |
| Initial Activity migration | Existing 200-source transaction, gate first, 25 ms stop-aware yield after nonempty commit |
| Historical summaries | Existing 16-trace writer chunks/fairness; short cursor/version markers now gated |
| Task outcomes | 100 summary rows per committed writer page; final marker only after completion |
| Endpoint inventory | 500 aggregate groups per fresh writer page; exact keyset including NULL-first log trace IDs |
| Standalone hierarchy, connections and metrics | Gate before SQLite/BEGIN; existing payload/merge and endpoint updates remain atomic |
| Retention | Existing derived lock, then gate, then one complete retention transaction |
| Activity read repair | Release old read snapshot, gated repair/commit, then fresh read with dirty check and hydration together |

No source cursor or SQLite read snapshot crosses endpoint writer admission. Pages retain the existing grouping and ordering keys; no index, retained cache, larger history limit or payload format was added. Source ingestion's existing commit-to-queue fence and caller-owned endpoint/hierarchy helpers remain intact. Constructor schema setup still precedes local workers. Other processes remain coordinated by SQLite; genuine busy/errors are not swallowed or retried.

Fatal migration errors retain their actionable stage instead of being relabeled as ordinary incoming indexing when dirty work remains. The error itself is preserved; no automatic recovery or optimistic readiness is introduced.

## Validation

The new regression module covers local source/migration/health contention; borrowed transaction ownership; external parent-claim merging; 200-source batch admission; exact 500-group endpoint keysets crossing 501 NULL and 501 non-NULL log groups; inter-page external commits; interruption/error markers and exact resume; 100-row outcome pages; standalone short writers; fresh Activity repair; visible fatal errors; and atomic retention rollback after an injected mid-delete failure.

Frozen expected outputs come from the actual 473 source: all ordered columns of endpoints/services/traces (2,004 source groups), 201 complete outcome summary rows, and hierarchy/identity tables with a fixed clock. No semantic field mask is used. The new 13-case module passed in 4.859 seconds with ResourceWarning treated as an error. The combined writer/catch-up/retention gate passed 45 tests in 23.926 seconds; another 51 existing hierarchy/hydration/storage tests passed in 16.482 seconds. These scopes are recorded separately in JSON. Independent real receiver/native/health integration is tracked separately.

## Limits and remaining evidence

This removes two demonstrated correctness/failure paths. It does not establish a cold-start latency, loaded API, CPU/RSS, WAL/storage or one-hour stability target. Keyset grouping can revisit source data across pages; its full-corpus cost is unmeasured. Native metadata may still wait behind one legitimate writer transaction. External writers that exceed SQLite's unchanged timeout still fail visibly. The failed 473 artifacts and all previous 503/deadline failures remain failures.
