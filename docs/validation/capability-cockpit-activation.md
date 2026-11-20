# Explicit cockpit capability activation

Recorded 20 November 2026. This slice makes the installed Prompt signals process proof available in the normal interactive cockpit with a named, opt-in descriptor. It does not publish a general third-party capability SDK or claim a performance improvement.

## Activate

```sh
python examples/prompt_signals_process/install.py --destination /absolute/private/capability
agenturm cockpit --capability-config /absolute/private/capability/capability.json
```

The flag is unavailable with `--once` because there is no pane in a snapshot. `--new-window` forwards the resolved descriptor path. A default cockpit launch does not read capability manifests, start a watcher, or launch a child. Named descriptors are validated before a receiver or new window starts. The pinned executable is copied and launched only when an exact selected scope has a current capture grant and the pane is queried. The existing generic pane registry renders it without a `terminal_ui.py` branch. Cockpit shutdown closes the capability host, process group, and policy watcher before the feed and receiver.

## Capture decision and data path

The UI thread only checks an in-memory gate. It requires a fresh Feed snapshot with the same receiver identity, no connection error or bootstrap state, global `capture_content=True`, and an exact service, session, and canonical agent health row that also says `capture_content=True`. Unknown or metadata-only scopes fail closed. The gate also requires an exact receiver health approval no older than **250 ms**. A separate watcher checks `/api/health` about every **100 ms** while the pane is selected. If a check fails, reports revocation, or the receiver disconnects, approval is withdrawn; the next pane query clears any warm model. A denial is sticky for that selected scope until the capability is restarted or the selected scope changes.

The capability worker verifies receiver health again before and after `/api/activity`; it requires the same receiver identity on the Activity response. The HTTP request is bounded to 200 rows, exact service/session/agent, no subtree, and no reviews. Only host-derived, content-free aggregate counts cross the child pipe. A slow receiver or capability cannot block Textual input. The capability does not read a local database independently and may connect to a configured receiver only through the authenticated cockpit API.

The **250 ms approval age is a bound on cached permission, not atomic revocation**. A backend policy change can appear in an already rendered model until the next watcher result or approval expiry and UI query. The receiver has no policy-fenced aggregate endpoint or push invalidation yet. This opt-in fixture therefore retains the timing limit rather than claiming immediate revocation. Count values remain observational, not an efficacy score.

## Validation

`tests.test_capability_cockpit_activation` installs the executable, exercises CLI forwarding and cleanup, checks exact HTTP query bounds against a real receiver handler with one Pi health row and 201 newer sibling events, and shows that backend revocation and disconnect clear a warm model while the Feed snapshot still says capture is allowed. Existing `tests.test_prompt_signals_process` and `tests.test_prompt_signals_capability` cover the process protocol, content-free pipe, malformed child, and 60×18 generic pane.
