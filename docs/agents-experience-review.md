# Agents and session inspector: assessment, plan, and results

Date: 2026-11-11. Scope: the existing Agents list, hierarchy, session inspector, and prompt reading workflow. No new product area or external summarization service.

## Critical assessment before implementation

The current interface makes the engineer reconstruct the story from records. Its strongest implementation is the conservative status classifier and exact hierarchy model; its weakest part is the reading order and the translation of captured data into useful labels.

1. **The inspector answers the wrong question first.** `show_agent` places every loaded prompt, oldest first, before current work, status evidence, and family context. A long first prompt hides the latest result several screens below. Assistant updates are omitted from the recent-work section. The first screen should explain the task, its current state, and the latest recorded progress.
2. **Task identity can be machine context.** Native user messages overwrite the task directly; display normalization recognizes only one input-reply wrapper. Environment, repository instructions, and skill scaffolding can become titles. Short steering such as “go ahead” can replace a useful description. No production demo-data seeding path was found: the template effect comes from generic labels, raw context, and placeholder values.
3. **Useful row space is spent on missing information.** Every task row starts its secondary line with unknown token/cost placeholders. Runtime/project and family context are displaced, while current work is deliberately omitted. Desktop rows reserve three lines for two lines of content: one third of their area is blank.
4. **Hierarchy controls are broken in the default workflow.** Today/Running filters re-expand every matching ancestor after collapse. Inspect-child and next-attention attempt to select hidden rows without revealing them. A completed parent with working children is correctly classified; preserve that distinction.
5. **Evidence scope is not consistently safe.** `show_agent` falls back from an explicit worker to untagged same-session events, and from an unidentified session to an entire service. This can misattribute another agent's work. Title fallback also indexes by session alone. Exact agent IDs must constrain every projection; legacy session fallback is permissible only when an exact identity was not recorded.
6. **A global recent slice is not a session history.** Quiet sessions can have no useful events in the current global activity window. Conversely, an event count from that window is not an all-time total. Fetch bounded recent evidence for the selected session, disclose its limits, and keep loading/failure distinct from no captured work.
7. **Prompt reading lacks selection and context control.** The current reader concatenates all prompts into one log. It cannot keep a particular request selected independently of history growth, and raw machine context crowds out the actual request. Preserve original captured text and make its disclosure explicit.

Clutter audit: a pane has one main outer border, but heading rules, a metadata sidebar, repeated status labels and usage placeholders compete with the content. At 80×24 the current fixture gives approximately 15 data rows out of 24 terminal rows. The redesign must replace chrome rather than keep adding toolbars. Remove per-row missing usage, empty leaf markers, duplicated prompt histories and repeated full identity fields from the Overview.

## User workflow and decisions

The product remains a full-screen, read-only Textual monitor with framework-managed terminal restoration. The common loop is: identify working agents → inspect their task and update → inspect a child or pending request → read a particular prompt → follow recorded activity → return without losing selection.

- **Agents:** two content lines per row. Task first; runtime/project plus actual latest work or family activity second. A separate state column describes only that agent. Exact descendants are indented, both working and attention counts remain visible, and ancestor-only context is labeled. Today remains the default and History remains available.
- **Overview:** task, status and freshness, latest request/steering when different, latest agent-reported update, bounded recorded work and results, and direct children. Label assistant statements as reported; an exit code or tool result is evidence about that action, not proof the objective is finished.
- **Prompts:** chronological selectable requests, one wrapped body at a time. Start at the latest request on first entry. Keep its stable ID and reading position when records arrive or earlier pages load. Known complete context blocks are separated for readability; show their presence and offer the original captured text. No broad XML stripping and no hidden loss of source evidence.
- **Details:** model, project, session IDs, reported usage, status explanation, capture coverage, and relationship provenance. Missing values are meaningful here, not filler on every agent row.
- All three modes have visible keyboard-reachable controls. Prompt previous/next, latest, earlier-page/retry, and original-text controls remain available at the minimum supported size.

Wide layout (150×40):

```text
Agents   Today / Running / History       collection freshness
Task / source                   State       Session overview
v Fix payment retries           Working     Fix payment retries
  Codex · payments · 2 working               State + last recorded update
  |- Verify API compatibility   Working     Agent reported: ...
  `- Check regression tests     Needs reply Recorded work: ...
                                             Children: ...
Enter inspect · Left/Right collapse/expand · / Search

Expanded:  Overview | Prompts | Details               Esc Back
Prompts:   request index | selected request, wrapped text
           dates/titles | context disclosure + original-text action
```

60-column layout (60×20; minimum 60×18):

```text
Agents / Today                         collection state
Task / source                              State
v Fix payment retries                      Working
  Codex · payments · 2 working, 1 needs attention
  |- Verify API compatibility              Working
  `- Check regression tests                Needs reply
Enter inspect · arrows tree · / Search

Expanded: Overview | Prompts | Details      Esc Back
Request selector / previous / next
Selected prompt title and time
Wrapped body occupies remaining reading space
Earlier prompts / original text available by keyboard
```

