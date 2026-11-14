# Terminal-only operation result projection

14 November 2026. This is a bounded performance prototype with exact evidence parity. It removes unnecessary full fact construction from a result-only consumer. It does **not** close the registered HTTP availability, 100 ms writer schedule, retained-history latency or complete-monitor resource targets.

## Contract

`normalize_operation_result` needs the recorded terminal observation, not command arguments, sanitized output, correlation fingerprints or an error signature. A private pure helper now shares the existing exit parsing, eight-entry/origin validation, strict error flag, reported-result precedence, conflict rules and result basis with the full operation-facts projection.

Shell exit code 1 remains a full projection. Any parsed code 1 among the bounded shell exit entries, including conflicting entries, takes the existing command/redaction/completeness path. A display subject cannot become proof of an informational grep/diff outcome. Other terminal states do not acquire new command or input claims. Unclassified detail keeps its original behavior.

The generic no-detail branch of operation supersession also consumes only these terminal results. Its nonempty producer/actor/session identity guard, operation-domain rule and unknown-result comparison remain unchanged. Qualified supersession still constructs and compares complete sanitized facts. A speculative negative input-eligibility shortcut was removed because the available workload did not establish that it was hot. There is no supersession performance claim, persistent cache, schema change or new public fact field.

## Frozen equivalence

The test corpus freezes the actual `e795f064c861cd59b54d1180697e4b19c9959b34` operation-facts module, SHA-256 `07a1991b6e77a9834eaa9a1297e5ad9b5b2b681981195e786eafb60904b1ca09`.

The committed fixture contains independently generated hashes of complete full-facts values and normalized rows for 5,897 deterministic cases, plus 451 supersession comparisons. These cover reported-label presence versus absence, every recognized terminal label, missing result/status fallback, strict booleans, null/malformed exits, integer limits and oversized/nonfinite values, conflicting exits, the eight-entry cap, malformed origins, shell code-one commands and controls, credential-bearing commands, Unicode/control/escaped keys and typed versus unclassified detail. Intentionally malformed calls compare their exception type as well as ordinary complete return values; this is parity, not a claim that arbitrary malformed values are valid inputs.

Actual production Store Activity replay and derived tuples were also checked on a generated fixture: 96 traces, 576 spans and 672 logs. Each mode processed 1,248 source jobs and produced 1,156 Activity rows. All complete Activity and derived-record tuple hashes matched before and after. Baseline operation-facts modules were selected before importing Store, Activity or derivation consumers in separate child processes; other application modules were identical. This isolates a module change, not two complete released applications.

## Diagnostic results and limits

The original generated fixture contains no linked log span IDs and mainly unqualified shell labels. Two short unprofiled ABBA phase diagnostics showed no reliable gain: Activity replay stayed around 1.5–1.7 seconds, and derivation around 1.8–2.1 seconds. Those null results are retained. They do not justify a general retained-history improvement claim.

An explicitly typed-enriched private variant added known synthetic command/exit/input/output attributes to shell records. All tuple hashes still matched. Replay was 3.81–4.25 seconds before and 2.93–3.05 seconds after; derivation was 5.02–5.05 versus 3.61–3.68 seconds. This exploratory variant demonstrates the targeted code path, not the original history's distribution. It is retained separately and is not a resource or latency acceptance result.

A stronger target diagnostic used the registered writer's actual two-span typed MCP payload, 60 batches at the same nominal 100 ms spacing, plus Sessions/MCP reads. The receiver and clients shared a process; each module mode ran in a separate process and private tiny database. Per-thread call counters were added, with no profiler. Every write attempt completed, including catchup after delays. This is **not** the registered fixed-deadline protocol. No late or failed response was filtered out.

| Target attempt | Qualified normalizations | Full facts called inside normalization | Writes starting over 100 ms late | GET 503 responses / 12 |
| --- | ---: | ---: | ---: | ---: |
| Before 1 | 861 | 861 | 9 | 3 |
| After 1 | 837 | 0 | 1 | 6 |
| After 2 | 820 | 0 | 6 | 5 |
| Before 2 | 861 | 861 | 13 | 8 |

All attempts accepted 120 span updates and ended with one trace, 61 unique spans and one agent node; no derived-worker error was recorded. Background refresh counts vary with scheduling, so total call counts are not a fixed-work CPU comparison. The substantive observation is one full projection per qualified normalization before, and none after for this non-shell-code-one payload.

All target attempts still violate the formal schedule and/or availability conditions. These were development diagnostics without a coordinated quiet window; some target sampling overlapped the exploratory variant. Timing differences are not acceptance evidence. The target ran an earlier prototype containing the subsequently removed eligibility precheck, but counted zero supersession calls, so that branch was not exercised. Final-source parity is checked separately after its removal. No new benchmark rerun was selected to replace these observations.

## Reproduction and validation

The golden corpus is in `tests/test_terminal_result_projection.py` and `tests/fixtures/operation_result_e795.json`; it runs without Git history or external data at test time. The fixture's source commit and module hash identify the immutable expectation generator.

The raw original phase, generic phase, typed exploratory and HTTP target records are retained in the adjacent `terminal-result-*.json` artifacts. `terminal-result-diagnostic-manifest.json` records source hashes, limitations and the actual diagnostic driver source. Those drivers reference the generated local fixture and explicit source tree used here; they are provenance artifacts, not a new supported user command.

The focused operation/fact/hydration/correlation/Review/reminder gate is rerun on the final source. Independent review found no blocker in the shared terminal projection and generic result-only supersession. Final combined-source CI and the unchanged full validation protocol remain required before any SLO can be marked achieved.

Final local gate: 64 tests passed in 31.881 seconds (`test_terminal_result_projection`, `test_operation_facts`, `test_operation_hydration`, `test_operation_correlation`, `test_workflow_evidence`, and `test_error_reminders`). This includes the final source after removing the unmeasured eligibility shortcut. The phase driver retained in the manifest includes later optional typed-enrichment/role-count reporting; the earlier null runs preceded those reporting options and used the same replay/derive loops without enrichment.
