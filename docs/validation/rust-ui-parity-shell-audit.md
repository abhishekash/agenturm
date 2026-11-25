# Rust UI parity · recovered shell audit

24 November 2026 · **Changes required; not a parity sign-off.**

Read-only review of `/tmp/opencode/agenturm-ui-parity-resume`, initially an uncommitted recovery from `/tmp/agenturm-ui02b-integration`. During review the owner preserved that recovery as **`2b162cbda0cd5c308b6ca5eb620853d55f3ce064`**, parent `a633c6816ee2d7568e56629e561687be2d3a8b97`. Locations below refer to that recovered candidate, **not this documentation branch**. Read the original integration prompt, parity plan, shared contracts, UI03–UI06 lane notes, inventory, and tagged `v0.2.8:docs/keyboard-navigation.md`.

**Evidence:** source tracing only. Reproductions below are proposed regression cases, not executed tests. P1 = privacy/wrong-evidence blocker; P2 = broken workflow or action contract. [HTML companion](rust-ui-parity-shell-audit.html).

## Findings

### F1 · P1 · Inline reader bypasses display gate and redraw/ACK lifecycle

**Locations:** `rust/agenturm-ui/src/workspace.rs:1586–1602,1681–1695,1827–1831,3917–3938,3966–3980`; `rust/agenturm-ui/src/workspace/prompt.rs:1066–1068,1244–1254,1321–1329`.

The event loop locks/checks only `workspace.prompt.active`; inline text lives in `prompt_nav.reader`. Closing or expiring the inline reader drops its ACK sender instead of moving it into `redact_then_acknowledge`. Ctrl+C quit only dismisses the full-screen reader. Ctrl+P also checks only `self.prompt`, so a palette can open over granted inline text. The new path does not preserve the existing proof/display fence.

**Repro/test:** deliver an inline body, close its `active` gate before drawing, and assert no body renders. For Esc, expiry, replacement and Ctrl+C, assert the ACK channel remains connected and unacknowledged until a public frame is flushed, then receives `()`. Assert Ctrl+P/F1 cannot expose an overlay over private content. Reuse the full-screen lifecycle rather than adding a provider-specific workaround.

### F2 · P1 · Captured first-line text survives reader redaction

**Locations:** `rust/agenturm-ui/src/workspace/prompt.rs:90–102,526–536,1295–1312,1321–1338`; newly reached through `rust/agenturm-ui/src/workspace.rs:1261–1276,3844–3863`.

`accept_reader_presentation` derives a title from the granted body's first nonempty line and stores it in the public request row. Close/expiry/revocation removes the reader but not that title; the navigator continues rendering captured text while reporting it cleared. This is an integration-exposed UI04 component defect, not a newly added title parser.

**Repro/test:** use a unique secret marker on the first line; read, close/expire/revoke, redraw wide and compact. Assert the marker is absent from every retained/rendered public label. Keep adapter-supplied public metadata distinct from grant-derived labels, and clear the latter with the body.

### F3 · P1 · Handoff ID is reinterpreted as a saved-task ID

**Locations:** `rust/agenturm-ui/src/workbench.rs:1879–1885`; `rust/agenturm-app/src/cockpit_workflows.rs:294–311,769–784`; `rust/agenturm-store/src/workspace_ledger.rs:672–707`.

The Handoffs list contains `HandoffEntry.id` from `list_handoffs`, but Preview/Export calls `task_handoff(id)`, which loads a **saved task**. Equal numeric IDs disclose/export unrelated task contents; absent task IDs falsely report the selected handoff unavailable. The previous `ExportHandoff` path resolves the exact handoff correctly.

**Repro/test:** create task #1 and handoff #1 with distinct markers; select handoff #1 → x → copy/export. Require its from/to/note, never task #1's brief/decisions. Use typed task-versus-handoff identities through both commands.

### F4 · P1 · Review copy/export/excerpts query latest evidence, not the displayed version

**Locations:** `rust/agenturm-ui/src/workbench.rs:188–192`; `rust/agenturm-ui/src/workspace.rs:2777–2793`; `rust/agenturm-ui/src/workspace/view_io.rs:109–118,155–159`; `rust/agenturm-app/src/lib.rs:2951–2985`.

