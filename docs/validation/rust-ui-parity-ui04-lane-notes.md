# UI-04 lane notes · prompts, session activity and cross-session history

**Lane:** UI-04 · session evidence UI owner
**Worktree:** `/tmp/agenturm-ui04-prompts` · branch `work/ui04-prompts` (from the integrated parity branch)
**Reference oracle:** Python **0.2.8** at `2802d79930728bb331db1d4d718d6dea1c9c374d`
**Status:** navigator, reader and bounded prompt-history page model implemented and
tested in the lane; shared dispatch, palette, help and app result wiring are
listed below for the UI-02/UI-03 integration pass (this lane did not edit those
files).

---

## 1. What changed

| File | Change |
| --- | --- |
| `rust/agenturm-ui/src/workspace/prompt.rs` | Added the integrated `PromptNavigator` (wide index, compact one-line chooser, reader/chooser/navigator focus, retained-page status, worker disclosure, grant/late-result states, deterministic rendering). Refactored the existing `PromptScreen` onto a reusable `PromptBody` + typed `PromptPresentation`, added `PgUp`/`PgDn`/`End`, inline `draw_region`, caption and presentation-at-open. |
| `rust/agenturm-app/src/prompt_history.rs` | New bounded prompt-history page model: exact keyset cursor, deterministic newest-first windowing, retained-cap disclosure, empty-scope distinction, typed `PromptHistoryError` and the existing granted list path. |
| `rust/agenturm-store/src/content_grant.rs` | Published `MAX_PROMPT_REFERENCES = 256` instead of a private duplicate, so the UI reports the real retained cap. |
| `rust/agenturm-store/src/lib.rs` | Re-exported the cap. |
| `rust/agenturm-app/src/lib.rs` | Declared the `prompt_history` module. |

No provider ID, harness ID or extension ID is switched on in the new UI. Request
labels come from a typed presentation supplied by the caller/adapter; the
legacy leading-envelope heuristic is retained only as the explicitly named
compatibility path in `PromptPresentation::legacy_envelopes`, and an
unnormalized capture is labelled **readable form unavailable**, never guessed.

### Reader reuse

`PromptScreen` still owns the existing grant plumbing: 25-second deadline,
`active` display gate, `acknowledge` redaction channel, readable/original
layout, tab stops, grapheme clipping and cell-width wrapping. The navigator adds
an inline `draw_region` for the inspector but does not weaken the fence. The
full-screen path used for MCP, edits, telemetry, evaluations and investigations
is unchanged.

---

## 2. Navigation contract (implemented in `PromptNavigator::key`)

### Wide (≥ 120 columns)

| Key | Result |
| --- | --- |
| `↑` / `↓` | Select older / newer; clamps at boundaries, never wraps. |
| `Home` / `End` | First loaded / newest loaded. |
| `Enter` / `→` | `PromptInput::Read { entry, original, epoch }`; focus becomes `Reader`. |
| `[` / `]` | Previous / next loaded request (select only while the navigator owns focus). |
| `l` | Latest loaded request. |
| `z` | Toggle the stored readable/original choice for the selected request only; no read. |
| `←` / `Esc` | `PromptInput::Back` (one layer: Prompts section → inspector tabs). |

### Compact (80×24 and 60×18)

| Key | Result |
| --- | --- |
| `↑`, `↓`, `Enter`, `Space` (chooser closed) | Open the one-line chooser; the selection does not move. |
| `↑`, `↓`, `Home`, `End` (chooser open) | Move the highlighted choice only. |
| `Enter` / `Space` (chooser open) | Confirm the highlighted choice **and read**, even when it equals the current choice. |
| `Esc` (chooser open) | Cancel; restore the prior selection and close the chooser. |
| `→` (chooser closed) | Read the current request without opening the chooser. |
| `Home` / `End` (chooser closed) | Select first / latest without reading. |
| `[` / `]` / `l` | Select and read the previous / next / latest request. |
| `z` | Toggle the stored presentation choice. |
| `←` / `Esc` | `PromptInput::Back`. |

