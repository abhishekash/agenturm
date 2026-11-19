# Rust kernel performance baseline · 19 November 2026

Source: merged main `c195966` (through PR #137), Python 3.12.3, Linux x86_64, Intel Core i7-1165G7 (4 cores/8 threads), 15 GiB RAM. The filesystem had about 2.1 GiB free. All inputs were generated in temporary directories. [Raw measurements](rust-kernel-baseline-2026-11-19.json) and [benchmark definitions](../performance.md) make the fixture and limitations inspectable.

## Whole app diagnostic

`python scripts/benchmark_app.py --idle-seconds 60` ran each scenario in a fresh process with a real SQLite Store, loopback receiver, feed and headless Textual UI. All seven built-in views were warmed. It excludes local harness discovery, real terminal rendering and plugin workers. The first refresh and 11 warm refreshes are separate; 12 samples are not the plan's 200-attempt/120-second latency gate.

| Generated case | Ingest + derived | First refresh | Warm refresh median | 60s idle CPU | Peak RSS | DB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 200 spans | 0.364 s | 172.947 ms | 27.290 ms | 6.10% | 92.72 MiB | 0.856 MiB |
| 20,000 spans | 14.807 s | 163.262 ms | 29.286 ms | 6.12% | 96.85 MiB | 16.975 MiB |
| 2,000 spans, 8 KiB payload each | 3.385 s | 168.402 ms | 33.632 ms | 6.26% | 103.66 MiB | 18.792 MiB |

The idle values exceed the proposed under-5% target. First refresh exceeds 100 ms in all three cases, but the endpoint and sample protocol differ from the plan's under-write input p95 gate. The warm figures benefit from existing response caching and must not be treated as sustained-write latency. This is one diagnostic run; no performance improvement has been demonstrated.

## Attribution and next target

`profile_ingest_writes.py --batches 60 --readers` used a separate generated tiny fixture and instrumented SQLite calls. It completed 60 writes and 12 read attempts in 6.42 seconds, with no missed write deadlines or derived error. Timed write p50/p95/max was 16.98/45.69/51.96 ms. Across all writes, connection wall time totalled 1,128 ms, the span `INSERT`/trigger path 643 ms, derived drain 187 ms, and commit 110 ms. These intervals overlap and must not be summed. Instrumentation changes timing, and this fixture has only 61 spans, so it identifies a plausible SQL/transaction cost, not a production bottleneck percentage. It gives no evidence that a standalone Rust parser will improve whole-monitor latency by the required 20%.

`benchmark_discovery.py` scanned 10,000 synthetic files in 100 directories and selected 50 recent sources at 58.891 ms median CPU per scan. `benchmark_source_preparation.py` used 50 synthetic sources and 200 activity rows. Across three local samples with ten generations and three consumers, prepared canonical-title median was 117.006 ms versus 161.134 ms without preparation; prompt-fallback median was 139.894 ms versus 218.151 ms. Metadata-only and oversized cases do not retain preparation and did not improve. These are component timings and do not rank whole-app costs.

**Next candidate:** profile active native ingestion under a representative retained-history fixture, separating source decode, SQL insert/trigger work, derived projection, feed reads and UI refresh. Optimize the measured dominant path first. A Rust process slice must preserve exact evidence semantics, include process/RSS overhead, and pass a same-fixture end-to-end comparison before default activation. The storage-authority cutover still requires separate Rust expert review and parity/rollback gates in the [execution plan](../rfcs/rust-plugin-kernel-plan.md).
