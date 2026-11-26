# UI-03 lane notes: command center and session inspector

24 November 2026 · branch `work/ui03-command-center` · Python reference **v0.2.8** at `2802d79930728bb331db1d4d718d6dea1c9c374d` · integrated parity baseline `546ea48`

This lane completes the command-center time scopes, the searchable session
chooser, the exact-agent/subtree scope and the three-section session inspector.
It follows the published [shared shell contracts](../rfcs/rust-ui-parity-shared-contracts.md)
and keeps the shell generic over model data: no provider switch, no captured-text
read and no second key or help table were added.

## What changed

### Command center

| Area | Behavior |
| --- | --- |
| Names | `command_center::name` prefers the task title, then the native nickname, then `Codex session · payments-api`, `Claude Code worker · agenturm` or `Codex worker w-1 · project`. It never renders `Name unavailable`. A worker identity makes the fallback specific. Unknown metadata stays explicit in Details and the inspector. |
| Project headings | Rows without a project heading now read `No recorded project` instead of `Project unavailable`. The heading is a coverage label, not a name. |
| Order | Unchanged approved order: project group by newest row, then row recency, then depth and exact source/session for deterministic ties. `Updated ↓` remains the fixed direction. |
| Coverage | The footer says `page counts · newest retained page` or `page counts · older sessions not loaded`; it never presents a loaded page as complete history. |
| Liveness | The footer and help state `recorded lifecycle, not live process state`. `Working` is a recorded state, not a claim that a process is alive. |
| Selection | Exact `(harness_id, session_id)` survives refresh, page change, Time scope, Errors, status filters, subtree scope and rename. A vanished identity selects the nearest deterministic row and reports it. |

### Time scope, Errors and search

- **Today** uses `agenturm_kernel::session_scope`. The host resolves the local
  calendar boundary for the exact read instant (including the offset at local
  midnight across a DST transition) and sends it as
  `SessionCatalog::day_start_unix_ns`. The tagged Python rule is preserved:
  current work of any age, pending input of any age, and only today's
  non-acknowledged outcomes; a reviewed outcome or acknowledged pending request
  drops out of Today. History accepts everything retained.
- **Keys:** `T` Today, `O` History, `E` Errors only, `S` subtree scope. Each is
  an inventory row and a palette row. The chip strip also shows `Today N`,
  `History N`, the status filters, `Errors N` and `Unknown N`.
- **Independence:** time scope, status filter, errors-only, search and exact
  session scope are separate fields. Counts describe the loaded page and say
  which facets they ignore.
- **Search** covers task title, project path, provider label, exact source,
  exact session, worker identity, native name/role/path/model provider, state
  and reason. Bytes are shown escaped, never interpreted.
- **Empty states** distinguish no catalog page, empty collection, an active
  search with no match, errors-only empty, Today empty and a status-filter miss.

### Session chooser

- `Ctrl+P → Choose agent session` opens `palette::Kind::Sessions`. Entries are
  frozen at open in roster order.
- Rows show the human name and detail with provider, project, native context,
  worker identity, exact source and exact session. The stable key is
  `session|<harness>|<session>`; duplicate titles and equal session IDs across
  sources stay distinct.
- Search accepts title, project, provider, session and worker text.
- Empty results stay open; `Ctrl+A` clears; `Enter` confirms; `Esc` cancels.
  Covered at 60×18. A disappeared identity reports
  `That session is no longer available. Choose another session.`

### Agent scope and relationships

- Exact-agent scope selects one recorded `(source, session)`.
- `S` adds only confirmed `delegated` descendants whose parent is retained on
  the page and in the same source family. `forked`, cross-source and
  chain-incomplete relationships are counted and reported, never guessed into
  the subtree.
- Alt+↑/↓/→ parent/child/attention paths and tree/flat/expand/collapse are
  unchanged. A cycle stays a separate row with no invented root.

### Inspector

`Enter` or `e` opens the exact selected session. The inspector has one tab
strip with three discoverable sections:

| Section | Content |
| --- | --- |
| **Overview** | Human name, provider, project, recorded state/reason, last recorded update; recorded turns; latest attributable progress; checks with pass/fail counts; edit counts; recorded failure count; usage summary with coverage; latest turn end; page/partial coverage; and the documented links `s Activity · Ctrl+R Review · t Traces · u Usage · P Prompts`. |
| **Prompts** | An entry point, not a second reader. `Enter`/`Down` on the tab, or `P`, sends `WorkCommand::Prompts(WorkAnchor)` for the exact session. UI-04 owns the navigator and reader. |
| **Details** | Exact source, session and worker; task title; display name; provider; project and provenance; native name/role/path/model; parent and relationship kind; recorded state and reason; last update and latest observation; review sequence/state; snapshot and history identity; pending-request count; collection coverage; capture-policy limit; usage coverage; diagnostic count. Missing values read `Unreported`, `No recorded …` or `Not reported in this roster projection`. |

