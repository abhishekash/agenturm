# Rust cockpit shared UI contracts

**Owner:** UI-02 · shared shell, keyboard and navigation
**Status:** published for UI-03, UI-04, UI-05, UI-06 and UI-07
**Reference oracle:** Python 0.2.8 at `2802d79930728bb331db1d4d718d6dea1c9c374d`
**Implementation:** `rust/agenturm-ui/src/{keymap,workspace,workbench,lib,theme,palette,help}.rs`

This document is the integration boundary for every lane that renders a cockpit
screen. The shell owns terminal lifecycle, shared navigation, focus precedence,
the action inventory, the command palette, contextual help and the footer. Lane
modules own their models, rendering and screen-specific actions.

The architecture constraint is unchanged: the shell is generic over
model data. It never switches on a provider ID, an extension ID or an app-level
special case. New screens arrive as kernel panes or typed host requests, and new
actions arrive as inventory rows plus a dispatch arm. Kernel ports, supervised
source adapters, harness crates and the `ExtensionPlugin`/capability/pane seams
are not bypassed.

---

## 1. Shell invariants

1. **One action inventory.** `keymap::ACTIONS` is the source for footer hints
   and `F1`/`?` help. Each row has a stable `id`, display keys, a label, a
   detail sentence, a group and the contexts where it applies.
2. **One palette contract.** `Ctrl+P` opens `palette::Palette`. It returns a
   stable key and never executes app work itself.
3. **No hidden focus.** Every key has one owner: private prompt, form, chooser,
   text field, palette, help, workbench, inspector or command center. Unknown
   keys are inert.
4. **No destructive help.** Opening or scrolling help mutates nothing in the
   underlying view. Drafts, selections, filters, cursor and scroll survive.
5. **Late data cannot replace a closed view.** Responses carry a navigation
   generation; a superseded generation is discarded with a visible reason.
6. **Nothing provider-specific in the shell.** Names and identities come from
   data. A missing title is derived by the owning screen; the shell does not
   invent a provider name or a `Name unavailable` label.

---

## 2. Root destinations and clipping

| Key | Destination | Notes |
| --- | --- | --- |
| `1` | Agents / command center | Python root destination |
| `2` | Activity | Python root destination |
| `3` | MCP | Python root destination |
| `4` | Traces | Python root destination |
| `5` | Host | Python root destination |
| `6` | Metrics | Python root destination |
| `7` | Findings | Python root destination |
| `9` | Operations | Rust-only retained operation evidence |
| `U` | Usage | Rust-only combined retained usage |
| `8` | Traces alias | Deliberate Rust compatibility alias |

`keymap::NAVIGATION` keeps `1`–`7` fixed. `keymap::EXTRA_NAVIGATION` holds the
Rust-only Operations and Usage destinations. Numbered strips may clip at narrow
widths; help and the palette always expose the full set. The command center may
show only five destinations in its one-line strip and add `F1 more`; this is a
presentation clip, not a removal.

Palette identifiers: `go.agents`, `go.activity`, `go.mcp`, `go.traces`,
`go.host`, `go.metrics`, `go.findings`, `go.operations`, `go.usage`.

---

## 3. Action inventory (`keymap.rs`)

```rust
pub(crate) struct Action {
    pub id: &'static str,        // stable identifier used by dispatch/palette
    pub keys: &'static str,      // documented keys, e.g. "Ctrl+P" or "F1 / ?"
    pub label: &'static str,     // footer/help/palette label
    pub detail: &'static str,    // one-sentence help and palette detail
    pub group: Group,            // VIEWS, SESSION, EVIDENCE, LIVE, SEARCH, ...
    pub contexts: &'static [Context],
}
```

- `Context` is `CommandCenter`, `History`, `Workbench`, `Investigation` or
  `Form`.
- `keymap::actions(context)` yields the contextual rows.
- `keymap::help_sections(context)` yields `(group, lines)` for help.
- `keymap::hint(&[id, ...])` yields deterministic footer text.
- `keymap::find_action(id)` resolves an identifier.
- `keymap::PALETTE_EXCLUDED` names keys that have no unambiguous palette
  equivalent (today only modifier-plus-direction agent-family navigation).

