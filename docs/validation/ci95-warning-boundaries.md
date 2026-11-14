# CI95 warning and status boundaries

## Observed CI failures

PR95 Python 3.14 failed because a SQLite ResourceWarning entered the CLI test’s redirected stderr. The command returned zero. Main Python 3.13 separately failed because the policy timer replaced the word “revoked” with a source-withholding notice. Main Python 3.14 also failed the CLI boundary. These failed runs remain failures.

## Warning ownership

The allocation that produced the earlier CI warning is unknown: its log has no allocation traceback. A separate Python 3.13 tracemalloc check on unchanged e795 proved four unclosed connections in the three Soak tests: three test fixture connections and the seed_history aging connection. The patch closes those four connections, preserving write commits. The same check now records zero ResourceWarnings. This does not attribute the earlier CLI failure to the alphabetically later Soak module.

## Assertions retained

Before CLI capture, explicit collection lets earlier fixtures’ warnings remain visible outside command output. Warnings emitted during the actual command and its post-call collection are recorded and must be absent. Exit zero, empty stderr, private bound-source omission, authored-note retention, and fresh headless-source success remain required. No global warning suppression is added. A controlled real SQLite leak from an earlier invocation remains visible; an injected leak inside this invocation makes the test fail.

## UI and validation

The queued decision test still requires no saved decision after revocation, usable editor fields, and successful saving of independently authored replacement text. It accepts either a revocation or withholding notice because the timer may update that notice. The seven affected checks passed on Python 3.12.3 in 5.009 seconds and Python 3.13.5 in 5.019 seconds. These are local functional checks; corrected integrated CI and all performance/resource gates remain separate.

[PR95 failed run](https://github.com/abhishekash/agenturm/actions/runs/43602237953) · [main e795 failed run](https://github.com/abhishekash/agenturm/actions/runs/43602301807)

[Standalone HTML](ci95-warning-boundaries.html) · [Allocation and boundary evidence](ci95-warning-boundaries.json)
