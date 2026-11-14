# Final combined-build validation protocol

Prepared 14 November 2026. **Harness preparation only: no soak, profiling, timed benchmark or new tmux session has been run for this protocol.** The coordinator approved the script-only boundary. Nine contract tests and tiny fixture-only smokes have passed; the final candidate must be frozen after PRs 80–84 before controlled execution.

## Scope and frozen inputs

The primary comparison should use baseline **B = `5e0c397`**, the main commit before PRs 80–84, and **C = the verified combined commit after those PRs**. The coordinator must confirm both exact hashes before registration. This measures the net combined change, rather than attributing every difference to one optimization. Previously reported 14.90% CPU and 112.96-second readiness values used different trees/conditions; they are context, not the paired baseline for this protocol.

Use one clean, committed harness revision with an explicit application-root argument so its child imports B or C without importing the other tree. Record application SHA and tracked-tree state, harness SHA, Python and dependency versions, lockfile hash, OS/kernel/CPU, terminal and tmux versions, dimensions, fixture manifest and timestamps. Each child must assert that `observatory.__file__` resolves under the intended application root; frozen harness helpers must also log their actual file paths/hashes. Do not let `from scripts...` accidentally select a different helper through application-root import precedence. Check the same manifest again after each run. An uncommitted or changing code tree makes a resource run non-publishable. A personal untracked review-note document is not a generated input and must remain untouched.

Build one common **raw** generated history seed, checkpoint/close it, record its file and logical-record hashes, and clone that seed into a new private directory for every run. Use the existing 5,751 traces / 31,000 spans / 41,000 logs / 36 historical sessions and 35-day window ending two days before the fixed anchor. Do not build each seed through the different running application versions: that would introduce version-specific derived indexes and different wall-clock anchors before timing. Preserve the existing parent/child/stale Codex-shaped files and OpenCode-shaped SQLite fixture, with the same event manifest and synthetic project paths on both sides.

“Cold” means a fresh process opening a fresh raw seed requiring its normal derived preparation. It does not mean the Linux page cache was flushed. Do not change host kernel/cache settings. Warm restart is a separate metric using that version's fully drained, closed database.

## Reuse and required harness adaptations

| Harness | Reuse | Required change before the final run |
| --- | --- | --- |
| `benchmark_monitor_idle.py` | Primary real receiver + real tmux cockpit CPU/RSS budget; pre/post health checks; graceful shutdown. | Common seed/application-root inputs; record useful first UI frame separately from receiver readiness; expand health gates and storage sampling. |
| `soak_monitor.py` | Generated native ingestion, parent/child/stale evidence, OpenCode text streaming, real keyboard acknowledgements, receiver/cockpit restarts, process sampling. | Remove additional source-projection work from heartbeat; add native inventory/pending-storage diagnostics and deterministic workload manifest; make cadence fixed before claiming paired active-load economics. |
| `verify_keyboard_tmux.py` | Real prompt navigation, compact resize, help and termios restoration. | Minimal environment and fixture home; direct multi-argument child execution; use the production terminal wrapper; extend to Review, Investigate, Usage and signal exits. |
| `soak_workflows.py` | Receiver replacement, outages, public Pilot actions, cache and integrity assertions. | Replace direct notification claims with `AttentionRunner` eligibility/delivery fixtures; add exact native resolution; exercise actual modal lifecycle rather than only inspector/help plus a separate pure Review cache. |
| `benchmark_resources.py` | Optional OpenCode-reader diagnostic only. | Count emitted logs as well as spans. It cannot demonstrate complete-monitor CPU/RSS, UI behavior, receiver read latency or actual database/WAL writes. |

The previous idle heartbeat called `app.workflow_items()` once per second. That added a consumer of the source preparation under measurement, and can disproportionately help the new cache. The committed harness now observes already published `source_map`, current display metadata and the published Workflow ledger instead. Keep heartbeat work identical across B/C; do not patch Textual's screen stack, timers, message pump or private rendering methods. Existing application-level observation hooks may be retained if documented and tested against both versions.

