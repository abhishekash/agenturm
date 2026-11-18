# Stable view navigation and session filter

## Report and reproduction

The top Agenturm panel disappeared while changing views. A synthetic keyboard/mouse journey reproduced it at 150×40: enter the Agents inspector, click Activity, then click Agents. The masthead changed from four rows to zero, back to four, then to zero again. Number-key navigation collapsed the destination inspector, making the effect differ from mouse tab navigation.

The cause was global CSS tied to local inspector state: `session-reading` hid the masthead and filter bar and compressed the navigation tabs; `compact.inspecting` also hid the masthead and controls. Returning to an expanded pane therefore moved the entire content area.

## Change

The masthead, tabs and filter-bar dimensions now depend on terminal size, not the active pane's expanded state. On short terminals the expanded session panel drops its border; the narrow Prompts mode also omits its repeated metadata row, preserving at least seven rows for prompt text at 60×18. The task heading remains visible and full metadata stays in Details. The 60-column menu layout reserves the same two filter rows across views. Expanding an inspector still replaces that view's list; it does not remove global context. Existing mouse/keyboard inspector restoration semantics are preserved.

The session selector now says **Session: All**. Its tooltip and reset option explain that it filters agents and their related evidence, while Today/History and Errors continue to apply. Choosing All clears only the session filter. It does not enable history or reset other filters; Host and global metrics are not presented as session-filtered data.

## Validation

- Before the fix, the new regression failed header visibility at 150×40, 80×24 and 60×18.
- Final corrected journey: **2 tests passed in 67.778 seconds**, traversing all seven views, expanding and returning from inspectors, using mouse tabs and number keys, and refreshing paused/live snapshots. Session-filter reset preserves the Errors filter.
- Synthetic screenshots reviewed at all three sizes. At 80×24 and 60×18 the one-line header remains visible; the single-pane reader scrolls and retains its actions. Below the existing 60×18 minimum, the existing terminal-too-small screen remains authoritative.
- The first compatibility run executed 67 tests in 385.306 seconds and found three failures: two minimum-reader space checks and a chooser test still searching the old reset label. The corrected reader preserves the original seven-row assertions; the chooser test now searches the visible All sessions label. All three corrective reruns passed in 24.093 seconds.
- No new borders, per-row markers or duplicated status indicators were added. One existing selector label now identifies its purpose directly.

**Final combined validation: 69 tests passed in 451.864 seconds.** This includes the new shell journey plus existing inspector layout, branding, observation deck, session scope picker, keyboard navigation and interruption evidence tests. `git diff --check` passed. The complete repository suite was not rerun for this layout change.

This is a targeted UI change, not an implementation of the proposed adapter/extension architecture. Restart the TUI after updating a local checkout; no package release is included.
