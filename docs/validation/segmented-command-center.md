# Segmented command center

Design review for the Agenturm task roster, 2026-11-27. Checked with public-metadata fixtures, isolated session histories, and the native terminal app.

## Intended behavior

The main loop is to find a task, identify its recorded state, and open its evidence. The screen separates application destinations, time and state filters, the task list, and the selected task. Context and commands follow the task instead of competing with its title.

This remains a full-screen Ratatui application with a master/detail layout on wide terminals and a task-first drill-down layout on smaller terminals. Existing session identity, ordering, tree expansion, filtering and inspector navigation remain the behavioral contract.

## Layout sketches

These sketches show hierarchy, not pixel-perfect output or test results.

```text
Wide (120+ columns, at least 24 rows)
Agenturm command center                                      F2 Themes  ? Help
╭ Workspace ─────────────────────────────────────────────────────────────────╮
│ Agents   Activity   MCP       │ Traces   Host   Metrics   Findings          │
╰────────────────────────────────────────────────────────────────────────────╯
Today / History                                            Group: Project
State filters
╭ Tasks ─────────────────────────────────────────╮ ╭ Selected task ─────────╮
│ Task                         Status   Updated  │ │ Task title              │
│ agenturm · 171                                  │ │ Recorded state / agent  │
│ › Implement agent hierarchy  Ready    4h ago    │ │ Project                 │
│   Assess plugin architecture Ready    12h ago   │ │ Recent activity         │
│                                               │ │                         │
│                                               │ │ Open task               │
│                                               │ │ Enter / P / s           │
╰───────────────────────────────────────────────╯ ╰─────────────────────────╯
Loaded-page scope / collection summary
Enter inspect   / search   Tab filter   : commands   ? help

60 columns (18-row minimum)
Agenturm command center                  F2 Themes  ? Help
╭ Workspace ───────────────────────────────────────────────╮
│ Agents   Activity   MCP                   : More views   │
╰──────────────────────────────────────────────────────────╯
Today / History                           Group: Project
State: All                                      Tab change
╭ Tasks ───────────────────────────────────────────────────╮
│ Task                                Status   Updated     │
│ agenturm · 171                                             │
│ › Implement agent hierarchy         Ready    4h ago      │
│   Assess plugin architecture        Ready    12h ago     │
│                                                          │
│                                                          │
│                                                          │
╰──────────────────────────────────────────────────────────╯
Loaded-page scope / collection summary
Enter inspect   / search   Tab filter   ? help
```

## Rendered frames

[Open the embedded wide, narrow and monochrome previews](segmented-command-center.html#rendered-frames). The previews render actual Ratatui cells from controlled fixtures.

## Clutter audit

| Signal | Previous roster | Design change |
|---|---|---|
| Borders | One preview border; navigation, filters and roster run together | One contour per functional region, no outer application frame or nested boxes |
| Task rows | Every ordinary task occupies two rows; the second commonly repeats only `Codex` | Ordinary tasks use one row; additional context appears only when it distinguishes an agent or task |
| Repeated boilerplate | A full project path heads the group and repeats in the preview; raw completion reasons and model-provider metadata fill the preview | Short, disambiguated project headings; bounded project path in preview; full metadata stays in the inspector |
| State | Large counters for every state, including zero values and an additional unknown counter | Wide state controls; compact active-state control; explicit state words remain in task rows |
| Selection | Strong emphasis across task names, controls and status text | Main selection should have the strongest emphasis; idle metadata stays quiet |
| Footer | Three or four dense lines of scope, shortcuts, project provenance and collector internals | Two lines with primary commands and a short status; full diagnostics in scrollable help |
| Always-on markers | Repeated provider subrows carry no distinction when all tasks use one provider | Tree markers only for expandable rows and a selection marker only for the selected row |

For 100 ordinary tasks without distinguishing secondary context, task content falls from 200 rows to 100 rows: a 50% reduction in row consumption. Contours and filter segmentation add a fixed amount of chrome. This is a layout calculation, not a measured whole-screen cell percentage. Total chrome-to-data ratio depends on terminal size, grouping and secondary context; final rendered frames must be reviewed at the floor as well as the wide size.

A status uses at most a word and one semantic color. Meaning does not depend on color. The design keeps border nesting at depth one.

## Responsive and state behavior

| Surface | Behavior |
|---|---|
| 120+ columns, 24+ rows | Task list with selected-task preview. The preview pins primary actions and bounds optional activity. |
| 80×24 | Full-width task list. Enter opens the inspector; Esc returns to the selected task. Secondary navigation is available through the command palette and help. |
| 60×18 | Single task pane; mixed-provider lists fold the update column to preserve task names; active filter remains explicit; names truncate by terminal cell width. Primary shortcuts remain visible. |
| Below 60 columns or 18 rows | An honest minimum-size notice replaces the roster. |
| Loading / unavailable / empty | Distinct body messages with retry or refresh guidance. |
| Search with no matches | Empty result message and reset guidance, without stale selected-task details. |
| Partial collection or a failed source | Concise status remains visible; full collector notice is reachable in help. Loaded-page coverage must remain discoverable. |
| More retained sessions | A selectable “Show more” row requests the next exact page. Counts describe the loaded page. |
| Monochrome | Labels, contours and selection shape preserve hierarchy without color. |

## Verification

| Check | Status |
|---|---|
| UI state and existing navigation tests | Passed: 469 app/UI unit and integration tests |
| Pinned wide, 80×24 and 60×18 rendered frames | Passed: 160×40, 120×32, 120×24, 80×24 and 60×18; 63 exported frames across color, monochrome and Current theme |
| Monochrome and Unicode-width rendering | Passed: reset colors, reversed selection, CJK, combining marks and ZWJ emoji |
| Empty, loading, disconnected, partial and paged states | Passed: distinct states and full collection status in help |
| Prompt history, Activity, MCP and Tools terminal journey | Passed at 120×32, 80×24 and 60×18: grant, read, redaction and terminal restoration |
| Terminal resize and restoration | Passed: 6 native terminal journeys covering resize, too-small recovery, help/palette, q and Ctrl+C restoration |
| Application and UI Clippy | Passed: all targets with warnings denied |
| Native bundle frame parsers | Passed: 7 parser regressions and both native default-cockpit PTY qualification modes |

The checks use synthetic public metadata and isolated histories. No live user history database was used for layout qualification.

## Reproduce

```sh
env -u NO_COLOR AGENTURM_UI_CAPTURE_DIR=/tmp/agenturm-ui-frames cargo test --offline -p agenturm-ui command_center_pinned_frames
NO_COLOR=1 AGENTURM_UI_CAPTURE_DIR=/tmp/agenturm-ui-frames cargo test --offline -p agenturm-ui command_center_pinned_frames
python3 scripts/render_rust_command_center.py /tmp/agenturm-ui-frames
cargo test --offline -p agenturm-ui -p agenturm-app
cargo clippy --offline -p agenturm-ui -p agenturm-app --all-targets -- -D warnings
```

Current-theme captures use the existing preferences loader with an isolated `XDG_CONFIG_HOME/agenturm-rust/ui.json` containing `{"theme":"Current"}`, plus `AGENTURM_UI_CAPTURE_THEME=Current AGENTURM_UI_PREFERENCES=1`. Each color profile runs in a fresh test process.