The active native soak currently schedules each next phase from the previous phase's completion. UI/API delays therefore change cycle counts and source volume. Keep that mode as functional/descriptive evidence. For a paired active-load claim, drive the same fixed number of manifest events on an absolute schedule independent of UI work, record lateness, and reject missed deadlines rather than silently skipping phases or bursting to catch up.

The keyboard script currently inherits the full environment and supplies one shell-command string to tmux. Adopt the native soak's minimal environment, child-only `Path.home()` fixture isolation and direct multi-argument execution. Do not repurpose `HOME` or `CODEX_HOME`. Set XDG config/state/data and OpenCode/Gemini fixture locations explicitly; no provider keys, personal preferences, native transcript roots or user's tmux configuration may enter the child. Use a private tmux socket/channel and only terminate owned children. Keep outputs private; publish a sanitized report rather than the artifact directory containing generated receiver tokens and raw health captures.

## Execution order and claims

### 1. Preflight, outside resource timing

Run the necessary harness contract tests and one bounded smoke cycle against B and C. Confirm API authentication, exact receiver identity, expected nonempty source/history counts, supported features, typed readiness, source schema versions, zero unexpected source/write errors and clean shutdown. Verify that the common raw seed and expected normalized facts agree. New features unsupported by B are explicitly unsupported, never recorded as successful empty results.

The preflight also verifies the revised heartbeat does not invoke `sources()`/`workflow_items()`, and that final native-inventory namespace checks perform a fresh full-health request rather than reusing a prefetched response or the identity-free liveness probe.

### 2. Primary logical-cold and untouched idle budget

Use two balanced pairs in registered order **B → C, then C → B**, with a fresh seed clone and child processes per run. For each run:

1. Start timing immediately before the receiver process starts; exclude fixture construction/copy time, but report it separately.
2. Record receiver index/native-discovery readiness and first useful Agents frame as separate times. A useful frame must contain the expected sources and retained history with fresh, correctly scoped data; an empty shell is not readiness.
3. Allow 60 seconds of explicit warmup after useful readiness, then measure 300 seconds at 120×36 with Agents visible, unpaused, no query changes, no keyboard actions, no writer and no coordinator API probes.
4. Sample owned process counters once per second through `/proc`. Compute CPU from cumulative CPU deltas divided by actual sampled wall time. Sum receiver and cockpit for default complete-monitor CPU; report driver separately. Compute summed receiver/cockpit RSS at each same-time sample, then its median/max. Individual process high-water marks remain separately labelled; adding separate peaks is only an upper bound.
5. After timing, verify full health, expected data, source/index state and the terminal frame. Quit normally, wait for receipt/derived/index drains, then stop the receiver. No forced termination counts as a passing shutdown.

Record all four runs, their ordering, per-role and whole-monitor results, readiness values, load observations and validity failures. Do not select the best run. If unplanned host activity or a harness failure invalidates a run, retain the result and reason; any replacement run must be labelled and balanced, not silently substituted. Profiling is a separate diagnostic run and cannot supply budget numbers.

### 3. Active stability and storage growth

Run the one-hour generated native soak on C using the reviewed manifest, with parent/child plus OpenCode outcomes, stale-session negative control and planned same-store receiver/cockpit restarts. Existing UI/API probes make this an active workload, not idle. If fixed absolute scheduling has not been implemented, its CPU/RSS numbers remain descriptive and must include completed-cycle/event counts.

Record database, WAL and shared-memory sizes before startup, after readiness, at fixed active-phase boundaries, before shutdown and after all connections close. Add public index status/queue coverage, source/derived row counts, SQLite page/freelist counts and, where supported, `dbstat` index bytes **outside untouched idle and latency windows**. Report source growth, index amplification, peak database-plus-WAL, post-drain size and WAL plateau. Do not force checkpoints or VACUUM during timing to conceal growth. Growing databases are expected when the workload adds records; distinguish growth per accepted event from unbounded growth with fixed input. Run integrity checks only after the timed phase.

