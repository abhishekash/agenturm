# Batched session projection

Status: bounded Sessions implementation and functional gates complete locally; paired latency measurement and integration review pending. This increment does not close cold-start, complete-monitor CPU/RSS or MCP targets.

## Change and fidelity

The selected session identity list is read once inside the caller's existing SQLite transaction. Heads, scalar totals and original-title lookup reuse that bounded list. Services and models are grouped in one SQL batch, capped to 129 distinct labels per owner/field before returning the existing 128-label set and completeness flags. No cross-request cache or additional production index is introduced.

One clipped label and one exact 128-character label can share a prefix. The previous `(label,complete)` DISTINCT could consume two cap positions for that prefix. The batch combines their completeness with `MIN` before ranking distinct labels. This preserves the warning while allowing up to 128 actual distinct labels; the correction has a dedicated regression.

The session-based limit, nullable owner boundaries, latest two run summaries, latest nonempty original title, scalar usage/cost totals and missing-data flags remain unchanged. Log-only collection is still handled by the existing Store path. Python receives bounded recent heads, one original title per owner and at most 129 labels per owner/field; historical prompt bodies are not decoded by this helper.

## Validation

Ten batch/history tests and 24 derived/allocation/unknown-usage/scope tests passed. The new tests compare the frozen `a601ad2` helper against the replacement on shared sessions, multiple and missing owners, partial usage, independent original titles and whole-Store log-only sessions. A deterministic concurrent writer deletes activity and changes labels/titles between statements; the read stays in its original WAL snapshot. A query-plan gate verifies the existing session index, and a transfer gate keeps two heads/one title/129 labels per owner bounds.

The verified generated retained fixture was copied read-only with SQLite backup from `/tmp/agenturm-stage-b-economics/profile.db`; the source was never opened as a Store or modified. The prepared copy has 32,079 spans, 42,765 logs and 6,046 traces across about 37.04 days. It is not the earlier 5,751-trace corpus and cannot support a direct PR #72 before/after claim. Legacy upgrade preparation is excluded from paired read timings. In one fixed prepared snapshot, old/new projections are identical: 40 owners, 79 recent heads and three original titles. Helper statements fall from 84 to five on that fixture.

## Measurement contract

`scripts/benchmark_session_projection.py` runs actual production `Store.sessions(limit=500)` using the original and replacement helper in alternating order, on the same prepared generated copy and process. It uses three warmups per helper and records all per-call samples. A separately paced synthetic writer runs during the second condition; write acceptance, errors and drain are checked. The script refuses a fixture changed since preparation. Actual production Store indexes are retained; no simplified table-only schema is used.

Final raw samples will record the agreed quiet interval, source hashes, fixture counts and p95 target outcome. These read measurements exclude receiver HTTP serialization, TUI rendering, native discovery and cold readiness. They cannot establish the complete-monitor CPU or memory budget. A missed under-write 100 ms p95 target remains explicitly open.
