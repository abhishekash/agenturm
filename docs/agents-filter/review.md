# Agents filter: compact recent sessions

Implemented on fix/agents-filter in an isolated worktree.

## Changes

The picker lists recent sessions first and keeps All agent sessions as the reset choice. Each choice occupies two physical lines, regardless of prompt length. Project, runtime and short worker or agent identity remain visible; long projects are clipped before identity. Missing tasks use Worker or Session plus the short identifier. Current labels the applied scope; the cursor previews a different choice until Enter applies it.

Search starts focused and matches full task text and identities. Arrow keys browse without clearing the query. Escape cancels without changing scope. Exact shared-session worker isolation and disappearance checks are preserved. The roster stays frozen while reading.

## Layout and clutter review

One outer border; no nested boxes. Replaced paragraph-height choices and ambiguous checkmarks with two-line rows and a single Current label. Long titles and preview lines use terminal-cell-aware ellipses. At 60×18 and 80×24, state and recency move to the preview, leaving identity in the list. Below 60×18 the resize message replaces controls; Escape remains available. Themes and monochrome retain text-based state distinctions.

## Validation

Eight picker tests passed: seven tests in the final focused suite plus one additional untitled-worker and long-project identity test. Coverage includes 147 choices, Unicode, ordering/ties, search beyond clipped text, themes, resizing, empty results, cancel/apply/reset, exact shared-session workers and disappearing choices. git diff --check passed.

Screenshots below use synthetic tasks and a roster of 146 sessions plus the reset choice. The before image uses the original main checkout; after images use the isolated implementation. Final frames were visually inspected at 120×36 (dark), 80×24 (light) and 60×18 (dark). Full repository tests and real-terminal PTY checks were not run for this picker-only change.