CPU/RSS segments must retain PID plus process start identity, so a restart or reused PID cannot create a negative CPU delta or pretend memory dropped within one process. Report RSS warm and last-minute medians and their observed interval. There is no specified universal RSS target; do not invent one. Unexplained persistent growth, violated cache bounds, leaked threads/fds or incomplete drains blocks a stability pass.

### 4. Native attention concurrency and offline catch-up

This is a separate C-only functional workload, not the default-monitor CPU budget. Use generated identities, private Workflow state, two cockpits and a standalone headless watcher sharing durable receipts. Use a fake notification sink; never send real desktop notifications or approve native requests.

Verify quiet initial inventory, one eligible delivery attempt across all observers, acknowledgement persistence without approval, exact matching positive resolution, unavailable storage without optimistic acknowledgement, and no repeat after delivery/restart. Move both request and resolution behind at least 210 newer unrelated Activity records and restart the watcher without a local hook binding. The bounded receiver native inventory must recover them. Test incomplete inventory, namespace replacement, sender/order conflicts and the 1,000-state limit; none may finalize a baseline or deliver an alert. Report the watcher PID/cost separately if a resource sample is collected.

### 5. Modal retained-memory and real keyboard lifecycle

Use fixed data with no ongoing writer, one long-lived cockpit and the same settled Agents frame between cycles. Warm 10 cycles, then perform 100 measured open/read/close cycles, sampling cockpit RSS, fd/thread counts and public application cache sizes after each close/settle interval. Use several exact source scopes to exercise Review eviction, not one repeatedly cached record. Do not force GC in the primary series; an explicit-GC diagnostic, if needed, is separately labelled.

The common B/C modal subset is inspector Overview/Prompts/Details, Review and Usage; include task/draft/handoff only where both frozen versions support the same path. Investigate and other new C-only paths get a separate series, not a misleading before/after comparison. Review remains within four cached selections / 8 MiB overall and 1,000 events / 2 MiB per history. Investigate remains within 250 rows, five pages and 2 MiB, without unbounded modal stacking. Native overlay snapshots remain bounded to two metadata wrappers. Cancellation and policy invalidation must clear derived content rather than revive it after restoration.

Extend the real tmux keyboard check at **120×36, 80×24, 60×18**, plus an explicit too-small frame. Cover prompt Up/Down/Home/End and reader scrolling; section changes; exact Review paging/refresh; Investigate Enter/search/result/source/back/cancel; Usage return; Pause/follow; opening and closing overlays without corrupting selection. Use public keys and observable frames, not private framework state mutation. A captured scope/selected-reference journal can corroborate the visible frame.

Verify normal quit and external SIGINT/SIGTERM through the production terminal wrapper, including a signal during startup and a modal/read cancellation. Check return status, terminal attributes before/after, a usable terminal marker, receipt drains and absence of surviving owned workers. Respect modal Ctrl+C semantics: cancellation/back is distinct from quitting the main cockpit. Separate Escape keystrokes from following bytes to avoid accidental Alt chords.

## Gates and reporting

A publishable resource result requires a fixed clean source manifest, the intended elapsed window, expected nonempty fresh data, continuous heartbeat coverage, correctly bound receiver/source ownership, ready indexes and complete native inventory where supported. Reject unexpected API/auth/read/schema/write errors, pending-storage failures, source/derived backlog that does not drain, skipped workload phases, forced shutdown, lost accepted receipts or missing process samples. Intentional outages in a fault scenario are separately marked and excluded from an “error-free resource budget” claim.

Keep targets explicit: **cold readiness <30 seconds; untouched default complete-monitor idle CPU <5% of one core; Sessions and MCP under-write p95 <100 ms** on the registered corpus/reference host. Receiver readiness and first useful UI readiness are both reported. Latency validation reuses the coordinator's frozen paired Sessions/MCP protocol with equal writer cadence/accepted batches and no overlapping soak/profiling/modal workload. Record sample counts and the exact quantile method; existing scripts differ, so standardize on empirical nearest rank before the run. A helper-only or reader-only result is not an API or complete-monitor result.

