# Keyboard navigation: audit and verification

The core workflow must be usable without a mouse. Press **F1** from any screen for a persistent, scrollable reference; it starts with the current workflow. **Esc** or **F1** returns to the same control. **?** opens the reference in the monitor; ordinary question marks stay text in search fields and editors.

## Agents and prompt reading

| Action | Keys | Focus / behavior |
| --- | --- | --- |
| Choose an agent | ↑ / ↓ | Agents list |
| Expand / collapse subagents | → / ← | Agents tree |
| Open inspector | Enter or e | Starts on Overview / Prompts / Details tabs |
| Choose section | ← / → | Tab strip |
| Enter section | Enter or ↓ | Prompts enters the request navigator; other sections enter their text |
| Choose directly from Prompts tabs | ↑ / ↓ | Enters request navigation and chooses older / newer immediately |
| Switch section from content | Ctrl+← / Ctrl+→; m cycles | Entering Prompts focuses its navigator |
| Choose a request in the wide index | ↑ / ↓ | Chronological list; starts at latest |
| First / latest loaded request | Home / End | Wide request index |
| Read the chosen request | Enter or → | Wide index |
| Open compact request choices | ↑ / ↓, Enter or Space | Standard dropdown below 120 columns |
| Choose in the open dropdown | ↑ / ↓, Home / End | Highlight a request; Enter confirms and reads, including an unchanged choice |
| Read without opening dropdown | → | Compact request picker |
| Return from text to request navigation | ← | Prompt text reader |
| Previous / next loaded request | [ / ] | Expanded Prompts; works from the reader as well |
| Latest loaded request | l | Expanded Prompts |
| Scroll the current request | ↑ / ↓, PgUp / PgDn, Home / End | Text reader; these do not choose another request |
| Read original captured context | z | Toggles readable / original request |
| Copy / preview export | c / x | Current request or selected evidence |
| Load earlier requests / retry | Tab to Earlier prompts / Retry prompts, Enter | Shown only when available |
| Review recorded work | Ctrl+R | Selected agent in Agents |
| Return to selected agent | Esc | Closes an open chooser first |

The footer changes with focus. A prompt navigator says **Choose**; a text reader says **Scroll**. Tab and Shift+Tab move through visible inspector controls. At wide sizes the chronological index sits beside the reader. At 80×24 and 60×18, the compact dropdown keeps the text area available. Crossing that breakpoint maps navigator focus to the equivalent control; a focused reader stays a reader. Live arrivals preserve the selected request and reading position.

Prompt history is bounded to 300 retained requests per selection. Earlier prompts loads an additional page when available. If the limit is reached, the status identifies it and directs the user to scoped Activity history; the UI does not claim a complete transcript.

## Other core navigation

| Workflow | Keyboard path |
| --- | --- |
| Monitor views | 1–7: Agents, Activity, MCP, Traces, Host, Metrics, Findings |
| Search | /; type normally; Esc returns to results |
| Results and details | ↑ / ↓ selects; Enter inspects; e expands; Esc returns |
| Activity / MCP / Traces history | [ newer page, ] older page, l latest |
| Related evidence | s activity, t trace where available; Alt+← returns to a previous evidence location |
| Agent family | Alt+↑ parent, Alt+↓ first child, Alt+→ descendant needing attention |
| Agent layout and order | f tree / flat; o sort options |
| Command palette | Ctrl+P |
| Workflow inbox | w; ↑ / ↓ works immediately; Enter reads an incident; Tab reaches actions |
| Workflow back | Esc leaves settings, then narrow detail, then the inbox |
| Review | Starts on scrollable evidence; R refreshes; c copies; x previews export; Tab reaches earlier evidence, Save task and Excerpt |
| Saved tasks | b; ↑ / ↓ and Enter selects and opens; n creates |
| Saved task detail | Starts on scrollable text; p Brief, d Decisions, e Edit, l Link, h Handoff |
| Saved task maintenance | r captures workspace; Ctrl+R reloads saved task |
| Decisions | Starts on the version chooser, or Add when empty; Enter opens choices; arrows choose; n adds; s supersedes |
| Editors | Tab / Shift+Tab fields; Ctrl+S saves; Esc closes a dropdown before handling unsaved edits |
| Handoff | PgUp / PgDn reads; Tab reaches copy, destination and write |
| Copy selection | Ctrl+C copies selected evidence or a text field's selection |
| Quit | q from the monitor; text fields retain normal typing |

