# Resource budgets and reproducible benchmarks

Agenturm monitors other processes, so bounded background work matters as much
as the speed of a single inspector. Use synthetic workloads for comparisons;
do not copy personal agent databases into benchmark fixtures.

## Whole-app baseline

```bash
uv run python scripts/benchmark_app.py
# Longer idle window, less affected by UI startup:
uv run python scripts/benchmark_app.py --idle-seconds 30
```

The benchmark runs each workload in a fresh process and temporary directory:

- Small: 10 traces with 20 spans each.
- Large: 2 traces with 10,000 spans each.
- Payloads: 2 traces with 1,000 spans each, with 8 KiB synthetic payloads per span.
- Actual SQLite storage, loopback receiver, HTTP feed, and headless Textual UI;
  all seven views are visited before measuring idle CPU and RSS.
- No local session discovery, personal preference reads/writes, paid models,
  or external network calls.

It reports ingestion plus derived-processing time, feed refresh latency, idle
process CPU, current Linux RSS, peak observed RSS where supported, and database
size. CPU includes the UI, receiver, and feed. Twelve refresh samples provide a
small empirical distribution, not a statistically reliable production p95;
compare on the same machine with the same Python version. Headless UI memory
and CPU do not establish rendering performance for every terminal emulator.
RSS includes Python, Textual, SQLite, and temporary allocations: it is not just
the size of persisted telemetry.
The first refresh and warm median are reported separately: an unchanged database
can reuse serialized API responses, whereas the first request must compute them.
Peak observed RSS is the larger of the OS high-water counter and the sampled
current RSS; those OS accounting mechanisms can differ slightly.

## Agent hierarchy benchmark

`tests/test_agent_hierarchy.py` builds a synthetic 1,010-node forest (ten roots,
ten children per root, nine grandchildren per child) and exercises the same
adjacency/projection code as the cockpit. On the development Linux host used for
this change (Python 3.13, warm process), three local runs measured approximately:

- graph construction and cycle validation: 6.1–6.6 ms;
- flattening the fully expanded forest: 5.1–5.7 ms;
- first-pass subtree summaries for all roots: 1.2 ms; repeated cached lookups are faster.

These are descriptive measurements, not a promised latency target. The fixture
is separate from the production 50-source discovery limit; it verifies bounded
projection cost without increasing real discovery windows.

## Native-reader microbenchmark

```bash
uv run python scripts/benchmark_resources.py
```

This isolates OpenCode reader polling for idle, unrelated database writes, and
one changed tool. It does **not** measure derived storage processing or UI work.
Avoid treating this microbenchmark as whole-app performance.

## Native discovery benchmark

```bash
uv run python scripts/benchmark_discovery.py
```

This scans 10,000 synthetic JSONL files in 100 directories, including 100 recent
sources, and selects the newest 50. It never opens personal session directories
or ingests the generated files. Discovery uses one streaming `scandir` traversal
and constructs paths only for recent candidates. It still checks old files for
new writes, so resuming an old session remains discoverable within the normal
ten-second discovery interval. Directory symlinks are not followed.

## November 2026 optimization measurements

On the same Linux / Python 3.12.3 host, with ten-second idle windows and all views
warmed, the baseline (with the navigation feedback bug fixed) and the optimized
response cache, summary rendering, and on-demand trace details measured:

| Workload | Refresh median before → after | Idle CPU before → after | RSS before → after |
| --- | --- | --- | --- |
| Small | 23.6 → 10.2 ms | 7.20 → 5.78% | 73.6 → 68.7 MiB |
| 20,000 spans | 39.0 → 9.4 ms | 8.00 → 5.73% | 69.6 → 68.7 MiB |
| 8 KiB payloads | 39.9 → 12.8 ms | 8.38 → 5.59% | 76.8 → 70.3 MiB |

The synthetic discovery benchmark measured 115.1 → 52.3 ms CPU per scan.
These are individual local runs, not portable guarantees. Cold refresh remained
roughly 220–325 ms; ingestion was essentially unchanged and RSS modestly lower. Continuous
writes invalidate the response cache, so these idle gains do not establish an
equivalent improvement under sustained ingestion. The whole-app benchmark still
excludes native discovery and real terminal rendering.

## Deterministic budgets

Regression tests enforce work limits rather than machine-dependent timings:

- Feed reads have at most four workers and sixteen prefetch slots per refresh.
  Workers do not publish snapshots. A changed filter or selection invalidates
  the pending snapshot.
- Each receiver response is limited to 16 MiB before JSON decoding. Decoded
  Python objects can require more memory than their serialized representation.
- Live feed polling waits two seconds between refreshes. Inventory and findings
  reads are cached for five seconds unless their scope changes.
- Watched JSONL reads are limited to 8 MiB per poll and 2 MiB per record.
  Oversized records are dropped through their terminating newline.
- OpenCode payloads are limited to 2 MiB per record and 16 MiB per poll before
  JSON is transferred into Python; deferred parts resume on later polls.
- Native lifecycle caches are limited to 64 KiB per entry and 1 MiB per map.
- Serialized API response caching retains at most 32 entries / 4 MiB of payload,
  with a 1 MiB per-response limit. Authentication runs before cache lookup. A
  dedicated read-only SQLite connection checks `PRAGMA data_version` before
  reuse and before publishing a computed result; commits from ingestion,
  derived processing, retention, and external writers invalidate the cache.
  Database replacement also invalidates it. Health, connections, hierarchy, and
  rolling usage windows bypass caching. The connection closes with the receiver.
- Summary cards skip unchanged text repaints. Programmatic view-menu updates
  cannot enqueue additional navigation commands.
- The cockpit fetches trace-detail payloads only while Traces is visible. Opening
  Traces requests a fresh snapshot; closing it invalidates any in-flight detail
  response. Trace summaries and session monitoring continue on every live poll.
- AI summary evidence retains at most thirty matching events.

These are component budgets, not a fixed whole-process RSS ceiling. Before
changing defaults, compare all workloads, inspect database growth, and check
that the optimization has not changed counts, correlation, or error semantics.