Navigation:

- The tab strip is one focus stop; the active tab is always visible and the
  focused strip shows `›` plus `READ: Enter`. Content focus shows `READ: content`.
- `←`/`→` choose a section on the tab strip. `Enter`/`Down` enter the content;
  on Prompts they open the UI-04 navigator. `↑` on Prompts also opens it.
- From content, `Ctrl+←`/`Ctrl+→` and `m` switch sections. Plain `←`/`→` stay
  with the content and evidence panes.
- `Esc` returns one layer: prompt overlay → inspector → roster. The roster
  keeps its filter, project grouping, selection and display offset. Footer
  hints come from `keymap::hint`; only the `READ: Enter` / `READ: content` state
  and page coverage are appended by the inspector.
- Section changes reload in place with `Change::SessionMode`; they do not grow
  the history stack. A pushed Activity/Review/Traces/Usage page returns to the
  same inspector section.

### App projection

`session_evidence::add_session_panes(snapshot, &RecordedSession)` now emits
`app.session-overview`, `app.session-details`, activity, review, usage, and the
MCP/operation/trace panes. `session_display::add_context` prepends the human
name, provider, native context, project provenance and recorded state/reason to
the Overview. Exact identity stays in Details.

## Shared dispatch added

One `keymap::ACTIONS` row, one dispatch arm and one palette entry per action:

| Action id | Keys | Context |
| --- | --- | --- |
| `session.today` | `T` | Command center |
| `session.history` | `O` | Command center |
| `session.errors` | `E` | Command center |
| `evidence.scope.subtree` | `S` | Command center |
| `session.sections` | `← / →` | History |

`session.inspector-mode` (`m`) is unchanged as an id; its behavior is now the
section cycle, documented in help. The legacy command-center help body was
replaced with a pointer to the shared scrollable reference, so there is one
help source.

**Deliberate narrowing.** `session.today`/`session.history` are inventory rows
in the CommandCenter context only. `T` already means “choose recorded turn” in
the History context; adding a second meaning would violate one-key-one-owner.
The History palette still reaches both scopes through the roster actions that
navigate back to Agents.

## Tests run

From `rust/`, with `CARGO_INCREMENTAL=0` after the workspace disk filled:

```sh
cargo build --offline
cargo test --offline -p agenturm-ui -p agenturm-app
```

Results (24 November 2026):

- `cargo build --offline`: success.
- `agenturm-kernel` session-scope unit tests: 3 passed (fixed UTC/+01:00/−05:00
  boundary, active work/pending input of any age, outcome and review matrix).
- `agenturm-ui` lib: **106 passed**.
- `agenturm-app` lib: **178 passed**; every integration test binary passed
  (`adapter_install`, `attention`, `attention_workflow_status`,
  `automatic_retention`, `configured_collection`, `eval_import`, `new_window`,
  `prompt_content`, `read_incarnation`, `review_receipt`,
  `sample_extension_composition`, `span_jsonl_follow`, `span_jsonl_import`,
  `standalone_trace_review`, `summary`, `systemd_service`, `task_cli`,
  `trace_run`, `trace_turn_review`, `workspace_backup`).
- `cargo fmt --check -p agenturm-ui -p agenturm-app -p agenturm-kernel`: clean.

Focused cases added in this lane:

- `session_scope::tests::*` (kernel boundary matrix).
- `workspace::tests::today_scope_uses_the_host_local_boundary_and_keeps_active_work_of_any_age`
- `workspace::tests::errors_and_subtree_scopes_publish_unsupported_relationships_and_keep_identity`
- `workspace::tests::session_chooser_searches_worker_identity_and_stays_usable_at_the_floor`
- `workspace::tests::inspector_sections_navigate_enter_and_return_with_the_same_roster_state`
- `workspace::tests::unnamed_sessions_render_human_context_never_a_name_unavailable_label`
- `session_evidence::tests::session_overview_and_details_keep_human_context_and_exact_identity_apart`
- Existing command-center, chooser, help, today-scope, tree and rename tests were
  updated for the new copy and section behavior.

## What remains

1. **Evidence view primary-event pinning (I-15).** Overview links open the exact
   session Activity, but do not yet pin a failed call, request or interruption
   as the initial selected event.
2. **Locate, acknowledgement and dismiss from the inspector (I-14/I-17).**
   `X`/`Y` remain roster actions; the inspector has no Locate or per-run
   seen/dismissed control. Dismissed failures are not a separate retention
   state in the catalog, so the Errors facet is a recorded `failed`/`interrupted`
   filter, not Python's two-conversation unseen-error reminder.
3. **Evidence subtree membership (A-24).** The roster and chooser honour exact
   agent + delegated subtree. Activity/MCP/Traces/Findings are still scoped to
   the exact session or turn; a subtree evidence query needs a kernel/query
   capability rather than a UI list rewrite.
