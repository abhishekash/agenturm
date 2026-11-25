# Rust UI parity: recovered shell integration

25 November 2026. **Implementation and focused regression checks; not full UI parity or release acceptance.**

Continues `a633c68` through recovery snapshot `2b162cb`. The first shell agent
stopped at a model quota; its successor also hit a usage limit. Both original
worktrees were preserved. The coordinator recovered the second patch into
`work/ui-parity-resume`, completed the fixes, and reran the tests directly.

## Reachable integration

- The inspector and roster/turn `P` routes render the same prompt navigator.
  Wide and compact layouts preserve exact source/session/turn identity.
  Unsupported worker listing is explicitly unavailable: main-session prompts
  are never relabelled with a requested worker identity.
- Prompt results now carry availability, retained-window/cap information and
  proof-bound older-page cursors. Earlier-page and retry palette actions issue
  typed app requests. The adapter's 256-reference window remains a real limit;
  a changed retained window is refused rather than silently replacing history.
- Inline granted readers use the existing active-display lock and timeout
  fence. Help, themes and palette cannot overlay the private reader. Close,
  replacement, expiry, revocation and quit extract the acknowledgement before
  dropping the body; the event loop redraws before sending it. Public request
  labels do not retain a captured first line.
- Review copy/export/excerpts carry the loaded snapshot, history identity,
  cursor, version, exact scope and event identities. The app rechecks the pinned
  page and refuses changed, replaced or pruned evidence. A write has explicit
  export intent; canceling its form cannot turn a late response into a clipboard
  request or consume a replacement form. Cancellation does not undo a write.
- Review control focus survives button activation; Tab/Shift+Tab and one-layer
  Escape return predictably. Turn Review queries exact turn/worker evidence.
- Excerpt/Review task drafts remain unpersisted until Save and replace their
  chooser visibly. Brief and decision writes keep expected revisions. Decision
  fields accept ordinary `a`/`r`; `Ctrl+K` changes kind. Unsaved text or kind
  changes require explicit discard, and a successful save reconciles the new
  revision before another edit.
- Handoff previews/exports resolve saved handoff IDs, not numeric saved-task
  IDs. Destination forms render above the preview, retain collision errors and
  refuse overwrite. Unsupported preview formats are rejected.
- Evidence filters stay pane-local; selection-scoped metadata copy/export uses
  the selected evidence. Task filters restore selection in the filtered index.
  Palette commands use context guards instead of sending conflicting letters
  into unrelated editors or lists.

## Audit disposition and regression evidence

The [independent audit](rust-ui-parity-shell-audit.md) concerns the **original
recovery snapshot**, not the corrected candidate. Its findings were not erased
or retrospectively marked passed. The following is coordinator verification,
not a second independent review.

| Finding | Corrective seam | Focused evidence |
| --- | --- | --- |
| F1–F2: private gate/ACK and retained captured labels | Shared reader lifecycle; metadata labels remain separate from granted bodies | `inline_private_reader_redacts_before_ack_and_never_caches_a_captured_label`; `inline_private_reader_checks_the_same_locked_display_gate_as_fullscreen` |
| F3: handoff/task ID confusion | Exact saved-handoff lookup and renderer | `saved_handoff_preview_and_export_never_resolve_a_colliding_task_id` |
| F4: latest instead of loaded Review | `PinnedReviewQuery` through the existing app reader | `review_receipt_preserves_older_page_snapshot_and_rejects_changed_proofs` |
| F5: late export becomes clipboard copy | Explicit pending copy/export intents and navigation generation | `cancelled_review_export_cannot_copy_or_consume_a_replacement_form` |
| F6: invisible destination form | Aligned input/render precedence | `handoff_destination_and_collision_are_visible_instead_of_the_preview` at 120×32, 80×24, 60×18 |
| F7: excerpt editor hidden/repeated persistence | Unpersisted task draft replaces chooser | `excerpt_draft_replaces_the_chooser_without_persisting_or_repeating_creation` |
| F8–F9: stolen letters, dirty discard, stale revision | Literal editor keys, explicit discard, response reconciliation | `decision_fields_keep_literal_words_and_success_rebases_to_the_saved_revision`; `task_brief_and_decisions_carry_the_read_revision_and_guard_dirty_drafts` |
| F10: invisible roster/turn prompt routes | Explicit navigator rendering before underlying history panes | `roster_and_exact_turn_prompt_routes_render_and_restore_their_origin` at all three sizes |
| F11: conflicting palette dispatch | Guarded availability and meaningful dispatch | `palette_actions_use_their_real_context_instead_of_conflicting_letters`; existing palette/dispatch tests |

Additional evidence: `worker_prompt_scope_without_worker_listing_proof_is_explicitly_unavailable`,
`review_control_order_is_deterministic_and_escape_returns_to_review`,
`review_copy_export_and_save_use_the_exact_anchor_and_keep_review_selected`,
and the evidence-filter regression in `workspace::tests`.

Executed focused checks:

```sh
cargo test --locked -p agenturm-ui --lib -j 1 -- --test-threads=1
cargo test --locked -p agenturm-app --lib -j 1 cockpit_workflows::tests -- --test-threads=1
```

The UI run passed **157 tests**. The app workflow subset passed, including the
new store-backed handoff/snapshot/scope cases. Logs are
`/tmp/opencode/agenturm-parity-ui-regressions.log` and
`/tmp/opencode/agenturm-parity-app-regressions.log`.
The initial diagnostic failures and intermediate failures remain in separate
logs; they are not counted as successful candidate runs.

## Limits still requiring qualification

- This is not the full paired screen/state/theme matrix against an installed
  Python wheel. The Python preparation uses exact tagged source and locked
  dependencies; the Rust empty-history PTY smoke cannot establish populated
  prompt, provider, attached-receiver, clipboard or private-redraw behavior.
- Headless buffer assertions exercise the shared lifecycle but do not prove a
  particular terminal emulator's flush, clipboard or color behavior. The final
  build/test/PTY ledger must identify the binary actually used.
- Worker prompt listing without retained worker proof remains unavailable.
  Older prompt paging cannot exceed the adapter window. Raw deliveries without
  adapter normalization label Readable unavailable; Original remains granted.
  Private-body copy/export remains disabled pending explicit policy acceptance.
- Review Git/Map reports unavailable without verified workspace binding.
  Provider-specific private-detail breadth remains FD2 work behind its existing
  proofs and grants. Incident acknowledgement persistence needs a ledger API;
  no local-only reviewed badge is invented.
- Full mouse equivalence, a whole-application compact/state matrix and all
  original inventory gaps are not closed by this integration. The older
  inventory is a baseline, not a current acceptance score.
- The independent preparation flagged the pre-existing provider command planner
  in `workbench/workflow_resume.rs`. This integration adds no new provider branch;
  moving that older planner behind an adapter capability remains architectural
  follow-up, not a silently accepted difference.

## Pluggable boundaries

The UI carries typed request, page and proof facts; it does not open provider
records or infer a worker from a main-session response. Store/grant enforcement,
receipt composition, task CAS and collision-safe exports remain in their owning
app/store seams. Theme/help/palette visual code remains independent and semantic.
No model, provider billing setting, global installation or release default was
changed by this continuation.
