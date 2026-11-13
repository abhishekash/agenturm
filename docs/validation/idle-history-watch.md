# Idle compatibility watcher measurements

13 November 2026. A separate Stage B increment after merged [PR #72](https://github.com/abhishekash/agenturm/pull/72). This narrows one recurring idle cost; the full implementation plan remains open.

## Change and correctness

The compatibility watcher checked the source-index dirty queue every 250 ms using a new SQLite connection. Its usually empty probe now leases a read-only connection. The lease and every cursor close before writer acquisition. When work exists, a separate close-on-context writer starts `BEGIN IMMEDIATE` and re-reads the real queue through the existing bounded 200-record drain. The probe is only a hint; no dirty state or query result is cached.

A write arriving after an empty probe waits for the next tick. Another receiver can consume observed work first; the second drain safely returns zero. Foreground Activity keeps its existing coherent source read/repair path. Polling interval, capture policy, ownership and shutdown rules are unchanged.

Seven new regressions cover idle connection reuse, external writes, both queue races, lease-before-writer ordering, background repair, shutdown and replacement. The focused source/cache/lease/receiver gate passed 32 tests in 5.48 seconds; independent peer verification passed 18 in 5.12 seconds. A previously flaky saved-task test now waits for the saved row rather than the short pre-reload idle state after modal dismissal; the actual Python 3.13 journey passed in 7.25 seconds. This changes the test's readiness barrier, not task persistence.

## Controlled comparison

The same three measurement scripts run sequentially in clean frozen checkouts with fresh generated 35-day corpora: 5,751 traces, 31,000 spans and 41,000 logs plus four native fixture sessions. No provider calls, personal transcript roots, tests or profiling run during the valid monitor intervals. Each uses a 15-second warmup and 60-second idle window with no driver API or keyboard activity. Native snapshots, namespace, terminal heartbeat, normal exits and derived-work drain all pass. Reads use 20 samples per operation, idle and under paced synthetic writes.

Baseline: `8ff10863f28869d11d4a6f37fb823fb5652cfe52` (merged PR #72). Candidate: `6376339b7db616290aa779b4a0be84ab43552392`. Harness SHA-256: `342131fb4ac802466a377d6f64b67cb6127e27d718eb6202db3b1e7f6f34971b`; all three script files match between the two checkouts. Both runs are healthy and publishable under this protocol.

| Measurement | Before | After |
| --- | --- | --- |
| Whole monitor idle CPU, one core | 17.80% | 14.90% |
| Receiver idle CPU, one core | 9.38% | 6.53% |
| Cockpit idle CPU, one core | 8.42% | 8.37% |
| Cold readiness | 107.50 s | 107.31 s |
| Sessions under writes p95 | 278.15 ms | 279.24 ms |
| Mcp under writes p95 | 146.96 ms | 149.09 ms |

The whole-monitor reduction is about 16% within this pair, mainly in the receiver. The nearly unchanged cockpit, cold readiness and session/MCP reads are consistent with the narrow change. The under-5% idle CPU, under-30-second readiness and under-100-ms session/MCP p95 targets remain open. This is not a whole-monitor completion claim.

These sequential 60-second samples and empirical p95 values are host-specific. The merged PR #72 baseline measured 15.90% in its earlier run and 17.80% here; do not splice different runs into a stronger comparison. Read-probe CPU is Store-only and is separate from the monitor CPU. Write batches vary with read duration. Sampled RSS and a single interval cannot establish long-run allocation behavior. Private health snapshots and databases are excluded from published artifacts.

Raw generated data: [before monitor](idle-watch-monitor-before.json), [after monitor](idle-watch-monitor-after.json), [before reads](idle-watch-reads-before.json), [after reads](idle-watch-reads-after.json). The [earlier retained-history report](history-read-economics.md) preserves the original baseline, intermediate regression and rejected reader-error run.
