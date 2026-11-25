# Rust cockpit UI parity implementation plan

26 November 2026 · Python reference **0.2.8** · Rust planning baseline **`fd398c1`** on main

Build the full Python 0.2.8 cockpit experience into the Rust terminal app. A user must be able to discover work, choose an exact session, read its latest progress and earlier requests, investigate history, review the result, and resume follow-up work with the same evidence, discoverable controls, and predictable back navigation.

The approved Agenturm command center remains the landing screen: full-width project groups, named tasks, selectable status counts, newest updated first, and Enter to open the inspector. Preserve the Rust evidence and permission model. Match Python's visual hierarchy, layout quality, density, focus, and workflow outcomes elsewhere. Record any layout, key, or policy difference for explicit acceptance.

## Implementation status — 26 November 2026

The interrupted UI and core migration records were consolidated. PR #396
(`afbdbdc`) merged the integrated UI candidate and Activity prompt-only filter
to `main`. Candidate evidence remains limited to its recorded scope: 991 tests
across 96 suites, strict app/UI Clippy, 159 fresh-process `NO_COLOR=1` UI
tests, selected empty and populated PTY journeys at 120×32, 80×24 and 60×18,
and a selected debug QA archive smoke. This is not full Python parity or final
release qualification. The current unmerged feature branch adds a route from a
selected session to that source's metric catalog; metric records themselves
remain source-scoped, so per-session OTLP Metrics still requires a session
identity in the typed metric contract. See the [qualification report](../validation/rust-ui-parity-acceptance.md)
and [continuation record](../validation/rust-ui-parity-resume.md) for exact
scope and limits.

| Lane | Consolidated state | Remaining implementation or acceptance |
| --- | --- | --- |
| UI-01 Reference inventory | Original 234-row Python 0.2.8 inventory retained. | Reclassify every row against the current candidate; attach source and journey evidence; resolve unaccepted required differences. |
| UI-02 Shared shell | Shared key/action dispatch, contextual help, command palette and theme routing integrated in the candidate. | Complete the full binding/control journey comparison, including forms, clipboard, mouse and all compact/error states. |
| UI-03 Command center and inspector | Today/History, chooser, named roster behavior, inspector Overview/Prompts/Details and nested navigation are implemented in the candidate. | Pin primary-event identity through linked Activity; add inspector Locate/acknowledge/dismiss with distinct dismissed-error semantics; add subtree evidence queries when the kernel exposes them; catalog capture policy. |
| UI-04 Prompts and history | Integrated navigator/reader, wide and compact modes, exact grant behavior, paging and late-response fencing are implemented. | Add cursor-backed paging beyond the adapter’s bounded window; resolve no-copy policy differences; proactively clear on idle-policy changes; qualify attached/provider journeys. |
| UI-05 Evidence and observability screens | PR #396 merged page-local evidence filters, typed MCP-call trace navigation, selection-scoped copy/export, Activity prompt-only filter over typed NativeRequest rows and explicit “Unreported” worker detail. Current feature branch adds a session-entry route to source-filtered Metrics; offline locked app compile passes. | Trace tree and linked-evidence traversal; typed MCP aggregate metric cohort; true session-filtered OTLP Metrics needs a typed session identity in metric facts; diagnostics/health/reconnect/retry integration. UI05-A/B/C and UI-Q acceptance remain open. |
| UI-06 Review and workflow | Review receipts, incident state/owner and safe retry, settings draft confirmation, resume-unavailable reason, task search CLI and handoff collision checks are implemented in the candidate. | Review copy/export/save-task/excerpt/draft/Git/map actions; turn-scoped Review; persistent incident acknowledgment; cockpit task search and Decisions/Brief/version selector; handoff preview/copy; provider private detail remains coordinated with FD2. |
| UI-07 Themes and visual system | Five palettes, semantic styles, help and palette fallback geometry are integrated. | Run the paired visual matrix; review the documented Solarized muted-on-panel contrast result (4.23:1); verify overlays and history at compact sizes. |
| UI-Q Installed parity | Selected coordinator qualification passed on the candidate and an extracted selected-debug archive. | Independent full identical-fixture matrix, exact release artifact and ARM64, live broker revocation in an isolated root-capable fixture, and final independent sign-off remain open. |

The candidate’s lane checks are not a release gate. Keep the pluggable
architecture: UI behavior consumes provider-neutral typed kernel facts;
provider-specific proof stays in adapters and grant-aware app services; panes
and capabilities use their extension seams. Do not hardcode a new provider in
the UI to close a presentation gap. Keep all unaccepted UI-Q items and the
default cutover gate open.

