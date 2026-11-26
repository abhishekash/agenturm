# UI-06 · Review, Workflow and saved work lane notes

23 November 2026 · Python reference **v0.2.8** (`2802d79930728bb331db1d4d718d6dea1c9c374d`)
· Rust branch `work/ui06-workflow` · worktree `/tmp/agenturm-ui06-workflow`

This lane owns Review evidence presentation, Workflow incidents/settings/resume,
saved-task search and handoff export. Shared shell files (`keymap.rs`,
`workspace.rs`, `workbench.rs`, `lib.rs`, `theme.rs`) and UI-05 evidence modules
were deliberately not edited. The hooks UI-02 must add are listed below.

## What changed

### Review

- `rust/agenturm-app/src/review_receipt.rs`
  - The receipt now exposes the reviewed-work sections the tagged Python Review
    screen shows: scope, summary and next inspection step, latest captured
    request, agent-reported update, recorded checks, recorded modifying
    operations, recorded errors, recorded usage, context events, an excerpt
    index, the older-evidence cursor and the raw recorded observations.
  - Checks are counted as passed/failed/unknown/unclassified. Every absent
    section is labelled explicitly (for example “No recognized validation
    records in the loaded evidence. This does not establish that no validation
    ran.”) instead of being implied from a task title or completion state.
  - Usage totals are exact for recorded input/output tokens and reported
    nanounit costs; a partly reported token set is marked partial. No price or
    token value is invented.
  - The excerpt index carries a stable `kind`, `label` and `source`
    (`event revision n`) for the checks, edits and errors that can seed
    `task draft --review-file` / save-as-task. This is metadata only: captured
    prompt or provider payload text is never reconstructed by the receipt.
  - Added `summary` and `excerpts` JSON fields for machine consumers while
    preserving the existing legacy shape when no format is requested. Copy
    (stdout) and export (`--output`) retain the same loaded-page scope; a page
    with older evidence is labelled partial.
- `rust/agenturm-app/src/cockpit_workflows.rs`
  - Added a version-linkage regression test: a decision note saved for one
    session evidence version remains attached to that version after a later
    admitted event advances the session receipt version. The saved and returned
    pages retain the exact source, session, snapshot and review version.

### Workflow

- `rust/agenturm-ui/src/workbench/workflow_alerts.rs`
  - Incident detail now begins with the **observed owner** (source · main
    session/worker · turn), a recorded approval-state line, and the exact
    delivery attempt and policy revision beside the existing delivery state.
  - Refresh is identity-aware: an incident that is no longer established keeps
    the nearest recorded selection and displays a stale notice; a changed
    history incarnation displays “Incident history changed · showing the latest
    page” instead of silently presenting the old selection as current.
  - Exact failed/unavailable deliveries remain the only retryable ones; a
    successful update is accepted only for the same authority, key, policy
    revision and next attempt.
- `rust/agenturm-ui/src/workbench/workflow_settings.rs`
  - Threshold drafts are protected by an explicit discard confirmation: the
    first Escape arms “Esc again to discard · Ctrl+S save”, typing cancels the
    arm, and only a second Escape leaves the screen. Desktop/snooze commands
    are unchanged and remain immediate receipts.
  - A desktop-only policy refresh preserves unsaved threshold drafts; a saved
    threshold response replaces them and reports “Thresholds saved”.
- `rust/agenturm-ui/src/workbench/workflow_resume.rs`
  - Added `WorkflowResume::unavailable_reason()` so a denied resume names the
    exact limitation (worker execution, unsafe/absent native identifier, or an
    unsupported provider) rather than only returning no command. Command
    rendering remains fixed to `codex resume` / `claude --resume` with a safe,
    provider-matched main-session identifier.
- `rust/agenturm-app/src/workflow_route.rs`
  - Local and remote route failures now state the explicit route boundary:
    local no-pane guidance tells the operator to configure `--route-file` for a
    receiver-host execution, and remote failures never guess or select a pane.
    Invalid provider/session identifiers are separated from route failures.
  - The existing explicit-route verification path (fake route, unique ancestry,
    re-read pane process, attach guidance) is retained and covered by tests.

### Saved tasks, decisions and handoff

- `rust/agenturm-app/src/task_cli.rs`
  - Added `agenturm task list --search <text>` over page-local titles, notes,
    workspace directory and exact linked source/session/worker/turn identity.
    Search is case-insensitive, never reads captured content, and reports
    `"page_local": true` so callers do not claim whole-history coverage.
    Empty search text is treated as no filter.
- `rust/agenturm-app/src/handoff_export.rs`
  - Added regression coverage for export collision (an existing Markdown/HTML
    pair is preserved and the second write fails with `AlreadyExists`; no
    staging directory is left behind) and for current-versus-superseded
    decision history, including `History` and `Supersedes n` labels.
- `rust/agenturm-app/tests/task_cli.rs`
  - Added an end-to-end test for page-local task search by title, note and exact
    linked run identity, empty results and the empty-search no-filter case.
- Existing task revision/CAS, decision supersession, brief, Git snapshot/diff,
  handoff rendering, private export and restore tests continue to pass.