Current shell identifiers include destinations, `cockpit.palette`,
`cockpit.themes`, `cockpit.help`, `session.choose`, `session.today`,
`session.history`, `evidence.scope.session`, `evidence.scope.all`,
`live.pause`, `live.refresh`, `diagnostics.open`, `diagnostics.retry`,
`history.{paths,commands,instructions,failures,failed-command}`,
`view.{review,prompts,traces,older-page,newer-page,latest-page,turn,mcp-payload,
file-edits,review-notes,mark-reviewed,copy,export,internal-reviews,workflow,
tasks,search,inspect,back}` and `cockpit.quit`.

**Adding an action (UI-03..UI-06).** Add one `id` constant, one `ACTIONS` row
with the correct contexts and group, and one dispatch arm. If it belongs in the
palette, also teach `Workspace::run_action` to run it by sending the same key or
by calling the same typed request. Do not add a second key table, a second help
string or a provider branch.

Footer rule: use `keymap::hint` for the shared portion and append only
screen-specific state (counts, page position, denied reasons). Do not repeat
global keys that the inventory already owns.

---

## 4. Focus and key ownership

`keymap::Focus` names the current owner. `Workspace::focus()` derives it from
state; no lane stores a second focus object.

| Focus | Owns | Must not receive |
| --- | --- | --- |
| `PrivatePrompt` | Esc/q dismissal, its reveal keys | Global navigation, palette, help |
| `ExportForm` | Esc, text, Enter submit | Global navigation |
| `WorkbenchEditor` | literal text, Ctrl+S, its dropdown Escape | Global single-letter actions |
| `AgentSearch`, `HistorySearch` | literal letters, digits, brackets, `?`, punctuation, Backspace | Destination digits, `/`, `?` help |
| `SessionChooser`, `HistoryChooser`, `Palette` | Escape first, arrows, Enter | Enclosing form Escape, Back |
| `Help` | scroll, F1/Esc/`?` close, q quit | Underlying view mutations |
| `EvidenceInspection` | scroll, PgUp/PgDn, Esc | Selection changes outside the inspector |
| `HistoryTable`, `CommandCenter`, `Workbench` | view keys | Private content reads |

Rules:

1. **Chooser Escape precedence.** A chooser (`SessionChooser`, `HistoryChooser`,
   `Palette`) consumes Escape before any enclosing form or screen. Escape
   restores the control that opened it and changes no data.
2. **Text fields keep literal text.** Letters, digits, `[`, `]`, `/`, `?` and
   punctuation are typed into a focused editor. Export forms and workbench
   editors own printable keys. `F1` remains available while editing and returns
   to the same draft.
3. **`?` outside text entry opens help.** In search/editors `?` is text.
4. **Tab / Shift+Tab.** In palette and help, Tab/Shift+Tab move through the
   visible choices in order. The command center deliberately keeps Python's
   status-filter Tab and inspector sections keep pane switching; both are
   documented screen-local exceptions, not shell-wide Tab semantics.
5. **Enter / Space activate.** `keymap::activate` is Enter or Space. The theme
   picker applies with either. Dialogs and forms must activate only their
   visible button/choice, never an off-screen action.
6. **`Ctrl+C` by focus.** Public loaded-evidence focus (`CommandCenter`, `HistoryTable`,
   `EvidenceInspection`) copies the loaded metadata view and stays open. `PrivatePrompt`, text editors, forms, palette and help
   keep terminal-interrupt/quit. `c` always copies, `x` exports and `q` quits.
7. **Drafts survive.** Help, resize and cancel do not clear a search draft,
   export directory, workbench edit or private prompt deadline/fence. Cancel
   explicitly restores the prior value; help never does.
8. **`q`.** Quits from the cockpit and from help when no private content is
   open. Text fields keep ordinary `q` typing.

Focus precedence in `Workspace::key` is: private interrupt → help overlay →
palette overlay → `Ctrl+P` → help shortcut → private prompt → form/investigation
→ workbench → search filters → history choosers → history keys → command center.

---

## 5. Command palette (`Ctrl+P`)

`palette::Palette` is a data-only overlay.

```rust
Entry {
    key: String,       // stable command id or "session|<harness>|<session>"
    label: String,
    detail: String,    // or an explicit unavailable reason
    group: String,     // inventory group label
    enabled: bool,
}
```

- **Open:** `Ctrl+P` outside private content. `Workspace::open_palette` builds
  entries from `keymap::actions(self.context())` and marks actions unavailable
  with a reason instead of hiding them when the context cannot use them.
