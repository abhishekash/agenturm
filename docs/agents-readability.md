# Agents readability: assessment and implementation

The reported screenshot exposed three practical failures: Claude runtime scaffolding was promoted to task identity, dense metadata competed with each task, and leaf rows barely indented beyond parent titles. Internal health labels also required users to understand collection mechanics before deciding what needed inspection.

## Plan and peer review

1. Correct task projection at its shared parser. Separate complete known runtime envelopes, consume recognised operational controls in either block order, and suppress canonical truncated caveat labels from old collectors. Preserve captured records and substantive requests.
2. Give every node a fixed disclosure slot and four cells per tree level. Continue ancestor guides through metadata and spacing. Retain exact recorded relationships, expand/collapse behaviour, and independent parent/child outcomes.
3. Use a short source line and whitespace. Remove zebra bands, the duplicate selection arrow, recent action text, and routine missing-parent boilerplate. Keep relevant worker counts and hidden branch progress.
4. Present three scan categories while preserving the existing evidence model: In progress, Needs attention, Ready for review. Explain uncertainty in the inspector. Keep actual input incidents, durable acknowledgements, and notifications independent of display categories.
5. Validate native capture/restart, persisted bad-label fallbacks, privacy, state changes, keyboard navigation, selection, themes, and narrow layouts; publish verified distribution artifacts.

Independent peer review approved this scope and caught a mixed-record edge case: a model command followed by multiline local stdout could still become a title. The parser now re-projects runtime context after consuming each control envelope. Independent retesting confirmed the fix and found no remaining blocker.

## Resulting layout

At 120×36 or larger each block has two content lines and two rows of spacing. At 24 or more terminal rows there is one separator row; shorter layouts retain two content lines. The list remains virtualised and scrollable. Wide terminals keep the inspector beside it; 80×24 and 60-column layouts prioritise the list and open the inspector with Enter.

Each level gains four cells even for a leaf. Metadata aligns beneath its title, without repeating the branch connector. The selection background remains visible in supported themes. Left collapses or moves to a parent; Right expands or enters a child. Original prompts and full task text remain in the inspector.

The main states are projections, not a new source of truth. Recent or bounded quiet unfinished turns are In progress. Explicit completion is Ready for review, including a completed parent whose child is still working. Failures, pending input, or unavailable status are Needs attention, with the exact cause in the inspector. Status uncertainty does not create approval incidents or notifications.

## Verification

Regression tests cover Claude caveat/control/output combinations, raw preservation, old truncated title fallback, live capture, reader restart and prefix identity. Rendered journeys cover four-cell title and metadata alignment, collapse/expand, hidden counts, three labels, dynamic status resolution, keyboard movement, and retained selection at 160×42, 120×36, 80×24, and 60×20. Additional suites exercise existing review, inspector, themes, saved workflow and privacy behaviour. Synthetic captures were inspected in dark and light themes; they contain no user session data.

## Further polish within this scope

- Validate the hierarchy against more installed Claude Code and OpenCode transcript versions, using anonymised fixtures with explicit capture provenance.
- Tune the task/source width balance against very long project names, deeply nested workers, and CJK text without hiding precise identity in the inspector.
- Add stable rendered-frame comparisons for the approved density and indentation, alongside behaviour tests.
- Investigate the existing long-session memory growth with repeated measured runs. The layout changes do not establish memory stability.