### Reader

| Key | Result |
| --- | --- |
| `↑` / `↓` / `PgUp` / `PgDn` / `Home` / `End` | Scroll only. The selected request never changes. |
| `→` (original layout) | Pan the captured text right; `Home` resets the column. |
| `←` / `Esc` | Close the reader, clear the text and return to the navigator (wide) or chooser (compact). |
| `[` / `]` / `l` | Change the selected request and immediately re-read it; the previous body is dropped before delivery. |
| `z` | Toggle readable/original in place and remember the choice. |
| `m` (worker scope with a verified main session) | `PromptInput::MainSession(anchor)`. |

### Resize and late data

- `Navigator ⇄ Chooser` follows the 120-column breakpoint.
- A focused reader **stays** a reader across the breakpoint (tagged behavior).
- An open chooser closes when the layout crosses to wide.
- The reader's visible **source line** (and offset within it) is retained when
  wrapping changes, not merely the wrapped row index.
- A page response with a superseded load token is discarded with
  `Discarded a late prompt response for a superseded view.`
- A delivery with a stale reader epoch reports `Late(Superseded)`; a delivery
  for a different request reports `Late(OutOfScope)`. Neither replaces the
  current request or screen.
- Rows outside the exact source/session/worker scope are filtered with
  `Ignored N request(s) outside the selected source, session or worker scope.`

---

## 3. Wide and compact rendering

- Wide: a `Requests · N loaded` index (ordinal + label + selection marker)
  beside a `Prompt i of N` reader pane. Before a grant the pane is
  metadata-only; after a grant the same pane renders the granted body with the
  `Request i of N · <age>` caption.
- Compact: one chooser line (`Request i of N · label ▾/▴`) plus the reader body.
  The open chooser is a bounded popup (one row per request, maximum ten visible,
  ellipsised by terminal cells) that never displaces the body.
- The status line is shared: loading, loaded count/older availability,
  `N retained · older history in Activity`, empty exact scope, capture
  disabled, missing identity, denial, revocation, expiry, missing capture,
  stale source and transport errors each have distinct copy.
- Availability states render at 120×32, 80×24 and 60×18 without panics; below
  the shared terminal floor the module shows `Terminal too small · resize · Esc back`.

---

## 4. Grant, scope and disclosure coverage

| Case | Behavior in the lane | Evidence |
| --- | --- | --- |
| Grant denied | `PromptStatus::Denied(reason)`; metadata list stays useful; activity/retry copy remains. | `grants_are_scoped_revoked_expired_and_late_deliveries_are_scoped` |
| Revoked while reading | Body dropped, `Revoked`, status says captured text cleared. | same test |
| Expired | At 25 s the body is dropped (`Expired`); caller reuses the redraw/ACK gate. | same test |
| Stale source | `PromptStatus::Stale(reason)` scoped to the attempted read. | `out_of_scope_pages_and_missing_capture_are_scoped_to_the_attempted_action` |
| Out-of-scope request | `PromptStatus::OutOfScope(reason)`; list/selection remain. | same test + key-identity rejection |
| Out-of-scope page rows | Filtered before merge; never widens source/session/worker. | same test |
| Truncated capture | Request flag; reader shows `Reader limit: text clipped · open Activity for the captured record`. | `unnormalized_capture_labels_the_readable_form_unavailable` |
| Missing capture | `PromptStatus::MissingCapture`; no invented selection. | scoped-status test |
| Late result after close/back | `Late(Superseded)`; previous epoch cannot display. | grant test |
| Worker context | Exact worker scope is enforced; disclosure says main-session requests are not included and offers `m` only when a main anchor was verified. | `worker_scope_discloses_main_session_navigation_without_broadening` |
| Metadata-only context | `Metadata only · captured text is unavailable for this source.`; no read prompt is offered. | availability render test |
| Capture disabled | `Content capture is disabled for this source.`; no fetch prompt. | availability render test |
| Unnormalized readable form | `Readable form unavailable · the adapter did not supply a context-block normalization.`; `z` shows the captured text. | unnormalized test |

