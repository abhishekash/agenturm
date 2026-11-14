# Shared claim contention: test contract correction

[PR87 Python 3.14](https://github.com/abhishekash/agenturm/actions/runs/43592510668/job/132440055787) failed one of 1,086 cases: simultaneous claim attempts encountered wrapped `SQLITE_BUSY` at `BEGIN IMMEDIATE`. Main subsequently passed all twelve CI jobs; both commits have identical tree `2c4e5c83ec0adcb5d5ddcaad0bef61d6be5ef0a1`. Both outcomes remain evidence.

A controlled temporary-DB reproduction paused a real holder after its real write transaction began. The contender failed closed after 501.6 ms, the holder claimed once, a later normal claim returned false, and exactly one durable claim existed. This proves the bounded-contention mechanism; the CI log cannot identify its particular scheduling or lock-holder delay.

The test now distinguishes only wrapped SQLite `SQLITE_BUSY`/`SQLITE_LOCKED` from false. After concurrent attempts finish, deferred contenders settle through ordinary non-retry claims. Exactly one winner and one durable claim remain mandatory; other errors still fail. Explicit failed-delivery retry assertions remain intact.

A deterministic event-coordinated test verifies zero optimistic claims before the holder commits, one afterward and a later false contender. Separate checks reject message-only errors, unwrapped SQLite errors, other codes and extended codes. No runtime timeout, retry, storage policy or notification behavior changes.

Local gate: **12 WorkflowStore tests passed in 0.675 seconds** on the final patch. These are fixture-only results, not resource or live-desktop measurements. No personal state was opened.