The final Markdown/HTML report must include raw artifact hashes, manifests, all validity failures, configured versus measured time, supported/unsupported feature coverage, counts/denominators, before/after values and each target's pass/open status. Synthetic fixtures cannot prove live Claude/Pi compatibility, participant adoption, or human reconstruction/resumption time. No such conclusion follows from these runs.

## Implementation boundary and current evidence

The approved preparation creates a separate harness-only commit for common seed/application-root selection, non-perturbing heartbeat, updated failure gates, exact native durable scenario, modal lifecycle coverage and tmux isolation. Add small deterministic tests for manifests/gates/scheduling and a short fixture-only smoke check. Freeze that harness commit before registering the final B/C run. No product behavior change, unstable/private Textual patch, long run or profile is authorized by this preparation document itself; execution awaits the coordinator's frozen candidate and quiet-window announcement.


## Implemented runner and evidence status

The script-only implementation adds `validation_support.py` and `validate_combined_monitor.py`; application behavior is unchanged. A common manifest includes the exact application root/commit/dirty state, frozen helper hashes, Python/dependencies, lockfile, host and tmux version. Application children reject a foreign imported application root. Native input is generated; notification delivery is replaced with a private file sink. Provider credentials and personal root variables are absent from child environments. Child-only `Path.home` fixtures, XDG paths, OpenCode database and project-directory filtering isolate discovery. The keyboard fixture records its application test-helper hash separately; it is a functional check, not the resource baseline.

The budget heartbeat observes published data and never calls `workflow_items()` or prepares another source projection. Useful-frame readiness and timed coverage reject read/schema/index/backlog/storage failures, stale display state and receiver changes. Unsupported native capability is explicit for B. A successful idle result requires v2 heartbeat coverage, real fixture sources, a clean unchanged application/harness and graceful receipt/index drain. CPU uses PID plus kernel process-start identity. Whole-monitor peak RSS is the maximum contemporaneous receiver-plus-cockpit sum, rather than the sum of separate process maxima.

The owned-artifact registry permits cleanup of registered working database clones and sidecars only, after children stop and raw results exist. It rejects seeds, unregistered paths, traversal and symlinks; all sidecars are checked before any deletion. File hashes and deletion intent remain in the registry. No source checkout, source corpus, personal telemetry or preferences are deleted. Full runs require **4 GiB free reserve plus clone preparation allowance**; current free space is about 2.8 GiB, so the default full-run gate refuses. The tiny smoke deliberately uses a 256 MiB reserve. Arrange approved owned-clone cleanup or another disk before registration; do not silently lower the full-run reserve.

The new candidate-only `native-race` path starts two real cockpits plus one headless watcher sharing durable state, checks quiet baseline, one fixture notification, durable acknowledgement across observer restart and exact resolution after 210 newer unrelated records per transition. It uses the remote receiver inventory without a local hook binding. The actual concurrency path has **not run yet**. Existing native API/runner contract tests remain the evidence for sender conflicts, receiver replacement, overflow, incomplete inventory and storage refusal; the new end-to-end path does not claim to reproduce every fault.

The `modals` path uses five exact generated source scopes, one long-lived real cockpit, public Review/Usage keys and optional candidate Investigate search/source/back keys. It records post-close RSS, fd/thread counts and cache occupancy for 10 warmup plus 100 requested cycles, refuses cache-bound/modal-stack violations and leaves RSS plateau judgement explicit. This path has **not run yet**. Review refresh/cancellation, Pause/follow and complex draft/handoff paths still require their existing targeted UI suites and the final manual keyboard journal; the runner does not label those paths covered merely because a modal opened.