Every read request carries exact `source`, `session`, optional `worker`,
request `turn`, event id, revision, snapshot id and history identity. Grant and
query enforcement stay in `agenturm-content`/`agenturm-app`; the UI stores
identifiers only.

### Copy and export

The reference copies the selected request's readable or original text and
previews an export (`c` / `x`). Rust's private prompt display remains **no-copy**:
`Ctrl+C` stays a terminal interrupt in private/reader focus, and `c`/`x` are
disabled while a granted prompt is displayed. This is the existing tighter
disclosure policy and is recorded as an **intentional difference awaiting
acceptance** (inventory P-09), not as parity. If accepted otherwise, the copy
must be a new grant-aware seam shared with evidence — never a direct text dump.

---

## 5. Retained evidence paging

Prompt history now has an explicit page model, separate from the roster's
History time scope:

- `PromptCursor { observed_at_unix_ns, event_id }` is an exact keyset, not a
  row offset.
- `PromptPage` reports the page rows, the retained window size, the adapter cap,
  whether the cap was reached, and the cursor for the next older page.
- `window()` sorts/deduplicates deterministically and applies the cursor before
  slicing, so identity cannot shift on equal timestamps.
- The adapter cap (256) is disclosed as `256 retained · older history in
  Activity`; an empty exact scope is `No user prompts were captured for this
  session` and is never presented as a complete transcript.
- Filters, selection, query and snapshot are not part of prompt history pages:
  prompts are scoped by exact source/session/worker and a pinned history
  identity. The existing Activity/MCP/Traces/Review page stack and its
  `[`/`]`/`l` identity rules are untouched (owned by the shell/UI-05).

The app page function is `prompt_history::load_page`, which calls the existing
grant-checked `content_preview::list_user_prompts`. Until the adapter exposes a
cursor beyond its 256-row window, "earlier" is **page-local within the retained
window** and is labelled that way. This is the honest current limit; the
inventory P-10 requirement (bounded adapter/kernel prompt-page cursor) remains.

---

## 6. History investigation verification

The tagged `Ctrl+F` journeys map to the existing Investigation overlay. Each
query kind was executed against a real fixture adapter in this lane, not merely
inspected:

| Tagged journey | Rust query kind | Current grant/scope behavior | Verification run |
| --- | --- | --- | --- |
| Recorded paths | `Path` | Private; exact source/session/turn/worker, snapshot pinned; unmatched candidates report `unavailable`. | `codex_edit_and_command_details_require_exact_occurrences_and_independent_approvals` — **passed** |
| Commands | `Command` | Private; literal phrase; source-confirmed reference. | same test — **passed** |
| Instructions | `Instruction` | Private; literal phrase; exact occurrence + reference. | `preview_requires_a_current_grant_and_exact_committed_pi_bytes` — **passed** |
| Failures | `Failures` | Metadata-only bounded candidate page; no private body without a grant. | `failure_investigation_includes_only_reported_log_errors_with_exact_scope`, `failure_candidates_page_retained_metadata_at_one_snapshot` — **passed** |
| Failed commands | `FailedCommand` | Private command query with failure confirmation. | Codex command test — **passed** |
| Open exact occurrence | `InvestigationInput::Reveal` | Private viewer rechecks current grant/history; Esc returns to the result list. | Pi/Codex tests — **passed** |
| Cancel / back | `Esc` | Pending result is dropped when the overlay closes; query is retained by the overlay. | existing UI tests |
| Query/filter/occurrence restore | Overlay retains owner, history identity and page; history page stack retains selection. | **Implemented** per inventory P-20; no new search UI was added. | existing `investigation_keeps_exact_turn_scope_and_returns_to_the_same_inspector` |
| Policy change while results are open | A replacement source invalidates the pinned history before a new read; results are not proactively cleared while idle. | App tests cover changed/replaced source denial. **Partial**: no idle policy-generation invalidation without an attempted read. | Codex/Pi tests |