- **Search:** typing edits the query in place; digits, brackets, `?` and
  punctuation are literal. Space is text. Up/Down and Tab/BackTab move;
  PgUp/PgDn, Home/End jump; Enter runs an enabled row; Esc cancels; Backspace
  edits; `Ctrl+A` clears the query. An empty result stays open with a clear
  message.
- **Stable selection:** filtering retains the selected key when it remains
  visible. Missing keys select the first deterministic row.
- **Run:** the palette closes before `Workspace::run_action` executes the key.
  Dispatch then uses the same path as the documented key, so there is one
  behavior, not a parallel implementation.
- **Sessions:** `Choose agent session` switches the palette to
  `palette::Kind::Sessions`. Entries are frozen when the chooser opens, in the
  roster's approved newest-first order. `session|all` clears only the session
  filter. A chosen identity is re-resolved against the current roster; if it is
  gone the chooser reports `That session is no longer available. Choose another
  session.` and changes nothing. Escape cancels before the monitor handles Back.
  The current scope is selected by stable key.
- **Themes:** `Ctrl+P` no longer opens appearance. Themes remain on `F2` and are
  an enabled palette row. Selecting it raises `Workspace::theme_request`; the
  event loop opens the existing reversible picker. Preview, Enter/Space apply,
  Esc restore and saved preference are unchanged.
- **Contents:** destinations; session chooser; Today/History; evidence scope to
  the exact session/turn or all sources; live pause/resume and refresh/retry;
  diagnostics; the five retained-history investigations; Review, Prompts,
  Traces, turns, MCP payloads, edits, notes, acknowledgement; page navigation;
  copy/export; internal reviews; Workflow and saved tasks; search, inspect, back
  and quit. Rows that cannot act show `Unavailable · <reason>`.
- **Contexts:** CommandCenter, History, Workbench, Investigation and Form expose
  the shared rows. The palette opens over a workbench editor without changing
  its draft; choosing a row closes the palette before dispatch. UI-06 should add
  workflow/task-specific rows and any guards for a dirty editor.
- **Rendering:** centred popup, search line, visible count, scrolling list,
  two-line detail/unavailable reason and `Enter / Esc` hint. It is usable at
  60×18; below 20×6 it shows a resize message and Esc cancel with no panic.
- **Privacy:** entries contain labels, ids and reasons only. The palette never
  reads captured prompt text.

---

## 6. Contextual help (`F1` / `?`)

`help::Help` is one persistent, scrollable overlay.

- `F1` opens from every non-private context, including editors, palette and
  session choosers, forms, history, workbench and investigation. Help over a
  chooser returns to the frozen chooser state. `?` opens outside text entry.
- The first section is the current context. Additional `THIS VIEW` lines name
  the exact session/turn, review state, available captured text, evaluations,
  MCP payloads and workflow draft behavior when present.
- Up/Down, PgUp/PgDn, Home/End scroll. Esc, F1 or `?` close; `q` quits when no
  private content is open.
- Closing returns to the exact originating control. The shell does not replace
  or reconstruct the underlying model, so drafts, selection, filter, cursor and
  scroll are untouched.
- Rendering uses one border, wraps by terminal cell width and shows
  `↑↓ / PgUp/PgDn Read` plus `Esc, F1 or ? close help` at 60×18 and above.
- The command center's legacy `agents.help` field and its direct-render test
  remain for UI-03, but the shell now routes `F1`/`?` to this shared overlay.
  UI-03 should delete the duplicated help text when it next edits
  `command_center.rs`; the shared overlay already includes every command-center
  action and the Rust-only destinations.
- Workbench's private `help` flag remains for direct `Workbench` tests; from the
  cockpit, `F1` is the shared overlay. UI-06 should not add a second help table.

---

## 7. Header, footer, themes and connection state

- Keep the `Agenturm` summary, the horizontal view strip, the active scope and
  the connection state recognizable. “Paused”, “Loading…”, “History reader
  stopped”, scoped denials and stale/partial notices stay visible.
- Footer priority: explicit error/message → pending load → current view hints →
  inventory hints. Use `keymap::hint` for the last two.
- Themes: five palettes (`Observation`, `Current`, `Bright`, `Dark`,
  `Solarized`) remain in `theme::PICKER`. `F2` previews; Enter/Space applies;
  Esc restores. Saved preference is `$XDG_CONFIG_HOME/agenturm-rust/ui.json`
  with owner-only writes; `AGENTURM_UI_PREFERENCES=0` disables persistence.
