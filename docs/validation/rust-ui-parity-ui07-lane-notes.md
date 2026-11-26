# UI-07 · Shared overlay visual and responsive follow-up

24 November 2026 · baseline `a633c68` · branch `work/ui-parity-visual-resume` ·
worktree `/tmp/opencode/agenturm-ui-parity-visual` · Python oracle **v0.2.8**
(`2802d79930728bb331db1d4d718d6dea1c9c374d`)

**Status: scoped implementation; Rust tests added, not executed. Full UI parity
and installed/paired acceptance are not claimed.**

[Self-contained HTML companion](rust-ui-parity-ui07-lane-notes.html)

## Scope and reference

Exclusive code ownership is `rust/agenturm-ui/src/{theme,palette,help}.rs`.
No shell dispatch, `lib.rs`, `keymap.rs`, `workspace.rs`, `workbench.rs`, app,
kernel, adapter or other screen was edited. The main agent is recovering shell
integration separately. The plan, shared Markdown/HTML contracts, UI-07 inventory
rows and UI-03/04/05/06 lane notes were read before editing.

Tagged `themes.py`, `ui_layout.py`, `terminal_ui.py`,
`tests/test_terminal_themes.py` and `tests/test_inspector_layout.py` were inspected
using `git show`, without checking out the tag. These establish palette roles,
compact evidence priority, the prompt breakpoint and focus expectations. They
are **source reference, not paired frames or executed Python tests**. Python's
theme chooser is Textual-owned (inventory V-02); exact rendering still needs its
installed wheel/dependency version.

## Concrete fixes

| Owned surface | Before | After |
| --- | --- | --- |
| Appearance | Below its threshold it returned without drawing, leaving an invisible active picker. Selection depended on background/bold. | Below its measured overlay floor it clears the frame and shows an Escape-first resize notice. Every selected theme has `›` plus semantic selection styling. |
| Appearance save error | A long wrapped error could hide cancel/retry; only the happy save path had coverage. | Cancel/retry precede a separate, clipped save-failure line. Preview remains reversible on error. Tests exercise failure, Esc restore, Space retry/apply, owner-only file mode and fresh-picker reload. |
| Palette/session chooser | A wrapped duplicate selected label could consume both detail rows and hide the unavailable reason. Long queries hid the editing cursor. Raw labels/groups were not escaped by this renderer. | Both detail rows show the reason, with an ellipsis on truncation. Labels clip on grapheme/cell boundaries; query tail and cursor stay visible; displayed controls are escaped. Low-priority group text is omitted before subject text. |
| Palette accessibility | Unselected disabled rows were color-only; undersized layouts could reserve no useful result rows. | Disabled rows say `[Unavailable]`; selected rows retain `›` and bold/reverse selection. The measured minimum reserves a visible selected result, two detail rows and cancel hint. No-match copy keeps `Ctrl+A` visible even at the overlay minimum. |
| Session count | Always subtracted one entry, even if no “All sessions” row was supplied. | Excludes only the explicit `session\|all` entry. No provider/session identity inference. |
| Help | Scroll calculations used width `area−4` and height `area−2`, but actual text used width `area−2` and height `area−3`. End could not reliably reach the final row. Wrapping iterated characters and clipped headings. | Scroll and rendering use the same inner rectangle and grapheme wrapping, including headings. End reaches the actual last body row. Resize preserves the logical line and display-byte anchor; too-small rendering leaves that anchor untouched. `usize` skipping avoids a `u16` scroll wrap. |
| Shared fallback | Resize messages could be drawn over old content, with Escape clipped off the far end. | Owned overlays clear the frame and put `Esc back · resize` first. Fallback does not cancel a preview/query or reset Help's reading position. |

## Screen budget and size contract

The five original RGB palettes and their names are unchanged: **Observation,
Current, Bright, Dark, Solarized**. All changes use semantic roles and the existing
selection/NO_COLOR helpers. There is no second action/key inventory, provider
branch, content read, private-text cache or execution path in these modules.

