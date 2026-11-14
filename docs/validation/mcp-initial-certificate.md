# MCP raw-source availability with certified cache reads

MCP HTTP inventory without agent/subtree/revision selectors can now read a coherent raw snapshot after initial preparation while unrelated derived queues catch up. Every MCP HTTP cache hit still certifies initial readiness and identity. Scoped requests keep their stricter dependency proof. No association SQL, output fields, captured content, retention rule or global readiness policy changes.

## Before and after proof

Six baseline test methods exposed five assertion failures: unscoped, service and session requests returned 503 solely because unrelated Findings/FTS/native queues were dirty; a current raw identity/output change also returned 503; and deleting the durable initial marker still allowed 200. Complete expected MCP bodies were compared, not selected fields. The original full-corpus active 503 remains a separate failure: its response body was not retained, so these tests establish a real failure path rather than proving that historical response's exact cause.

## Certificate, snapshot and cache contract

Only the absence of all `agent_id`, `subtree` and `scope_revision` query parameters selects raw-source admission. Blank values, false-looking values and duplicate selector keys remain strict.

All supported MCP HTTP routes check Python initial-ready/fatal/stopping state, durable `trace_derived_v15`, expected receiver namespace and file device/inode before any cache lookup. Fresh misses repeat that proof inside the same BEGIN as raw spans/logs/metrics inventory and hydration. Strict selector routes additionally check the same-snapshot global queue dependencies, even when parsed agent ID is empty. Direct Store calls with an actual agent ID also recheck derived readiness before reading hierarchy.

Healthy preflight can retain the existing bounded response cache. Entries are populated by certified misses; any committed database write or file replacement invalidates them using the existing cache observer. When preflight observes globally dirty queues, newly admitted raw reads bypass cache get and put. If a write races a healthy preflight, existing version invalidation plus fresh snapshot certification remains authoritative; the initial healthy flag is not a claim that no queue can become dirty afterward.

Existing read-only connection leases prevent a vanished path from being recreated. Availability/stat/open/lease identity failures become typed 503; genuine raw-query exceptions preserve their original class. Publication rechecks receiver namespace and file identity, rejecting even a replacement with the same receiver UUID.

The HTTP certificate is not a new default for every direct unscoped `Store.mcp_page()` call. Direct unscoped calls without private HTTP certificate arguments retain their previous raw-method semantics.

## Adversarial checks and review

Tests cover raw commits before BEGIN and coherent old complete snapshots after BEGIN; warmed-cache marker deletion and fatal/stopping state; marker commits between certificate and lookup; source ownership/output changes; actual `redact_internal_source` with no private input/output surviving dirty reads; scoped post-preflight reassignment; blank/false/duplicate selectors; same-UUID replacement; and path disappearance both between stat/open and before the raw miss. Authentication remains checked before data access.

The narrow 20-case certificate/cache gate passed in 5.199 seconds; independent peer review ran the same 20 in 4.446 seconds with no remaining blocker. The final 63-case source ownership, association, inventory, cache and snapshot gate passed in 37.014 seconds with ResourceWarning treated as an error. An initial 63-case run had one test-adapter error: the existing reassignment DB proxy lacked `in_transaction`. Only read-only forwarding was added; its hook and PRIVATE_B negative assertions remain unchanged. The affected race separately passed in 0.438 seconds. These gate counts overlap and are not summed.

## Healthy-cache diagnostic and limits

A generated 256-MCP-span in-process HTTP-handler diagnostic alternated ten-request batches with and without response caching. Before this change, warm cached batches used zero raw inventory queries (about 0.0065 seconds per ten); cache-disabled batches used ten (0.225–0.264 seconds). With certification, healthy cached batches still used zero raw queries plus ten certificates (0.0083–0.0089 seconds), versus ten raw queries when cache was disabled (0.083–0.088 seconds). Bodies matched exactly. Timings are descriptive under development load, not a controlled before/after speed comparison or complete-monitor/API/CPU acceptance claim.

The failed full-corpus run is preserved. A fresh frozen functional startup/workflow/restart/drain run is required; one-hour stability, cold readiness, loaded API, CPU/RSS and storage targets remain open. The JSON companion retains exact source hashes, baseline failures, all gate outcomes and cache diagnostic data.
