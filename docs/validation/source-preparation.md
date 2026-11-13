# Bounded session preparation

13 November 2026. Stage B increment after [PR #73](https://github.com/abhishekash/agenturm/pull/73). This measures source projection work only. Complete-monitor CPU, cold readiness and read-latency targets remain open.

## Change and boundaries

The Agents roster, attention controller and Workflow repeatedly prepare the same session identities, readable first-request titles and exactly attributed evidence. They now share static preparation within one snapshot object. Health, process/collector freshness, attention and sorting are recalculated from the current clock. Review acknowledgements remain outside this cache.

An app retains at most two preparations, allowing separate live and paused objects. A detached input comparison catches nested in-place changes, receiver/scope changes, source removal and ownership changes. Weak snapshot references avoid retaining unrelated trace/MCP/metric responses. Admission and copying share one traversal, limited to 500 source rows, 200 activity records, 512,000 scalar characters, 20,000 nodes and depth 24. Oversized/custom inputs bypass retention. Returned cached rows are independently copied so consumer decorations cannot mutate another projection.

Capture policy overrides retained content. Fresh metadata-only health can coexist with an old Activity buffer after a failed read. Titles, selected evidence, prompt history and generated summaries must not republish that content. The same-receiver Review policy matches exact producer/actor/session, including the known Codex internal-review alias, when stored and native session display IDs differ. Paused Agents refresh their policy even while their evidence snapshot is frozen. Late prompt/evidence responses cannot repopulate discarded histories. An in-flight AI summary rechecks receiver and capture policy before provider dispatch and before storing its result. These guards cannot retract a request already dispatched while capture was permitted.

Any current capture denial clears and bypasses preparation retention. An unrelated modal uses the uncached controller path because its single read cannot amortize admission. The roster and Workflow use the shared path. No global prompt cache, provider call or private framework patch is introduced.

## Projection microbenchmark

Run `.venv/bin/python scripts/benchmark_source_preparation.py`, adding `--consumers 2` or `--consumers 1` for the other cases. Each case has 50 synthetic sources and 200 activity records; seven samples each prepare 20 new snapshot generations. The consumer count is reads per generation. Both paths are warmed, order alternates, admission/copy costs are included, and output equivalence is checked. The canonical-title case has a recorded original title; the fallback case derives it from an attributed prompt. No receiver, SQLite, native discovery, terminal renderer, provider or personal state participates. No tests/profiling ran during these final measurements.

Frozen source: `bc235988f4fc85e4f4630e9073fdf1619829a1c4`, Python 3.12.3, tracked source clean. Script SHA-256: `55a7d084de6718ac07ac7986007a110070b2922c16d056be1b5c136c5f42df36`. The later modal-routing change does not alter the benchmarked projection functions. All times below are median elapsed milliseconds for 20 generations, not per-read latency or CPU percentages.

| Reads per generation | Canonical title: uncached → prepared | Prompt fallback: uncached → prepared |
| --- | --- | --- |
| 1 | 101.759 → 171.019 ms | 139.525 → 204.384 ms |
| 2 | 200.129 → 195.471 ms | 295.603 → 254.044 ms |
| 3 | 304.974 → 233.285 ms | 450.474 → 277.614 ms |

Three consumers improved by about 24% / 38% in these cases. Two canonical-title reads were nearly flat; a single read regressed because admission has a cost. These results justify selective reuse, not a general performance claim. Actual benefit depends on reuse and payload shape; metadata-only and oversized feeds bypass retention. A first diagnostic with separate budget/copy passes regressed the three-read canonical case by about 3%; it prompted the single-pass admission change. A second diagnostic had uncertain overlap with peer tests and is excluded from the final measurements.

Raw final results: [three consumers](source-preparation-three.json), [two consumers](source-preparation-two.json), [one consumer](source-preparation-one.json). A new complete-monitor comparison is still needed before making further idle CPU claims. Prior valid complete-monitor measurements remain in [the watcher report](idle-history-watch.md).

## Correctness and remaining work

Focused gates cover source mutation/ownership/namespace, clock-driven health, cache bounds and weak references, consumer isolation, exact capture-policy aliases, paused UI, discarded late responses, and mocked AI dispatch/publication. Independent review additionally compared 24 native, announced, stored and SDK projections against the previous implementation across health-clock boundaries. Wide/short roster navigation and theme/resize regressions remain part of acceptance.

The existing Overview still needs the shared, source-backed operation/failure facts in Stage C and frozen Git correlation in Stage D. Stage B's under-30-second cold readiness, under-5% complete idle CPU and under-100-ms session/MCP write-load p95 targets are not met by this microbenchmark. Later stages and external adoption/live-provider outcomes remain open in the [full implementation plan](../rfcs/review-notes-completion-plan.md).
