# Real-terminal idle diagnostic · 19 November 2026

Measured merged main `12ce67f` (through PR #138) on Linux x86_64, Python 3.12.3, Intel Core i7-1165G7. `benchmark_monitor_idle.py` launched an isolated receiver and actual Textual cockpit under tmux with a generated four-session fixture. No provider calls, keyboard input or timed API probes ran in the idle interval. Source files were clean, the receiver and cockpit exited normally, and there were no native read errors. [Compact raw results](real-terminal-idle-2026-11-19.json) retain the exact process split.

| Unprofiled 60-second idle after 30-second warmup | CPU of one core | Peak sampled RSS |
| --- | ---: | ---: |
| Cockpit | 5.72% | 63.57 MiB |
| Receiver | 2.23% | 52.89 MiB |
| Whole monitor | 7.95% | 116.47 MiB |

First receiver readiness was 1.10 seconds; the first useful agent rows appeared at 3.94 seconds. The idle result is above the reviewed plan's under-5% whole-monitor target. It is a **diagnostic**, not the registered 300-second publishable idle gate: this run lasted 60 seconds and used no retained-history fixture. Disk reserve was lowered to 1 GiB for this generated-only run because the host had about 2.1 GiB free; the full-history/4 GiB gate was not attempted.

A separate 20-second cProfile run after 10-second warmup reported 17.60% whole-monitor CPU. Profiling overhead makes that number incomparable to the unprofiled result. The captured profile spans startup and shutdown, so it cannot assign precise percentages to the timed idle window. It does show frequent cockpit style work (46,113 selector checks and 908 stylesheet applications over the profiled lifetime) and regular feed refreshes; the receiver profile included 1,137 SQLite executes and 6 local-runtime snapshots. Blocking `select`, locks and shutdown waits dominate cumulative profile time and are not CPU hotspots.

The unprofiled split makes cockpit refresh/rendering the first idle target. The earlier generated whole-app and tiny write diagnostics also point to SQL/transaction costs under writes. A Rust parser alone has no measured path to the under-5% idle target or the required 20% end-to-end slice improvement. Next, time active native decoding, SQLite insertion/derivation and cockpit refresh separately on a representative retained-history fixture with enough disk. Compare one bounded candidate to the same fixture, preserving capture and session-status semantics. See the [execution plan](../rfcs/rust-plugin-kernel-plan.md) and [whole-app baseline](rust-kernel-baseline-2026-11-19.md).
