# Canonical Activity reuse: constraints and attribution

Direct reuse of the existing Activity index cannot replace bounded trace Activity construction without changing payload, scope or privacy behavior. Three generated counterexamples remain mismatched with `trace_derived_v15=1` and an empty Activity dirty queue. This review changes documentation only.

## What still consumes work

Separate subprocesses imported before `5239214` and after `9f3d1d7` (runtime identical to scheduled `5ab5cfd`). Exact `benchmark_http_tails.payload` snapshots contain 141/281 spans, yielding 140/256 bounded Activity rows. Startup, source preparation and one warm derivation precede one cProfile derivation per source/size. Complete stored summary, Activity, error-reference and Findings hashes match at both sizes, with no excluded fields.

At 256 Activity rows after the success-only optimization, total instrumented derivation time was 0.497s:

| Work | Calls | Instrumented cumulative time |
| --- | --- | --- |
| Activity construction | 1 | 0.284s |
| `evidence_detail`, nested within construction | 256 | 0.186s |
| Findings | 1 | 0.084s |
| Findings ownership construction, nested within Findings | 257 | 0.031s |
| Recorded-agent collection, three different scopes | 3 | 0.0275s |

Full operation-fact calculations fell from 140/256 to twenty. Activity construction remains dominant. The roughly 6% owner-construction candidate was not pursued: its small attribution does not justify additional shape proof. Naïve deferral also masks existing malformed-input exceptions; non-string attribute keys and failing service string conversion were reproduced with no credential hits.

These overlapping cProfile times are diagnostic attribution, not unprofiled speedups or API/CPU/cold-start/RSS acceptance results. Raw module hashes, callers, complete projection hashes and reproduction scripts are retained in the JSON companion.

## Why clean indexed rows are insufficient

| Generated case | Bounded canonical result | Existing index result |
| --- | --- | --- |
| Actual MCP benchmark arguments/output changed at the same owner and timestamp | Full Activity changes | Entire indexed payload remains exactly equal; detail is empty |
| Same-owner linked log is outside the selected 256-log window | One inferred span event | Zero indexed span events: the retained linked log supersedes it |
| Raw record exceeds 64 KiB | Zero events from omitted attributes | One event projected from the full raw source |

The first fixture uses trace `52349924f50f8353d3ddf1f101972042`. The others use traces consisting of 32 `b` or 32 `c` characters and span `1111111111111111`. All use disposable generated Stores; the JSON includes exact fixture inputs/scripts. Hydrating the benchmark row invokes `_activity_for` again.

`activity_index_v1.payload` intentionally strips detail, preserves presence flags, truncates subjects to 8,000 characters and adds indexed identity fields. Its span projection considers up to sixteen linked retained logs. `derive_trace` selects raw spans/logs independently under 256-record, 64 KiB-record and 2 MiB-per-signal limits, plus the exact root. Those windows can produce different supersession. Hydration rebuilds detail and clips it to 8,000 characters; it is not a full-payload parity adapter. Existing tests require indexed rows to omit captured arguments and output.

Durable markers and dirty triggers certify repaired indexed metadata in the same transaction. Initial migration drains Activity before historical summaries; incoming and older external writes use durable queues. Neither fact restores absent bodies or proves equal bounded scope.

## Unimplemented options and required proof

Ephemeral exact-source sanitized-fragment reuse would need exact source/window, capture/revocation, namespace, bounded lifetime and async/external-writer proofs. A versioned projection would additionally need retention/privacy/storage budgets, migration and older-writer invalidation. Neither option is approved or implemented; no new captured content is retained.

Before either can proceed, full serialized parity must cover native/SDK/MCP sources, foreign actors and aliases, owner/resource changes, insertion/update/deletion, older writers, linked logs inside/outside windows, roots outside newest spans, stable ties, all size limits, clipped/redacted evidence, missing markers/dirty queues, metadata-only capture/revocation and complete/partial Findings/error references. Unknown proof must retain the original constructor.

The strict 420-write failures, genuine 503s, missed deadlines and post-abort progress errors remain failures. No performance target closes from this review.
