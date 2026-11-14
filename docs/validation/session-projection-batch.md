# Batched Sessions reads on retained history

Status: this bounded implementation improves the paired full `Store.sessions` read on the generated corpus below. The under-write 100 ms p95 target remains open. Integration CI, whole-monitor CPU/RSS, cold readiness and MCP targets are not closed by these measurements.

## Change and fidelity

The selected session identity list is read once inside the caller's existing SQLite transaction. Recent heads, scalar totals and original-title lookup reuse that bounded list. Services and models are grouped in one SQL batch, capped to 129 distinct labels per owner/field before returning the existing 128-label set and completeness flags. Repeated identical per-owner label arrays are expanded once. No cross-request cache or additional production index is introduced.

One clipped label and one exact 128-character label can share a prefix. The previous `(label,complete)` DISTINCT could consume two cap positions for that prefix. The batch combines their completeness with `MIN` before ranking distinct labels. This preserves the warning while allowing up to 128 actual distinct labels; the correction has a dedicated regression.

The full Sessions method also batches uncovered log selection. It materializes compatible covered trace/owner cutoffs once, then selects the newest 50 uncovered candidates **per requested session** before hydration. It preserves the original timestamp/digest order, requested-session order, nullable-owner behavior, non-Activity log fallback and raw log session identity. The cutoff is applied after coverage filtering, so an older uncovered error cannot disappear behind hundreds of newer covered records. All reads share the existing `BEGIN` snapshot.

Unqualified, nonfailure log records cannot establish an operation input fingerprint or terminal failure. They still contribute their existing session metadata and lifecycle signals, but skip the repeated reminder projection. All typed operation envelopes, qualified semantic attributes, failures and compact summary recovery references retain their conservative correlation path. Output-only JSON cannot supply a passing command or resolve an earlier failure.

The session-based limit, latest two run summaries per owner, latest nonempty original title, scalar usage/cost totals and missing-data flags remain unchanged. Python receives bounded recent heads, one original title per owner and at most 129 labels per owner/field. The bounded log window remains 50 candidates per selected log session. Historical prompt bodies are not decoded by the scalar session helper.

## Functional verification

The 74-test backend/native/operation/batch gate passed in 19.829 seconds. After replacing the duplicated failure-label check with the shared reminder predicate, the final 21-test batch/reminder gate passed in 7.240 seconds. Earlier 40 batch/history/reminder/correlation/allocation/unknown-usage tests passed in 12.176 seconds. These are overlapping focused gates, not a full-suite total. Integration CI on the combined branch is still required.

Tests compare the frozen `a601ad2` helper and full Sessions method against the replacements on shared sessions, multiple and missing owners, partial usage, independent original titles, log-only sessions, tied timestamps and mixed raw event kinds. Coverage includes 250 newer covered logs before older uncovered failures, foreign/unknown owners, exact typed failure→success recovery, unqualified JSON output, and stale Activity session indexes. Deterministic WAL writers change raw logs, labels, titles and indexes between selection and hydration; the reader retains its original snapshot. Query-plan and transfer gates verify the existing session index, constant five helper statements, two heads/one title/129 labels per owner bounds.

Reproduce the main gates from this branch:

```sh
python -m unittest tests.test_session_projection_batch tests.test_history_read_economics tests.test_error_reminders tests.test_operation_correlation tests.test_agent_usage_allocation tests.test_snapshot_unknown_usage
python -m unittest tests.test_storage_derived tests.test_summary_scope tests.test_native_storage_retry tests.test_operation_facts tests.test_operation_hydration tests.test_operation_correlation tests.test_session_projection_batch
```

## Controlled paired measurement

Runtime source was frozen at `85ef5b4`; the benchmark name-collision correction is script-only commit `44c5155`. The first invocation refused before warmup/timing because an argparse variable shadowed the read function. It produced no samples or writes. The corrected run below is the accepted comparison. Raw data retains all samples and file hashes: [final paired reads](session-projection-paired.json).