Attribution is exact: the overlay shows source/session, event/revision, turn and
worker; unavailable rows stay in the result page and permission failures are
scoped to the attempted query/read.

---

## 7. Verification run in this lane

Commands were run from `rust/` on this worktree.

```
cargo build
cargo test -p agenturm-ui --lib
cargo test -p agenturm-app --lib
cargo test -p agenturm-ui -p agenturm-app
# this environment links many integration binaries at once; the clean
# qualification run was serial to avoid a transient linker bus error:
CARGO_BUILD_JOBS=1 cargo test -p agenturm-ui -p agenturm-app -- --test-threads=1
```

Results:

- `cargo build` — **passed** (full workspace, debug).
- `cargo test -p agenturm-ui --lib` — **112 passed, 0 failed**.
- `cargo test -p agenturm-app --lib` — **182 passed, 0 failed**.
- `CARGO_BUILD_JOBS=1 cargo test -p agenturm-ui -p agenturm-app -- --test-threads=1`
  — **exit 0**, 25 test binaries green (3 unit targets, 20 integration
  targets, 2 doc-test targets), 0 failures.
- UI-04 focused UI tests — **15 passed** (wide/compact navigation, unchanged
  Enter, resize source-line preservation, live arrivals keeping the selected
  request, page identity/cap/empty scope,
  denied/revoked/expired/late/out-of-scope/missing-capture, worker disclosure,
  availability rendering at three sizes, unnormalized presentation, reader
  PgUp/PgDn/End, existing private-reader presentation tests).
- UI-04 focused app tests — **5 passed** (deterministic pages, cap disclosure,
  empty scope, duplicate/timestamp identity, missing identity before read).
- Fixture-backed prompt/investigation integration (real plugin host + debug
  adapters, not skipped):
  - `preview_requires_a_current_grant_and_exact_committed_pi_bytes` — **passed**
  - `codex_preview_reveals_only_exact_completed_native_user_items` — **passed**
  - `codex_edit_and_command_details_require_exact_occurrences_and_independent_approvals` — **passed**
  - `claude_and_kiro_preview_the_exact_native_user_rows` — **passed**
  - `failure_investigation_includes_only_reported_log_errors_with_exact_scope` — **passed**
  - `failure_candidates_page_retained_metadata_at_one_snapshot` — **passed**

The fixture runs used
`AGENTURM_RUNTIME_PLUGIN_HOST=target/debug/agenturm-plugin-host` with the
matching `target/debug/agenturm-{pi,codex,claude,kiro}-harness` adapters. No
test is claimed beyond what was executed here.

---

## 8. Exact hooks for UI-03 / shell integration

### 8.1 Model ownership

Add to `Workspace` (owned by UI-03 in `workspace.rs`):

```rust
prompt_nav: Option<prompt::PromptNavigator>,
prompt_anchor: Option<WorkAnchor>,   // exact source/session/worker/turn
prompt_read_epoch: Option<u64>,      // echo from PromptInput::Read
```

Construct with `PromptNavigator::new(PromptScope { source, session, worker,
turn, capture, main_session })`, then one `begin_load()` +
`accept_page(token, entries, meta)` when the prompt page arrives. Convert the
existing `PromptEntry` rows with `PromptRequest::from(&entry)`; attach labels
only from adapter-normalized facts. `PromptCapture` must come from the source
capability/policy, never inferred from an empty list.

### 8.2 New focus variants (`keymap.rs`, UI-03)

- `Focus::PromptNavigator`
- `Focus::PromptChooser`
- `Focus::PromptReader`