## Reference and scope

- **Python oracle.** Use the actual tagged app at `2802d79930728bb331db1d4d718d6dea1c9c374d`. Inventory screens, controls, and states from `observatory/terminal_ui.py`, `ui_keyboard.py`, `themes.py`, and the tagged screen tests. Pair [the Python keyboard guide](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/keyboard-navigation.md) with [the tagged app source](https://github.com/abhishekash/agenturm/blob/v0.2.8/observatory/terminal_ui.py).
- **Rust baseline.** Preserve theme support, the named roster, scoped evidence tables, paged Codex Review edits, MCP inventory and linked traces, metric history and exemplars, combined usage, Findings, Host, and Workflow actions. The merged cross-host route is available for native attachments with explicit configuration. Existing checks qualify the recorded versions and scenarios. [Earlier UI comparison](../validation/rust-ui-journey-parity.md) · [Independent results](../validation/rust-migration-test-plan-results.md) · [Cross-host routing PR](https://github.com/abhishekash/agenturm/pull/387)
- **Product boundary.** Cover the Linux cockpit and interactive launch/attach behavior: standalone live collection, existing local history, broker access, native receiver, and the explicitly supported Python receiver. A mode without a capability must explain the limitation and leave navigation usable. Keep Python as a test oracle. SDK breadth, storage and service qualification, macOS support, and release policy remain in the [migration plan](rust-migration-remaining-plan.md).

## Implementation order

| Task | Owner | Starting point | Deliverable and dependencies |
| --- | --- | --- | --- |
| UI-01 · Reference inventory | UI lead | Partial audit exists | Build a row for every Python view, control, binding, and visible state. Record the Rust entry point, baseline, status, owner, and acceptance case. Start here and lock the shared key and focus contracts before merging dependent changes. |
| UI-02 · Shared shell and input | UI shell owner | Ratatui workspace, numbered views, help and five themes exist | Unify navigation, focus, layout and status behavior. Add the searchable command palette and reconcile bindings. Establish shared contracts for UI-03 through UI-07. |
| UI-03 · Command center and inspector | Session UI owner | Named roster and Overview/Activity/Review routes exist | Complete the Today/History, session chooser, and Overview/Details workflows. Ship inspector navigation with a Prompts entry point coordinated with UI-04. |
| UI-04 · Prompts and retained history | Session evidence UI owner | Scoped prompt picker, private reader, and paged history exist | Build the integrated prompt navigator and back stack. Reuse existing proof and query paths. Agree on session scope and layout with UI-03 before editing shared dispatch. |
| UI-05 · Observability screens | Evidence UI owner | Primary queries and screens exist | Close interaction and visual differences in Activity, Operations, MCP, Traces, Metrics, Usage, Findings, Host, and history search. Per-view work can proceed in parallel after shared contracts. |
| UI-06 · Review, Workflow and saved work | Workflow UI owner | Review and decision flows, saved tasks, and request actions exist | Complete paired workflows and form navigation. Coordinate provider edit/MCP detail with FD2 and receiver acceptance with CF7. |
| UI-07 · Themes and terminal states | UI visual owner | Five palettes and responsive layouts exist | Compare and close gaps in semantic styling, compact layouts, and the full state matrix. Integrate after each screen stabilizes. |
| UI-Q · Installed parity acceptance | Separate test agent | Earlier baselines have partial paired coverage | Sign off the full matrix against the exact installed Rust candidate and tagged Python build. Feed reproducible failures back to the feature owners. |

UI-03, UI-04, UI-05 and UI-06 can build and review their own models and rendering in parallel. The UI shell owner integrates edits to shared `workspace.rs` dispatch and help. Give each lane one owner for shared files, and keep focused component checks with the lane. Keep broad paired testing with the independent test agent.

## UI-01 · Build the parity inventory

Start with the tagged keyboard guide and palette, every root view, every inspector section and nested screen, the help and diagnostics surfaces, and actions exposed by a visible control. Record **implemented, qualification pending**, **implementation gap**, **source evidence unavailable**, or **intentional difference awaiting acceptance**. Attach the reference and Rust source locations to each row. Treat untested behavior as unknown until checked.

Record the Python and Rust entry key or control, the expected visible fields, initial focus, actions available, exact selection on return, and expected behavior when access or data is unavailable. Include mouse equivalence wherever Python's exposed controls are clickable. Require a documented keyboard route for every action. Add discovered missing behaviors to the lane that owns the view.

## UI-02 · Shared cockpit shell, keyboard and navigation

- Keep the Agenturm summary, horizontal views, active scope and connection state recognizable. Preserve `1`–`7` for the Python root destinations and document the Rust-only Operations and Usage paths. Expose clipped destinations through help and the palette.
- Restore `Ctrl+P` as the searchable cockpit command palette. Rust currently also uses it for themes. Keep themes on `F2` and offer Themes as a palette entry. Include destinations, session chooser, Today/History, evidence scope, live pause/refresh, diagnostics, retention history searches, and current-view actions. Limit each context to meaningful or explicitly unavailable commands.
- Use one action inventory for key dispatch, footer hints, and scrollable contextual `F1` help. `?` opens help outside text entry. Help opened from an editor, compact chooser, or active inspector returns to the originating control and preserves drafts and scroll.
- Define which control receives each key. A chooser owns Escape before the enclosing form. Text fields keep literal letters, digits, brackets and punctuation. Tab and Shift+Tab reach visible controls in order. Enter and Space activate buttons where compatible. Resolve `Ctrl+C` copy versus terminal interrupt by focus and document the behavior. Preserve unsaved drafts through help, resize and cancel.
- Restore views by typed identity and navigation context. Save the history incarnation, source, session or worker, turn, evidence identity, page or cursor, active filter, focus and scroll as applicable. If an identity is gone, show why and use a deterministic nearby selection. Discard late responses for a closed or superseded context. Keep the event loop responsive while reads are pending.
- **Implementation anchors.** `rust/agenturm-ui/src/keymap.rs`, `workspace.rs`, `workbench.rs` and the per-view modules. Reuse the existing request/response and terminal lifecycle paths.

## UI-03 · Command center and the session inspector

### Command center and session history

1. Retain project headings, counts, human-readable native task titles with agent nickname fallback, and Updated ↓. Keep exact identities in Details. Group and row order follow recency with deterministic ties, and refresh, paging, filtering and resize preserve selection by identity.
   - **Never render a literal `Name unavailable` label.** When neither a task title nor a native nickname exists, derive a concise, human-understandable name from available provider, role, project and session context (for example `Codex session · payments-api`, `Claude Code worker · agenturm`). Keep the missing metadata and exact IDs explicit in Details and the inspector, where unknown values stay labelled as unknown. Apply the same rule to the palette session chooser, inspector headers, workflow incidents, and saved-task run lists. Verify titled sessions, nickname-only subagents, duplicates and genuinely unnamed sessions.
2. Provide **Today** and **History** as explicit time scopes alongside the approved status filters. Today includes active work and pending input that started earlier, plus qualifying outcomes according to the tagged behavior. History includes earlier retained work, including unreviewed completions and dismissed failures. Verify the Python boundary with a fixed clock and timezone. Keep time scope, search, error filtering and session selection independently discoverable. Mark counts as page or scope counts according to their actual coverage.
3. Expose a searchable **Choose agent session** control and palette action. Search title, project, provider, session and worker identity. Keep empty results, cancel, clear and confirm usable at 60×18. Preserve exact source identity when titles or native IDs collide.
4. Preserve confirmed tree/flat presentation, expand/collapse, parent/child and descendant-attention paths, and exact-agent versus evidence-backed subtree scope where the reference offers it. Keep unsupported worker relationships explicit. A recorded lifecycle state never establishes current OS process liveness.
5. Document the approved landing-screen order as a deliberate difference from Python's fixed status groups and selectable update sort direction. Exercise retained lookback and session search while preserving newest-first order.

### Inspector

Enter or `e` opens the exact selected session with three discoverable sections. Preserve a selected session through section switches, live arrivals, width changes, and nested navigation.

| Section | Required behavior |
| --- | --- |
| **Overview** | Lead with the task, project, provider, recorded state/reason and last update. Show the latest attributable progress, turn/event summaries, checks, edits, failures and usage when available. Link to exact Activity, Review, Traces and related evidence. Label bounded, partial or absent evidence. |
| **Prompts** | Place the request navigator and reader inside the inspector, with the full contract in UI-04. Keep the current `P` shortcut as a direct entry where it fits. |
| **Details** | Separate stable source/session/worker identity, native context and provenance, collection status, observed relationships, timestamps, usage coverage and available diagnostics from the Overview summary. Show unknown, missing or unsupported values explicitly. |

Give section tabs, the active section and its read/navigation focus visible indications. Support the Python section controls where compatible, including Left/Right on the tabs, Enter or Down to enter, and the documented section-switch shortcuts from content. Capture and resolve any conflict with the current Rust Activity and Review paths. Esc returns one layer at a time to the same roster filter, project, task and scroll. Keep selection stable when the displayed title changes.

## UI-04 · Prompts, session activity and cross-session history

1. **Wide prompt navigation.** At 120 columns show a chronological request index beside the reader. Open with the newest available request selected and the navigator focused. Match Python's Up/Down, Home/End, Enter/Right to read, and Left to return from the reader. Reader scrolling keeps the selected request fixed.
2. **Compact prompt navigation.** At 80×24 and 60×18 use a one-line chooser and devote the remaining body to the reader. Enter confirms the current choice even if unchanged. Enter, Space or arrows open the chooser as documented. Preserve equivalent focus and the visible text position on resize. Support `[`, `]`, `l`, earlier pages and retry from their documented contexts. Distinguish loaded count, retained/capped history, and an empty exact scope.
3. **Private reader.** Show Readable and Original for the same granted captured request with `z`. Reuse existing exact proof, timeout, revocation and redraw clearing. Keep metadata-only contexts useful. Scope every read to the selected history/source/session and supported exact worker/turn; explicitly disclose any main-session navigation from a worker context. Compare copy and export behavior with the reference, with any tighter Rust disclosure policy documented and accepted. Cover grant denied, revoked, expired, stale, out-of-scope, truncated, missing capture, and late result after close.
4. **Retained evidence paging.** Distinguish roster History from older/newer/latest pages of Activity, MCP, Traces, Review and other paged views. Preserve filters, selection, query and snapshot through page changes, boundary cases, live pause, reconnect and restart. Search the intended retained scope where the workflow requires it and disclose page-local limits where they apply.
5. **History investigation.** Wire the tagged `Ctrl+F` journeys for recorded paths, commands, instructions and failures to the available Rust evidence and current grants. Verify each query kind before classifying it as implemented. Open the exact occurrence and make Escape or Alt+Left restore the query, filters, occurrence, page and focus. Scope unavailable and permission errors to the attempted action.
6. **Implementation anchors.** Extend workspace session modes and `workspace/prompt.rs`, the prompt workbench model, `agenturm-app/src/cockpit_workflows.rs` and the existing private content viewer. Keep query and grant enforcement in the app layer. Keep retained identifiers in the UI navigation model.

## UI-05 and UI-06 · Remaining screen and workflow coverage

| Surface | Implementation work and acceptance journey |
| --- | --- |
| **Activity and Operations** | Match populated table columns, filters including prompt/operation views where applicable, and selected evidence detail. From a session or interrupted turn, open a tool or check, inspect its result with recorded provenance, follow related evidence, and return to the same event. Verify unavailable attribution and exact worker scope. |
| **MCP** | Compare configured, idle and observed inventory, server filtering, call search/paging and related traces. Open a filtered call, read its exact permitted payload or reason it is unavailable, follow its linked trace, and return with the same server, query and call. Extend provider-specific payload viewing only where FD2 has retained proof and a valid grant. |
| **Traces** | Match trace tree/list navigation, parent/child context, timestamps and span evidence. Move between linked Activity, call, exemplar and trace, then return to the originating selection. Include missing links, partial trace, and private span detail when authorized. |
| **Metrics and Usage** | Verify complete retained catalog search and descriptions, select an older series and history page, open an exact exemplar and return. Compare seven-day chart/list, source/session filters, combined and separate evidence totals, cost units/currencies, exclusions, per-day coverage and unreported data. Keep chart values readable in compact and monochrome modes. Verify the current FD3 candidate. |
| **Findings and diagnostics** | Group and count findings and show exact occurrences, observed scope, source and available project/process labels. Verify command, span, evaluation, log, and supported attached-receiver source routes, retry, policy changes, and return. Compare diagnostics, index/receiver health, reconnect and detail copy. |
| **Host** | Verify live Linux processes, recorded history, remote source chooser, metadata provenance, staleness, and scoped search. Inspect an exact process and return through refresh/reordering, source switch and process exit. Clearly label current versus recorded evidence. |
| **Review** | Open exact session or turn Review from the roster or inspector. Cover edit list, older evidence, checks and errors, selected edit reveal, acknowledgement, notes, version-linked decisions, excerpts and save-as-task. Return to the same version and selection after nested views. Use the existing whole-session Codex browser. FD2 owns remaining provider proofs and missing detail. |
| **Workflow** | Cover inbox focus, incident detail, observed owner, Seen, review, verified local Locate/resume, configured cross-host attach guidance, settings, thresholds, snooze, notification, and exact failed-attempt retry. Show clear unavailable/stale cases. Route and Seen must preserve the request selection and its recorded approval state. |
| **Saved tasks, decisions and handoff** | Open, search, create and edit tasks. Link exact runs and workspace context, add and supersede decisions, preview or export a handoff, then reopen persisted work. Verify version selector, dirty editor, cancel and export collision behavior. |
| **Copy, export, preferences and overlays** | Confirm current selection versus loaded-view export scope, format and destination. Give clear success or denial. Verify read-only modes, help, palette, theme chooser, alerts and private display boundaries. |

## UI-07 · Visual and responsive acceptance

Use the existing five palettes (**Observation, Current, Bright, Dark, Solarized**) and semantic surface, text, selection, focus and status styles. Compare theme chooser preview, cancel, apply and reopen persistence. Review each screen against the same Python fixture at the same size. Compare framing, spacing, header hierarchy, table density, selected backgrounds and inspector readability. Check focus and status with `NO_COLOR`, using a visible marker or text as well as any color cue.

Use one border per content region. At 60 columns measure rows or cells spent on repeated headers, padding, decorative status cues and nested borders, and keep evidence visible first. Prefer one clear selection cue and one concise status label. Truncate labels by terminal cell width and show their full value in detail. Include CJK, combining marks, emoji and long names.

```text
120×32 · session inspector                      60×18 · session inspector
Agenturm / project / session                   Agenturm / session
Overview   [Prompts]   Details                 Overview [Prompts] Details
┌ Requests ─────────┬ Prompt ───────────┐      Request 4 of 7  [Choose ▾]
│ 1  Earlier request│ Readable / Original│      Readable / Original
│ ›4 Current request│ Scrollable text   │      Scrollable prompt text
└───────────────────┴───────────────────┘      ↑↓ Scroll  Esc Back  F1 Help
← Requests  ↑↓ Scroll  Esc Back  F1 Help
```

At 80×24 use the compact prompt chooser and collapse optional evidence columns before hiding actions. At 60×18 use one active pane, move details to a drilldown, leave the selected title/status/age or primary evidence readable, and keep Help and Back reachable. Map focus to the equivalent control when resizing. Determine the actual minimum and provide a resize message with a usable exit below it.

Cover startup, populated, empty, search with no matches, loading, partial, stale/disconnected, denied, error/retry and too-small states. Keep the previous permitted view and focus through refresh. Clear protected content as required by the existing access fence. Verify normal quit, interrupt, failure and supported terminal handoff restore the terminal.

## UI-Q · Separate acceptance handoff and completion gate

The independent test agent owns the paired installed run. Feature owners run targeted state and rendering checks for the behavior they change. Reuse Python Textual Pilot captures, Rust Ratatui buffer assertions for layout and styles, and the existing PTY/tmux fixtures for actual key delivery, resize and terminal restoration. Use comparable deterministic data in both apps and separate actual-harness collection evidence from visual fixtures.

| Gate | Required evidence |
| --- | --- |
| Reference inventory | Each Python view, action, binding and state maps to a working Rust journey or an explicitly accepted difference. Annotate evidence unavailable by provider and mode. |
| Screen and state comparison | At 120×32, 80×24 and 60×18, paired frames and journeys cover the screen matrix, themes, empty/loading/error and relevant revoked and disconnected states. Record mismatches and resolve required gaps. |
| Keyboard and navigation | Record starting focus, keys, opened evidence identity, nested return, resize behavior and saved selection. Cover inspector → Prompts → older request → original → Review or Activity → return, plus chooser, forms, history, help and copy. |
| Exact candidate | Record the Rust source SHA, archive and executable SHA-256, Python wheel/tag, OS and architecture, terminal environment, fixture version, size, actual result, and artifact/capture paths. Run the affected broker/attached and provider cases on the frozen candidate. |
| User acceptance | Confirm accepted deviations, including the command-center order, any binding differences, and grant-fenced disclosure. Every required mismatch has a fix and rerun. Full UI parity requires the exact installed candidate to pass; release and default cutover retain their separate gates. |

Use [Q6 in the remaining migration plan](rust-migration-remaining-plan.md#q6-installed-paired-ui-and-workflow-journeys) and [the delegated test plan](../validation/rust-core-feature-test-plan.md) for execution. Each result should link to its UI task and identify the reference state, actual Rust state, and reproduction keys. Prior test results stay attached to the binaries that produced them.
