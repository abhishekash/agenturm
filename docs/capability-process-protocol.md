# Process capability proof: Prompt signals

This is an independently installed, fixture-only capability process. It demonstrates the third architecture block: a declared capability can supply a validated pane through the existing generic UI registry. It is **not** a universal plugin SDK, a production credential interface, or a normal cockpit configuration option yet.

## Explicit admission and startup

```sh
python examples/prompt_signals_process/install.py --destination /absolute/private/path
```

The installer creates three new files: an executable fixture, `prompt-signals.toml`, and `capability.json`. It refuses existing entries and symlinked destination components. The descriptor names one absolute manifest, pane identity, fixed columns, and the `scoped-aggregate-read` grant. The manifest pins the executable SHA-256, provides `capability-adapter/1`, and requires host `pane-model/1`. `read_capability_config` parses the descriptor and manifest without importing or starting the executable. `CapabilityProcess.start` checks the artifact hash again, copies the verified bytes to a private temporary directory, launches with a minimal environment, and requires a framed v1 handshake.

The fixture can be wired by a trusted caller with `PromptCapabilityHost(store, read_capability_config(path), capture_allowed).start()` and passed to the generic shell as `ui_panes=(host.contribution(),)`. The caller must close the host. There is no CLI setting that enables this installed pane in a normal Agenturm cockpit launch; that composition and a production policy service are future work.

## Host-owned data boundary

The host requires an exact service, session, and canonical agent ID. It checks the current capture decision before reading Activity, before invoking the child, and before publishing a result. The UI query checks the decision again even when it has a warm cached model; revocation immediately returns unavailable and clears that cache. The policy callback should be a fast current-state decision. A process crash, malformed frame, invalid model, or timeout becomes a generic unavailable state.

The host queries one bounded Activity page, filters it again to the exact scope, and derives only typed counts and an observed time interval. It sends the child `state`, `observed_events`, `user_messages`, `shell_actions`, `first_shell_after_user_ms`, and `window_complete`. It does **not** send prompt text, tool output, paths, session or agent identifiers, or Activity payloads. A partial recent page is explicitly degraded. These are observational signals; the interval does not establish prompt effectiveness or causality.

The child returns a `PaneModel` envelope with bounded row data. The host validates its schema and, for this proof, checks every cell against the host-derived facts. A child cannot add arbitrary free text or fabricate another count. The generic pane registry renders the result without a `terminal_ui.py` change. The UI query only reads/schedules a cache; SQLite and child I/O run on a background worker. The host refreshes a warm cache on a later UI query after two seconds and quarantines a child after three consecutive failures until restart. There is no automatic UI repaint event from the worker; a normal shell refresh displays the next cached result.

The framed protocol uses a four-byte big-endian length and strict JSON, with a 64 KiB control limit. Sequence numbers match responses to requests. On POSIX, the child starts in a dedicated process session; normal close and timeout signal its process group, including ordinary helpers. On systems with `waitid(WNOWAIT)`, the host observes the leader without reaping it before the final group kill, so its group ID stays reserved. Other POSIX systems skip the graceful wait and send the final group kill before reaping. If an outside caller already reaped the leader, the host skips group signaling because that ID might have been reused; helper cleanup in that case is best effort. A child that deliberately detaches can escape the group. The installer and this process proof require POSIX; the `/proc` helper regression runs on Linux. This proof does not sandbox the child or deny operating-system network access; the fixture itself makes no network calls. Arbitrary third-party code trust, remote accounts, and production capture-policy persistence remain outside this proof.

## Validation

```sh
.venv/bin/python -m unittest tests.test_prompt_signals_process tests.test_prompt_signals_capability
```

The tests install the artifact; exercise exact scope, capture denial, warm-cache revocation, a content-free pipe, crash, timeout, helper cleanup, malformed model, and symlink refusal; and pass the installed process contribution into `ObservatoryApp` at 60×18.