Every visible button is reachable with Tab / Shift+Tab and activated with Enter or Space. Letter shortcuts belong to their screen; digits, letters, brackets and question marks remain literal while editing. Native dropdowns handle their own arrow keys and Escape before their parent screen handles Back.

## Audit findings and changes

1. **Prompts entered its text instead of its navigator.** Up/Down therefore scrolled one request, while changing requests required discovering several Tab stops. Entering Prompts now focuses the wide index or compact picker; Enter/Right reads, and Left returns to navigation.
2. **Resizing wrapped table row identities repeatedly.** The internal render cache now keeps string keys, preserving exact row and request selection through repeated column rebuilds.
3. **Several core screens opened without a useful focus target.** Workflow now focuses the queue, or its report when empty. Saved-task detail focuses its text. Decisions focuses its version selector, or Add when empty.
4. **Priority Escape bypassed open task dropdowns.** Dropdowns now consume Escape first. A dirty editor still requires explicit confirmation before discarding its draft.
5. **Keyboard help disappeared before it could be read.** F1 now opens a persistent, scrollable reference from the monitor, modal screens and editors, returning to the prior control and preserving drafts. The current workflow appears first.
6. **Unchanged choices and tab focus were omitted from tests.** Compact Enter confirmation now reads even without a changed value. A section change explicitly refreshes its footer even when the tab strip keeps focus.
7. **Long compact choices consumed several rows each.** Request choices now stay on one line with an ellipsis. The reader retains the captured request text; truncation only affects the navigation label.

## Verification

The new keyboard journeys cover wide and compact prompts, every loaded request, current-value confirmation, navigator versus reader scrolling, original selection across resize, help return after resizing, literal text entry, selector-first Escape and unsaved drafts. Existing Workflow and inspector tests now assert entry focus instead of manually supplying it. Independent peer review approved the final source without blocking findings and reran the affected compact/help cases. The broader local UI integration passed 45 tests; final direct-tab-arrow and hint checks also passed. All six new keyboard journeys passed against the final built wheel in an isolated Python 3.13 environment, and its Python module bytes match the source.

Code commit `e8ef81f4b35a9e6219075a3b579b66b6b33ae4fb` passed all 12 jobs in [CI run 43174305004](https://github.com/abhishekash/agenturm/actions/runs/43174305004), including the full 603-test suite on Python 3.11–3.14 and every JavaScript/SDK integration job. Base Python environments skip 14 optional integration cases, whose dependencies are exercised by separate SDK jobs. The final documentation and repeatable tmux-check commit may still have an automatic CI rerun pending at admin merge; the check itself passed its 20-step journey and help invocation locally.

Real-terminal verification passed 20 checks in an isolated **tmux 3.4** server, at **120×32, 80×24 and 60×18**. It used synthetic records and private config/state directories. Delivered keys covered direct Up from Prompts tabs, every navigator direction, Home/End, Enter/Right reading, Ctrl+Left/Right section changes, unchanged compact confirmation, F1 help and return after resize. Normal quit returned status 0 and restored the complete original termios settings. This is independent of headless Pilot tests. It does not establish remote SSH behavior, every outer terminal emulator, participant comprehension or multi-hour soak stability.

Repeat the actual-key journey from a development checkout with tmux installed:

```sh
uv run python scripts/verify_keyboard_tmux.py
```

The check creates a private tmux socket, removes it on exit, prints a JSON result, and exits nonzero with the captured frame on failure. It never attaches to an existing tmux server or uses personal session records.

The self-contained HTML guide embeds actual rendered states. It was inspected at desktop/light (1280 pixels) and mobile/dark (390 pixels), with all three images loaded and no horizontal page overflow.
