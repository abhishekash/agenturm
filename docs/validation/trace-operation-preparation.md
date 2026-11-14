# Reuse sanitized facts within one trace derivation

This experiment reuses operation facts already calculated by Findings when the same derivation builds its payload-free error references. It builds on the reviewed scratch/UDF reuse, writer coalescing and ASCII sanitization changes. Default `derive_trace` still returns its original four-tuple, and no stored format, API, source cap or ownership rule changes.

## Lifetime and trust boundary

The core creates a private preparation context **inside each trace's existing write transaction**. After `_activity_for` constructs canonical Activity, the context binds at most 512 original Python object identities. The original Activity list remains alive and is read without mutation. Textual source IDs, fingerprints and copied occurrence wrappers are never cache keys.

Findings retains its full-fact calculation semantics. Only the resulting **sanitized facts** may be serialized into this context, with an aggregate 2 MiB UTF-8 byte limit. No source event, raw body, Store-wide map, global cache or persistent row is retained. Misses, foreign object copies, omitted identities and facts that do not fit recompute the original calculation; existing entries are never evicted. A returned fact dictionary cannot mutate the serialized retained copy.

Error-reference construction obtains facts using the **original canonical event before copying its identity wrapper**. Already compact `operation_reference=True` events deliberately bypass the full cache and use the existing `correlation_facts` protocol, including its unknown/conflicting/version rules. Reusing Findings' full calculation for these compact references would be incorrect.

The context clears in `finally` on success, missing traces and exceptions. Rebinding clears and refuses reuse across another trace. Optional internal parameters connect the existing calls; public defaults remain compatible. The unchanged atomic transaction, source invalidations, 16-trace chunks, fairness delay and output references retain their existing authority. The context makes no new capture-policy or approval decision.

## Correctness gates

- **47 tests passed in 19.730 seconds:** new context tests plus history, storage, retention, writer contention and current native turn proof.
- **42 tests passed in 5.009 seconds:** operation facts, correlation, hydration and opt-in workflow patterns.
- An earlier overlapping 31-case Findings/error/preparation gate passed in 6.553 seconds. Overlapping cases are not added as independent coverage.

Ten frozen original-e795 hashes compare the complete derived summary/Activity/Findings/timestamp and error-reference results. The context-enabled path must match each frozen projection, and its error references must match the original uncached construction before clearing. Additional cases cover foreign actors, compact references with contradictory full facts, credential-bearing and clipped inputs, copied-object misses, detached consumer results, the 512-event and 2 MiB bounds, no eviction, attempted rebinding and an injected Findings exception after the context has retained facts.

Necessary existing test wrappers now accept and forward internal keyword arguments. Their interruption, concurrency, source/retention and rollback assertions and side effects remain unchanged. Tests use generated records; no personal telemetry, preferences or provider is needed.

```sh
python -m unittest tests.test_operation_preparation tests.test_history_read_economics tests.test_storage_receiver_hardening tests.test_storage_derived tests.test_ingest_writer_contention tests.test_opencode_turn_proof -q
python -m unittest tests.test_operation_facts tests.test_operation_correlation tests.test_operation_hydration tests.test_workflow_patterns -q
```

## Isolated-source development diagnostic

Both arms use separate child processes importing their own exact source root. The parent includes the same scratch reuse, writer coalescing and ASCII changes as the candidate; only this context differs. Each arm receives a private SQLite backup of the same generated 96-trace / 576-span / 672-log source. Per-span cached statistics are cleared before timing.

The measured call is the actual `Store._refresh_summaries` path, including error references, writes of derived rows, existing 16-trace transactions and fairness delays. Fixture cloning, initialization and output inspection are outside the clock. These are short experiments under development load, not a registered quiet comparison.

| Unprofiled order | Parent | Context |
| --- | --- | --- |
| Parent → context | 2.834 s | 1.885 s |
| Context → parent | 3.015 s | 1.956 s |

A separate instrumented pair counted **2,312 → 1,156** full operation-fact calculations. All six runs have the same hash over every stored summary, Activity, Findings and error-reference field; only JSON formatting is normalized. The complete [sanitized diagnostic](trace-operation-preparation-diagnostic.json) records source import paths, raw times, counts and limits. Its parent/context roles are **not** the registered whole-application baseline `5e0c397` and final candidate.

The reduction justifies this bounded context in the measured path. It does not prove HTTP p95, cold readiness, whole-monitor CPU, sustained RSS, index/WAL growth or soak targets. Serializing and parsing facts adds work and retains up to 2 MiB of sanitized data per active trace derivation; tiny traces, capacity misses and different workloads may benefit less. Independent optimization percentages must not be added together. Final combined resource measurements and integration CI remain separate gates.