### Preferences and private display

- `rust/agenturm-ui/src/workspace/review_visibility.rs`
  - Saving the internal-review display preference now syncs the parent
    directory after the atomic rename. Tests reject non-canonical (`TRUE`,
    CRLF), oversized and symlinked preference files; saving replaces a symlink
    rather than writing through it. The preference remains off by default and
    scoped to an enabled preferences directory.

### Exact file set

```text
rust/agenturm-ui/src/workspace/review_visibility.rs
rust/agenturm-ui/src/workbench/workflow_alerts.rs
rust/agenturm-ui/src/workbench/workflow_resume.rs
rust/agenturm-ui/src/workbench/workflow_settings.rs
rust/agenturm-app/src/cockpit_workflows.rs
rust/agenturm-app/src/handoff_export.rs
rust/agenturm-app/src/review_receipt.rs
rust/agenturm-app/src/task_cli.rs
rust/agenturm-app/src/workflow_route.rs
rust/agenturm-app/tests/task_cli.rs
```

## Verification run in this worktree

```sh
export RUSTUP_HOME=/tmp/agenturm-rustup
export CARGO_HOME=/tmp/agenturm-cargo
export PATH=/tmp/agenturm-rustup/toolchains/1.100.0-x86_64-unknown-linux-gnu/bin:$PATH

cargo build -p agenturm-ui -p agenturm-app --offline
cargo test  -p agenturm-ui -p agenturm-app --offline
```

Final results:

- `cargo build -p agenturm-ui -p agenturm-app --offline`: success.
- `agenturm-ui` library suite: **79 passed, 0 failed**.
- `agenturm-app` library suite: **177 passed, 0 failed**.
- `agenturm-app` integration suites: **81 passed, 0 failed** across the test
  binaries in the final run.
- `rustfmt --edition 2024 --check` on every changed Rust file: clean.

One unrelated `automatic_retention` integration case failed once with a broken
pipe/reset while the workspace cache lock was contended; it passed standalone
and in the final full run. No changed module is involved in that test.

## What remains

These are real gaps, not accepted differences. Each needs shell or app work
that this lane must not land in shared files.

1. **Review screen actions.** The cockpit Review route still exposes retained
   panes, edit list, older evidence, acknowledgement (`y`), decisions (`n`) and
   traces (`t`), but not the tag-parity Copy (`c`), Export (`x`), Save task,
   Excerpt, Draft (`d`), Git (`g`) or Map workspace (`m`) actions. The receipt
   model for Copy/Export and the excerpt data now exist; the shell dispatch and
   forms do not.
2. **Turn-scoped Review.** `Ctrl+R` opens session Review and `HistorySelection::Turn`
   has no Review section. Tagged Python can open exactly scoped checks, edits and
   errors for a turn; Rust still needs the exact turn/worker Review route.
3. **Save task from Review.** `task draft --review-file` and `approve-draft`
   already accept a review receipt, but the cockpit has no action that writes a
   bounded review draft or opens the task editor from Review.
4. **Workflow incident review acknowledgement.** `Seen` persists per pending
   request. Completion/failure incidents have no persisted local “reviewed”
   acknowledgement, so an incident cannot be marked reviewed independently of
   delivery retry. The ledger needs an incident acknowledgement API before the
   UI can honestly show one.
5. **Task search in the cockpit.** The CLI supports `task list --search`; the
   shared task list has no `/` search field, clear action or page-local
   disclosure. The `WorkCommand` variant also does not carry a query yet.
6. **Decisions/Brief/version selector.** The app CLI supports `brief`, `decide`,
   `decisions`, supersession and expected revisions, but the cockpit has only
   task title/note editing. A version chooser, dirty-editor confirmation,
   decisions list and brief fields need new `WorkCommand` variants and shared
   workbench dispatch.
7. **Handoff preview.** The cockpit exports a saved handoff (`x`) but has no
   preview/copy of `task handoff --format markdown|html|json` or a destination
   form that stays open on collision. The export layer already returns the
   explicit `AlreadyExists` failure.
8. **Provider edit/MCP detail.** Review edit reveal is limited to the existing
   whole-session Codex browser and reports “Edit preview is unavailable from
   this reader” otherwise. Remaining provider proofs and detail belong to FD2
   and must stay behind retained proofs plus valid grants.
9. **Cross-host route discovery.** Routing is explicit only. There is no route
   browser or hidden discovery; the remote cockpit must be reopened with the
   configured route file. That is intentional and must remain so.
10. **Receiver acceptance.** The Python receiver Review path and current
    CF7/paired terminal acceptance are separate gates. This lane added unit and
    app-level tests only; no installed Python-versus-Rust paired capture is
    claimed here.

## Exact shared-shell hooks for UI-02

### Shared action/key contract

Suggested keys follow the tagged Python guide; UI-02 owns the final keymap and
must publish them through the same action inventory used by dispatch, footer and
`F1` help.

