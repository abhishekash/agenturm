# Evidence reading surfaces

## Contract

Agenturm remains a read-only monitor. Formatting must not change stored evidence,
copy/export payloads, attribution, capture-policy checks, or reported status.
The reading path is selection → short event identity → aligned metadata → labeled
evidence → related activity/provenance. A message body is not a duplicate title.

- Panel gutters align headings, metadata and section labels. Section bodies have
  a two-cell hanging inset; wrapped metadata stays in its value column.
- Narrative Markdown renders emphasis, lists, links, fenced code and tables.
  Tables stack into labeled records below 80 content cells. Raw/original text,
  command output, JSON and diffs preserve source whitespace.
- No nested frame per field. One blank row separates sections. Long identifiers
  wrap rather than disappear. Body width comes from the actual pane, not the
  console's initial size; expanding, switching tabs and resizing reflow content.
- Existing keys, selection, exact-source identity, scrolling, raw copy and
  capture-policy invalidation remain intact. No automatic link/network access.

## Surface audit

| Surface | Treatment |
| --- | --- |
| Activity/event inspector | Short message identity; formatted body; inset sections |
| MCP inspector | Shared sections; structured arguments/results; literal output |
| Agent overview/details/prompts | Shared section rhythm; original capture stays literal |
| Trace/span inspector | Shared metadata and structured attributes; panel gutter |
| Metrics, host, findings | Shared metadata and inset evidence; panel gutter |
| Finding and search exact sources | Labeled, inset literal source fields |
| Review and Workflow | Generated report headings with inset literal bodies |
| Saved-task details/decisions | Labeled bodies, preserved authored whitespace |
| Handoff | Already formatted Markdown; responsive table treatment |
| Usage, Diagnostics, keyboard help | Keep intentional plain-text alignment; reflow at pane width |
| Export/privacy preview | Exact raw serialization, never interpret Markdown |
| Lists, inputs, editors | Keep column/tree/source indentation; not prose renderers |

## Acceptance

Use synthetic prose with emphasis, long links, a comparison table, nested bullets,
code and Unicode. Assert content, inset, wrapping, link destinations, copy
fidelity and no horizontal scrollbar at wide, 120×40, 80×24 and 60×18 sizes.
Exercise expand/back, hidden-tab updates, resize and source replacement. Existing
multi-agent, evidence, copy, theme and capture-policy regressions must still pass.

## Validation

- The initial three headless regressions failed on the 0.2.6 baseline: duplicate
  message title, raw Markdown, and missing body inset. Log:
  `/tmp/opencode/agenturm-content-baseline.log`.
- New tests cover all seven inspectors, formatted links with retained destinations,
  nested lists/tables/JSON/code, exact raw copy/export, original whitespace,
  modal reflow, hidden replacements and clearing retained content.
- Follow-up checks passed the new content tests plus trace-selection navigation
  and off-thread handoff reflow. The wider focused runs exposed label/indentation
  expectations, which now assert the new layout without removing content checks.
- Synthetic headless captures were inspected at 180×48, expanded 120×40, and
  compact 60×18. The initial ImageMagick conversion omitted SVG text; CairoSVG
  rendered it, but font fallback does not match the user's terminal. Captures
  are under `/tmp/opencode/agenturm-content-*.svg` and `.png`.
- Tested runtime: Python 3.13.5, Textual 8.2.8, Rich 15.0.0 on Linux.
- The first full local run exposed two additional heading-case expectations
  (`Direct children` and `Latest work`); their content/navigation assertions
  remain intact. A later 46-test focus also found the `Recent work` expectation
  and a late first-paint race after hidden content replacement. The log now
  rebuilds immediately on its own show/resize event when geometry is valid,
  retaining deferred rendering only for unknown geometry. The 10-test content
  and core-flow follow-up passed, as did five consecutive executions of the
  hidden replacement/copy/export journey. Release is gated on the full CI matrix
  for the exact PR head, not on preparation-time local results.
- Wheel and source distribution for 0.2.7 build and pass `twine check`. An isolated
  installed-wheel smoke check verifies the version, responsive Markdown rendering,
  original message copying and CLI import. Lockfile, compile and whitespace
  checks pass.
- This is not a claim of real-terminal clipboard, screen-reader, font,
  link-opening or cross-platform visual validation.

## Release content sync

| Destination | Status |
| --- | --- |
| App README/usage/changelog | In sync with 0.2.7 reading behavior; no changes to capture or install boundaries |
| Website | Not synchronized for 0.2.7; `src/pages/index.astro` and demos remain in a separate dirty checkout, preserved untouched |
| GitHub profile | General copy in `README.md` remains compatible; older tower artwork in `scripts/build_profile_assets.py` remains visual drift, not changed by this release |
