# Feed capture revocation across incomplete refreshes

This change closes a client publication gap: a settled Feed could receive an authoritative same-receiver capture denial, then fail its indexing or data-readiness check before publishing that policy. Its previous Snapshot and open evidence views could retain permission and captured content.

## Safety contract

- Publish only negative capture policy from a same-known-receiver health response before ordinary index readiness, and again from independent final health. Preserve the old data timestamp; do not present a successful data refresh or fresh lifecycle evidence.
- Purge captured Snapshot fields and content caches on denial. Keep bounded identity/policy metadata sufficient to deny the affected scope. Missing-session denial for a known actor remains actor-wide; it cannot grant another session permission. An unrelated actor sharing a session ID remains independent.
- Fence parallel refreshes, cache writes and late callbacks with a client safety generation. Navigation cancellation cannot cancel negative policy. A failed or partial positive refresh cannot restore access. Restore only through a fully coherent refresh with matching explicit positive policy.
- Keep the last revocation generation after restoration. Frozen Review, receipt, Usage, prompt and source contexts from before a denial remain invalid even when the UI missed the entire deny/allow transition. New exactly scoped evidence may bind to its own published or request-start Snapshot. Completion callbacks never bind an old result to current permission.
- A Feed lifetime has a bounded random generation seed, so another Feed cannot reproduce its first revision counter. Stored source projections with a client binding fail closed if the live client generation is missing or different. New evidence may be captured explicitly; old derived content is not silently rebound. Independently authored task notes remain available.
- Receiver replacement does not relabel old evidence. Review JSON and other existing guarded egress paths withhold the old body; deliberate fresh selection uses the new namespace.

There is no new public ExecutionScope format or backend authorization token. Client safety metadata travels through existing capture-policy signatures. The client keeps at most 500 scoped denial/revision entries; overflow conservatively denies content rather than silently evicting an active denial. This client mechanism does not provide a durable revocation ledger or prove that an unobserved denial occurred while the client was offline.

## Existing consumer preservation

Fresh evidence after explicit restoration remains usable. Review deliberately refreshes exactly scoped evidence; comparison caches check each frozen projection's own window; decision excerpts carry a transient frozen binding through the editor and remove it before persistence. Task continuity evaluates each contributing projection separately, so an old and a newly approved draft for the same scope do not share authorization. CLI handoff and export preserve that projection binding and withhold client-bound source drafts when authenticated receiver health cannot prove the client generation.

MCP flagged references bind from the published display Snapshot. Findings and Investigations bind at published selection or request start and retain that binding through asynchronous hydration. Export callbacks freeze their original display context. These bindings are distinct from a later live policy check: both must pass.

## Validation scope

All fixtures and API responses are synthetic. Tests use real Feed publication, actual Textual screens and temporary task stores; there are no provider calls, native agent launches, personal transcript reads or resource measurements.

The regression cases cover initial and final health denials, indexing not ready and HTTP 503, exact actors sharing a session, actor-wide missing-session denials including an unlisted session, malformed identities, bounded overflow, partial restoration, namespace replacement, cancellation, late parallel allow/error responses, paused prompt buffers, a deny/allow transition without an intervening UI tick, actual Review JSON and export/Usage guards, explicit fresh Review/decision/handoff, and restart generation collisions. Existing Feed concurrency and source-preparation contracts are rerun alongside the focused cases.

Focused results and independent peer review are recorded with the implementation handoff; this document makes no full-suite, CI, throughput or adoption claim.

### Recorded local gates

- `tests.test_feed_capture_safety`, `tests.test_feed_concurrency`, `tests.test_source_preparation`, and `tests.test_capture_cli_bindings`: 62 tests passed in 45.067 s before the last decision-editor clipboard/display refinement.
- The decision-editor refinement plus fresh excerpt persistence and existing turn/task continuity: 14 tests passed in 25.207 s. The final added visible-buffer assertion and frozen context fallback were then checked with two actual UI journeys: passed in 7.619 s.
- Findings/Investigations binding regression and compatibility tests: 24 tests passed in 44.707 s; an independent reviewer reran the seven new contracts successfully in 3.548 s.
- Additional CLI compatibility coverage: 15 tests passed in 7.038 s. The independent peer reproduced the original rapid-reallow, actor-wide unlisted-session and replacement-receiver failures before the fixes, then verified their repaired behavior and fresh positive continuity journeys.

The full combined application, CI and final performance protocols remain integration gates owned by the main implementation. These overlapping local counts are individual test runs, not an additive count of unique tests.