`PromptReader` must have the **same precedence as `PrivatePrompt`**: before
help and palette, `Ctrl+C` remains interrupt, `F1` is not routed into the
granted body, Escape clears the body and returns to the previous prompt focus.
`PromptNavigator`/`PromptChooser` are public-list focuses and precede history
page keys.

### 8.3 Dispatch (`workspace.rs`, UI-03)

While the Prompts section is active, route this key set to
`PromptNavigator::key` **before** global history actions (`[`, `]`, `l`, `z`)
and before global navigation:

```
Up Down Home End Enter Right Left Space [ ] l z m Esc
```

Return semantics:

| `PromptInput` | Shell action |
| --- | --- |
| `Read { entry, original, epoch }` | Send the existing `WorkCommand::RevealPrompt(Box::new(entry), delivery)`, store `delivery` receiver and `epoch`. On delivery call `navigator.accept_reader(epoch, (event_id, revision), text, receiver, now)`; then run the shared `active`-gate/redraw/ACK path. `original` must be applied to the constructed screen (`PromptScreen::set_original`) or the typed presentation path. |
| `Handled` / `Ignored` | No shell action. `Ignored` may be used to report an unavailable action. |
| `Back` | Leave the Prompts section for the inspector tabs; keep the selected session and inspector scroll. Increment the navigation generation. |
| `ReaderClosed` | Focus is already returned to navigator/chooser; redraw. |
| `MainSession(anchor)` | Only after re-resolving the anchor against the roster; open `WorkCommand::Prompts(anchor)`. Never construct a main-session identity. |
| `LoadEarlier` | Call the app's prompt page command with the cursor; replace/append only through `accept_page`. |
| `Retry` | Re-issue the current page at the same exact scope/snapshot. |
| `NoRequests` | Show the current status; no read. |

On reader close, revocation, expiry or back: call `navigator.take_reader()` /
`close_reader()` / `revoke()` / `expire()`; move `acknowledge` into the existing
`redact_then_acknowledge`, redraw **without** the body, then send the ACK. This
is the same contract as `dismiss_prompt`/`expire_prompt` today.

### 8.4 Rendering

In `draw_workspace_base`, when `prompt_nav` is active render the inspector
section tabs and then `navigator.draw(frame, area)` instead of the flat
Workbench Prompts list. Use `navigator.status_line()` for the view status and
`navigator.hints_for(width)` in the footer. Keep the full-screen
`PromptScreen` path for all non-prompt private content.

### 8.5 Action inventory, palette and help

Add one `keymap::Action` row per user-visible action (UI-02/UI-03):

| Proposed id | Keys | Context | Palette availability |
| --- | --- | --- | --- |
| `prompt.read` | `Enter / →` | PromptNavigator, PromptChooser | enabled with a selection |
| `prompt.older` | `[` | PromptNavigator | "No older loaded request" |
| `prompt.newer` | `]` | PromptNavigator | "Already at the newest loaded request" |
| `prompt.latest` | `l` | PromptNavigator | enabled with a selection |
| `prompt.presentation` | `z` | PromptNavigator, PromptReader | "Readable presentation unavailable" when unnormalized |
| `prompt.back` | `Esc / ←` | all three prompt focuses | enabled |
| `prompt.main-session` | `m` | PromptNavigator (worker only) | "Worker has no verified main session" |
| `prompt.earlier-page` | `Tab to Earlier prompts, Enter` | Prompts section | "No earlier retained requests" |
| `prompt.retry` | `Tab to Retry prompts, Enter` | Prompts section | "No failed prompt load" |

`keymap::hint` must use these ids; `help_sections` must add a PROMPTS section
showing the exact source/session/worker scope, loaded count, retained cap,
reader expiry and the Escape order (chooser → reader → section → roster).
Palette `view.prompts` should open the inspector Prompts section, not the
standalone Workbench list.

### 8.6 App-side typed result (UI-03 + app owner)