At 120 columns, retain list/preview when height permits; at 80×24 and a 60-column split the list wins, and Enter opens a full-screen inspector. At less than 60 columns or 18 rows, give a truthful resize message instead of a broken work surface.

## Implementation sequence

1. Capture baseline rendered frames with synthetic but realistically messy records. Keep private transcripts out of fixtures and documentation.
2. Add a pure presentation layer for recorded text, known-context classification, meaningful labels, exact evidence selection, and bounded progress descriptions. Apply it to existing captured records and new native task assignment without changing the raw record.
3. Add a bounded on-demand selected-session activity cache with request deduplication, selection guards, loading/error/partial states, and revision-driven refresh. Keep prompt paging separate; do not fetch prompt history until Prompts is opened. Bound cached prompt count and content, and disclose limits.
4. Replace the expanded wall of text with Overview/Prompts/Details. Add selected-prompt navigation, explicit original captured text, consistent copy behavior, and focus/scroll preservation.
5. Improve agent rows and hierarchy navigation. Honor manual collapse in Today/Running; search may temporarily reveal matches. Reveal ancestors before selecting hidden children, retain exact relationship rules, label context ancestors, and keep independent parent/child lifecycle state.
6. Validate, obtain an independent implementation review, resolve findings, update usage documentation, and deliver the normal repository PR/merge workflow.

## Acceptance and verification

- Real user text, known context-only text, mixed context plus user request, short follow-ups, multiline/code/Unicode content, and credential-redacted records.
- Explicit workers sharing a session, multiple services with the same session ID, legacy untagged session records, and no identity: no cross-agent evidence.
- Live, quiet, waiting, completed, unknown, announced, missing-parent and conflicted-parent states; metadata-only content stays absent.
- Overview shows latest reported work before historical prompts. It never invents completion, tests, progress percentages, costs, or counts beyond the selected evidence scope.
- Default Today collapse works; hidden child/attention navigation succeeds; filters preserve context; counts distinguish matching agents from rendered rows.
- Prompt selection and body scroll survive live updates and earlier-page insertion. Copy matches the selected mode and original/readable setting. Failures have a retry and retain prior evidence.
- Bounded requests, no UI-thread I/O, no per-tick history fetch, no stale asynchronous results after a selection change.
- Rendered frames at 150×40, 120×40, 80×24, 60×20 and the 60×18 floor, plus too-small state, long content, ASCII/monochrome and CJK. Verify keys and layout using Textual's [official headless testing APIs](https://textual.textualize.io/guide/testing/).
- Run focused parser/state/interaction regressions, then the repository suite and appropriate benchmark smoke checks. Preserve existing shutdown and data-budget contracts.

## Independent plan review

The peer reviewer approved this direction with required changes, incorporated above: fix default-filter collapse and hidden-target navigation; distinguish an objective from terse steering; narrowly recognize context wrappers and preserve source text; guard/deduplicate the evidence loader; distinguish reported results from verified action outcomes; preserve prompt ID and scroll; bound history; keep the 60×18 floor usable; and make copy/export semantics explicit. Existing tests for the old monolithic prompt layout must be replaced with tests of these user-visible behaviors, not weakened to accept either design.

## Completion and remaining craft work

The implementation now follows the plan. Agents have two-line task-first rows, exact indented delegation, compact family counts, working collapse/expand controls, and navigation that reveals hidden child targets. A short acknowledgement no longer replaces a meaningful task. Known leading runtime context is separated from the readable request; the original captured text remains accessible.

The expanded inspector has three explicit modes. **Overview** shows the latest agent-reported update, the latest recorded command/result, six recent actions, and children. **Prompts** shows one selected request, opens at the latest request, and preserves selection and scroll while earlier pages or live updates arrive. **Details** holds identifiers, reported usage, capture coverage, relationship provenance, and the evidence behind status. Missing timestamps say “Time not reported.” A completed agent is described as having reported completion, not as independently verified success.

Local feedback exposed two further usability problems: the mode buttons did not make keyboard navigation apparent, and repeated metadata distracted from reading. The inspector now uses one native tab strip. Left/Right selects sections, Enter/Down enters the reader, Ctrl+Left/Right changes sections from the reader, and Tab/Shift+Tab stays within the inspector. Escape respects an open selector before returning to the selected agent. Task, runtime and status stay visible above the content; the wide prompt index replaces the duplicate dropdown. Section changes retain reading position, and resizing from a wide index to a narrow selector restores usable keyboard focus. Compact tabs omit the extra underline and margins to preserve seven reader rows at 60×18.

