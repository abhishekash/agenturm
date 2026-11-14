# Respect local derivation ownership

The compatibility worker could derive locally queued source versions before the normal worker's quiet period or maximum-delay threshold. This change leaves that work with its owner and coordinates local writer admission. It is based on combined candidate `029719b`; it changes no API read guard, native authority, workload cadence, deadline or acceptance target.

## Attribution before the change

A short diagnostic ran an unchanged candidate in a separate receiver process, with external authenticated clients and generated records. It used up to 120 writes on 100 ms slots, two reads per second, the existing missed-slot refusal and no burst catch-up. It recorded only transaction waits/holds, derivation metadata and original HTTP attempts, avoiding per-statement SQL logging.

That particular diagnostic completed all 120 accepted writes with no missed slot and retained all 24 reads, including **12 real 503s**. It is not a successful performance protocol: the independent fixed-cadence runs had already failed, workloads differ, and no controlled quiet window was used.

There were **42 derivations: 30 compatibility and 12 normal-worker calls**. Every compatibility call observed its trace locally pending before both the 250 ms quiet threshold and one-second maximum-delay eligibility. There were **zero duplicate derivations of an identical fixture source version**. The issue was premature derivation of successive versions, not repeated work on already repaired versions. Fixture `COUNT/MAX(end_ns)` observations identify those generated versions only; they are never a production correctness token.

The longest measured writer hold was 173.77 ms. A raw request waited 128.90 ms in `BEGIN IMMEDIATE`, using 0.35 ms of thread CPU. Another 104.12 ms wait overlapped 79.74 ms of recorded writer holds, leaving 24.38 ms outside them. This is consistent with busy-handler or scheduling overhead but does not identify SQLite backoff as the sole cause. SQLite documents its [sleeping busy-handler behavior](https://www.sqlite.org/c3ref/busy_timeout.html). A local admission gate can reduce avoidable handoff, not eliminate real long transactions.

The complete [compressed light diagnostic](compat-owned-before.json.gz) and [sanitized evidence ledger](compat-owned-evidence.json) retain all observations and the earlier invalid full-history HTTP run's exact errors/source hash. The latter stopped at 44/420 and 35/420 writes with unequal inputs; no tail quantile is promoted from that failed protocol. The coordinator's later small fixed-cadence run also aborted after 114/420 writes. Its later reads after writer abort are not evidence of a loaded interval. These failures are retained, not replaced by the shorter diagnostic's accepted count.

## Locking and scheduling contract

- A per-Store writer gate covers each existing source transaction through its **after-commit, non-deriving async queue publication**. Spans, logs, progress, outcomes and internal-source redaction use the same admission context. A synchronous fallback runs only after the gate is released.
- Derivation retains the original outer derivation lock and acquires the writer gate only for each existing 16-trace transaction. Compatibility releases its Activity-drain transaction and writer gate before requesting the derivation lock. No condition is held while waiting for a writer gate, derivation lock or SQLite writer.
- Pending jobs and at most 16 popped/in-flight jobs are observed atomically under the existing condition. Normal and demand batches select at most 16 oldest eligible jobs; original first timestamps, quiet/max-delay eligibility and retry handling remain. In-flight ownership clears and notifies in `finally`, including failure. Backlog fairness waits occur outside all locks.
- Compatibility skips healthy locally owned work and rechecks ownership **inside the actual derived write transaction**. It never deletes a dirty marker merely because a job is owned. Dead/stopping workers cannot claim deferral. An external write to an owned trace is included by that scheduled worker; an unowned external trace remains eligible for compatibility repair.
- A scalar scheduling cursor examines at most 64 dirty IDs, takes at most 16 eligible traces, advances only to the last examined ID, and wraps at the end. This avoids starvation behind a hot owned prefix without copying all pending IDs or scanning an unbounded queue in one transaction. The cursor proves no source version or readiness.

The original under-transaction dirty/cache check remains authoritative. Force and normal-worker repair retain the missing-cache path. **Compatibility may still defer an owned missing-cache job**; its owner or a deliberate force repairs it. Initial historical preparation is not deferred. An empty Python queue never certifies completed source/index state, and errors or capacity pressure do not become empty successful payloads.

The gate coordinates these source and derivation paths inside one Store. Other Store instances/processes and independent metadata/metric writers still rely on SQLite's existing transaction rules. No new payload cache, persistent schema/index or global writer registry is introduced. The gate does not hold the entire historical rebuild or its fairness sleeps.

## Failure paths and verification

The **79-case gate passed in 43.960 seconds**, covering the new ownership tests, existing catch-up/ingestion, external-writer watch, legacy storage retries, same-snapshot reads, storage/retention and interrupted history. An earlier overlapping 36-case gate passed in 16.788 seconds. Independent source/test review found no blocker.

New tests cover the committed-source-before-queue barrier, pending and popped/in-flight deferral, ownership changing after compatibility selection, truthful `IndexNotReady`, an external trace behind more than 64 owned IDs and cursor wrap, force/unavailable-worker repair, all five producer paths, synchronous derivation outside writer admission, raw rollback, oldest 16/16/8 batches and outside-lock backlog fairness. Existing error/retry, shutdown, missing-cache, namespace, redaction and source-snapshot assertions remain intact. Two hand-built worker fixtures only initialize the new empty in-flight set; their original assertions are unchanged.

```sh
python -m unittest tests.test_compat_owned_jobs tests.test_incoming_catchup tests.test_ingest_writer_contention tests.test_idle_history_watch tests.test_native_storage_retry tests.test_derived_readiness_snapshot tests.test_storage_derived tests.test_history_read_economics tests.test_storage_receiver_hardening -q
```

On source/queue failure, locks release; committed dirty source is never declared ready solely by admission. Jobs not admitted remain eligible for compatibility repair. Worker retries retain their original first timestamp and do not overwrite a newer pending entry. The change has **no after-performance result**: the next integrated strict 420-write run is a separate coordinator gate. HTTP tails, cold readiness, CPU, RSS, index/WAL growth and final soak targets remain open.
