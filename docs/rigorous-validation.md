# Rigorous validation after the Rust revert

12 November 2026. This checks the Python Agenturm application, including Agents, inspector prompts, keyboard navigation, receiver recovery and durable workflows. Rust remains reverted.

## Verdict

**Functional checks pass after the fixes; memory stability remains an open finding.** The lifecycle-fix source, `5c7e9629176a944cbc6b726d32d041bc73635642`, passed all 12 jobs in [CI run 43202168712](https://github.com/abhishekash/agenturm/actions/runs/43202168712). Testing found and fixed terminal restoration and process exit-status bugs. Installed-package and actual terminal checks passed. The soak result and resource qualification are recorded below; a functional pass is not a claim of memory stability.

## Failures reproduced and fixed

| Trigger | Before | After |
| --- | --- | --- |
| External SIGTERM while running | Terminal flags left in raw mode | Exact original termios restored; exit 143 |
| External SIGTERM during startup | Terminal flags left in raw mode | Exact original termios restored; exit 143 |
| External SIGINT | Process reported success, exit 0 | Normal cleanup and exit 130 |
| UI runtime exception | Process reported success, exit 0 | Normal cleanup and exit 1 |

A real pseudoterminal reproduced all four failures before the patch. External signals now request Textual's normal shutdown through handlers installed before the terminal driver starts. The CLI propagates its return code. Previous signal handlers are restored, including across repeated runs. Normal quit returns 0. Existing Ctrl+C selection-copy behavior is retained.

The regression tests verify the full original termios settings, alternate-screen exit, cursor restoration, worker cleanup, signal handlers and return codes. Startup termination uses a two-way pipe handshake so the signal lands during mount without a guessed delay. An independent peer reran the lifecycle checks and approved the implementation.

One initial CI attempt correctly caught an outdated mock in the offline-startup test: it mocked synchronous `run()` after the implementation moved to `run_async()`. The fixture now uses an awaited `AsyncMock` and asserts the successful return status. That 609-test source matrix is green; the later focus regression increases discovery to 610 tests.

## Reflow focus regression

Independent tracing of a real feed and installed SSH UI showed that resize could queue reader focus before restoring the prompt chooser about 0.23 seconds later. When focus is lost during reflow, the fallback now uses the current section's normal entry control: the request navigator for Prompts and the reader for Overview/Details. It does not replace an already focused, visible reader.

The new regression clears lost focus, queues three reflows before deferred focus is applied, and requires the compact prompt chooser. It then enters the reader and resizes, requiring reader focus to survive. This failed on the prior implementation (`agent-detail` instead of `agent-prompt-select`) and passes with the change. The peer approved this minimal fix. The earlier persistent SSH focus observation was not deterministically reproduced, so the transient trace is not presented as a complete explanation of that observation.

The final application and test source is `47e2e6476f9f66a61c46b61e1137e82c5f31e252`; its full Python/SDK results are recorded in [CI run 43205162279](https://github.com/abhishekash/agenturm/actions/runs/43205162279). Discovery now contains **610 tests**, with 14 optional SDK cases skipped in base environments. The matrix covers Python 3.11–3.14 and the eight JavaScript/SDK jobs listed below. The linked run retains each job's outcome and exact test duration. The additional regression and final-source installed/UI checks are separate from the earlier 609-test matrix.

## Regression and installed-package evidence

| Layer | Actual result | Scope |
| --- | --- | --- |
| Lifecycle CI Python 3.11 | 609 tests, 14 optional skips; 600.127 seconds | Full discovery |
| Lifecycle CI Python 3.12 | 609 tests, 14 optional skips; 655.346 seconds | Full discovery |
| Lifecycle CI Python 3.13 | 609 tests, 14 optional skips; 537.861 seconds | Full discovery |
| Lifecycle CI Python 3.14 | 609 tests, 14 optional skips; 603.440 seconds | Full discovery |
| Other eight CI jobs | All passed | Node 22/24; OTLP; CLI automatic capture; OpenAI Agents SDK; Pydantic AI; LangChain; Google ADK |
| Local existing suite | 603 tests, 14 optional skips; 974.586 seconds | Baseline before the six added regressions; final CI is the exact-source result |
| Local lifecycle + reconnect | 22 tests passed; 23.295 seconds | Final runner and offline-startup fixtures |
| Final installed wheel | 20 tests passed; 112.699 seconds | 13 shutdown, four prompt and three keyboard journeys; Python 3.13.5 |
| Final dependency-floor check | 20 tests passed; 98.856 seconds | Installed wheel with Textual 8.2.0 on Python 3.12.3 |
| Final UI integration | 20 tests passed; 170.941 seconds | Prompt, keyboard and inspector layout suites |
| Wheel/source identity | All 53 packaged Python module bytes matched | Fresh isolated environment, imported outside checkout; Rust absent |

The optional SDK cases are skipped in base Python environments. Separate CI jobs install and exercise their integrations; skipped cases are not counted as executed passes.

## Keyboard and terminal checks

The installed wheel passed the reusable actual-key tmux journey twice: **20 checks with normal color and 20 with `NO_COLOR=1`**, at 120×32, 80×24 and 60×18. Synthetic prompts include Unicode and long text. The journey delivers physical terminal key sequences and verifies the visible result, rather than supplying focus manually.

It covers entering inspector tabs; direct Up from Prompts; older/newer selection; Home/End; Enter/Right to read; Left to return; Ctrl+Left/Right between sections; compact selector arrows and unchanged confirmation; F1 help; selection/focus through resize; and normal quit with exact terminal restoration. These checks complement headless application tests. The [keyboard guide](keyboard-navigation.html) documents the complete shortcut map and focus rules.

SSH evidence: the installed wheel passed **15 checks over actual loopback OpenSSH transport**, twice consecutively with the final probe. The disposable Debian container used Python 3.12.14 and OpenSSH 10.0p2. Arrows, Home/End, Enter, Ctrl+Left/Right, compact prompt selection, F1 and resize propagation worked at 120×32, 80×24 and 60×18. The remote shell verified exact termios restoration, and SSH returned 0. Temporary keys and known-hosts stayed inside the private fixture; the container and tmux server were removed.

Early SSH probe attempts sampled transient focus and read tmux pane-death before process exit status was available. The final probe requires stable visible state and waits for the actual exit status. Independent live-receiver tracing also completed 16 resize transitions. The reflow fallback is covered in its own regression above. The rebuilt final wheel also passed the 15-stage SSH journey with exit 0. These results establish local SSH transport behavior, not WAN latency, disconnect recovery or every terminal emulator.

## Receiver, CLI and saved work

A separate installed-package check used a real loopback receiver, HTTP feed and headless UI. It stopped the receiver, observed the connection error, exercised F1 and Activity navigation while offline, then replaced the receiver on the same port with a new database and UUID while reusing native agent/session IDs. It waited for the new non-empty roster and prompt history, and verified old prompt history was excluded. Recovery passed.

Installed CLI checks also passed: non-TTY use returns an actionable error with exit 2; help returns 0 on stdout; authenticated one-shot snapshots work; missing authentication returns 2; metadata-only capture and `NO_COLOR` output contracts hold. Saved-task CLI checks cover create, brief updates, immutable decisions and supersession, stale revision exit 3, Markdown/HTML handoff, no-overwrite exit 2, SQLite backup and private file permissions.

All checks used temporary synthetic data. No personal session records, provider calls, real notification delivery or user tmux sessions were used.

## Live soak and resources

Soak result: **1201.10 measured seconds, 400 cycles, two observed outages and two recovered receiver replacements**, with process exit 0. The separate durable fixtures produced 400 incidents, acknowledgements, exclusive claims and resolutions. All SQLite integrity checks passed. This long run used application source `5c7e962` imported before the final reflow fix, and the earlier helper before positive-payload assertions were strengthened.

| Observed metric | Corrected 20-minute run |
| --- | --- |
| Concurrent API reads | At most four, asserted |
| Review cache | At most four entries; largest sampled payload 46,160 bytes |
| Individual review history | Asserted ≤1,000 events and ≤2 MiB |
| Sampled open file descriptors | 9–9 |
| Sampled thread count | Up to 16 |
| RSS | 69.73 MiB at first sample → 124.99 MiB at last; peak sampled 125.58 MiB |

**Memory stability is not established.** RSS continued to rise despite bounded review caches and stable sampled descriptors. This is a mixed headless workload with a growing durable ledger, not a fixed-size production terminal session. The cause has not been isolated; it needs retained-allocation profiling and a comparable production-terminal run. The explicit SQLite probe closure did not eliminate the rise. Functional success must not hide this resource finding.

The final hardened helper ran separately on final application source `47e2e64`: **62.11 seconds, 16 cycles, both outages/replacements recovered, exit 0**. It additionally requires non-empty namespace-confirmed exact review evidence, a non-empty fresh roster using the API's `latest_task` field, exact reopened brief/link content, and stopped receiver/feed workers before publishing success. Those stronger assertions and the reflow fix are covered by this shorter run, not retroactively by the 20-minute run. CLI checks rejected seven invalid-duration/path/overwrite cases. Independent peer review approved the helper.

Numeric evidence: [20-minute samples](validation/2026-11-12-soak-20m.json), [final-source smoke](validation/2026-11-12-soak-final-source.json), [SSH result](validation/2026-11-12-ssh.json). The long-run helper SHA-256 was `907dc95448e1526f03550dcf773ab9c92607e35076ce15ca28839ab5fc9fb1e2`; the committed helper includes the later assertion and cleanup hardening.

The repeatable helper is `scripts/soak_workflows.py`. It runs an actual local receiver and HTTP ingestion with six synthetic workers, three parent/child pairs and bounded span IDs. The monitor runs headlessly, resizes through 120×32, 80×24, 60×18, 45×14 and 160×48, and exercises inspector/help navigation. Two deliberate receiver outages must be observed before same-port replacements recover. Scoped review reads share the feed's concurrency budget; obsolete fixture records are pruned, and every SQLite database receives an integrity check.

Because the headless controller intentionally disables persistence, attention and task durability are exercised separately through their real modules. Each cycle creates a waiting request, verifies exclusive delivery claims across store instances, simulates failed delivery and retry, acknowledges it, reopens the store, resolves it with fresh Working evidence, and checks sensitive task text is absent from persisted receipts. Task brief/link updates are reopened as well. Synthetic delivery does not exercise the desktop notification backend.

An earlier 20-minute functional run completed 407 cycles but its raw SQLite measurement connections were only used as context managers, which do not close SQLite connections. The helper now explicitly closes those probes and measures file descriptors. That earlier RSS series is not attributed to a product leak. Resource claims below use the corrected repeat. Separate traced-allocation probes are diagnostic only, because tracing changes the process footprint.

Repeat from a development checkout:

```sh
uv run python scripts/verify_keyboard_tmux.py
NO_COLOR=1 uv run python scripts/verify_keyboard_tmux.py
uv run python scripts/soak_workflows.py --duration 1200 --output /tmp/agenturm-soak.json
```

The soak requires a new result path, finite duration of at least 30 seconds and an existing output directory. Success is now written after cleanup completes. It writes periodic JSON samples. Check both the final JSON and process exit status; a partial progress file does not establish a completed run.

## What remains worth doing well

1. Investigate the observed RSS rise. Profile retained allocations over a longer fixed-load session, distinguish bounded renderer caches from retained application objects, and set an evidence-based steady-state memory budget. This run does not establish multi-hour memory stability.
2. Repeat the same keyboard journey in named outer terminal emulators and over a genuinely remote SSH link with latency and disconnects. Loopback transport cannot establish WAN behavior.
3. Observe AI engineers doing the existing tasks without coaching: find the active agent, understand its subagents, read all requests, identify recorded progress, and resume saved work. Automated assertions establish behavior, not comprehension.
4. Keep testing boundary cases of existing workflows: very long request histories, large families, partial capture, stale evidence, and recovery while reading. Preserve explicit coverage limits and stable focus as the interface evolves.

Independent peer review approved the lifecycle change, minimal focus fix and final hardened helper, and reviewed the report for provenance and false-positive claims. The final documentation was rendered at 1280-pixel desktop/light and 390-pixel mobile/dark widths with no horizontal page overflow.

No participant study, every-emulator compatibility, actual desktop notification delivery, WAN resilience or multi-hour stability is claimed. The report's HTML is self-contained and supports light/dark mode.
