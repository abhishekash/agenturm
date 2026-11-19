# Idle cockpit refresh checkpoint · 19 November 2026

The Textual cockpit now checks for a changed feed snapshot before doing idle chrome and table work. It keeps the half-second timer so a new snapshot is displayed promptly. Unchanged live snapshots can skip a refresh for up to two seconds; reconnecting, a requested retry, selected text and direct user actions continue through the regular refresh path. This addresses the repeated style and render work identified in the earlier [real-terminal diagnostic](real-terminal-idle-2026-11-19.md).

An isolated receiver and actual Textual cockpit ran under tmux on Linux x86_64, Python 3.12.3, Intel Core i7-1165G7, using the same generated four-session fixture. No provider calls, keyboard input or timed API work ran during each idle interval. Both processes exited normally, the visible fixture stayed available and no native read errors were reported. [Compact measurements](idle-refresh-2026-11-19.json) retain the source revision, duration, process split and errors.

| Unprofiled run after warmup | Cockpit CPU | Receiver CPU | Whole CPU | Whole peak RSS |
| --- | ---: | ---: | ---: | ---: |
| Clean parent `0bd31e9`, 60 s after 30 s | 5.92% | 3.28% | 9.20% | 134.20 MiB |
| Timer-only diagnostic, 60 s after 30 s | 5.28% | 3.05% | 8.33% | 134.50 MiB |
| Snapshot-coalesced candidate, 60 s after 30 s | 4.23% | 3.15% | 7.38% | 115.09 MiB |
| Clean committed candidate `bffdb45`, 300 s after 60 s | 4.02% | 3.08% | 7.10% | 114.80 MiB |

The paired 60-second diagnostic decreased whole-monitor CPU by 1.82 percentage points, or about 20%, on this generated fixture. The 300-second clean-source run confirms the direction but is not a paired 300-second comparison. Its harness marks the clean duration as publishable; this report does **not** treat it as the reviewed retained-history acceptance gate. The host had under 2 GiB free, so the run lowered the reserve to 1 GiB and did not clone the retained-history seed. Whole-monitor idle CPU remains above the under-5% target. RSS differences are observations, not a memory improvement claim.

A separate cProfile diagnostic included startup and shutdown and cannot assign precise CPU percentages to the timed interval. It showed repeated Textual selector and stylesheet work. The unprofiled split still attributes roughly 3% of one core to the receiver. Follow-up performance work should separately measure receiver polling and retained-history behavior, then repeat the registered full gate when the required disk reserve is available.

Validation: 47 existing cockpit, search, projection and first-frame tests plus two focused refresh behavior tests passed. `compileall` and `git diff --check` passed. The new tests cover unchanged snapshots, new snapshots, the two-second age bound, reconnecting, explicit retries and selected text.
