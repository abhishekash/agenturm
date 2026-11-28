# Navigation performance: indexed retained history

## Diagnosis

On 28 November 2026, the running native 0.3.15 process showed about 170% CPU
(one logical core is 100%). The host had approximately 9 GiB available memory
and no swap use. A read-only investigation found about 1.7 million observations
in a 1.3 GiB history database. Its recent-history query scanned observations
and used a temporary sort; selecting 256 records took 4.342 seconds in one
diagnostic sample. No personal observation content was exported, and the live
database was not modified by this investigation.

The cockpit's navigation loader, workflow notification checks, and background
collector all use retained-history reads. A small result limit did not bound
the underlying SQLite work.

## Change

- Schema 28 adds ordered time and source/time indexes for recent pages.
- Exact session filters and older-page cursor boundaries are direct predicates,
  rather than nullable-parameter OR expressions that hide index seeks.
- Exact source/session reads use the existing ordered session index. Without
  planner statistics, SQLite otherwise sometimes prefers the source/time index
  and walks other sessions before finding an older selected session.
- A partial source/session/kind/time index selects lifecycle and relationship
  facts without scanning every tool or message when a relationship is absent.
- The normal writer migrates recognized stores transactionally under the
  existing writer lease. Read-only inspection never migrates an archive.

Observation identities, source authorization, snapshot watermarks, correction
revisions, cursor order, retention, and content grants retain their existing
semantics. Tests cover older read-only stores and grant-preserving migration.

## Reproduce

```sh
cargo test -p agenturm-store --locked history_read::tests
cargo test -p agenturm-store --release --locked \
  history_read::tests::benchmark_history_pages -- --ignored --nocapture
```

The benchmark creates and removes its own temporary SQLite database with one
million synthetic metadata records. `AGENTURM_BENCH_ROWS` changes the row count.
Bulk seeding is not an admission/ingestion benchmark. Each invocation is a fresh
process; within it, the same store is measured without and then with the new
indexes. It warms each query, measures 12 reads of 256 and 16,384 records, and
compares complete returned pages for equality. It also reports index-build
time and database growth. These are warm query distributions, not production
percentiles or cold-cache measurements.

## Local measurement

Linux 7.0.0 x86-64, Intel i7-1165G7 (8 logical CPUs), 15.7 GiB RAM,
Rust 1.100.0 release build, bundled SQLite. Twelve warm samples per case on
one million synthetic metadata records:

| Read | Without new indexes, median (min–max) | Indexed, median (min–max) |
| --- | --- | --- |
| 256 observations | 6,606.544 ms (3,352.880–9,896.085) | 2.012 ms (1.914–3.265) |
| 16,384 observations | 5,904.269 ms (3,522.262–10,368.278) | 71.861 ms (69.016–129.634) |

For the 256-record page, SQLite work fell from 999,999 full-scan steps,
one sort, and 52,002,382 VM steps to 256 scan steps, no sort, and 12,404
VM steps. Index construction and checkpoint took 3,192.5 ms. Database size
grew from 208.61 to 295.44 MiB (86.83 MiB of extra indexes).

Other desktop processes and concurrent workspace/release compilation were
active during this run; the especially wide baseline range is not a controlled
isolated-machine latency result. The deterministic work reduction and exact
page equality are stronger evidence than a speedup ratio. The timing command's
peak RSS includes compilation and must not be reported as Agenturm app memory.

Local validation: the complete workspace test suite passed, including 183 UI
tests and 158 Store tests (one opt-in benchmark excluded). Workspace/all-target
Clippy passed with warnings denied. Some process-adapter integration tests
require explicit binary paths; CI additionally exercises those configured paths.

## Regression budgets

A 50,000-record fixture exercises recent, source-only, deep older-page, exact
session, and absent-session reads through the actual Store API. Each 64-record
page permits no SQLite sort, at most 65 full-scan steps, and fewer than 50,000
VM steps. An absent relationship lookup must use the sparse fact index, perform
no full scan or temporary sort, and stay below 100 VM steps. These are work
budgets, not machine-dependent latency assertions.

## Limits and upgrade

This fixes a measured database bottleneck; it does not establish whole-app
idle CPU, ingestion throughput, peak RSS, real-terminal rendering cost, or
keypress-to-visible-frame p95. Session-catalog aggregation still scans retained
session evidence, and sparse projections may examine many nonmatching rows.
The additional indexes consume disk space and add write work. The first writer
open must build them and can take longer on a large store. Stop older Agenturm
versions before upgrading a shared store: schema 28 is not readable by 0.3.15.
The currently published 0.3.15 installer does not contain this source fix.

The patch changes no capture or sharing boundary. App README installation and
privacy claims require no change for this unreleased storage fix. A separate
website checkout still advertises Python/PyPI installation in
`src/pages/index.astro`; that pre-existing drift is outside this patch. The
GitHub profile and visual assets were not reviewed.

[Self-contained HTML version](navigation-performance.html).
