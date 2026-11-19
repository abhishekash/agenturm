# Fake outbound connector checkpoint

Recorded 20 November 2026, rebased onto `be0b560`. The change adds a separate `action-adapter/1` process proof, `OutboundQueue` with host-owned SQLite policy and delivery rows, a pinned standalone loopback executable, installer and fixture-only HTTP sink tests. No source inbox, session projection, findings dispatch or UI file is modified.

The proof explicitly grants `fake-sink` and `write_fixture_metric` before enqueue. The outbox stores a stable idempotency key, claims before sending, and exposes queued, retryable, in-flight, uncertain, delivered, blocked and dead-letter states. Revocation blocks queued/retryable work. A timed-out in-flight request is not retried until status lookup resolves it; failed lookup remains uncertain. Crash recovery is explicit and requires the previous dispatcher to have stopped. The local fake sink provides idempotent POST and definitive key lookup; this does not establish those semantics for real destinations.

Validation:

```sh
python3 -m unittest tests.test_fake_outbound_connector
python3 -m py_compile observatory/outbound_delivery.py examples/fake_outbound_connector/agenturm_fake_outbound.py examples/fake_outbound_connector/install.py
git diff --check
```

Fifteen focused outbound tests and 32 tests with neighboring manifest/framing/process suites passed after rebase: explicit grant and no send on enqueue, restart persistence and idempotency, concurrent claim exclusion, revocation, bounded retry/dead letter, accepted-then-timeout lookup, missing-status retry with the same key, failed lookup remaining uncertain, recovery after a stranded claim, descriptor/destination/artifact scope, and accepted delivery after revocation. Installer regressions reject linked destination directories, linked ancestors, linked entries and dangling links without writing to their targets. A live-dispatch regression shows recovery refuses a concurrent claim. The fixture uses only `127.0.0.1`; no real message or account is contacted.

This is an architecture contract proof, not a production outbound connector. The action schema is deliberately fixed to a bounded integer, grants are not an OS sandbox, and no automatic source-to-output pipeline exists. The dispatch/recovery fence uses a local POSIX file lock, not a distributed lease. General endpoint policy, credentials, shared process admission, user-facing delivery controls and destination-specific uncertain-outcome semantics remain open.
