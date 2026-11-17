# Responsive cockpit startup and collection feedback

The interactive cockpit now mounts before checking or creating its receiver.
`CockpitStartup` runs on the existing feed worker; it owns any receiver it starts
and closes a receiver that finishes initialization after cancellation. A slow or
unauthorized receiver is not treated as a missing receiver. Explicit endpoints
do not start local receivers. Metadata-only attachment still checks capture
policy before any data reads. The independent final health check used for native
inventory and capture-policy proof is unchanged. `--once` keeps its existing
readiness and complete-snapshot rules.

The Agents view separates connection freshness from history completeness.
Available agents remain visible when historical reads fail; the list and footer
label the unavailable slice. Read failures, ages, source limits and unconfirmed
parent/child relationships are available in Diagnostics, along with Refresh now.
Authentication failures and outages remain explicit. No observations or
unavailable history without any agent rows cannot become confirmed zero working
agents or zero attention items. Reconnect feedback uses the fixed status areas
after the first snapshot, preserving workspace geometry.

## Regression checks

```sh
.venv/bin/python -m unittest tests.test_cockpit_startup \
  tests.test_status_confidence_ui \
  tests.test_reconnect_readability.ReadableJourneys.test_background_refresh_and_reconnect_keep_workspace_still -v
```

This focused run passed 12 tests on Python 3.13.5 / Textual 8.2.8 on Linux.
Generated records only; no personal database or source logs are used.

- A baseline test fails because receiver probe and full health run before the
  app opens; the new path makes no receiver request until the app mounts.
- Request-count checks reject a redundant startup full-health read.
- Controlled barriers hold a probe and a receiver constructor while the actual
  headless app handles keys and resize. Agent rows appear after release.
- Cancellation rejects late publication and closes a late receiver once.
- A real 60×18 PTY renders while a generated database constructor is held, then
  accepts `q` and restores terminal attributes, alternate screen and cursor.
- Status journeys cover 120×36, 80×24 and 60×18; incomplete history remains
  explicit without a global warning, Diagnostics exposes the failure and refresh,
  and unavailable counts remain unknown. Reconnect geometry is also asserted.

These are behavioral and work-budget checks, **not startup-latency benchmarks**.
They do not prove a cold-start percentile, fast indexing of arbitrary retained
history, or elimination of every receiver timeout. The installed 0.2.3 instance
that motivated the report was not upgraded or restarted by these changes.