The source is a verified synthetic retained-history database, copied with SQLite's read-only backup API. The original database was never opened as a Store or modified. The measured copy contains 32,170 spans, 42,765 logs and 6,047 traces over about 37.76 days. This includes 91 synthetic spans and one trace from the earlier local comparison. It is not the earlier 5,751-trace corpus and cannot support a direct PR #72 comparison.

The fixture initially carried experimental indexes from earlier diagnostics. On our private copy only, the script compared every named index with a fresh frozen production Store. It removed nonproduction `live_summary_owner_v1`, `trace_activity_cover_v1` and `activity_log_preview_v3`; it recreated `spans_source_text_v1` with byte-identical production DDL (the expression itself was unchanged). The final copy matches the complete production named-index manifest. See [normalization provenance](session-projection-index-normalization.json); the final raw artifact embeds that exact manifest. The production schema and original source are unchanged.

The accepted interval was **2026-11-14 10:00:47.820211–10:01:09.905624 UTC**, 22.085 seconds. Root and all implementation agents held local heavy tests, browsers and Git activity. Existing user/background Agenturm processes were preserved on their separate databases; background host noise was not eliminated. This is one short paired experiment, not a distribution across machines or repeated runs.

The benchmark alternates 20 original/current reads per condition in one process and the same prepared file, following three warmups each. Baseline uses the frozen original **whole method and helper**; candidate uses the full replacement method and helper. Fixed-snapshot helper and whole-method outputs both matched before timing. The second condition ingests a synthetic span about every 100 ms: 91 batches were accepted, 89 during the read phase, with zero errors and a clean writer drain. p95 uses the nearest-rank 19th ordered observation of 20; all raw observations and maxima are retained.

| Full `Store.sessions(limit=500)` | Original median | Candidate median | Original p95 | Candidate p95 |
| --- | ---: | ---: | ---: | ---: |
| Idle | 305.11 ms | 201.23 ms | 312.93 ms | 220.50 ms |
| Under paced writes | 357.21 ms | 225.19 ms | 441.02 ms | 286.34 ms |

Within this pair, idle p95 is about 29.5% lower and under-write p95 about 35.1% lower. The under-write maximum is 459.34→376.23 ms. The absolute under-write p95 is still **286.34 ms**, above the 100 ms target. Receiver HTTP serialization, UI rendering, native discovery, cold migration/readiness, monitor CPU and monitor RSS are excluded. No claim about those budgets follows from this endpoint measurement.

## Diagnostic trail and limitations

The preserved [earlier helper-only comparison](session-projection-helper-only-extra-indexes.json) used the same original Sessions method on both sides, changing just the helper. It also retained the experimental indexes described above. It showed no reliable under-write p95 improvement (344.03→357.12 ms), so it was not accepted as the final performance result. Its raw artifact remains unchanged; it should not be compared directly with the normalized final run.

An instrumented SQL/cProfile diagnostic then identified the repeated per-session candidate query and unqualified reminder hashing as major costs. In three calls on the normalized generated copy, candidate statements fell from 120 to three, while helper queries fell from 86 to five per call. The diagnostic includes profiler and wrapper overhead and concurrent development load; its wall times are not performance evidence. `scripts/profile_session_projection.py` reproduces that diagnostic on an explicitly synthetic prepared copy.

The accepted benchmark checks fixture bytes, inventory, readiness, production index manifest, nonempty results, fixed-snapshot equivalence, accepted write batches and writer shutdown, and records source hashes. An independent post-run check matched every recorded hash to the frozen worktree. Preparation and any legacy derived-index migration happen outside timing. The database copy is disposable and is mutated by the synthetic writer, so rerunning requires another newly prepared copy. The remaining resource gates require the combined application and real receiver/UI workload; this increment does not mark all of Stage B complete.

Root independently reviewed the bounded candidate SQL, ownership cutoff, typed reminder path and disclosed measurement limits before integration. The report and completion ledger HTML companions were checked in Chrome at 1280/light and 390/dark with no page overflow or external assets. Integration CI is pending.