| Terminal | Appearance popup / list rows | Palette popup / result rows | Help inner body |
| --- | --- | --- | --- |
| 120×32 | 68×14 / 10 (five choices) | 88×24 / 17 | 118×29 |
| 80×24 | 68×14 / 10 (five choices) | 78×22 / 15 | 78×21 |
| 60×18 | 58×14 / 10 (five choices) | 58×16 / 9 | 58×15 |

These are **source-derived geometry**, not observed installed frames. Every
overlay has one border. Help's footer is outside its border; the palette has
one search line, one count line, two reason rows and one hint line. At 60 columns
Escape and help/picker controls remain visible without nested borders.

Measured component floors (outer gutters and borders included):

- Appearance **22×11**: five options plus two hint/error rows.
- Palette **22×10**: one scrollable selected result plus two detail rows.
- Help **20×6**: three text rows plus border/footer.

Below those floors the owned component displays its resize notice. Escape is
still consumed by the same overlay state machine. Extremely tiny buffers such
as 1×1 are panic-safety cases, not readable interfaces. These component minima
**do not change or certify the application's global minimum** (baseline
`lib.rs` still declares 38×8; Python's acceptance floor is 60×18).

## Added regression coverage — execution pending

All new Rust cases use existing Ratatui `TestBackend`/buffer or pure state seams;
no harness or dependency was added. In addition to the existing overlay tests:

- `theme::tests::overlay_text_keeps_graphemes_and_cell_budgets`: CJK, combining
  acute, skin-tone/ZWJ emoji, flags, variation selectors, control escaping and
  clipping budgets, including a glyph wider than the region.
- `theme::tests::essential_overlay_text_and_selection_have_readable_rgb_contrast`:
  supplied RGB body/search, selected text, overlay muted hints and status pairs.
- `theme::tests::failed_save_keeps_preview_cancel_and_space_retry_reopen_work`:
  persistence failure, cancel, retry/apply with Space, reload and Unix 0600.
- `theme::tests::appearance_selection_has_a_marker_and_semantic_style_in_every_theme`:
  all five themes at 120×32, 80×24, 60×18 and 22×11; exact selected semantic
  background/foreground or NO_COLOR reverse/bold/reset-color assertions.
- `theme::tests::undersized_appearance_replaces_the_frame_and_keeps_escape_working`:
  both floor boundaries, 19×5 and 1×1; preview identity is retained until Esc.
- `palette::tests::selected_long_label_does_not_hide_the_unavailable_reason`:
  reason, Unicode label, ellipsis, selection style and disabled activation at the
  three acceptance sizes and the component minimum.
- `palette::tests::resize_keeps_selected_identity_and_the_editing_end_of_the_query_visible`:
  long Unicode query, exact selected key and suffix/cursor through resize/fallback.
- `palette::tests::empty_and_no_match_states_keep_cancel_and_clear_reachable`:
  distinct empty/no-match copy, no fabricated selection, clear and recovery.
- `palette::tests::session_count_excludes_only_an_explicit_all_sessions_entry`:
  correct session count both with and without a supplied “All sessions” row.
- `palette::tests::minimum_palette_scrolls_to_the_selected_result_and_below_floor_clears`:
  selected-last-row visibility at 22×10 and non-destructive fallback below it.
- `help::tests::end_reaches_the_actual_last_body_row_at_each_size`: true inner
  geometry, final entry on the final content row, Home and intact Unicode.
- `help::tests::help_reflow_preserves_the_logical_reading_anchor_and_too_small_is_non_destructive`:
  wide/compact reflow, logical line/byte containment and Esc.
- `help::tests::headings_wrap_and_terminal_controls_are_displayed_as_text`:
  a genuinely wrapped heading, escaped controls and NO_COLOR reset colors.

**Run here:** `rustfmt --edition 2024 --check` on the three owned modules and
`git diff --check` — both clean. Static Python checks confirmed all **80 RGB
roles match the tagged Python source** and the owned overlay contrast pairs
exceed 4.5:1. The HTML parsed with Python's standard HTML parser, contains one
embedded stylesheet and no external asset elements; no browser visual check
was run. No Cargo
build, check or test was run: the coordinating agent explicitly prohibited
builds because of memory pressure. No existing lane's earlier green result is
attributed to this commit.

### Commands for the integrating agent

From `rust/`, after memory is available (NO_COLOR must use a separate process;
`colors()` caches the environment):