`verify_keyboard_tmux.py` preserves optional raw frames/results, uses direct argv and the production terminal wrapper, checks normal quit or external SIGINT/SIGTERM in the reader, and can send a signal from startup mount before the first settled frame. It checks terminal attributes and a post-restoration output marker. The navigation case includes 120×36, 50×14 too-small, 80×24 and 60×18 sizes. These modified real-terminal paths are prepared but **not run during this preparation**.

Nine contract tests passed in 0.302 seconds on the latest small gate. Tiny fixture smoke exercised 12 spans / 20 logs / 3 traces, raw clone equality, seed preservation and registered clone cleanup. Against clean Stage F `31daa3b`, the real AttentionRunner fixture also passed failed delivery, blocked automatic retry, explicit successful retry, acknowledgement across restart, unrelated Working not resolving, exact native resolution and metadata-only persistence. The baseline-capability smoke explicitly reports native unsupported. Smoke manifests record the development harness as dirty and cannot be used as performance evidence. Raw outputs are retained under `/tmp/agenturm-harness-smoke-f-20261114` and `/tmp/agenturm-harness-smoke-b-20261114`.

## Registered commands, after freeze and quiet-window approval

Use the same committed harness checkout and interpreter for every command. `B_ROOT`, `C_ROOT`, `HARNESS_ROOT` and `VALIDATION_PYTHON` below are task-specific shell variables, never replacements for `HOME` or `CODEX_HOME`. Freeze exact B/C hashes and register the run IDs, disk reserve and pair order with the coordinator first. Build the common seed through B once; prepare and close clones sequentially outside timing. The full fixture uses the original generated builder's missing derived marker so each fresh clone performs normal startup preparation. Page-cache flushing and forced checkpoints during timing are prohibited.

```sh
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/validate_combined_monitor.py" prepare-seed --app-root "$B_ROOT" --output /tmp/agenturm-final-common-seed
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/benchmark_monitor_idle.py" --app-root "$B_ROOT" --seed /tmp/agenturm-final-common-seed/raw-seed.db --output /tmp/agenturm-final-pair1-b --warmup 60 --seconds 300
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/benchmark_monitor_idle.py" --app-root "$C_ROOT" --seed /tmp/agenturm-final-common-seed/raw-seed.db --output /tmp/agenturm-final-pair1-c --warmup 60 --seconds 300
```

Register pair 2 in reverse order, C then B, with new output directories and the same common seed. Preserve each result before guarded clone cleanup, then check disk reserve again. Do not overlap either pair with tests, browser actions, another benchmark, profiler, soak or native/modal workload. An observed failure invalidates that result and remains in the report; no silent replacement of a bad run.

Separate generated functional commands are:

```sh
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/validate_combined_monitor.py" native-race --app-root "$C_ROOT" --output /tmp/agenturm-final-native-race
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/validate_combined_monitor.py" modals --app-root "$C_ROOT" --output /tmp/agenturm-final-modals --cycles 100 --warmup-cycles 10 --investigate
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/verify_keyboard_tmux.py" --app-root "$C_ROOT" --output /tmp/agenturm-final-keyboard
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/verify_keyboard_tmux.py" --app-root "$C_ROOT" --output /tmp/agenturm-final-keyboard-sigterm --shutdown sigterm
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/verify_keyboard_tmux.py" --app-root "$C_ROOT" --output /tmp/agenturm-final-keyboard-startup-sigint --shutdown sigint --signal-at startup
"$VALIDATION_PYTHON" "$HARNESS_ROOT/scripts/soak_monitor.py" --app-root "$C_ROOT" --seed /tmp/agenturm-final-common-seed/raw-seed.db --output /tmp/agenturm-final-active-soak --duration 3600
```

Run both external signal choices at reader and startup where the registered matrix requires them. The one-hour active workload remains descriptive because phases follow completion time; no paired active economics claim is permitted. The final report must separately state all unexecuted commands, failed gates and remaining open targets. The CPU/readiness/latency targets have **not been reevaluated by this preparation**.