- `NO_COLOR` removes colour, not structure: selection uses reverse/bold and
  status text remains readable.

---

## 8. View identity, restoration and late responses

Restoration is by typed identity plus navigation context, never by screen
position alone.

| Context | Stored identity |
| --- | --- |
| Roster | filter, time scope, exact session scope, search, grouping, tree/flat, collapsed identities, selected `(harness_id, session_id)`, display offset |
| Session inspector | `HistorySelection::Session(row, view)` with source/session, snapshot incarnation, history identity, review version |
| Review edits | `HistorySelection::ReviewEdits(row)` plus pinned snapshot/history identity |
| Turn | `HistorySelection::Turn(turn)` with source, session, worker, turn and snapshot |
| MCP search | exact scope anchor, query and read snapshot |
| MCP traces | source, event, revision and read snapshot |
| History page | snapshot id, history identity, selected pane title/id, reading scroll, inherited operation/finding filters |
| Pages | full cached newer `PageView` stack; older cursor bound to the same snapshot id |
| Evidence | finding/operation filters and the selected occurrence where the owner exposes it |

Behavior:

- `AgentList::apply_filters` restores the selected session by exact identity and
  keeps the nearest prior row only when that identity disappeared; it sets
  `lost_selection` so the caller can show a reason.
- `UiModel::restore_view` restores the pane by title and scroll when it still
  exists. A missing pane keeps the nearest deterministic row and the caller
  reports `"<pane> is no longer in this page · kept the nearest view"`.
- `OperationTable::inherit_filters` and `Findings::set_filter` preserve filters
  across page/refresh changes. Evidence-row identity remains UI-05 work.
- A refresh never clears the independent time scope or session scope.
- A `Change::LatestHistory` keeps the selected pane and scroll; `[` restores the
  whole cached newer page including its reading position.

Late responses:

- `Workspace::request` stamps the in-flight request with `generation`.
- Closing a history view, returning to Agents, closing a workbench surface or
  superseding a review context increments `generation`.
- `Workspace::finish` discards a response whose stamped generation differs and
  publishes `Discarded a late response for a closed or superseded view`.
- The single-pending/deferred-request queue and the loader thread remain the
  backpressure boundary. `event::poll(80 ms)`, resize and overlay redraws keep
  the loop responsive while reads are pending.

---

## 9. Lane integration

**All lanes**

- Do not edit `keymap.rs`, `workspace.rs` (shared dispatch), `workbench.rs`
  (shared shell), `theme.rs`, `palette.rs`, `help.rs` or `lib.rs` except through
  the UI-02 owner. Add models in your own module and request integration points
  in the pull request.
- Add a `keymap::Action` row for every new user-visible action, with one id, one
  context list and the keys you actually dispatch.
- Keep palette entries data-only and use exact identities for sessions, turns,
  evidence and snapshots.
- Never render `Name unavailable`. Derive a human name from title, native
  nickname, provider, role and project context; keep unknown values explicit in
  Details.

**UI-03 · command center and inspector**

- Replace the command-center help body with `keymap::help_sections(Context::CommandCenter)`.
- Wire Today/History to `AgentList::time_scope` (the shell default `All` is
  History). Confirm the tagged day boundary and timezone; the provisional shell
  rule is UTC-day + active/pending.
- Use the existing `Choose agent session` palette action and keep the scope
  filter independent of search/status/error filters.
- Preserve the selected `(harness_id, session_id)` through title changes,
  resize, filter and refresh. The shell already does this; keep it that way.

**UI-04 · prompts and retained history**

- Add prompt actions to the inventory and palette with exact request identity
  and scope. Help must return to the current prompt navigator or reader without
  moving the request or text position.
- Keep Escape precedence: compact chooser → reader → Prompts section → roster.
- Report retained caps, page limits and unavailable private content through the
  status/footer contract, not by inventing a selection.

**UI-05 · observability screens**

- Extend `keymap::ACTIONS` for Activity/MCP/Traces/Metrics/Usage/Findings/Host
  filters and detail actions; use `keymap::hint` in their footers.
- Preserve occurrence identity, active filter, page cursor and snapshot across
  drilldown/return. Report a missing occurrence and choose the nearest row by
  the same identity rule.
- Keep `/` ownership inside the focused filter. `F1` still opens help with the
  draft intact.