```sh
CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui --lib theme::tests
CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui --lib palette::tests
CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui --lib help::tests
NO_COLOR=1 CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui --lib theme::tests
NO_COLOR=1 CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui --lib palette::tests
NO_COLOR=1 CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui --lib help::tests
CARGO_BUILD_JOBS=1 cargo build --offline --locked
CARGO_BUILD_JOBS=1 cargo test --offline --locked -p agenturm-ui -p agenturm-app
```

## Contrast observations and remaining visual findings

Static sRGB contrast arithmetic gives minimum essential text/background ratios
of 12.87, 14.61, 13.68, 13.87 and 10.50 for the five themes respectively;
selected text/background minima are 11.28, 13.76, 11.97, 9.71 and 7.17. Overlay
muted text on `surface` and status colors on `surface` exceed 4.5:1. These are
RGB calculations, **not terminal-emulator or ANSI-fallback measurements**.

**Finding:** the unchanged tagged Solarized `muted` on `panel` pair is **4.23:1**,
below the essential-text 4.5:1 target. Owned muted hints/counts use `surface`;
search text uses `text` on `panel`. Other screen owners must audit uses of
muted-on-panel or approve a semantic fallback/color change. This lane does not
claim full-theme contrast qualification. NO_COLOR delegates default contrast
to the terminal and preserves structure; it does not force black/white colors.

Other findings are reported without editing their owners' files:

1. **Full-screen compact layout/floor:** `lib.rs`, `evidence_view.rs` and shared
   workbench/shell modules retain their own floor/layout policies. Component
   fixes do not establish one active pane at 60×18 or a coherent global floor.
2. **Global character wrapping:** `lib.rs::wrap_display_width` still iterates
   characters, not graphemes. Only these overlays now use `theme::wrap_cells`;
   other evidence/detail callers still require CJK/combining/emoji audit.
3. **Long palette values:** labels/reasons explicitly disclose clipping, but
   there is no scrollable full-label/detail reader in the palette. The selected
   session's inspector remains the intended full-value route. Arbitrarily long
   disabled reasons/identities need explicit acceptance or a shared detail seam.
4. **Focus/overlay integration:** help-over-editor/chooser, F2 theme dispatch,
   focus restoration after close, private-content fences and prompt reflow are
   shell-owned. No integrated key-delivery or prompt-reader journey was run.
5. **State matrix:** this patch covers overlay populated, empty, no-match,
   unavailable, save-error/retry and too-small states. Startup, async loading,
   partial/stale/disconnected, permission revocation, refresh safety and cached
   permitted evidence need full-screen owner/UI-Q runs. It does not fabricate
   these states inside data-only overlays.
6. **Terminal restoration:** quit, interrupt, failed load and supported handoff
   need real PTY termios checks, including private-frame clearing/ACK. This lane
   does not own terminal lifecycle and did not run them.

## Deliberate differences and acceptance gates

- Rust keeps F2 appearance and a dedicated five-choice picker; Ctrl+P remains
  the data-driven cockpit palette. Exact Python framework chooser/mouse parity
  is not claimed. Palette reasons now take priority over repeating the title.
- The approved project-grouped landing screen, prompt no-copy policy, remaining
  provider capability gaps and workflow/task gaps are unchanged; see the
  UI-03/04/05/06 notes rather than interpreting this lane as their completion.
- UI-Q must pair the **same fixture** in installed Python and Rust at 120×32,
  80×24 and 60×18, all five themes plus NO_COLOR, every root view/inspector and
  state, and below-floor resize/quit cases. Record exact source/executable hashes,
  wheel/dependency versions, OS, terminal environment and capture paths.
- Re-run actual inspector → Prompts → older request → original → Review/Activity
  → return, forms/drafts/help/chooser resize, copy/export and receiver cases.
  Buffer assertions cannot certify terminal rendering, mouse equivalence,
  clipboard behavior, permissions, live response races or termios restoration.

**Pluggable seam impact:** none changed. Palette entries remain host-supplied
metadata with stable keys; themes remain semantic; Help reads the existing
inventory. No provider-specific rendering, fallback data or bypass was added.
