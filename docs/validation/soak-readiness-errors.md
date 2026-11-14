# Soak readiness: stop on explicit indexing errors

The readiness harness previously retried a successful health response containing `indexing.error` until the ordinary startup deadline. It now fails immediately on that explicit error, before checking readiness or sleeping again. A dedicated `ReadinessIndexError` remains outside the transient `OSError` handler, so the original failure category is retained instead of being replaced by a later readiness timeout.

This changes the acceptance harness only. It does not repair indexing, change production readiness, increase a deadline or turn a failed run into a pass. The frozen `473d338` result remains unchanged: its 600-second deadline expired after 600.907 seconds, with zero timed samples. Its original result SHA-256 remains `b825f0121726d07adb1f1aef8c6b9167d928c574d6a3b6ed5242277db4e3c237`.

## Behavior and artifact boundary

- An explicit indexing error takes precedence even if the same response claims ready or native polling is incomplete. Empty or absent error values do not create a fatal failure. Both readiness flags must be actual booleans `True`; malformed truthy strings or numbers cannot certify ready.
- Ordinary incomplete migration and native initial polling keep the existing 25/600-second deadlines and 0.2-second cadence. Transient connection `OSError` failures retain their existing retry behavior.
- Initial, restart and final waits append at most eight phase-exit observations to `readiness_observations`. Observations retain status, elapsed time, poll/connection-error counts, boolean readiness/native-poll state, a bounded processed count and fixed stage/error categories. The exact known SQLite message is classified as `sqlite_database_locked`; other explicit indexing failures receive a generic category.
- A timeout observation describes only the last successfully received health metadata, which may precede subsequent connection failures; it is not a guaranteed current response. If no health response was received, its metadata basis is unavailable.

No raw health/session body, captured prompt/path, receiver identity, configuration, arbitrary error/stage text, connection-exception text, environment or credentials enter this observation artifact. A failure exception carries only the safe observation and category. The existing run error list retains that failure; later unmet workload checks do not erase it.

Prompt failure means immediately after receiving an explicit error response. The existing timeout of an in-flight health request is unchanged; this is not a whole-request timing guarantee.

## Focused validation

| Interpreter | Scope | Result |
| --- | --- | --- |
| Python 3.12.3 | Eleven new readiness contracts plus three existing soak fixtures, ResourceWarning treated as error | 14 passed in 0.796 s |
| Python 3.13.5 | Same focused gate, ResourceWarning treated as error | 14 passed in 0.866 s |

Tests cover a first fatal poll without sleep, contradictory ready-plus-error, incomplete native polling, normal not-ready to ready, empty errors, transient connection failures, unchanged timeout, malformed truthy flags, last-successful metadata, bounded observations and artifact privacy. A real temporary loopback HTTP server returns HTTP 200 with an indexing error; the existing Api client makes exactly one health request and the wait raises without another poll. It is not a production receiver or a resource experiment.

A controlled helper clock reproduces a configured 600-second wait failing on the first explicit SQLite error response with one poll and zero sleeps. That zero elapsed test clock is a functional assertion, not a cold-start measurement. The [minimal machine-readable report](soak-readiness-errors.json) retains this observation, interpreter gates and the unchanged original result hash.

```sh
python -W error::ResourceWarning -m unittest tests.test_soak_readiness_errors tests.test_soak_monitor -q
```

The full retained-history readiness, active HTTP availability/cadence, CPU/RSS/WAL, terminal and one-hour acceptance gates remain open. No provider, native agent or personal data was used, and no frozen source database, configuration, log or result was modified.