| Context | Key | Hook |
| --- | --- | --- |
| Review | `[` / `]` / `l` | Keep existing newer/older/latest; label “Earlier” on `]`. Preserve `snapshot_id`, page cursor, selected edit and scroll. |
| Review | `c` | Copy the current exact review receipt (`ReviewFormat::Text/Markdown`) to the clipboard; no export directory. |
| Review | `x` | Open the existing export form with `ReviewOutput { format: Some(..), directory: Some(..) }`; show the app’s exact success path or `AlreadyExists` failure. |
| Review | `Tab` / `Shift+Tab` | Reach `Earlier`, `Refresh`, `Copy`, `Export`, `Save task`, `Excerpt`, `Back`; Enter/Space activate. |
| Review | `R` | Re-read the same exact scope/snapshot; discard late responses for a closed/changed review. |
| Review | `y` | Existing mark-reviewed; acknowledge only this evidence version and keep the selected edit. |
| Review | `n` | Existing review decisions; return to the same version and selection. |
| Review | `s` (or a form) | Save task: write a bounded review draft and open the task editor, or invoke `task draft --review-file` through a new command. |
| Review | `Esc` | One layer at a time: excerpt/chooser → edit list → Review → inspector/roster, preserving the exact version and selection. |
| Workflow | `w` | Inbox focus on the queue; immediate Up/Down. |
| Workflow | `Tab` | Incidents; preserve selected alert by `(id, scope, evidence)`; the alert model already reports stale notices. |
| Workflow | `r` / `s` | Seen for the exact pending request; keep the cursor and the recorded approval state. |
| Workflow | `l` | Locate; show the route guidance verbatim, never change selection on denial. |
| Workflow | `c` | Copy the reviewed resume command; preview returns to the same row. |
| Workflow | `T` | Retry only the exact failed/unavailable delivery attempt/authority. |
| Workflow | `S` | Settings; Escape now arms a discard confirmation when threshold drafts are dirty. |
| Tasks | `/` | Task search using the new page-local `task list --search`; empty query clears; disclose page-local coverage and preserve selected task id. |
| Tasks | `v` / `Enter` | Decisions/version chooser; add/supersede with expected revision; dirty editor requires explicit discard. |
| Handoff | `x` | Preview/copy/export with format and destination; keep the panel open on collision and name the existing path behavior. |
| Copy/export | `Ctrl+C` | Copy selection only; `x` exports the loaded/selected scope. Keep text fields’ literal typing and chooser-first Escape. |

### Dispatch additions UI-02 must make

1. Add Review Copy/Export/Save-task/Excerpt/Draft/Git/Map commands to
   `WorkCommand` (or reuse `WorkCommand::ExportView` with a review document) and
   route them from `Workspace::key`. The app already owns receipt rendering
   (`review_receipt::render`) and task draft/decision CLI paths.
2. Add a turn-scoped Review `HistorySelection` (or make `SessionView::Review`
   carry an optional exact turn/worker) and open it from the inspector and the
   turn picker. Preserve the exact session/turn on return.
3. Add a task-search field and a Decisions/Brief sub-screen in `workbench.rs`,
   with `WorkCommand` variants carrying the page cursor, query and expected
   revision. Keep the query and page-local limits visible.
4. Extend the Workflow help/footer for the new settings discard behavior and
   for “Observed owner / approval state / delivery attempt” in incident detail.
5. Add palette entries for Review, Workflow, Incidents, Saved tasks, Task
   search and “To review” scope. Keep `F2` themes; ensure `Ctrl+P` restores the
   originating control and drafts.
6. Surface `review_receipt`’s `summary`, `excerpts` and partial-page labels in
   the Review screen and export form so Copy/Export never imply complete
   history or verified correctness.

## Pluggable-seam implications

- The review receipt is a provider-neutral projection of retained observations.
  It classifies checks/edits/errors/usage without switching on a provider ID.
- Provider edit and MCP payload detail remain behind the existing
  `RevealReviewEdit` / `RevealOperation` commands, FD2 proofs and valid grants.
  Missing detail is labelled unavailable; no provider-specific UI branch was
  added.
- Workflow routing is explicit configuration only. The local route, remote
  route file, unique-pane proof and “Seen only acknowledges locally” wording are
  preserved. No hidden discovery, pane guessing or provider-hardcoded attach
  path was introduced.
- Cross-host attach guidance lives in `workflow_route` and is transport-neutral;
  the configured route words are treated as the operator’s own authenticated
  transport.
- Task search and handoff export operate on operator-authored metadata and
  exact recorded identities, not captured body text.

## Residual risks

- The final acceptance still requires the installed Rust candidate paired with
  the tagged Python build at 120×32, 80×24 and 60×18, including real terminal
  keys and receiver cases. This lane’s evidence is unit/integration and
  component rendering only.
- The Review screen remains shared-shell-owned; the receipt and incident models
  are ready, but the visible parity of Copy/Export/Save task/Excerpt cannot be
  claimed until UI-02 lands the dispatch and forms.
- Incident acknowledgement persistence is a ledger feature, not a UI flag; do
  not add a local-only “reviewed” badge that disappears on restart.