Review requests carry only a `WorkAnchor`, losing the displayed snapshot, history incarnation, review version and older-page cursor. The loader explicitly queries with `snapshot_id: None, before: None`. Copy/export and the excerpt index can therefore describe newer/different evidence than the Review being inspected, despite their loaded-scope labels.

**Repro/test:** pin Review at snapshot A, page older, admit event B (also test history replacement), then copy/export/open excerpts. Require the exact pinned scope/page or an explicit stale refusal. Pass typed snapshot/history/version/cursor proof through the app seam.

### F5 · P2 · Late Review export response becomes an unsolicited clipboard copy

**Locations:** `rust/agenturm-ui/src/workspace/view_io.rs:59–60,80–95`; `rust/agenturm-ui/src/workspace.rs:1240–1258`.

Esc closes the export form during an in-flight write without invalidating its context. Completion infers operation type from whether a form happens to exist: no form means copy to clipboard. Cancel an export while pending and its successful response is treated as a copy; open a replacement form and the old response instead consumes/reports that new destination. The workspace generation does not distinguish these form instances.

**Repro/test:** delay the export response, Esc, optionally open another form, then complete it. Assert no OSC52 request, no replacement-form mutation, and an accurate original write receipt. Carry operation intent/form generation in the pending change/result.

### F6 · P2 · Handoff destination form owns keys but never renders

**Locations:** `rust/agenturm-ui/src/workbench.rs:1535–1576,2145–2172`.

Preview → x sets `handoff_export` but keeps `handoff_preview`. Key dispatch checks export first; drawing checks preview first and returns. The user types a destination into an invisible form while the old preview still advertises copy/export keys. Keeping the form on collision does not make it visible.

**Repro/test:** use a valid preview fixture, press x, type a path, inject a collision, and assert destination, draft path and error render at 120×32, 80×24 and 60×18. Align key and drawing precedence; test the composed screen, not just option presence.

### F7 · P2 · Excerpt-created task editor remains hidden behind its chooser

**Locations:** `rust/agenturm-ui/src/workbench.rs:847–865,1503–1533,2111–2143`.

Enter on an excerpt creates a task and applies `TaskDraft`, but never clears `review_excerpts`. Both dispatch and drawing continue prioritizing the excerpt chooser over `self.draft`. The promised editor is hidden; pressing Enter again can create additional persisted tasks before the operator ever edits/saves one.

**Repro/test:** Review → o → choose excerpt → Enter → apply real draft result. Require the task editor and its exact excerpt identity, then verify Enter cannot create a duplicate. Consider keeping a draft unpersisted until explicit save.

### F8 · P2 · Decision editor consumes ordinary letters and silently discards dirty text

**Locations:** `rust/agenturm-ui/src/workbench.rs:1641–1686`.

Unmodified `a`/`r` change decision kind before text handling in every field, so ordinary words cannot be typed faithfully. Esc unconditionally drops a dirty decision draft despite the explicit discard-confirmation contract (only Brief implements that guard).

**Repro/test:** type `rare` into decision text, rationale and scope; require exact text and unchanged kind. Edit a populated decision, Esc once, and require a confirmation with the draft retained; a second explicit discard may close it. Move kind selection to a separately focused control or modified binding.

### F9 · P2 · Successful decision save leaves the old revision/editor active

**Locations:** `rust/agenturm-app/src/cockpit_workflows.rs:265–292`; `rust/agenturm-ui/src/workbench.rs:894–905,1641–1668`.

Saving returns a fresh `TaskDecisions` revision, but `apply` replaces only `task_decisions`; `decision_draft` still owns focus and retains its old revision. The next save submits a stale expected revision, and the UI does not transition back to the newly saved version. CAS enforcement exists; successful-result reconciliation is missing.

**Repro/test:** add/supersede at revision N, apply the real N+1 result, then edit/save again. Require a closed or rebased draft and the selected saved decision at N+1; verify conflict responses preserve authored text without silently dropping CAS.

### F10 · P2 · Roster/turn prompt routes create an invisible navigator