4. **AI summary (I-18).** Not implemented. It requires an optional extension
   capability with a provider/model label and an explicit egress grant; a UI
   branch is not acceptable.
5. **Capture policy in Details (I-08).** The roster projection has no capture
   policy field, so Details says it is not reported here instead of inferring
   it. Add it to the catalog projection when the app exposes it.
6. **Mouse (A-20).** Still keyboard-first. Any mouse route must call the same
   action id.
7. **Provider-specific payload/detail.** Unchanged and owned by UI-05/FD2; the
   inspector does not read captured text.
8. **Hour-of-boundary DST edge.** The host resolves the offset at the read
   instant and at local midnight; the kernel arithmetic stays a pure function.
   A paired fixed-zone PTY run against the installed candidate is still UI-Q
   work.

## Exact hooks for the shell integration pass

### UI-02 (shared shell)

- Keep the five new inventory ids and their contexts. `keymap::hint` works
  unchanged. `Focus` gained no variant: inspector tab focus lives in
  `Workspace::inspector`, because it is screen-local navigation, not a modal
  owner. `Esc` still falls through to the shared history pop.
- `ui::render_selected_pane` is now `pub(crate)` and tolerates an empty pane
  list. The inspector module uses it; no shell layout was duplicated.
- The command center no longer renders `agents.help`. `F1` routes to the shared
  `help::Help`. The `agents.help` field remains only for direct-render
  compatibility.
- Palette: `session.sections` focuses the inspector tab strip; the other four
  run their documented dispatch. No palette entry executes app work directly.

### UI-04 (Prompts)

- The only entry hook is
  `WorkCommand::Prompts(WorkAnchor { source, session, worker: None, turn: None })`
  for the exact inspector session, also reachable with `P`.
- The shell sets `inspector.tab = Prompts` before the request. When the
  navigator closes, the tab strip remains focused on Prompts; `Esc` then
  returns to the inspector section, then the roster.
- Do not render a second section reader: replace or overlay the inspector body
  inside the existing `Workbench` flow, and keep the same anchor semantics for
  worker/turn scope.
- If UI-04 needs the current section from inside the navigator, read
  `inspector::Tab` through the workspace helper rather than storing a second
  focus object.

### UI-05 / UI-06

- Use `RecordedSession::provider_label()` and `display_name()` for human labels.
  Do not add `Name unavailable`; the fallback is already data-derived.
- `SessionCatalog::day_start_unix_ns` is the resolved local boundary. Reuse it
  for any Today/History query instead of recomputing UTC midnight.
- `RecordedSession::worker_id` is the exact recorded lifecycle worker. Use it
  for chooser/detail labels; it does not establish live process ownership.
- `agenturm_kernel::session_scope::in_session_time_scope` is the neutral policy
  for saved-task run lists and workflow incidents that need the same boundary.

## Pluggable-seam implications

- **Kernel, not UI, owns the time policy.**
  `agenturm-kernel::session_scope` is data-only, provider-free and testable with
  a fixed day start. The host resolves the timezone; the UI never assumes UTC.
- **The catalog stays the session seam.**
  `SessionCatalog::day_start_unix_ns`, `RecordedSession::worker_id` and the
  provider/native labels are derived from recorded data. No adapter is named in
  the shell.
- **Inspector sections are projections, not provider branches.**
  Overview and Details are generic `PaneModel` rows produced by the app from
  typed observations and the session record. A new adapter contributes
  observations; it never edits the inspector.
- **Prompts stay a capability.** The inspector only requests the exact
  `WorkAnchor`; UI-04's adapter-backed prompt capability owns paging, grant
  fencing and presentation.
- **Scope honesty.** Subtree membership uses only reported relationships. A
  missing capability reports an unsupported relationship; it does not fall back
  to a provider-specific hierarchy.
- **Display labels remain a presentation fallback.** `provider_label()` keeps the
  existing source-family mapping (`harness.codex` → `Codex`). It never routes or
  selects evidence. A harness-provided display-name capability would remove the
  last label mapping from the UI; until then it is confined to the roster,
  chooser and inspector headings.

## Intentional differences to accept

1. The approved landing order remains project-grouped, newest-first with a
   fixed `Updated ↓`; Python offers status-first and a selectable direction.
2. Today/History are independent time-scope chips with `T`/`O`, not members of
   Python's single filter select. Status filters stay independent.
3. The inspector's Prompts section opens the shared UI-04 navigator overlay
   instead of embedding the reader in the tab body in this change.
4. `session.today`/`session.history` are palette/scoped actions only in the
   command center to avoid overloading History's `T` turn picker.
5. Recorded lifecycle wording never implies live process health; the inspector
   labels every state as recorded.