**UI-06 · review, workflow and saved work**

- Add form/button navigation with Tab/Shift+Tab order and Enter/Space
  activation. A dropdown consumes Escape before the form.
- Add palette rows for settings, alerts, retry, resume, save task and handoff.
  Use typed request identities and explicit unavailable reasons.
- Preserve unsaved drafts through help, resize and cancel; require explicit
  confirmation before discarding dirty edits.

**UI-07 · visual and responsive acceptance**

- Compare the shared help/palette/theme overlays at 120×32, 80×24 and 60×18.
- Keep one border per region, one clear selection cue and readable status text
  with `NO_COLOR`. Use `theme::PALETTE` semantic colours; do not hardcode RGB.
- Verify focus returns to the same control and the draft survives after resize.

---

## 10. Verification in this lane

Run from `rust/`:

```sh
cargo build --offline
cargo test --offline -p agenturm-ui
```

Focused tests added in this lane:

- `keymap::tests::python_root_destinations_stay_one_to_seven_and_extras_are_explicit`
- `keymap::tests::every_context_has_help_and_every_action_id_is_unique`
- `keymap::tests::focus_rules_keep_text_and_chooser_precedence`
- `keymap::tests::footer_hints_come_from_the_inventory`
- `palette::tests::*` (literal search, disabled rows, Escape ownership, 3 sizes)
- `help::tests::*` (scroll/return, 3 sizes)
- `theme::tests::preview_cancel_and_saved_selection_are_distinct` (F2 only;
  `Ctrl+P` is not appearance)
- `workspace::tests::ctrl_p_opens_the_searchable_palette_and_themes_stay_on_f2`
- `workspace::tests::session_chooser_keeps_exact_identity_and_owns_escape`
- `workspace::tests::help_preserves_text_drafts_scroll_and_returns_to_the_same_control`
- `workspace::tests::help_from_a_compact_chooser_and_export_form_returns_with_state_intact`
- `workspace::tests::palette_marks_current_view_actions_unavailable_without_a_selection`
- `workspace::tests::refreshing_a_pane_keeps_its_scroll_and_reports_a_missing_identity`
- `workspace::tests::late_responses_for_a_closed_context_are_discarded`
- `workspace::tests::ctrl_c_copies_public_evidence_but_keeps_interrupt_in_text_entry`
- `workspace::tests::today_and_session_scope_survive_refresh_without_moving_identity`
- `workspace::tests::palette_runs_a_clipped_destination_and_reports_it_in_help`
- `workspace::tests::palette_help_and_theme_overlays_stay_usable_at_the_floor`

## 11. Known gaps handed to dependent lanes

1. **Today boundary (UI-03).** The shell's `TimeScope::Today` is a disclosed
   provisional rule (UTC day + active/pending). The tagged Python boundary and
   timezone need the fixed-clock matrix.
2. **Evidence row identity (UI-05).** Pane/scroll/filter identity is preserved;
   selected operation/finding/point identity across pagination needs the owner's
   stable key and a missing-identity notice.
3. **Workbench palette coverage (UI-06).** Shared rows work over workflow,
   tasks and investigation; workflow-specific palette rows, button Tab order and
   dirty-editor confirmation still need the surface owner.
4. **Mouse equivalence (UI-03..UI-07).** The shell is keyboard-first. Any mouse
   route added by a screen must call the same action id.
5. **Visual matrix (UI-07).** Overlay spacing, semantic status styles,
   monochrome cues and CJK/emoji clipping need the paired visual pass.

## 12. Acceptance cases owned by this contract

- `1`–`7` reach the unchanged Python destinations; `9`/`U` stay available in
  help and palette when clipped from a narrow strip.
- `Ctrl+P` searches by label/detail/group; digits, brackets and `?` stay text;
  Enter runs exactly one enabled command; Esc cancels without side effects.
- The session chooser returns the exact source/session identity or reports a
  disappeared session; Escape belongs to the chooser.
- `F1`/`?` opens scrollable help from the command center, inspector, chooser,
  workbench and form; Esc/F1/`?` returns with the same focus, draft and scroll.
- `Ctrl+C` copies in public loaded-evidence focus and remains an interrupt in private
  or editing focus.
- Refresh, page change and resize keep the pane, scroll, filter and exact
  session; a vanished identity shows why and selects deterministically nearby.
- A late response for a closed view never replaces the current screen.