**Locations:** `rust/agenturm-ui/src/workspace.rs:1261–1276,2032–2034,2490–2527,3696–3705,4285–4299`.

Every successful anchored Prompts result now becomes `prompt_nav` and removes the workbench. Its renderer exists only inside a `HistorySelection::Session` page. `P` directly from the roster has no history page, and `P` from a turn has a Turn selection: both leave the previous screen visible while the unseen navigator owns input. Session analytics can also return before that renderer.

**Repro/test:** roster P and exact-turn P at all three sizes; assert visible navigator, exact scope, usable read/back, and unchanged origin selection. Attach the navigator to an explicit route that all entry points can draw; do not infer its parent from whichever history page is underneath.

### F11 · P2 · Palette availability/dispatch can perform a different action

**Locations:** `rust/agenturm-ui/src/workspace.rs:3039–3042,3125–3158,3493–3527,3541–3544`; `rust/agenturm-ui/src/workbench.rs:1748–1764`; `rust/agenturm-ui/src/keymap.rs:488–500,576–588,832–860`.

Concrete mismatches, beyond missing labels:

- Saved tasks → palette **Add decision** is enabled but sends `n` to the task list, opening **new task**, not a decision. **Copy handoff preview** is enabled on the Handoffs list but sends an unhandled `c` without opening a preview.
- **Evidence first or last** and **Evidence page or detail scroll** both send `/`, opening a filter instead of their advertised action.
- Prompt **Retry** sends R and **Load earlier** sends Tab, neither handled by `PromptNavigator::key`; both are advertised enabled whenever a navigator exists.
- From an exact Turn page, keyboard Ctrl+R works but palette Review is disabled because `selected_session()` excludes Turn.

**Repro/test:** table-driven palette execution tests must assert the emitted typed command/state and exact target for each entry, including wrong-screen/empty/dirty contexts. Label-presence tests cannot establish parity. Resolve an explicit intent or mark unavailable; do not blindly reuse a context-conflicting letter.

## Other unverified integration gates

- Prompt result wiring still hardcodes capture `Enabled` and default page facts (`workspace.rs:1265–1272`); typed cap/disabled/denied/page status from UI04 is not integrated. The 256-reference window therefore lacks its intended cap disclosure. `LoadEarlier`/`Retry` variants alone do not make those controls reachable.
- Turn/worker prompt proof needs app verification: history P clears worker/turn in the command (`workspace.rs:2516–2519`) while retaining the original anchor in `Change::Work`; the list loader clones that requested owner onto returned session prompts (`cockpit_workflows.rs:505–529`), and navigator scope matching omits turn (`prompt.rs:809–813`). Do not certify exact worker/turn history from these labels.
- Review Git/Map are explicit unavailable messages, not implemented workspace operations. Turn Review acknowledgement/notes and Review/task return identity need end-to-end qualification.
- Task search is UI page-local; its commands still lack query/versioned page identity. Filtered refresh restores an unfiltered task index (`workbench.rs:993–996`); selection correctness needs a filtered/reordered-page regression.

## Verification and architecture boundary

**Not run:** Cargo build/test/clippy, Rust or Python UI execution, PTY/tmux keys, grant/ACK concurrency, real adapter/receiver scenarios, visual screenshots, browser rendering, or installed-candidate acceptance. Existing lane test claims are not results for this recovered candidate. The owner retains the memory-limited build gate. Responsive light/dark HTML is authored with embedded CSS and no external assets; structural validation is not visual acceptance.

**Document check passed:** Python HTMLParser assertions for balanced tags, unique anchors, local links, eleven paired finding IDs, embedded CSS, responsive/dark declarations and the recorded candidate SHA. No browser was used.

Positive source observations: task Brief/Decision writes carry expected revisions; the shell uses typed app commands instead of reading provider files; payload reveal still uses existing grants. Fixes should preserve kernel ports, supervised adapters, harness crates, ExtensionPlugin/capability/pane and process-plugin-host seams. Do not repair missing scope facts by provider branches or by weakening grants/CAS. This reviewer changed only this Markdown and its HTML companion in the isolated audit worktree.
