# Core feature development: delegated test plan

Updated 26 November 2026. The user assigned this implementation session to
**core feature development** and a separate agent to test execution. This plan
is the handoff contract. It supplements Q1–Q10 in the
[remaining-work plan](../rfcs/rust-migration-remaining-plan.md).

## Synchronized prior execution and current handoff

The separate test agent's [Q1–Q10 execution report](rust-migration-test-plan-results.md)
uses frozen `18aa702` (PR #381, schema 21). Its 833-test/optimized baseline
passes are preserved and reviewed, but do not qualify the newer CF1–CF5 source
`3135060` (PR #382, schema 22).

Current PR #382 CI stopped before Rust tests/SDK follow-ups on an unused
`ObservationStore` import in a Store test. The synchronization fix removes it;
scoped Store lib/test Clippy and a genuine metric-point identity upgrade
regression pass. The reference Review fixture's `--help` now documents and can
show the required import-path setup without loading the reference first.

The PR #394 standalone-fixture `--locked` failure was caused by a stale fixture
lockfile. PR #395 merged the offline lockfile correction as `6c9ec8e`; the exact
fixture build passes locally with `--offline --locked`, and fresh merged CI is
running. The full PR #395 coordinator status remains separate from this plan's
delegated source and artifact tests.

For the next core broad execution use the current integrated main head, record its
exact SHA and freeze artifacts. Keep this matrix pending until those actual
CF1–CF5 and related Review identity scenarios run. Add an explicit shell loop
(`for script in scripts/*.sh; do bash -n "$script" || exit; done`) rather than
`bash -n scripts/*.sh`, which checks only one input. The separate-UID executor
context and known staging helpers are documented in the results report.

## Ownership and execution

- **Feature developers:** implement the feature slices below; run only focused
  compile/parser/state checks that protect the changed behavior; provide source
  head, exact patch scope, focused result and known limitations.
- **Test execution agent:** own joined workspace/Clippy, actual native collection,
  broad terminal/privacy journeys, schema 21-to-22 receipt-clock upgrade, SDK matrix and artifact/KVM
  qualification. Record executable hashes and results in this plan or linked
  validation records. Return defects with exact scope and reproduction.
- **Integration owner:** combine reviewed changes, publish the exact source head
  for testing and fix returned defects. Do not duplicate the test agent's
  campaigns or call an implemented feature release-qualified prematurely.

## AP1 follow-up: `task create --snapshot`

Implementation is on `feature/task-create-snapshot`. The flag is opt-in: a
plain task create must remain metadata-only, while the flag captures Git only
from the supplied local `--workspace`. If workspace capture or persistence
fails after task creation, return the saved task and an explicit
`snapshot_error`; do not hide the task ID or imply the snapshot succeeded.

| Scenario | Required result | Status |
| --- | --- | --- |
| Create with `--snapshot` in a clean Git workspace | Saved task includes the captured repository, head, branch and change state; returned task revision reflects the saved capture. | PASS: integration fixture verifies top-level task ID, revision 2, repository/head/branch, zero change counts and persisted capture. |
| Create with `--snapshot` in a dirty workspace | Capture remains read-only, excludes untracked file contents, and reports tracked/staged/unstaged state without claiming agent attribution. | PASS: staged/unstaged/untracked counts are exact; worktree status and Git index are unchanged; no private file body appears in output. |
| Create without `--snapshot` | No Git capture occurs; existing task creation result and revision remain unchanged. | PASS: task creation succeeds for a non-Git directory, revision remains 1, and later show has no Git snapshot. |
| Capture unavailable or workspace missing | Task remains saved and result includes its ID plus a clear `snapshot_error`; no false success claim. | PASS: missing repository and omitted workspace both preserve task ID/revision and return top-level `snapshot_error`. |

Test-owner validation ran on `/tmp/agenturm-task-create-snapshot`, branch
`feature/task-create-snapshot`, source base `afbdbdcf28f3e31f34eb3363c05b997cbad68526`
plus the dirty implementation and test changes. The focused command
`cargo test --offline --locked -p agenturm-app --test task_cli task_create_snapshot
-- --nocapture` passed all 3 new tests. The app library command
`cargo test --offline --locked -p agenturm-app --lib -- --quiet` passed 189
tests. Strict lint passed with
`cargo clippy --offline --locked -p agenturm-app --all-targets -- -D warnings`;
`cargo fmt --all -- --check` also passed. These are local Git fixtures, not
release-artifact qualification. The merged `18aa7026be1041b93a0b8517874f5b518da44777`
(PR #381) and its existing 833-test / terminal-KVM results remain historical
baselines, not results for this slice.

## Workflow incident history toggle (`h`) follow-up

Implementation review and focused test-only regressions ran on
`feature/rust-workflow-history-toggle`, base `fd398c1`, with the owner’s dirty
footer change and test-only additions in
`rust/agenturm-ui/src/workbench/workflow_alerts.rs`. No production code was
changed by the test lane. The acceptance matrix is:

| Scenario | Expected result | Status |
| --- | --- | --- |
| Completion older than 10 days | Hidden initially; `h` reveals it; second `h` hides it. | PASS: regression uses an observation one nanosecond older than the sampled cutoff. |
| Recent completions within the cutoff | Visible both before and after toggling. | PASS: regression checks rows 60 seconds and one day newer than the cutoff. Exact equality to a real-time cutoff is not separately tested. |
| Old Failure, Interruption, Tokens, USD threshold and Elapsed threshold | Remain visible with history toggle off. | PASS: regression places each kind far outside the completion window. |
| Selection on a visible row / selection on a row hidden by `h` | Preserve identity when visible; when hidden select first visible row, with stable deterministic behavior. | PASS: focused tests verify selection survives toggles and falls back to the first visible row only when hidden. |
| Refresh/reorder/same-id row from another history or incarnation | Preserve exact matching alert only in the same history incarnation; otherwise clear stale detail and fall back safely. | PASS: reorder preserves the selected exact alert/detail on the same history; a different history clears detail and falls back. |
| Enter detail, press `h`, Esc | Detail remains scoped; `h` does not change the list while detail is open; Esc returns to the selected incident. | PASS: focused regression exercises the detail and return path. |
| Footer at 120×32, 80×24, and 60×18 with and without retry | Toggle state and available actions remain readable without clipping. | PASS: rendered-frame regression verifies all controls in retry/no-retry and default/revealed states. Longest hint line is 51 terminal cells; all tested lines fit 60 columns. |

Footer cell widths were measured with Unicode East Asian width: the retry
footer lines are 44 and 46 cells in the default state, or 44 and 51 cells with
older completions revealed; no-retry lines are 47/36 cells by default and
25/51 cells after reveal. All fit at 60 columns.

Focused
`cargo test --offline --locked -p agenturm-ui workbench::workflow_alerts::tests -- --nocapture`
passed 5/5. Full UI library tests
`cargo test --offline --locked -p agenturm-ui --lib` passed 80/80. Strict
`cargo clippy --offline --locked -p agenturm-ui --all-targets -- -D warnings`
and `cargo fmt --all -- --check` passed. These are local UI tests, not an
installed paired Python/Rust journey; that broader parity remains pending.

## Searchable action palette (`Ctrl+P` / `:`) follow-up

PR #396 already integrated the shared searchable command/session palette,
context-aware action registry and F2 theme picker. The previous standalone
palette prototype's 88-test run on `fd398c1` is historical and does not qualify
the rebased follow-up. This branch only adds `:` as an alternate launcher and
within-word fuzzy search over the existing command entries; no second palette
implementation or command registry is introduced.

| Scenario | Expected result | Status |
| --- | --- | --- |
| `:` and Ctrl+P launch / text-entry focus isolation | Both launch the existing command palette outside text entry; while the agent search field is focused, neither opens the palette and `:` remains literal input. | PASS: `cargo test --offline --locked -p agenturm-ui workspace::tests::colon_opens_commands_but_stays_literal_in_text_search --quiet` (1 passed). |
| Exact and fuzzy theme search | `theme` finds Themes without cross-field false matches; `thms` remains a valid within-word fuzzy query. | PASS: `cargo test --offline --locked -p agenturm-ui palette::tests --quiet` (10 passed), including exact `theme` and fuzzy `thms`. |
| Session chooser and selection return | Ctrl+P retains the shared session chooser; selecting a session preserves identity, and Escape returns to the prior context. | PASS: `cargo test --offline --locked -p agenturm-ui workspace::tests::ctrl_p_opens_the_searchable_palette_and_themes_stay_on_f2 --quiet` and `cargo test --offline --locked -p agenturm-ui workspace::tests::session_chooser_keeps_exact_identity_and_owns_escape --quiet` (1 passed each). |
| F2 theme routing and footer | Themes remain on F2 and footer hints come from the shared key inventory. | PASS: the Ctrl+P/F2 test above and `cargo test --offline --locked -p agenturm-ui keymap::tests::footer_hints_come_from_the_inventory --quiet` (1 passed). |
| Full UI library and static checks | Rebased shared palette compiles and all UI library tests pass with strict Clippy and formatting. | PASS on final amended HEAD `8ee7f87`: `cargo test --offline --locked -p agenturm-ui --lib --quiet` (161 passed); `cargo clippy --offline --locked -p agenturm-ui --all-targets -- -D warnings`; `cargo fmt --all -- --check`. |

Focused qualification completed on the rebased source head. These unit tests do
not establish installed paired Python/Rust parity or palette PTY journeys;
those remain in UI-Q. The full workspace build/check is tracked separately.

## Linux bundle PTY filter assertion follow-up

PRs #398/#399 reported `cockpit did not show confirmed filter for
bundle-pi-session`. The captured terminal showed the expected command center,
`/ bundle-pi-session`, one selected `Pi session` row, and `Project unavailable
1`. The failing assertion was the helper's `Enter inspect` footer check:
filtered rosters display `Enter / Esc search` even after Enter finishes search
editing. Enter remains handled by the selected-row inspector route after
editing ends; the helper's subsequent Enter and exact Overview check cover
that behavior. This is a stale PTY expectation, not evidence of a product
navigation regression. `scripts/validate_rust_default_cockpit.py` now accepts
the filtered-search footer and either `Project unavailable 1` or `No recorded
project 1`, while retaining the exact search, selected-row and one-project
count checks. Linux bundle PTY reruns are still required to close failed CI
results.

## Scoped CI fix confirmations — 26 November 2026

These local checks confirm the specific follow-up commits only. Full PR CI and
broader feature qualification remain pending.

| PR / commit | Focused result | Exact local checks |
| --- | --- | --- |
| #398 / source `54483d8`, filter fix `d66af1a`, Inspector fix `303f654` | PASS: strict workspace Clippy; Pi harness 28 tests; Pi edit reveal 2 tests; Claude MCP reveal 2 tests. Validation-only fixes pushed. | `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`; `cargo test --offline --locked -p agenturm-pi-harness --quiet`; `cargo test --offline --locked -p agenturm-content --lib pi_edit_reveal --quiet`; `cargo test --offline --locked -p agenturm-content --lib claude_mcp_reveal --quiet`. |
| #399 / source `07e3691`, filter fix `e9a823a`, Inspector fix `8474a5d` | PASS: locked standalone example test, including host compatibility (1 passed); validation-only fixes pushed. | `cargo test --locked --manifest-path examples/rust_prompt_signals_extension/Cargo.toml`. |
| #400 / source `2bdc1c5`, filter fix `505baaa`, Inspector fix `64fbf6a` | PASS: exact task CLI regression sees documented `agenturm task add` (1 passed); validation-only fixes pushed. | `cargo test --offline --locked -p agenturm-app --test task_cli tasks_refuse_missing_or_unrelated_history_without_creating_workspace -- --exact`. |
| #402 / source `e0d03f2`, filter fix `f834074`, Inspector fix `f6ff4d9` | Helper journey check passes; validation-only fixes pushed. A full verifier smoke run exited 0 on an available 0.3.9 archive, but that archive predates this branch and is not #402 artifact provenance or qualification. | Synthetic two-session helper journey; `python3 -m py_compile scripts/validate_rust_default_cockpit.py`; `git diff --check`; `scripts/verify-rust-linux-bundle.sh /tmp/agenturm-039-workflow-release-candidate/agenturm-0.3.9-linux-x86_64.tar.gz`. |

The latest script-only Inspector correction was applied and pushed to #398
(`303f654`), #399 (`8474a5d`), #400 (`64fbf6a`), #401 (`e921b0b`), and #402
(`f6ff4d9`). On all five branches the focused synthetic two-session journey
passed: exact ID search yields exactly one selected row; its displayed human
title must equal the Inspector's `Session name`; the expected `Provider` and
`Project No recorded project` fields must match exactly; Escape returns to the
same selection. The helper strips the Inspector's `│` box border when reading
field values. Both `Project unavailable` and `No recorded project` group
labels are accepted at exact counts one and two. `python3 -m py_compile` and
`git diff --check` pass for each worktree. Full current-bundle verifier runs
remain pending in CI for #398–#402; the stale #402 0.3.9 smoke artifact is not
qualification evidence.

These results do not replace the running/required PR CI, Linux bundle PTY rerun,
or remaining feature and artifact acceptance checks.

## Lockfile and metric-verifier follow-ups — 26 November 2026

Each PR received a follow-up lockfile correction and a validation-only change
to `scripts/validate_rust_metric_descriptions.py`. The verifier now reads
exactly one guarded positive integer `SCHEMA_VERSION` from that branch's
`rust/agenturm-store/src/lib.rs`, rejects missing, duplicate or out-of-range
values, and compares SQLite `PRAGMA user_version` to the parsed value. It does
not weaken the schema assertion.

| PR | Lockfile correction | Schema-verifier correction | Source schema |
| --- | --- | --- | ---: |
| #398 | `0de2acc` | `1c20f8b` | 26 |
| #399 | `07e3691` | `3c822ee` | 24 |
| #400 | `1ce358f` | `3202fc4` | 24 |
| #401 | `a8de0cd` | `6413e96` | 24 |
| #402 | `c6fb111` | `1260295` | 24 |

The following focused source checks were run on the listed earlier source
heads; they are recorded for context and do not qualify the later lockfile
commits. Full CI reruns against all five updated heads are pending.

| PR | Prior targeted check and result |
| --- | --- |
| #398 (`54483d8`) | `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`; `cargo test --offline --locked -p agenturm-pi-harness --quiet` (28 passed); `cargo test --offline --locked -p agenturm-content --lib pi_edit_reveal --quiet` (2 passed); `cargo test --offline --locked -p agenturm-content --lib claude_mcp_reveal --quiet` (2 passed). |
| #399 (`07e3691`) | `cargo test --locked --manifest-path examples/rust_prompt_signals_extension/Cargo.toml` (host compatibility test passed). |
| #400 (`2bdc1c5`) | `cargo test --offline --locked -p agenturm-app --test task_cli tasks_refuse_missing_or_unrelated_history_without_creating_workspace -- --exact` (1 passed). |

## UI-05 focused handoff — trace-linked session evidence — 26 November 2026

PR #404 (`feature/rust-trace-linked-evidence`, commit `11bee013`) changes the
session Evidence trace detail rows in
`rust/agenturm-app/src/session_evidence/table.rs`. This is a focused
implementation handoff, separate from UI-Q paired Python/Rust journey and
screen acceptance. UI-Q remains pending.

| Acceptance case | Focused result |
| --- | --- |
| Preserve source, session, and trace identity in the selected trace details | PASS by source review: the trace row emits `Trace`, `Source`, and `Session` details from the selected trace context. |
| Preserve target source and linked operation identity per span | PASS by source review: span detail includes `target` and `linked operation` fields when explicitly present. |
| Render roots before their descendants and retain multiple roots | PASS by source review: roots are visited in input order, with each reachable subtree traversed before the next root. |
| Represent a parent omitted by the bounded page as detached | PASS by source review: rows with an unresolved parent are labeled `Detached span · parent not on this page`; their children remain visible. |
| Keep cyclic or malformed duplicate-ID rows visible without looping | PASS by source review: traversal tracks visited row indices; duplicate span IDs bind parent lookup to the first row deterministically, while cycles/unvisited rows are emitted through the detached fallback. |

Focused test evidence on the exact PR head:

| Check | Result |
| --- | --- |
| `cargo test --offline --locked -p agenturm-app --lib session_evidence::tests::trace_table_reports_only_an_exact_root_spans_recorded_tokens -- --exact` | PASS, 1 test. Existing exact-root token/trace-pane behavior remains green. |
| `cargo test --offline --locked -p agenturm-app --lib` | PASS, 189 tests. |

Limitation: the current app test suite has no direct regression test for the new
hierarchical span-row rendering cases (multiple roots, a bounded-page detached
parent, parent cycles, or duplicate IDs). Those cases above are code-review
findings, not runtime test results; add focused fixture coverage before treating
those edge cases as fully qualified. No UI-Q paired journeys, terminal-size
matrix, or installed-bundle checks were run in this focused handoff.

## UI-05 follow-up — trace-to-operation navigation — 26 November 2026

PR #405 was merged as `d97be0f` (`feat: navigate from traces to linked
operations (#405)`). Its tree hash matches feature commit `ac1bac0` exactly.
The change adds an `o` jump from a selected trace span to its linked operation.
The implementation touches `rust/agenturm-app/src/session_evidence.rs`,
`rust/agenturm-app/src/session_evidence/table.rs`,
`rust/agenturm-ui/src/evidence_view.rs`, `rust/agenturm-ui/src/lib.rs`,
`rust/agenturm-ui/src/operation_table.rs`, and
`rust/agenturm-ui/src/workspace.rs`.

| Acceptance case | Status |
| --- | --- |
| Resolve by typed target source, session, and operation ID; additionally match reported worker and turn | PASS: focused UI regression selects the exact typed row even when operation IDs collide across workers and sources. |
| Fail closed for zero, multiple, or filter-hidden matching operations with a clear message | PASS: focused regressions assert each specific error. |
| Select the exact operation row | PASS: focused regression checks the selected row's worker and turn identity. |
| After collapsing detail, Escape restores the originating Traces pane, row, and filter position | PASS: workspace key-sequence regression opens the filtered trace row, chooses a linked span, enters and collapses operation detail, then Esc returns to the same Traces row and filter. |

Focused tests and checks ran on test-only PR #407,
`test/trace-span-picker-acceptance`, based on merged main `4948ac5` (PR #406).
Test commit `bc4aa6eb6c1ababebe3c64cd35cac63747266952` was merged as
`fa74cb6b975391db8b7557bb29fc1f16e9699ac6`. It changes only tests in
`rust/agenturm-ui/src/operation_table.rs` and
`rust/agenturm-ui/src/workspace.rs`; it adds typed operation target cases and
the chooser/cancel/detail-collapse/Escape key sequence.

| Command | Result |
| --- | --- |
| `cargo test --offline --locked -p agenturm-ui trace_target -- --nocapture` | PASS, 2 focused tests: exact typed selection and zero/ambiguous/filter-hidden failures. |
| `cargo test --offline --locked -p agenturm-ui trace_span_picker_cancel_and_jump_restore_exact_filtered_origin -- --nocapture` | PASS, 1 workspace key-sequence test; confirms picker labels, cancellation preserves origin, chosen operation is exact, detail collapse, and Escape restores filtered trace row/position. |
| `cargo test --offline --locked -p agenturm-ui --lib` | PASS, 168 tests including the three new regressions. |
| `cargo clippy --offline --locked -p agenturm-ui --all-targets -- -D warnings` | PASS. |
| `cargo fmt --all -- --check` | PASS. |
| `cargo test --offline --locked -p agenturm-app --lib session_evidence::tests::trace_table_reports_only_an_exact_root_spans_recorded_tokens -- --exact` | PASS, 1 test on merged main `4948ac5`. |
| `git diff --check` | PASS. |

No UI-Q paired Python/Rust journeys, full terminal-size matrix, or
installed-bundle checks were run. Keep this focused navigation result separate
from broader UI parity qualification.

## FD1/FD2 audit note — Claude/Pi evidence and private edit proof — 26 November 2026

This section records the feature audit and the follow-on test-only handoffs.
No product implementation was changed by these test tasks.

| Slice / evidence | Status and next validation |
| --- | --- |
| FD1 Claude async worker links | Implemented and tested. Keep native exact Claude check/lint/build attribution unknown: available `is_error` evidence is generic and does not prove a native command result. |
| FD1 Pi parent/child semantics | Native evidence has no same-session parent/child field. Distinct-session `parentSessionId` and sibling fork handling are implemented; do not infer same-session ancestry. |
| FD2 Claude Review edit detail | Implemented on current main with exact retained source/session/turn/operation binding and current-grant gates. Feature work is complete; Claude-specific proof/current-grant regression cases remain delegated. |
| FD2 exact Claude edit proof | Pending test coverage: accept only the selected Claude edit under its exact retained source, session, turn, and operation identity; reject another edit sharing a path or display label but differing in one identity component. |
| FD2 current authorization grant | Pending test coverage: require the current grant at reveal and deny absent, stale, replaced, or revoked grants; recheck after staging and before text display. |
| FD2 private-content handling | Pending test coverage: failed or cancelled proof must not reveal text; verify no private text enters row labels, generic metadata, logs, or the UI when the grant changes before delivery. |
| PR #408 Pi Review edit route | Implemented on `abac29c`: exact Pi `FileChange` occurrences are projected into the existing `RevealReviewEdit` path, using the independent `PiEditDetail` grant and native event/index proof. |
| Pi Review edit proof and grant | Test-only PR #409 (`7ffe3d8`) is merged. App row preserves exact source/session/turn/operation/event/revision; missing session/turn/operation and wrong native event prefix produce no route. Content proof tests reject wrong session/operation/index, changed summary, stale receipt and unsafe inputs. Pi grant test verifies default denial, separation from Codex edit grant, revoke denial and fresh regrant. These focused tests do not replace installed private-source/UI privacy acceptance. |
| Pi MCP payload detail | Implemented by PR #410 (`c715b5d`) with a distinct `PiMcpDetail` grant and exact native payload proof route. Focused test-only regressions are in progress on current main; see the CF12 table below. |


Test-only PR #409 (`test/pi-review-edit-proof`, head
`7ffe3d879bf3c0f99b03011b1522c785e90bd5f2`) is merged into `main`; it changes
only `rust/agenturm-app/src/inspector_activity.rs` and
`rust/agenturm-store/src/lib.rs`.

| Focused command on base `abac29c` | Result |
| --- | --- |
| `cargo test --offline --locked -p agenturm-app --lib inspector_activity::tests::pi_review_edit_route_keeps_exact_private_occurrence_identity -- --exact` | PASS, 1 test. |
| `cargo test --offline --locked -p agenturm-content pi_edit_reveal -- --nocapture` | PASS, 2 native proof tests. |
| `cargo test --offline --locked -p agenturm-store pi_edit_detail_grant_is_independent_default_denied_and_revocable -- --nocapture` | PASS, 1 test. |
| `cargo test --offline --locked -p agenturm-store v24_to_v26_preserves_legacy_grants_and_separates_pi_edit_from_claude_mcp -- --nocapture` | PASS, 1 test. |
| `cargo clippy --offline --locked -p agenturm-app --all-targets -- -D warnings` | PASS. |
| `cargo fmt --all -- --check`; `git diff --check` | PASS. |

## CF12 — Pi MCP private payload proof and grant

PR #410 (`c715b5d`) adds `PiMcpDetail`; it remains a supported private-detail
feature. Test-only PR #411 (`test/pi-mcp-payload-acceptance`, head
`4c7fe6e`) is open against current main `fcb9768` (after #409). The focused
regressions cover an exact
admitted assistant row/cursor, session, operation ID, content index, native
call ID, tool-name shape and timestamp; stale/missing receipt, absent row,
duplicate row, and worker evidence fail closed. Store tests cover an
independent default-off Pi MCP grant, separation from PiEdit and Codex MCP
grants, wrong source/fingerprint denial, revocation and regrant, and the
v26→v27 migration preserving existing Codex MCP/Pi Edit grants while leaving
Pi MCP unapproved.

| Focused command on base `fcb9768` | Result |
| --- | --- |
| `cargo test --offline --locked -p agenturm-content pi_mcp_reveal -- --nocapture` | PASS, 2 native proof tests. |
| `cargo test --offline --locked -p agenturm-store pi_mcp_detail_grant_is_independent_default_denied_and_revocable -- --nocapture` | PASS, 1 grant test. |
| `cargo test --offline --locked -p agenturm-store v26_to_v27_adds_pi_mcp_grant_without_changing_existing_grants -- --nocapture` | PASS, 1 migration test. |
| `cargo test --offline --locked -p agenturm-content --lib` | PASS, 8 tests. |
| `cargo test --offline --locked -p agenturm-store --lib` | PASS, 152 tests after updating the existing v24 migration assertion from v26 to v27. The first full run exposed that stale version expectation; no product defect. |
| `cargo clippy --offline --locked -p agenturm-content -p agenturm-store --all-targets -- -D warnings` | PASS. |
| `cargo fmt --all -- --check`; `git diff --check` | PASS. |

Still pending: end-to-end host authorization through the private reveal
command with a real retained Pi JSONL fixture; explicit source-configuration
fingerprint replacement and worker-null selection checks through the Store
route; and a captured UI/log assertion proving private arguments never appear
in generic row metadata or logs. The core proof unit test is not installed
artifact or complete privacy qualification.

OpenCode MCP payload remains pending, not retired. A native audit found 9,471
standalone tool parts and 18,331 assistant messages, with zero `mcp__` records;
the adapter has only coarse metadata and no bounded input reference or
commitment. No payload feature should be inferred until a native OpenCode
record provides a testable argument-retention shape.

The Claude-specific cases are delegated validation requirements, not feature
development tasks. Existing generic Pi edit and Claude MCP reveal checks do not
substitute for these Claude Review edit proof/current-grant cases.

## Codex native outcome attribution audit — 26 November 2026

The read-only November 2026 rollout audit covered 284 Codex rollout files and
33,683 native `CommandExecution` completion rows. These rows contain exact item,
thread, turn, status, and exit identity. The current parser handles supported
direct test/lint/build outcomes. One `exit_code = -1` completion remains generic;
three `turn_aborted` rows have no turn identity and remain session scoped.
Compound JS/exec and collaboration outputs lack structured result and child
IDs, so there is no safe additional outcome implementation to claim from this
evidence. Codex harness compile passed. No source PR was needed for this audit.
| #401 (`000549b`) | `cargo test --offline --locked -p agenturm-ui workbench::workflow_alerts::tests -- --nocapture` (5 passed); full UI lib (163 passed); strict UI Clippy and fmt check passed. |
| #402 (`8ee7f87`) | Palette tests (10 passed), focused Ctrl+P/F2, `:` focus isolation, chooser/Escape and footer tests (1 each); full UI lib (161 passed); strict UI Clippy and fmt check passed. |

On every updated branch, `python3 -m py_compile
scripts/validate_rust_metric_descriptions.py` and `git diff --check` passed. A
guarded source-constant parse found exactly one integer schema constant in
each branch: 26 for #398 and 24 for #399–#402. The database-creation verifier
itself has not been rerun on a current branch artifact; CI must execute it.

## PR #401 Workflow completion history check — 26 November 2026

Validated the rebased branch `feature/rust-workflow-history-toggle` at
`000549b` (based on current `origin/main`); the shared PTY correction is pushed
as `e921b0b`. Five focused Workflow alert tests
passed, covering the `h` reveal/hide behavior, recent completions and other
incident visibility, selection preservation/fallback, same-history refresh and
stale-detail clearing, `h` being ignored in detail, and footer controls at
120×32, 80×24 and 60×18.

Exact results: `cargo test --offline --locked -p agenturm-ui
workbench::workflow_alerts::tests -- --nocapture` (5 passed);
`cargo test --offline --locked -p agenturm-ui --lib --quiet` (163 passed);
`cargo clippy --offline --locked -p agenturm-ui --all-targets -- -D warnings`;
`cargo fmt --all -- --check`; and `git diff --check` all passed. This qualifies
the focused rebased UI source tests only; installed paired Python/Rust parity
and broader PR #401 CI remain separate gates.

## Feature handoff board

| ID | Developer scope | Implementation worktree | Test owner/status |
| --- | --- | --- | --- |
| CF1 | Exact native Claude/Pi presence before ordinary activity; existing SessionDiscovered contract. Codex already implements header discovery. | `/tmp/agenturm-native-presence` | Separate test agent; not run on new source. |
| CF2 | Whole-session paged Review edit browser pinned to selected history, source/session, ReviewVersion and snapshot; reuse existing private edit proof. | `/tmp/agenturm-review-session-edits` | Separate test agent; not run on new source. |
| CF3 | Trusted installed-adapter list/show/remove with pin-checked removal and atomic catalog publication. Install already exists. | `/tmp/agenturm-adapter-lifecycle` | Separate test agent; not run on new source. |
| CF4 | Body-free OTLP error/fatal logs in failure investigation, including explicit `--kind log`; existing Findings private log view unchanged. | `/tmp/agenturm-core-development` | Separate test agent; not run on new source. |
| CF5 | Implemented: exact native Claude worker-spawn facts and declared-child metadata in session evidence. | `/tmp/agenturm-native-presence` + `/tmp/agenturm-review-session-edits` | Separate test agent; execute on the supplied integrated source head. |
| CF6 | Implemented: combined deduplicated usage/cost totals that exclude and count reports overlapping at exact `(target source, session)` identity; unattributed telemetry contributes with a disclosed boundary. | Development branch `feature/combined-usage-dedup` | Separate test agent; not run on new source. |
| CF7 | Implemented: cross-host terminal routing through explicit route files and endpoint-keyed saved receiver profiles; fixed read-only tmux and `/proc` probes resolve one exact remote pane or report unavailable. | `/tmp/agenturm-route-profile`, `feature/receiver-route-profile` (base `afbdbdc`; dirty implementation tree) | Test-owner unit regressions pass; live receiver/transport and frozen-artifact scenarios remain not run. |
| CF8 | Implemented: Claude private edit detail through the harness-generic operation contract — adapter `file_edit` summaries, schema-23 grants with a v22 upgrade path, bounded credential-screened reveal under a Claude fingerprint domain, cross-kind grant denial. | Development branch (see PR) | Separate test agent; not run on new source. |
| CF9 | Implemented: Claude async launch attribution — a `worker_launch { child_worker_id }` fact from the real `isAsync`/`async_launched` record shape, with no completion claim and the checkpoint wire format unchanged. | Development branch (see PR) | Separate test agent; not run on new source. |
| CF10 | Implemented: OpenCode patch-path evidence and independent schema-24 edit grants; canonical content commitment, provider-shaped local/broker proof and Review/turn selection. No invented diffs or add/delete counts. | `feature/opencode-edit-detail` | Installed/paired qualification remains with the separate test owner. |
| CF11 | Proposed Codex JS `exec_command` wrapper checks. Recognition stays gated until native rollout evidence contains both an explicit exit-code-to-throw guard and a matching structured `isError` result. | `/tmp/agenturm-codex-check-wrapper` | BLOCKED: Nov rollout scan found zero qualifying guards and no structured `isError` on custom tool outputs; no check inference is qualified. |

Worktree paths identify current ownership, not release artifacts. The developer
must supply the final integrated commit before the test owner freezes a binary.
Do not build different sources concurrently into one Cargo target directory.

## UI-05 follow-up handoff — not run

PR #396 (`afbdbdc`) merged the integrated UI candidate and Activity evidence
filter to `main`. The current unmerged branch
`feature/ui05-session-metrics` adds contextual session-to-Metrics navigation
into the global catalog. Do not count the prior 991-test and PTY evidence as qualification for
these follow-up changes.

| ID | Required test | Acceptance | Status |
| --- | --- | --- | --- |
| UI05-A | Activity prompt-only filter and page query | At 120×32, 80×24 and 60×18, toggle `F` and the palette command. Only typed `Native request` events remain; prompt text is not read or inferred. Combine `/` search with the toggle, handle an empty prompt set, preserve the selected event when it remains visible, preserve mode across older/newer page replacement, and restore all events on the second toggle. Verify another pane and MCP `f` server filtering are unaffected. Compare the tagged Python `Prompts` button journey. | Not run; implementation `81c7357`. |
| UI05-B | Activity worker attribution | Inspect ordinary activity with a recorded worker, no worker, WorkerSpawn and WorkerLaunch. Show exact worker, `Worker: Unreported`, or separately named `Parent worker: Unreported` as appropriate. Verify the same rows at all three sizes; do not infer a parent or child ID. | Not run; implementation `81c7357`. |
| UI05-C | Session-to-Metrics navigation and honest scope | From roster and inspector, press `K` or invoke the palette action and open the global Metrics catalog. Verify its query is unscoped (`source_id=None`), the UI does not imply these series belong to the selected session, and Escape returns to the same session selection. Record that current `otlp.*` source IDs do not link to ordinary `harness.*` session IDs, so true per-session Metrics is still pending typed linkage. | Passed scoped PTY on `b57af26` at 100×26: K and palette opened the global catalog, explicit non-link scope was shown, Escape restored the same row, normal quit restored terminal. True per-session metric linkage remains open. Evidence: `/tmp/agenturm-ui05-k-navigation.txt`. |

The merged bundle PTY failure is a stale test expectation in
`scripts/validate_rust_default_cockpit.py`: it checks for `Project unavailable 1`
but the latest command center copy is `No recorded project 1`. The evidence
shows the requested search and selected row. The test owner updates and reruns
that assertion against the frozen source; this implementation branch does not
edit the test script.

Run these against a frozen integrated source and binary. Keep UI-Q's paired
screen matrix, live broker/private grant cases, release archive and independent
sign-off as separate gates.

## CF1 — native session presence

**Developer handoff:** Claude/Pi implementation integrated in the development
branch. Forty-one focused adapter tests and scoped strict Clippy passed. The composed app lib/bin compile check passed with all four slices and schema
22. A focused prefix-race regression and unknown-time Claude checks passed;
25 focused Store upgrade/retention/backup checks passed. These are developer
checks, not a final artifact qualification. These checks do not close the delegated rows below.

**Expected product behavior.** Supported exact native identity can register a
session before ordinary activity. Pi uses its native timestamp; recognized
Claude title/name metadata without a timestamp uses explicit unknown time 0,
never file mtime or a fabricated producer timestamp. Discovery
is neutral recorded evidence; it must not create Working, completed, approval
or current process-liveness claims. Existing native name/project provenance and
newest-updated selection rules continue to apply.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Pi exact session header without messages | One neutral presence fact; correct source/session/native timestamp; roster after commit/reopen. | Not run |
| Claude supported timestamped metadata without activity | One neutral presence fact for exact native session; native names remain separately verified. | Not run |
| Claude snapshot prelude without bound identity | No invented session, identity or execution state. | Not run |
| Missing timestamp on exact Claude title/name metadata | Neutral retained presence with time 0 and Updated Unknown; no execution/liveness or recency claim. | Not run |
| Invalid/null/conflicting timestamp, duplicate/conflicting identity, untimed user stub | Unsupported input remains absent or diagnosed; no wall/mtime fallback. | Not run |
| Managed retention with unknown-time presence | Native time remains 0; schema 22 ages only the exact neutral record by its original batch admission clock; it survives a current default cutoff and expires when that admission age passes the cutoff. | Not run |
| Replay/re-admission/restore with admission aging | Neither exact retry nor same event in a new batch refreshes age; restore preserves admission clock, event digest and unknown native time. Old batches remain default-unknown admission time. | Not run |
| Other zero-time records | Existing diagnostic/activity retirement remains unchanged; no general retention exemption. | Not run |
| Header plus normal facts or relationship | No duplicate session registration/count; ordinary lifecycle remains authoritative. | Not run |
| Poll replay/restart/old cursor | Stable event identity and idempotent admission; only verified original header can backfill. | Not run |
| In-place source rewrite during header backfill | Reject mixed prefix/header proof and preserve prior cursor; repeat existing Codex as well as new Claude/Pi paths. | Not run |
| Guardian/internal-review exclusion and legacy decoder mode | Existing exclusions and legacy wire compatibility retained. | Not run |
| Actual simultaneous Codex and Claude/Pi sessions | Account for selected, queued and absent sources; distinguish actual native witnesses from disposable fixture cases. | Not run |

Exercise cold collection, empty history, reopen, later user/tool/lifecycle
activity and renamed metadata. At 120×32, 80×24 and 60×18 verify human names,
neutral fallback status, recency ordering, selected source/session stability and
correct inspector return. Codex header discovery is a regression, not new work.

## CF2 — complete session edit browsing

**Developer handoff:** implementation integrated. Seventeen focused App/UI
checks and scoped Clippy passed. Independent source review confirmed history,
snapshot, source/turn/worker and current private-grant fences. The composed app
lib/bin compiles with the other core features. No broad terminal campaign ran
in this implementation session.

**Expected product behavior.** Review can find an older supported Codex edit
behind unrelated activity rather than being limited to its loaded overview.
The existing per-turn edit picker and exact `codex.edit_detail` preview continue
to work. Seventeen focused App/UI checks and scoped Clippy passed on the lane
source; final composed source compile and delegated acceptance are separate. A metadata selector is not private-content authority.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Earlier edit beyond initial examined range | Explicit examined/partial coverage, continuation and eventual exact result. Empty matching page can still be partial. | Not run |
| Multiple turns/operations with same labels | Correct source/session/turn/worker/event/revision retained; no sibling edit substitution. | Not run |
| Paging while new writes arrive | Same read snapshot, no shifted/duplicated traversal; stable selection and return. | Not run |
| Review decisions and overlays | Enter/escape through browser/private preview returns to original Review and unchanged decision version. | Not run |
| Local edit reveal | Exact retained-at-snapshot membership, current grant and original source proof required. | Not run |
| Broker edit reveal | Existing verified-edit broker authority retained; no fingerprint/private body exposed as metadata. | Not run |
| Revoked grant, changed source/history, prune/restore | Deny or redact; no stale body publication or silent move to newer evidence. | Not run |
| Unsupported worker/provider/edit shape | Clear bounded unavailable/coverage result; no invented proof. | Not run |

Run actual keyboard journeys at all three terminal sizes. Check query-key
ownership, loading/error/empty states, long paths, terminal restoration and
private output lifetime/ACK fences. Private body stays out of copy/export,
ordinary metadata, diagnostics and saved decision content unless separately
explicitly authorized by the existing workflow.

## CF3 — installed adapter lifecycle

**Developer handoff:** implementation integrated in the development branch.
Four focused management/installation tests and strict app lib/bin Clippy passed.
Independent source review found no publication/pin-CAS compatibility blocker.
The actual packaged command and live collector journeys remain delegated below.

**Expected commands.** `adapter list --root DIR [--json]`,
`adapter show --root DIR --plugin-id ID [--json]`, and
`adapter remove --root DIR --plugin-id ID --sha256 CURRENT [--json]`.
The existing trusted local install/replace flow remains compatible.

| Scenario | Required observation | Status |
| --- | --- | --- |
| List/show populated valid private root | Exact catalog identities/pins and deterministic machine-readable output; no executable starts. | Not run |
| Absent root or entry; invalid flags/ID | Useful error and exit status; reads never create directories/catalogs. | Not run |
| Correct removal pin | Only selected entry unregisters atomically; unrelated entry fields remain unchanged. | Not run |
| Stale pin during replacement | Compare-and-swap fails under lock; newly installed pin remains registered. | Not run |
| Last entry removal | Valid empty versioned catalog, with defined list/show/collector behavior. | Not run |
| Failure before catalog publication or held lock | Original catalog remains usable; no partial publication. | Not run |
| Symlink/ownership/mode/path substitution | Reuse existing private-root/path/pin validation; no traversal or unsafe read/write. | Not run |
| Artifact/history/grant preservation | Removal retains artifacts and all unrelated history/content approvals; no garbage collection or service mutation. | Not run |

After install, remove, list and reinstall, explicitly select the supported
adapter in collection and prove changed/missing pins fail closed. This is
trusted local lifecycle management, not a remote downloader or sandbox claim.

## CF4 — OTLP log failure investigation

**Developer handoff:** implementation complete in the accompanying source.
One focused process regression passed, exercising ordinary error/fatal logs,
explicit activity-error flags, source/session separation, no body/source
reference output, kind filters and cursor/query binding. This is a development
check only; the delegated acceptance rows below remain not run.

**Expected product behavior.** `investigate failures` and its cockpit overlay
include retained ordinary OTLP error/fatal log metadata and activity logs that
explicitly report an error. `--kind log` selects only these log candidates.
Public rows carry closed labels and exact recorded metadata, never log bodies.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Ordinary ERROR/FATAL severity with/without trace | Correct closed error/fatal result and optional exact trace/span IDs; source-only records remain discoverable. | Not run |
| Activity log reported_error true/false | Only explicitly reported errors match; normal usage/cost/activity logs remain absent. | Not run |
| Source/session/turn/worker filters | Exact scope retained; source-only log not attributed to a selected session without evidence. | Not run |
| `--kind log`, another kind, changed cursor filter | Typed kind filtering and query/snapshot-bound cursor denial; no cross-query continuation. | Not run |
| Empty page followed by older error log | Examined/matched counts and continuation remain honest; no false exhausted scope. | Not run |
| Authenticated/broker allowlist and restart | No hidden-source metadata; durable results after reopen. | Not run |
| Captured text, unsafe severity label or secret body | Ordinary output/JSON/copy/export has no body or producer-supplied severity label; no new private source authority. | Not run |
| Shared UI/CLI behavior | Body-free label, exact occurrence and prior selection return; existing failed command source route unchanged. | Not run |

Do not merge evaluation outcome inventory into this test's claim. Evaluation
revisions are a separate current-outcome contract and need their own design if
later added to unified failure investigation.

## CF5 — exact Claude worker-spawn evidence

This feature is integrated with CF1–CF4 in the accompanying development source.
Developer checks passed for three Claude decoder cases, the Runtime contract,
two Store round-trip/projection cases and the scoped presentation case; scoped
Clippy passed on the native/kernel lane. These are not installed qualification. The contract is a closed
`worker_spawn { child_worker_id }` fact; record identity names the exact
spawning parent source/session/optional worker/turn/operation. A child-worker
declaration does not claim a separate session or live process.

Native proof must correlate assistant Agent/Task declaration UUID and tool-use
ID to one non-error user result with exact `sourceToolAssistantUUID`, same
session/worker/turn and structured completed status/agentId. Only five actual
historical completed records were verified initially. Async statuses, text,
message ancestry and directory placement are not positive proof.

Delegate: correct parent/child through commit/reopen; duplicate/replay;
sibling worker and same-session different turn; conflicting/reused tool-use ID;
wrong declaring assistant UUID; malformed/self child; non-completed/error/
multiple results; bounded checkpoint overflow and old cursor continuation;
exact declared-child Activity/inspector details and page coverage. Verify no
prompt/tool-body copy and no roster recency/hierarchy changes. All delegated scenarios
remain **not run** until the test owner records actual results on the frozen head.

## CF6 — combined deduplicated usage totals

Implemented with the FD3 combined slice. The combined per-UTC-day projection
excludes reports whose overlap is established through exact
`(target source, session)` identity and counts the exclusions instead of
summing them: native reports are excluded when same-day usage-bearing telemetry
claims their exact source and session, and attributed telemetry rows are
excluded when same-day native reports exist for the claimed identity.
Unattributed telemetry carries no identity link and therefore contributes;
that boundary is disclosed, not inferred away. Combined costs merge the
per-trace-deduplicated and log-only cohorts per currency; native assistant
cost records that forks can duplicate keep `native_origin` with no span
identity and stay outside combined totals.

**Developer handoff:** six focused Store usage tests (exact-identity overlap
exclusion, unattributed contribution, snapshot-pinned combined equality,
revision correction, and older-receiver pages without the combined block),
76 UI lib tests with the new default Combined chart mode, joined workspace
all-target strict Clippy and fmt passed on the development branch. These are
developer checks, not installed qualification.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Native plus attributed telemetry at one source/session/day | Both sides excluded and counted; combined tokens exclude both; all cohort views unchanged. | Not run |
| Attributed telemetry without same-day native rows for the claimed identity | Contributes its deduplicated tokens; no exclusion is recorded. | Not run |
| Unattributed telemetry and native-only days | Combined equals the nonduplicated evidence; no speculative cross-layer deduction or hidden merging. | Not run |
| Combined costs | Trace and log-only costs merged per currency; fork-copyable native-origin records excluded; distinct currencies never converted. | Not run |
| Terminal disclosure at 120×32, 80×24 and 60×18 | Combined mode renders first; `n` reaches excluded-overlapping counts and per-currency combined cost; `s` returns to the unchanged native/span/trace/log views. | Not run |
| Snapshot, revision and retention | A pinned snapshot keeps its combined block; a native revision correction reclassifies only at the newer watermark; retention/scan flags keep marking partial days. | Not run |
| Older attached-receiver page | A usage page without the combined block deserializes with the block absent; every existing view keeps working. | Not run |
| Scan/cohort limits | Days beyond trace-cohort or raw-scan bounds keep their existing partial flags; combined never claims coverage the cohort did not read. | Not run |

All CF6 scenarios are **not run** until the test owner records actual results
on the frozen integrated head.

## CF7 — cross-host terminal routing

Implemented with the FD4 routing slice. An attached native cockpit accepts an
explicit operator-authored `--route-file` (one command word per line, for
example `ssh` and `user@receiver-host`); the legacy Python receiver protocol
rejects it. `l` verifies the exact current request, then runs three fixed,
read-only, bounded probes through the route: a remote `/proc` snapshot whose
matching processes are proved by exact byte comparison of the resume command
tail (NUL-count, basename and length all checked), the tmux pane inventory,
and a pane-process re-verification. The unique resolved target is presented as
an attach command; Agenturm never selects a remote pane, answers, or approves.

**Developer and test-owner results:** on `/tmp/agenturm-route-profile`, branch
`feature/receiver-route-profile`, base `afbdbdc` plus the current dirty source
and test changes, `cargo test --offline --locked -p agenturm-app --lib --
--quiet` passed all 194 tests. Focused saved-profile tests passed with
`cargo test --offline --locked -p agenturm-app saved_route_ -- --nocapture`
(3 passed), and the offline/no-token command passed with
`cargo test --offline --locked -p agenturm-app
forget_route_completes_offline_without_a_token_file -- --nocapture` (1 passed).
The tests cover endpoint-keyed profile isolation and receiver-identity mismatch,
narrow forget, owner/mode checks (0700 directory, 0600 file), route-file
validation and size/non-regular rejection, saved-file and directory symlink
rejection, atomic replacement without a staging-file leak, native-only option
parsing, and `--forget-route --endpoint ...` deleting an existing profile
without a token file or available receiver. These unit checks use local files
and a closed loopback port; they do not qualify a real transport or frozen
artifact. `cargo fmt --all -- --check` and `git diff --check` passed.
Strict app lint also passed with
`cargo clippy --offline --locked -p agenturm-app --all-targets -- -D warnings`.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Attached cockpit with a real route to a receiver host | One unique verified pane resolves; the presented attach command opens the exact recorded owner's terminal. | Not run |
| Route semantics | A route that does not join arguments the way `ssh` does fails closed with unavailable, never a guessed target. | Not run |
| Ambiguous, stale or missing remote state | Two candidate panes, a changed pane process between probes, or no matching process report unavailable with the request selection preserved. | Not run |
| Worker/unsupported provider/missing configuration | Worker requests, non-Codex/Claude/Kiro sources, unsafe session identifiers and absent route files fail closed with exact reasons. | Not run |
| Probe hygiene | Probes are read-only, bounded in output and time, carry no session bytes in script text, and leave no temporary files behind on the receiver host. | Not run |
| Return selection at all three sizes | 120×32, 80×24 and 60×18 preserve the Workflow selection after routing results and errors. | Not run |
| Python receiver protocol | `--route-file`, `--save-route` and `--forget-route` with `--receiver-protocol python-v0.2.8` are rejected at parse time. | PASS in parser unit fixture; a real Python receiver was not contacted. |
| Saved receiver route | `--route-file PATH --save-route` writes a private per-user profile keyed by endpoint and records the authenticated receiver identity; a later attach reuses it only if the same identity authenticates. `--forget-route --endpoint http://127.0.0.1:PORT` removes by endpoint without contacting the receiver or requiring a token file. Verify mode 0600 file, mode 0700 directory, symlink rejection, atomic overwrite, missing/corrupt profile behavior and a receiver replacement at the same endpoint. | Partial PASS in unit fixtures: permissions, symlinks, atomic replace, endpoint scope, identity mismatch/replacement, native option parsing and offline/no-token forget pass. Corrupt profile, online authenticated attach, and frozen artifact remain not run. |

Live receiver/transport and frozen-artifact CF7 scenarios remain **not run**;
the saved-profile unit results above do not qualify an installed candidate.

## CF8 — Claude private edit detail

Implemented with the FD2 first provider slice. The operation-detail reveal
contract is harness-generic: a shared protocol recognition contract derives
the `FileEdit` summary both in the Claude adapter fact and at reveal
re-recognition, `claude.edit_detail` grants live at store schema 23 (with a
verified v22 upgrade that preserves prior approvals), the source-side reveal
locates the exact assistant row by committed byte offset within the admitted
proof range, re-verifies session/worker/timestamp/operation identity, and
extracts a bounded payload — strings truncated on char boundaries with an
explicit `truncated` flag, and credential-shaped or bidirectional text
withholds the whole payload. Cross-kind grants deny explicitly; malformed
inputs withhold without failing the line; no turn identity is invented
store-side or re-derived source-side.

**Developer handoff:** adapter unit tests for Write/Edit/MultiEdit counts,
case-insensitivity, no-leak and skip-on-malformed; a schema v22→v23 upgrade
regression with prior-grant retention; and a real-binary integration test
covering grant requirement, bounded reveals, 3 KiB→2 KiB truncation,
wrong-turn/wrong-operation/malformed denials, cross-kind denial,
changed-source denial and revoke — all green alongside every pre-existing
Codex/Pi/Kiro content test. Workspace strict Clippy and fmt clean.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Installed binary + real Claude adapter against a real transcript | Grant-gated bounded reveal of exact edits; no content in query output. | Not run |
| Schema upgrade on a live database | v22→v23 keeps prior approvals; `claude.edit_detail` grants become accepted; older kinds unchanged. | Not run |
| Truncation and screening | Long content truncates at 2 KiB on a char boundary with the flag; credential-shaped or bidi text withholds the payload. | Not run |
| Cross-kind and revoked grants | `codex.edit_detail` on a Claude source (and vice versa) denies; revoke denies; changed source bytes deny. | Not run |
| Dense lines and malformed shapes | Dense batches keep the 256-record/64 KiB budget; missing input, non-file tools and out-of-range MultiEdit withhold without failing the line. | Not run |
| Terminal reveal at all three sizes | 120×32, 80×24 and 60×18 render the bounded edit payload with selection preserved. | Not run |

All CF8 scenarios are **not run** until the test owner records actual results
on the frozen integrated head.

## CF9 — Claude async launch attribution

Implemented with the FD1 Claude-async slice, licensed by the
[native evidence record](rust-fd1-native-evidence.md). A `worker_launch` fact
correlates an assistant `Agent` tool_use declaration to its structured
`toolUseResult {isAsync: true, status: "async_launched", agentId}` through the
same declaration tracking, first-consumer-wins consumption and turn-ownership
fences as the completed-spawn proof. It claims a child identity and a
launched state only — no completion, no resolution, no liveness. The
checkpoint wire format is unchanged; evidence presentation says "launched"
without completion wording, and the "Declared child workers on page" count
still counts completed spawns only — a disclosed difference, not a silent
semantic change.

**Developer handoff:** worker_spawns unit tests with the real record shape
(launch + declaration consumption; completed path unchanged; deny cases for
`isAsync` false/missing, other statuses, missing/invalid agentId, errors and
wrong assistant), a line-level adapter test (launch fact emitted, no spawn
fact, no private-text leak), store round-trip and admission tests with the
deny-unknown-fields case, and a presentation test asserting no completion
wording — all green on the rebased head with workspace strict Clippy and fmt.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Real async launch in an actual Claude transcript | One launch fact with the exact child agentId and parent operation; no completion wording anywhere. | Not run |
| Declaration consumption and ambiguity | A later result for the same declaration never double-claims; duplicates and errors stay withheld. | Not run |
| Checkpoint continuity | The declaration checkpoint format round-trips unchanged across restart; older cursors stay readable. | Not run |
| Relationships projection and roster | Launches appear in the relationships projection without claiming completed parentage or roster recency. | Not run |
| Declared-child count disclosure | The "Declared child workers on page" count stays spawn-only and the launched rows are visibly separate. | Not run |

All CF9 scenarios are **not run** until the test owner records actual results
on the frozen integrated head.

## CF10 — OpenCode patch-path detail

The [implementation and acceptance handoff](rust-opencode-edit-detail.md)
defines the source inventory, canonical content commitment, `oc5` sweep,
schema-24 independent grant, provider-shaped edit proof, bounded path-only
reveal and separate installed/paired acceptance matrix. It does not claim
native diffs, add/delete counts, exact first-turn attribution or worker linkage.
All installed/paired CF10 scenarios remain **not run** until the test owner
records results against the final integrated source and artifact.

## CF11 — Codex JS exec-wrapper check gate

**Native evidence review (26 November 2026):** a structure-only scan of the
available November Codex rollouts found no wrapper containing one literal
`exec_command` with an explicit `exit_code`-to-throw guard (0 observed). The
observed `custom_tool_call_output` rows did not contain a structured
boolean `isError`; their output was represented as a list or string. Therefore
the proposed JS-wrapper check signal has no observed native producer contract
and cannot safely generate typed test/lint/build facts. Keep recognition
disabled and do not infer check success or failure from command text, prose,
or native exit text alone. The wrapper feature remains gated until a real
serialized Codex example supplies both required witnesses on the same matched
tool-call/result identity.

The same scan found 2,341 `spawn_agent` function calls whose stringified
arguments carry `task_name`, `message`, and sometimes `fork_turns`. Of 2,089
outputs matched by native call ID, all were strings and none carried a typed
child identity. Keep this collaboration family unsupported for relationship
facts; names remain sourced from native Codex metadata, not task text.

The following acceptance cases are intentionally **BLOCKED / not run** until
such a native witness is captured. Synthetic fixtures can be used after a
producer example establishes the shape; they cannot replace that evidence.

| Acceptance case | Required behavior once the native contract is established | Status |
| --- | --- | --- |
| Literal test, lint and build command | One literal test/lint/build command plus an explicit nonzero `exit_code`-to-throw guard is eligible for a pending typed check. | Blocked: no qualifying native wrapper observed. |
| Matched structured output | Emit only after the corresponding call ID receives a structured boolean `isError`; false means passed and true means failed. The call and check share one operation identity. | Blocked: native outputs expose no structured `isError` field. |
| Checkpoint round-trip | A pending call/category survives a harness restart and emits only when the matching output arrives. | Not run; feature remains gated. |
| Multiple/dynamic/template commands and absent guard | Multiple calls, dynamically computed or template commands, and missing guards do not fabricate checks. | Not run; feature remains gated. |
| Unknown output, wrong call ID, other tool names | Unstructured text, mismatched output identity and tools other than exact `exec` cannot create or resolve a check. | Not run; feature remains gated. |
| Content privacy | Raw command and output content never appears in projected check facts. | Not run; feature remains gated. |

Do not use unit-test passes from the speculative implementation snapshot as
qualification evidence. When a real producer sample becomes available, add
test-only fixture coverage, run the focused Codex harness suite, strict harness
Clippy and fmt, and update this section against the exact source commit.

## Scoped CI fix confirmations — 26 November 2026

These local checks confirm the specific follow-up commits only. Full PR CI and
broader feature qualification remain pending.

| PR / commit | Focused result | Exact local checks |
| --- | --- | --- |
| #398 / `54483d8` (Pi edit and Claude MCP content fixes) | PASS: strict workspace Clippy; Pi harness 28 tests; Pi edit reveal 2 tests; Claude MCP reveal 2 tests. | `cargo clippy --offline --locked --workspace --all-targets -- -D warnings`; `cargo test --offline --locked -p agenturm-pi-harness --quiet`; `cargo test --offline --locked -p agenturm-content --lib pi_edit_reveal --quiet`; `cargo test --offline --locked -p agenturm-content --lib claude_mcp_reveal --quiet`. |
| #399 / `07e3691` (prompt-signals example lockfile) | PASS: locked standalone example test, including the host-compatibility integration test (1 passed). | `cargo test --locked --manifest-path examples/rust_prompt_signals_extension/Cargo.toml`. |
| #400 / `2bdc1c5` (task CLI help assertion) | PASS: exact task CLI regression now sees the documented `agenturm task add` command (1 passed). | `cargo test --offline --locked -p agenturm-app --test task_cli tasks_refuse_missing_or_unrelated_history_without_creating_workspace -- --exact`. |
| #401 / `000549b` (Workflow history rebased on current main) | PASS: 5 focused Workflow tests; full UI library 163 tests; strict Clippy, fmt and diff checks. | `cargo test --offline --locked -p agenturm-ui workbench::workflow_alerts::tests -- --nocapture`; `cargo test --offline --locked -p agenturm-ui --lib --quiet`; `cargo clippy --offline --locked -p agenturm-ui --all-targets -- -D warnings`; `cargo fmt --all -- --check`; `git diff --check`. |

These results do not replace the running/required PR CI, Linux bundle PTY
rerun, or remaining feature and artifact acceptance checks.

## Related Review identity fixes — delegated regression

Typed broker Review receipts now pin the authenticated history identity for
`--format` and directory export as well as linked traces. In the existing
separate-UID fixture, export one exact turn as JSON, verify its identity,
snapshot and user occurrence, then draft a task from the file. Restore into a
new incarnation and reapprove the prompt: old receipt must reject without
excerpt bytes; a fresh receipt must draft. Repeat directory export and confirm
unformatted plain Review retains its existing output contract. **Not run.**

Local Review marks and decision notes now open their ledger against the selected
logical history and snapshot/incarnation. Replace the database path between
selection and mutation, including equal numeric receipt counts: deny before
creating a sidecar or saving a note against the replacement. Repeat after a
same-path incarnation change. Existing broker workspace-marker behavior remains
separate. Two focused Store regressions passed; actual UI/restore regression is
**not run** and belongs to the test owner.

## Joined and release checks delegated unchanged

After the integrated head is supplied, execute relevant Q1–Q10 from the
[remaining-work plan](../rfcs/rust-migration-remaining-plan.md): joined Rust
checks, actual producer/SDK interoperability, Linux x86-64/ARM64, current exact
artifact install/upgrade/recovery, paired Python-quality UI journeys and
separate-UID privacy/authority checks. Real Kiro, production endpoints and
native macOS retain their external dependencies; do not fabricate a pass.

No public release/default switch is requested by this development handoff.
The one-release-per-rolling-24-hours rule and unsupported-profile limits remain.

## Results and defect handoff template

For each execution record: feature/test ID; source commit; executable/archive
SHA-256; OS/architecture; native producer/version or fixture; terminal size;
expected and actual result; PASS/FAIL/BLOCKED; exact log/artifact path. Do not
combine results from different source heads into one qualified candidate.

For each defect return: minimal reproduction, exact owner/source/session scope,
last expected state, failed invariant, and affected feature paths. Developers
fix that defect and return the new source head. The test owner decides the
necessary regression breadth; developers do not restart the entire campaign.
