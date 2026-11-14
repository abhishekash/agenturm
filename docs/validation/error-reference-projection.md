# Reuse qualified error-reference metadata

14 November 2026 · B readiness diagnostic and narrow implementation. This is a diagnostic CPU-work investigation, not an accepted cold-start, latency, RSS or whole-history performance comparison.

## What the bounded profile measured

The generated retained fixture was opened read-only and backed up to private diagnostic databases. `Store` was never instantiated on the source. The source held 32,079 spans, 42,765 logs and 6,046 traces. Three diagnostic-only indexes were removed from the copy; its resulting 51 named indexes matched a fresh production Store at baseline `449f64f`.

A full constructor profile was stopped because replaying all 74,844 source records under cProfile was too large for this bounded investigation. Its incomplete run is not used for timing claims. A separate private backup used a test-only constructor monkeypatch to defer full migration, then explicitly profiled 500 log and 500 span Activity jobs and 16 distinct traces selected from latest/oldest/largest-trace lists. The trace sample contained 78 spans and 115 logs. Its queues were deliberately sampled, so it cannot establish whole-history readiness or startup cost.

| Diagnostic phase / function | Calls | cProfile cumulative seconds |
| --- | ---: | ---: |
| Activity sample | 1,000 source jobs | 2.612 total |
| SQLite `execute` during Activity replay | 4,931 | 2.028 |
| `_activity_for` during Activity replay | 1,000 | 0.301 |
| `normalize_operation_result` during Activity replay | 1,338 | 0.0019 |
| Derived trace sample | 16 traces | 1.227 total |
| `operation_facts` during derived refresh | 544 | 0.859 |
| `safe_value` during derived refresh | 1,712 | 0.758 |
| `trace_error_references` | 16 | 0.544 |
| Its `correlation_facts` calls | 332 | 0.534 |
| Credential scanning | 195 top-level calls | 0.070 |
| `_review_owner` | 16 | 0.010 |

Cumulative times overlap and must not be added. cProfile overhead, concurrent work, fixture selection and partial migration make these diagnostic observations only. The useful result is repeated semantic work: error-reference generation traversed the same 166 canonical events twice through full operation qualification. Double tool-result normalization was cheap in this particular Activity sample, and owner proof/credential scanning were not the first target.

## The narrow change

`trace_error_references()` now qualifies each original source once through the existing `error_reference()` / `operation_metadata()` protocol. Both existing scans then consume those compact, server-generated references. The pending-failure and successful-step algorithms, sorting, limits, fingerprints and strict-later rule remain unchanged. There is no global cache, retained fact/output cache or omitted redaction/owner check.

The temporary references contain identity, terminal flags and sanitized fingerprints; command bodies, arguments, output text and raw attributes are absent. A temporary Boolean-derived status label preserves legacy references whose failure was reported only through `status`; that label does not enter returned references. The original Activity is not mutated.

## Verification before any controlled comparison

Fourteen fixture groups freeze the exact serialized reference output of baseline `449f64f`: typed retries, equal timestamps, different operations, foreign producer/actor/session/turn, existing caller namespace binding, unknown/redacted/clipped inputs, printed JSON roles, compact inputs, unsupported versions/conflicts, missing/duplicate sources, informational nonzero search results and separate 20-failure/20-success limits. Outputs must remain byte-for-byte identical. The unknown compact status case caught and prevented an initial behavior regression.

Across these fixtures, full raw-fact qualification calls fell from **237 to 134**: once for each raw source event, with existing compact references never replayed as raw evidence. This is a call-count result, not a CPU or cold-start improvement claim. Returned reference schemas retain no command, output, argument or body content.

The focused gate passed **46 tests in 16.860 seconds**, including the frozen outputs/call-count proof and existing reminder, typed provenance and owned-Findings regressions. Runtime edits are confined to this one function; core, storage, Findings and credential scanners are unchanged. The coordinator peer-reviewed the narrow patch and frozen outputs before integration. The self-contained report passed Chrome checks at 1280 px/light and 390 px/dark with no page overflow or external assets; mobile rendering was visually inspected. Any controlled resource comparison remains a separate coordinated step.

The source fixture, personal telemetry, preferences and review notes remain untouched. Diagnostic scripts and profiles are under `/tmp`; they are not release benchmarks.