`WorkResult::Prompts(Vec<PromptEntry>)` currently cannot express capture
policy, denial or retained-cap facts. Add a typed result (name is the app
owner's choice):

```rust
PromptHistory {
    entries: Vec<PromptEntry>,        // newest first
    retained: usize,
    retained_cap: usize,
    earlier: bool,
    limited: bool,
    availability: PromptAvailability, // CaptureDisabled | MissingIdentity |
                                      // EmptyScope | Denied(reason) | Unavailable(reason)
}
```

and a page command carrying `anchor` plus an optional `prompt_history::PromptCursor`.
`prompt_history::load_page` already returns these facts; map
`PromptHistoryError` 1:1 to availability. Keep provider logic in the adapter.

### 8.7 Typed presentation seam (adapter → app → UI)

The current `PromptDelivery` sends raw text bytes. To remove the compatibility
heuristic, extend the prompt delivery/result with a typed presentation:

```rust
PromptPresentation {
    original: String,
    readable: Option<String>,   // adapter-normalized body
    hidden_context: usize,      // count of typed leading context blocks
    truncated: bool,            // reader/export limit reached
}
```

Adapters normalize their own envelopes and send the facts; the UI calls
`PromptNavigator::accept_reader_presentation`. Until then `accept_reader` uses
`legacy_envelopes`, and an unknown normalization is labelled unavailable. This
keeps provider parsing out of `workspace/prompt.rs`.

---

## 9. Remaining gaps

1. **No shared dispatch yet.** The navigator is not reachable from the built
   cockpit until the UI-02/UI-03 integration pass adds the fields, focus
   variants, dispatch arms and render hook above. This is the lane boundary, not
   a hidden claim of parity.
2. **App result lacks typed status/page facts.** `prompt_history` is ready, but
   `WorkResult::Prompts` does not carry it. Paging from Tab/Earlier is therefore
   not user-reachable yet.
3. **Prompt paging is window-local.** The adapter/kernel still returns at most
   256 references with no cursor; "Earlier prompts" cannot go past that window.
   The UI labels the cap honestly rather than claiming a complete transcript.
4. **Context normalization remains adapter work.** The compatibility heuristic
   is still used for legacy deliveries; unnormalized captures are labelled.
5. **Copy/export** remains the intentional tighter no-copy policy (P-09); it
   needs explicit acceptance or a grant-aware shared seam.
6. **Mouse equivalence** (inventory P-24) is not implemented; the lane stays
   keyboard-first and records this as a keyboard-only acceptance candidate.
7. **Integrated PTY journey** (inspector → Prompts → older request → original →
   Review/Activity → return) was not run because the shared render/dispatch hook
   is not present in this branch. Component state/render tests and the real
   content/investigation fixtures were run.
8. **Investigation idle policy invalidation** (P-18) is partial: a changed source
   is refused on the next attempted read; results are not proactively cleared
   while idle.
9. **Activity/MCP/Traces/Review page identity** was not changed; it relies on the
   existing page stack and remains UI-05/shell ownership.

---

## 10. Pluggable-seam implications

- **Kernel ports / adapters:** the navigator consumes typed request metadata and
  a typed presentation; it does not parse harness files or provider envelopes.
  The only compatibility parser is explicitly named and isolated.
- **Grants:** listing, cursor proof and reveal remain in `agenturm-content`.
  The UI never issues a text read; it carries event/revision/snapshot/history
  identity back to the app. Revocation/expiry clear the body before ACK.
- **No provider branches:** request labels, capture policy and worker/main
  relationships arrive as facts. Missing facts are labelled unavailable
  (`Readable form unavailable`, `Metadata only`, `Prompt history is unavailable`).
- **Extension panes:** nothing in this lane bypasses ExtensionPlugin/capability/
  pane seams; the navigator is a generic session-evidence view over host-supplied
  facts and is only rendered when the shell owns the Prompts section.
- **Scope honesty:** out-of-scope rows are dropped with a visible count, exact
  worker scope never inherits untagged or sibling requests, and main-session
  navigation is offered only from a caller-verified anchor.
