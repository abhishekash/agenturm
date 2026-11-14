# Raw write contention: diagnostic and atomic publication

14 November 2026. This increment addresses an observed write-path cause of the invalid final HTTP validation. It does **not** close the 100 ms writer deadline, authenticated HTTP availability, retained-history read-latency or complete-monitor resource targets.

## Reproduced cause

A tiny generated database was sufficient to reproduce write stalls. The diagnostic sends the registered payload shape over real authenticated loopback HTTP: two span updates per batch, one repeatedly updated root and a new MCP child, scheduled every 100 ms. Sixty batches mean 120 accepted span updates, with an expected final raw footprint of one trace and 61 unique spans. No native discovery, provider, user transcript or notification is involved.

The receiver and clients share one Python process in this diagnostic. SQLite execute calls, transactions and derivation are instrumented. All configured attempts finish, including catchup after a stall; this is deliberately **not** the registered fixed-window acceptance protocol and its timings are not publishable endpoint or resource measurements. SQL timings include trigger work, and enclosing categories overlap; totals must not be added across categories.

At source `e795f064c861cd59b54d1180697e4b19c9959b34`, the initial 60-batch run without readers recorded:

| Diagnostic observation | Recorded value |
| --- | ---: |
| Raw POST `BEGIN IMMEDIATE` waiting, 60 calls | 1,451 ms total; 334 ms maximum; 7.4 ms thread CPU |
| Separate agent-node insertion waiting/work, 120 statements | 703 ms total; 330 ms maximum |
| Compatibility worker derivation of the same active trace | 17 calls; 1,361 ms total; 139 ms maximum |
| Asynchronous derived worker derivation of that trace | 7 calls; 733 ms total; 153 ms maximum |
| Activity drain in raw POST transaction | 60 calls; 214 ms total; 5.3 ms maximum |
| Connection commits across the POST transactions | 120 calls; 145 ms total; 2.6 ms maximum |

This run points to writer acquisition behind long derived transactions, including the second identity transaction, rather than a long commit/checkpoint as the observed tail cause. This is evidence from this fixture, not proof about every possible checkpoint or retained-history workload. A second tiny run with Sessions/MCP readers also stalled: seven writes started more than 100 ms late and five of twelve GET attempts returned 503. All responses remain in the raw diagnostic records.

The first diagnostic command completed HTTP work, shutdown and its JSON output, then exited with a reporting-only `KeyError` because its console renderer used `duration_ms` instead of `elapsed_ms`. That console bug was corrected before the second run. Its first raw artifact is retained with this qualification; it is not a successful acceptance run.

## Bounded implementation

Raw span ingestion now persists derived agent identities in its existing SQLite writer transaction, using only records accepted by native-generation and capture-policy guards. Raw spans, Activity indexes, identity merges and hierarchy revisions either commit together or roll back together. Standalone identity registration keeps its existing connection lifecycle and validation, conflict handling and bounded node limit.

Queued background repair now checks its durable dirty row **inside the acquired writer transaction**. If another worker already repaired the current snapshot, it skips redundant derivation. An absent `trace_summaries` row remains repair work even with no dirty marker: asynchronous missing-cache read repair cannot be stranded. Explicit refresh and historical backfill remain forced. An external source update before writer acquisition is included; one that commits afterward retains a new dirty job. The guard adds no result cache, schema or queue, and does not relax readiness or exact-snapshot ownership.

## Remaining performance boundary

The post-change diagnostic with readers still missed six write slots and returned five 503 responses. Both workers still legitimately derive changing evidence, and one bounded trace derivation can exceed 100 ms while holding SQLite's writer lock. Removing a second raw-write acquisition and redundant clean jobs is insufficient to guarantee the registered cadence. No latency improvement is claimed from these short, instrumented, uncontrolled runs.

Further work must reduce individual derived transaction cost or change computation/publication with an equally strict source-version/snapshot contract. Slower writer cadence, skipped attempts, filtered samples or weakened readiness are not fixes. Final retained-history validation must use complete frozen source trees and the unchanged authenticated HTTP acceptance protocol.

## Reproduction and tests

`scripts/profile_ingest_writes.py --batches 60 --readers --output /tmp/generated-ingest-profile.json` creates and removes its own tiny database and receiver. It never accepts a path to an existing database. The script is an opt-in diagnostic, explicitly marks output nonpublishable, bounds batches to 1–200, retains raw HTTP responses and individual SQL/phase timing, and records source-file hashes.

The focused storage gate ran 63 tests in 24.891 seconds successfully: new transaction/coalescing contracts, idle compatibility polling, derived snapshot readiness, agent hierarchy and native storage retry tests. New cases cover one raw/identity transaction, rollback with no orphan identities or indexed Activity, rejected native records, forced and missing-cache repair, and deterministic external writes before acquisition and after the held snapshot.

Compressed raw diagnostic JSON is retained beside this report: `ingest-write-tiny-before.json.gz`, `ingest-write-tiny-readers-before.json.gz`, and `ingest-write-tiny-readers-after.json.gz`. The first two identify the immutable baseline source; the third records hashes of the modified runtime files. These are diagnostic observations, not a paired performance acceptance result. A final three-batch script smoke separately checks fixture counts and cleanup after the reporting/cleanup refinements.

A final eight-case gate, including the actual generated-only diagnostic subprocess and explicit connection closure checks, passed in 3.325 seconds with `ResourceWarning` treated as an error. Independent source review found no remaining blocker in the coalescing and atomic publication changes. These local gates do not substitute for the final combined-source CI and retained-history validation.
