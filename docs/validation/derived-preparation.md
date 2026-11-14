# Reuse connection-local derivation setup

This narrow change removes repeated temporary-schema setup and function replacement while preserving exact derived facts. It is based on `e795f064c861cd59b54d1180697e4b19c9959b34`. The existing 16-trace write chunks, inter-chunk fairness delay, source transactions, invalidations, retention and API readiness contracts remain unchanged.

## Finding and change

On a generated 96-trace fixture with 576 spans and 672 logs, the original implementation registered two SQL functions and performed three scratch-table DDL statements per trace. Direct scratch DDL took only 33–36 ms in an instrumented 4.6–5.2 second run: **it was not the dominant cold-derivation cost**. Findings qualification and operation-text sanitization dominated that Python profile, whose per-character tracing overhead is itself material.

However, repeating temporary-schema changes and replacing functions prevents useful preparation reuse. The new implementation prepares the same statistics callback and session normalization function once per connection, marked by its private temporary scratch table. Both callbacks are pure for their inputs and registered as deterministic. The callback's calculation body is unchanged.

Every usage aggregation clears scratch rows before processing and in `finally`. A later trace with identical span IDs cannot join an earlier trace's rows. A rolled-back table creation is detected and prepared again. The temporary table and functions disappear with the caller's existing connection; there is no global connection map, retained Python trace cache, persistent schema/index, new worker or API change. The scalar cache's existing source-change triggers still invalidate statistics; ancestry/leaf selection is recomputed on every derivation.

SQLite's [authorizer documentation](https://www.sqlite.org/c3ref/set_authorizer.html) explains that authorization callbacks run during statement compilation, including recompilation after schema changes. They count authorization actions, **not prepared statements**. Its [function documentation](https://www.sqlite.org/c3ref/create_function.html) specifies connection-local registration and the deterministic flag. The diagnostic below is consistent with less repeated preparation, rather than materially fewer VM instructions.

## Short diagnostic

The same generated source was cloned before each arm, outside the clock. Per-span cached statistics were cleared before each run. Each arm derived all 96 complete tuples in the existing 16-trace-per-connection pattern, with commits and connection closure inside the clock. No source corpus, personal configuration, provider or live runtime was used.

| Unprofiled development order | Original e795 | Reuse |
| --- | --- | --- |
| Original → reuse | 2.301 s | 1.903 s |
| Reuse → original | 2.316 s | 1.879 s |

All four complete serialized result hashes are identical. These two short pairs show lower wall time here; they are **not a controlled performance claim**. Other development work was permitted, and no reference-host quiet window was registered.

| Separate instrumented run | Original | Reuse |
| --- | --- | --- |
| Function registrations | 192 | 12 |
| Scratch DDL executions | 288 | 6 |
| Compile-time authorization actions | 72,396 | 5,952 |
| Approximate VM steps | 593,400 | 590,400 |
| SQL executions | 3,846 | 3,948 |

The extra bounded preparation checks and deletes increase executed statement count. The benefit here comes with far fewer compile-time authorization actions, rather than fewer executed SQL statements. [The complete sanitized diagnostic](derived-preparation-diagnostic.json) retains both unprofiled pairs and the separate instrumented observations. Its `original`/`reuse` labels do not refer to the registered whole-application baseline `5e0c397` and final candidate.

## Correctness and review

The focused **30-test gate passed in 13.967 seconds**, covering storage derivation, exact owner allocation, unknown usage and current native turn proof. Ten frozen hashes generated from the immutable original e795 implementation compare the **entire** serialized summary/activity/Findings/timestamp tuple and exact error references. They cover multiple traces with overlapping span IDs, wrapper usage, invalid/oversized usage, log-only evidence, source usage changes, reparenting, cycles and child deletion. Existing gates also cover late children, deep ancestry, source caps, retention ordering and bounded evidence.

New failure tests inject an exception after filling scratch, prove cleanup before rollback, derive again without old rows, and roll back the original scratch creation before retrying on the same connection. Preparation happens once per successful connection lifetime and main-database schema remains identical. The fixture loads frozen expected hashes at test time; shallow CI needs no historical Git objects. Read-only peer review found no blocker.

```sh
python -m unittest tests.test_derived_preparation tests.test_storage_derived tests.test_agent_usage_allocation tests.test_snapshot_unknown_usage tests.test_opencode_turn_proof -q
```

The scratch relation retains only one trace's compact statistics at a time, not raw captured payloads. SQLite may retain temporary pages up to the largest trace seen by that existing connection; a full-process memory/storage bound is not inferred from clearing rows. The final combined cold/HTTP/CPU/RSS/WAL/soak gates remain open. This patch does not repair duplicate derivation, writer transaction contention or the separate sanitization hotspot; those require independently reviewed evidence.
