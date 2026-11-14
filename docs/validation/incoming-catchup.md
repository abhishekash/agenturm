# Bounded incoming derivation catch-up

This follow-up to [the snapshot-fidelity fix](derived-readiness-snapshot.html) lets an asynchronous receiver briefly request existing worker progress before rejecting an incoming read. It does **not** serve old cached usage as current, hide 503s, or claim the final latency/resource targets are met.

## Why this is needed

The normal worker waits for 250 ms of quiet, with a one-second maximum delay. With writes arriving at 10 Hz, a full health request can repeatedly see pending derivation and keep the cockpit in its loading state. The earlier correctness fix deliberately retained that truthful unavailability. A small, bounded demand allows the existing worker to try a normal derivation sooner.

## Contract and bounds

- A non-ready data preflight or full health request may use **one absolute 75 ms catch-up budget**. Initial migrations, recorded worker/migration failures, shutdown, synchronous receivers and lightweight `health?probe=1` do not request work. The historical run-summary marker must be durably complete.
- Demand uses the existing condition and pending dictionary: one shared deadline and a completion generation counter, with no second job queue, additional worker, retained result cache or schema/index change. Overlapping requests share existing jobs; an expired demand cannot turn into an always-on priority mode.
- The worker may select at most **16 oldest pending trace IDs** using a bounded heap, derive them through the existing implementation, then return to its normal quiet-period policy. Existing per-trace first timestamps, deduplication, retries, transactional derivation and shutdown drain remain in force. Selection scans pending keys but retains only 16 heap entries.
- Metadata checks use a short read-only SQLite connection, close it before waiting, and check durable readiness plus **all existing global dirty queues**. SQLite busy timeouts shrink with the remaining deadline; successful statements and completion wakeups never reset the budget. No pooled read lease or SQLite transaction remains while the HTTP request waits on the existing condition.
- Completion wakes waiters after normal or compatibility derivation, including failure. An empty Python pending dictionary never certifies readiness: popped/in-flight work and external-writer queues remain unavailable until the actual database snapshot is clean.
- A successful catch-up still goes through the existing data snapshot guard. A new source commit after catch-up can therefore produce a genuine 503. A commit after the guarded read begins can produce a coherent older snapshot, as documented by the earlier fix.
- Full health composes its receiver time, receiver identity, capture status and local-reader status after the attempt, using the actual checked indexing result. Existing final receiver-publication checks remain authoritative. Authentication, Host/Origin checks, content policy and raw MCP query semantics are unchanged.

The 75 ms is the catch-up allowance, **not** a whole-request latency promise or a hard real-time scheduling guarantee. Existing HTTP authentication, readiness preflight, response construction, final policy/namespace checks, SQLite work and scheduler delays are all charged to the request. A budget expiration, large backlog, missing marker, unknown/error state or incomplete external queue remains non-ready; data requests return 503 rather than a fabricated empty or stale complete response.

## Functional verification

The new availability cases plus the previous source-snapshot regression gate passed **17 tests in 17.367 seconds**. A separate compatibility, recovery, retention, receiver replacement, read-pool and response-cache gate passed **32 tests in 7.305 seconds**. These are functional tests under development load, not controlled or publishable performance measurements. Root integration passed 42 tests in 25.078 seconds; the strengthened accepted-counter case passed separately in 1.674 seconds. Independent peer review found no runtime blocker. Integration CI remains a separate gate.

The new cases cover:

- Eight accepted writes scheduled at 100 ms intervals through a generated two-span fixture, using authenticated real HTTP health and Sessions reads. Every successful Sessions total matches its returned source timestamp/counter, never omitting a write accepted before the read. A view can omit only commits arriving after its guarded snapshot begins. The final total is eight and the source still contains two spans.
- A held slow worker returning visible 503 within a bounded functional deadline, with no read lease retained during the condition wait; the later settled result is 999.
- Four concurrent timed-out requests sharing one existing trace job, all four returning 503, followed by the settled 999 result. No duplicated job or token contribution is introduced.
- An external dirty source with an empty local pending dictionary staying unavailable until normal compatibility drain.
- Initial migration, migration error, recorded worker failure, absent durable marker and lightweight health probe never arming demand.
- Database replacement during a full-health catch-up returning 409 instead of publishing the old receiver identity with the new state.
- Forty queued jobs: the priority batch contains exactly the oldest sixteen, with all forty drained once through the existing worker.
- An injected derivation failure preserving the original retry path and completion notifications, then clearing the error only after successful normal derivation.

[The complete tiny-fixture smoke results](incoming-catchup-smoke.json) retain every HTTP status, including intentionally unavailable slow-worker, concurrent and external-only cases. They include source hashes and explicitly set `publishable_performance: false`. No result is discarded because it is unavailable. No personal source corpus, native runtime, user configuration or provider service is used.

```sh
python -m unittest tests.test_incoming_catchup tests.test_derived_readiness_snapshot -v
python -m unittest tests.test_idle_history_watch tests.test_recovery_index tests.test_storage_receiver_hardening tests.test_read_connections tests.test_response_cache -v
```

## Cost and remaining acceptance

Demand can reduce write coalescing: a foreground reader may cause a trace to be derived on each request instead of waiting for a quiet interval. This can increase CPU, write transactions, fsync/checkpoint work and contention. Dirty requests also open bounded metadata connections and inspect existing queue heads. No idle timer or periodic extra query is added, but those active-load costs still require measurement.

The small fixture demonstrates a useful functional path, not a p95 improvement, a large-corpus availability target, whole-monitor CPU/RSS compliance or a storage/WAL bound. All final frozen-candidate HTTP, idle/active monitor, cold-readiness, modal-retention, soak, shutdown and storage gates remain open. Larger/external workloads may still legitimately return 503. Feed and UI policy semantics are outside this change.