Session evidence is fetched asynchronously for the exact service/session/worker. One recent window of at most 100 events replaces the previous window. Prompt history retains at most 300 requests, with a 16,000-character reader bound per text field; limits are disclosed. Opening a clipped prompt in Activity queries its retained source record, including records outside the latest global window. Unknown capture coverage is distinct from metadata-only capture. Source rows index activity once, avoiding a scan of all activity for every agent.

The interface supports 150×40, 120×40, 80×24, 60×20, and a 60×18 floor. Wide screens offer a prompt index alongside the body; small screens retain the selector and keyboard controls. Below the floor, a resize message replaces the clipped work surface. Usage and raw identity fields remain in Details instead of occupying every row.

## Implementation review and validation

The independent reviewer approved the plan and reviewed the implementation twice. Review findings led to concrete fixes: literal JSON and indentation preservation, terminal-control sanitization, exact AI-summary evidence and worker usage scope, live prompt refresh, original-text copy/export semantics, selected-prompt handoff to Activity, title reset on file rotation, family count clipping, and reading space when optional actions are enabled. Final reviewed Agents, Overview, and 60×18 Prompts frames had no remaining visual blocker.

The initial full suite passed 481 tests with 14 optional-dependency skips. After local feedback, 23 focused query/feed tests and three end-to-end keyboard, resize and partial-history UI tests passed. The final repository-wide regression result is recorded after the follow-up release gate. Screenshots in the HTML companion use synthetic records, not private transcripts. They show actual Textual-rendered interfaces. Screenshot rasterization uses DejaVu Sans Mono for tree-glyph fidelity; the application does not require that font.

The user’s local startup failure was reproduced: a three-second activity timeout discarded the whole snapshot, and connections took nearly two seconds. Read-only profiling on the retained database measured activity at 2.25 seconds and connections at 1.66 seconds. Activity now filters and selects ordered candidates before building full log payloads, preserving search and cursor semantics. Connection counts are grouped once with consistent text identity instead of repeatedly scanning each service. Representative updated reads took 0.70–1.00 seconds and 0.07–0.19 seconds respectively. Three cache-cleared refreshes through an isolated updated receiver against the live database completed in 0.846, 1.205 and 0.675 seconds, each with 16 native agents and 189 stored sessions. These are local observations, not portable performance guarantees.

Database contention also occurred during active collection. Agents can now publish the healthy native roster when optional stored reads time out. Partial coverage is explicit; global history has a separate retry budget and last-success timestamp. Scoped evidence is retained only when the full filters, pagination, hierarchy revision and trace error filter match. Authentication and malformed responses remain failures. The peer reviewer required and verified the scope guard, narrow-layout reading space, focus policy and cross-section scroll restoration. Database journal mode and timeouts remain unchanged; enabling WAL on the installed SQLite version was rejected after checking the [documented SQLite WAL-reset issue](https://sqlite.org/wal.html#walreset).

The earlier small-fixture smoke benchmark measured a 9.6 ms warm median refresh, 4.6% idle CPU over ten seconds, and about 71 MiB current RSS. Those numbers describe the fixture, not a large retained database. Unicode, combining marks, emoji, ASCII tree mode, monochrome, light/dark themes and narrow widths were exercised. The final review retains actual-terminal coverage and long-running soak work as further craft improvements.

No new production dependency, remote summarization service, telemetry schema, or fabricated progress data was introduced. The optional existing AI-summary action remains opt-in. New on-demand evidence reads use the existing local receiver API.

## Further work: improve the craft of these same features

1. **Build a lasting readability corpus.** Keep a small, redacted set of real Codex, Claude, Pi, OpenCode, and OTLP transcripts covering long instructions, sparse capture, tool failures, restarts, and nested delegation. Pin the expected human task, state explanation, and visible first screen. This would catch adapter changes that quietly make labels confusing again.
2. **Measure comprehension with engineers.** Ask first-time readers to identify what is running, which child needs help, and the latest observed result. Record task time and incorrect interpretations. Refine wording and reading order against those results; target five seconds for the first two questions rather than adding controls.
3. **Exercise long-running churn.** Add repeatable soak scenarios with hours of prompt arrivals, reconnects, nested workers, selection changes, and branch collapse. Track memory, request count, refresh latency, and cursor/scroll stability. The structural bounds and targeted tests are in place; a soak validates their sustained behavior.
4. **Broaden terminal fidelity checks.** Maintain a small actual-terminal matrix for tmux, SSH, light/dark, monochrome, ASCII tree mode, CJK, combining characters, and emoji. Test real keyboard/mouse/focus and restoration behavior alongside headless frames. Isolate the intermittent async test teardown observed during review so regressions finish deterministically.
5. **Reduce implementation coupling.** Move session reader state and pure presentation construction out of the large application class as a behavior-preserving refactor. Keep scope matching shared across rows, Details, Activity, summaries, and usage so future changes cannot reintroduce conflicting interpretations.
