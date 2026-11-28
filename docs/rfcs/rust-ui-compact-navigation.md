# Rust cockpit compact-navigation contract

## Goal

Keep the command center task-first. The view switcher is navigation, not a
workspace panel, and should not take more vertical space than its labels need.

## Contract

- The command-center view switcher occupies exactly one terminal row and has no
  enclosing box.
- At widths below 100 cells, show `1 Agents`, `2 Activity`, `3 MCP`, and the
  `: More views` affordance. At wider sizes, show the existing `1`–`7` root
  destinations as well. Existing extra destinations remain reachable as before.
- Preserve the established numeric view mapping, selection identity, sorting,
  evidence/status truth, prompt-history default, and all permission checks.
- At 120×32, 80×24, and 60×18, the task panel starts on row 5 (zero-based row
  4); the compact list remains readable and selected-task inspection continues
  through the existing Enter/Escape flow.
- At 80×24, the core tasks remain visible without scrolling. No status,
  relationship, permission, or prompt evidence is inferred to improve the
  layout.

This is a focused usability slice, not a redesign of the history/evidence
screens or the command-center row model. The task panel and its existing
responsive inspector behavior remain unchanged.

## Regression coverage

`command_center_navigation_is_one_row_and_preserves_task_screen_budget` renders
the real command center into Ratatui `TestBackend` buffers at the three target
sizes. It asserts the visible navigation contract, single-row geometry, task
panel start, and compact task visibility. Existing command-center and workspace
tests continue to cover stable selection and keyboard navigation.
