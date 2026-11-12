# Agenturm terminal experience specification

Status: initial implementation in progress; validation and remaining parity work are tracked below.
Research date: 2026-10-25.

## 1. Product direction

**A calm, task-first terminal workspace: see what needs attention, read the
evidence, and follow the investigation without losing your place.**

The visual reference is [Agenturm's website demo](https://www.agenturm.dev/#demo).
Translate its hierarchy into terminal cells: restrained surfaces, clear titles,
generous separation, a short list beside focused evidence, and one obvious next
action. The experience must remain clear with real, incomplete, rapidly changing
session data and hundreds of historical records.

### Primary jobs

1. In five seconds, understand which sessions are working and which need attention.
2. Identify a session by its task and project, even when its runtime has no title.
3. Explain a status using recorded evidence and collection freshness.
4. Follow session → event/tool call → trace, then return to the exact origin.
5. Read and copy evidence while new records arrive without content jumping.

### Technology recommendation

**Use current Textual 8.2.x for the redesign, with purpose-built presentation
components and an explicit navigation model.** Agenturm already declares
`textual>=8.2,<9`. Its present constraints are primarily information hierarchy,
component composition, and interaction policy; research has not established a
renderer limitation that requires a rewrite.

**OpenTUI is the first alternative to evaluate if Textual fails a defined UX or
performance gate.** Its TypeScript component model and native renderer make it
particularly relevant to a bespoke, evidence-rich terminal workspace. Bubble Tea
v2 is the strongest Go alternative. A migration remains a valid outcome, with
the same design and acceptance criteria applying to every implementation.

This is a terminal product. Typography uses the user's terminal font and cell
grid. Hierarchy comes from weight, spacing, contrast, alignment, and disclosure;
the website's proportional fonts and pixel layouts are not terminal guarantees.

## 2. Research: modern frameworks and exemplary tools

Versions below were checked against upstream release metadata on the research
date. Capabilities are documented claims, not Agenturm comparative benchmarks.

### Framework assessment

| Technology | Verified release | Relevant strengths | Agenturm decision |
| --- | --- | --- | --- |
| [Textual](https://github.com/Textualize/textual/releases/tag/v8.2.8) | 8.2.8 | Python integration, CSS-like layout, reactive widgets, workers, command palette, headless interaction and SVG snapshot testing | Recommended delivery path; build a coherent component system instead of styling every existing widget independently |
| [OpenTUI](https://github.com/anomalyco/opentui/releases/tag/v0.6.8) | 0.6.8 | Zig native core, TypeScript, React/Solid bindings, flexbox, selectable content, code/Markdown/diff components, renderer diagnostics | Leading challenger for a bespoke frontend; validate native packaging, selection, input and lifecycle across supported terminals |
| [Bubble Tea](https://github.com/charmbracelet/bubbletea/releases/tag/v2.1.0) + [Lip Gloss](https://github.com/charmbracelet/lipgloss/releases/tag/v2.0.6) | 2.1.0 / 2.0.6 | Elm-style state updates, cell-based rendering, color downsampling, keyboard/mouse and clipboard support, broad Charm ecosystem | Strong choice for a Go frontend; rebuilding the receiver too would add unrelated migration scope |
| [Ratatui](https://github.com/ratatui/ratatui/releases/tag/ratatui-v0.30.2) | 0.30.2 | Rust, direct cell/layout control, flexible custom widgets | Relevant if measured rendering/resource requirements justify a lower-level implementation; more interaction plumbing to own |
| [Ink](https://github.com/vadimdemedes/ink/releases/tag/v8.0.0) | 8.0.0 | React component model and Yoga flexbox; substantial CLI adoption | Capable option; no demonstrated advantage over OpenTUI for this particular multi-pane frontend |
| Terma | Posting beta pins 0.27.0 | Emerging Go option used by Posting v3 beta | Watchlist. The pinned dependency is verified; a successful app alone does not establish framework support or suitability |

OpenTUI's [0.6.8 runtime documentation](https://opentui.com/docs/getting-started/runtime-support)
specifies Bun 1.3.14+ or Node 26.4+ with experimental FFI, with additional platform
qualifications. It publishes eight native targets but documents narrower test
coverage. A TypeScript migration therefore includes distribution and CI work;
it cannot simply reuse Agenturm's existing Node 22/24 SDK lanes.

### What to learn from finished products

These are design references, not claims that Agenturm has run comparative usability
or performance tests against each app.

| Reference | Pattern to study | Application in Agenturm |
| --- | --- | --- |
| [Posting](https://posting.sh/guide/) | Cohesive visual system, deliberate pane focus, inspectable structured responses, contextual commands | Treat the evidence inspector as a designed reading surface; give the current pane a clear focus cue |
| [Harlequin](https://github.com/tconbeer/harlequin) | A serious multi-pane workbench, searchable data and keyboard discoverability | Separate navigation, results and evidence; make pane transitions predictable |
| [gh-dash](https://www.gh-dash.dev/) | Filtered work queues with a selected-item preview | Attention and working filters; stable task selection with contextual detail |
| [Yazi](https://yazi-rs.github.io/docs/quick-start/) | Fast selection/preview navigation, keyboard-first operation | Local selection feedback immediately; expensive evidence loads asynchronously |
| [Lazygit](https://github.com/jesseduffield/lazygit) | Focused panes and contextual drill-down/back navigation | Preserve origin, scope, scroll and selection across linked investigations |
| [Crush](https://github.com/charmbracelet/crush) | Strong terminal identity and rich session presentation | Consistent spacing, restrained accents and readable tool evidence |
| [OpenTUI's production use](https://github.com/anomalyco/opentui) | Native-rendered component UIs used by OpenCode | Evaluate rich evidence rendering as an integrated experience rather than isolated widgets |

Important correction to older TUI lists: Posting's stable 2.x line was a Textual
reference; [3.0.0-beta.1](https://github.com/darrenburns/posting/releases/tag/v3.0.0-beta.1)
is Go and its [pinned dependencies](https://github.com/darrenburns/posting/blob/v3.0.0-beta.1/go.mod)
include Terma and an Ultraviolet fork. Do not describe that beta as Bubble Tea or
use its screenshots as proof of current Textual capabilities.

### What the website teaches

The fetched demo explicitly labels its data synthetic and provides both a guided
three-step investigation and a seven-view explorer. Its strongest ideas are:

- One dominant list and one dominant evidence panel.
- A short descriptive headline before raw details.
- A highlighted next action connected to the selected evidence.
- Muted timestamps and provenance rather than equal-weight metadata columns.
- A visible investigation thread.

The website currently uses dark green surfaces and a yellow-green signal accent;
the checkout's `Current` theme uses purple-gray surfaces and amber/lavender.
Section 5 proposes a website-aligned theme explicitly rather than assuming these
already match. The demo's simplified “waiting on tool response” narrative must
not replace Agenturm's more precise live-status classification.

## 3. Current architecture and design opportunities

The inspected checkout already provides incremental table updates, compact/full
inspectors, seven views, four themes, history navigation, selection-preserving
refreshes, conservative session status, and protected shutdown.

Relevant implementation boundaries:

- `observatory/terminal_ui.py`: composition, view state, navigation and rendering.
- `observatory/ui_layout.py`: responsive column budgeting.
- `observatory/themes.py` and `observatory/branding.py`: semantic colors and legend.
- `observatory/terminal.py`: snapshots and background feed.
- `observatory/session_status.py`: evidence-based classification.
- `docs/performance.md`: bounded ingestion, polling and worker requirements.

The current composition spends vertical space on a masthead, four summary cards,
shortcut legend, tabs, search/status rows and scope controls. A large inventory
then competes with a fixed-width inspector. The redesign should reclaim that
space for tasks and evidence and make common investigations easier to discover.

## 4. Information architecture

### Default workspace: Agents

The default screen has four regions:

1. **Identity and health:** `agenturm / All sessions`, working count, attention
   count and a compact monitor-health label.
2. **Navigation:** Agents, Activity, MCP Calls, Traces; Host, Metrics and Findings
   accessible through a `More` menu. Existing `1`–`7` shortcuts retain their mapping.
   An active secondary view appears in the navigation in place of `More`'s label.
3. **Workspace:** task list and contextual evidence inspector.
4. **Status line:** filter coverage, collection freshness or current operation.

The persistent top shortcut legend contains exactly **Expand, Quit, Palette,
Search, Diagnostics**, with their existing keys. Additional actions appear in
their pane, the palette, and full help.

Replace large always-visible usage/MCP cards with a compact operational summary.
Detailed usage belongs in Metrics; MCP inventory belongs in MCP Calls. Working
and attention counts remain directly visible because they support the primary job.

### Session list

Default to two-line task rows:

- Line 1: selection marker, **task**, status aligned in a reserved right column.
- Line 2: runtime · project · latest short action · recorded age, in lower contrast.
- Wide layouts may give recorded age its own fixed column.
- IDs, model, token totals and full paths live in expanded detail.

Title priority: meaningful source title → first nonempty user prompt →
`Untitled session · <runtime>`. Normalize whitespace and clip by display cells.
Keep the full recorded prompt available in detail. Do not generate titles with
an LLM as part of routine browsing.

Filters: `All`, `Attention`, `Working`, `Finished`. Counts are computed within the
selected session scope and review visibility; text-search results show their own
`N matches / M in scope` label. Working counts use the classifier's active flag.
Attention includes explicit input waits, possible stalls and recorded failures;
collection problems have a separate monitor-health indicator. Finished includes
completed, interrupted, failed/error and incomplete sessions; idle remains in All.
Attention and Finished can overlap, so their counts are not additive.

Initial ordering: attention first, then working, then other sessions, then completed
history; newest recorded activity breaks ties, with stable session IDs as a final
tie-breaker. While the user is navigating, freeze order and update cells in place.
Announce queued additions/reordering with `List changed · Refresh order` in the
pane. Apply on explicit refresh or when re-entering the view, preserving selection
by ID. Avoid silently moving the row under the cursor.

### Evidence inspector

Default reading order:

1. Runtime / project breadcrumb.
2. Task title, at most two lines in preview.
3. Precise status and one short sentence explaining its evidence.
4. Latest relevant recorded action and its timestamp.
5. Up to six preview lines of evidence, with an explicit truncation marker.
6. One context-sensitive primary action and secondary Activity/Details links.

Primary action rules, in order:

- Selected failure with an exact linked event: **Inspect failed call**.
- Explicit input wait: **View recorded request**, plus “Reply in the agent's terminal.”
- Selected event with a trace link: **View linked trace**.
- Otherwise: **Open activity**.

Use a nearby error as a linked observation, not an asserted cause of a wait.
When the link is missing, display “No linked trace recorded” and expose Activity.
Never fabricate commands, progress summaries, timings or causal explanations.

Expanded detail reveals evidence provenance, lifecycle, complete stored text,
model, IDs, usage and collection diagnostics in labeled sections. “Complete”
means the stored bounded record; indicate upstream/ingestion truncation when known.

### Other views

- **Activity:** chronological evidence with type, short action and recorded time;
  preserve Prompts/Errors filters and paged history. Follow-live is explicit.
- **MCP Calls:** server/tool, state, duration; details separate arguments, result
  and error. Show missing server identity as “Not reported”.
- **Traces:** operation tree and selected-span evidence. Unknown duration stays
  unknown; a waterfall is optional only when reliable timestamps exist.
- **Host:** process evidence, with exact session association shown separately.
- **Metrics:** units, scope, reported coverage and missing-data labels. No invented
  price estimates or implied session attribution for service-wide metrics.
- **Findings:** review queue with occurrence count and exact evidence navigation.

## 5. Visual system

### Proposed default theme: Observation

The following tokens translate the website's terminal surfaces. Status colors
are proposed terminal adaptations; validate contrast before shipping.

| Role | Color | Use |
| --- | --- | --- |
| Canvas | `#151a17` | Main terminal background |
| Raised surface | `#1c221e` | Inspector, input and evidence blocks |
| Rule | `#394239` | Quiet separators |
| Primary text | `#edf1e7` | Tasks and evidence |
| Secondary text | `#abb4a5` | Project, source and time |
| Interaction accent | `#dceb5a` | Focus, selection marker and primary action |
| Selection surface | `#2b3429` | Selected row, with semantic colors retained |
| Working | `#86d9ed` | Activity/progress evidence |
| Completed | `#86e3b5` | Completion icon; completed task text remains subdued |
| Attention | `#ffbd69` | Explicit waits and possible stalls |
| Error | `#ff8799` | Recorded failure |
| Collection issue | `#c5adff` | Delayed monitor/backlog |

Existing saved themes remain selectable; new installations use Observation after
design acceptance. Changing a theme translates evidence, selection and syntax
roles as well as chrome. Status identity must survive selection and monochrome.

### Geometry and typography

- Two-cell outer gutters on wide screens; one on narrow screens.
- One-cell horizontal pane padding and a one-cell divider/gap.
- One separator per boundary; avoid nested boxes around every element.
- Bold only task titles, current view and the most important evidence heading.
- Use sentence case for actions and labels. Reserve small uppercase labels for
  sparse section headings, not every datum.
- Every screen has one clear focus indicator. Row selection remains visible when
  focus moves to the inspector, but does not masquerade as keyboard focus.
- Essential text targets at least 4.5:1 contrast against its rendered background
  in supplied true-color themes; check ANSI fallback separately.
- Use plain Unicode with ASCII alternatives; essential functions require no
  Nerd Font, emoji, graphics protocol or custom terminal font.
- Animation is optional and localized. Static `Working` text must communicate
  activity fully; no pulsing panels, animated backgrounds or continuous repaint.

### Wide wireframe

Illustrative synthetic data; the exact column budget must be cell-tested.

```text
 agenturm / All sessions                     2 working · 1 attention   Live
 e Expand   q Quit   ctrl+p Palette   / Search   d Diagnostics
 Agents   Activity   MCP Calls   Traces   More
 ──────────────────────────────────────────────────────────────────────────
 All 3   Attention 1   Working 2   Finished 0
 Search sessions…

 TASK / SOURCE                          STATE       │ CODEX / CHECKOUT-API
                                                    │ Investigate checkout
 ▸ Investigate checkout failure         ! Input     │ failure
   Codex · checkout-api · Request · 12s              │
                                                    │ ! Waiting for input
   Fix keyboard navigation              ↳ Working   │ Recorded request for
   OpenCode · storefront · Edit · 3s                 │ a user response.
                                                    │
   Review retry handling                · Quiet     │ Latest recorded request
   Pi · gateway · Model call · 44s                  │ Which retry policy
                                                    │ should I use?
                                                    │
                                                    │ View recorded request →
                                                    │ Activity · s   Details · e
 ──────────────────────────────────────────────────────────────────────────
 3 sessions in scope                         Last successful collection: 1s
```

This sketch shows hierarchy rather than a pixel-perfect rendering. `Input` and
`Quiet` are compact status labels; the inspector always expands their meaning.

## 6. Responsive layout contract

Breakpoints use available terminal cells, including height. Determine widths
from the actual child content area after gutters and dividers.

| Size | Layout |
| --- | --- |
| ≥120 columns and ≥30 rows | List + right inspector; inspector roughly 38%, clamped to 40–64 cells; two-line rows |
| 96–119 columns and ≥30 rows | Split only if list ≥52 and inspector ≥36 cells fit after chrome; otherwise list/detail navigation |
| 80–95 columns, or 18–29 rows | Full-width list; Enter/e opens full-width evidence; Esc restores the list |
| 60–79 columns and ≥18 rows | Compact list; task and status on separate lines when necessary, reduced metadata; views in a compact switcher |
| <60 columns or <18 rows | Minimal usable session list and single-record detail; show a quiet size recommendation; search, palette and quit remain reachable |

At 80×24 the identity, five-action legend, navigation, filters/search, headings
and status line together consume at most eight rows, leaving at least sixteen
for workspace content. Render at least six two-line session rows. The legend may
wrap at smaller sizes; never clip a key/action pair into misleading text.

No application-level horizontal scrolling is required. Long commands wrap;
expanded code may offer an explicit horizontal-scroll mode. Rewrap evidence
after actual pane geometry changes. Resize preserves selection, expanded state,
query and reading position by semantic anchor, not stale line offsets.

## 7. Interaction model

| Action | Behavior |
| --- | --- |
| ↑/↓ | Select rows; update cached preview immediately; never wait for I/O |
| Enter | Focus/open selected evidence; compact layouts open full-screen detail |
| e | Toggle expanded detail and return to its originating selection |
| Tab / Shift+Tab | Cycle visible controls in spatial order; clear focus cue |
| / | Focus search for the current view; preserve its existing query |
| Ctrl+P | Open fuzzy command palette; commands include current state and shortcuts |
| Esc | Dismiss top overlay, then text selection/search focus, then expanded detail, then linked navigation; consume one layer per press |
| Alt+Left | Return to the previous investigation location |
| s | From Agents, open selected session's Activity |
| t | Open an exact linked trace when available |
| o | Toggle updated-time sort; working stays first, terminated stays last |
| 1–7 | Preserve existing view mappings, including views under More |
| p | Freeze display updates; collection continues; show “Display paused” |
| c | Copy selected text, otherwise current evidence; show concise success/failure |
| d | Open diagnostics; return to the original context on close |
| q | Exit from navigation mode through complete cleanup |

Retain `a` reset, `v` reviews, `x` reviewed export, and history `[`, `]`, `l`
behavior. Document all actions in the palette/help rather than the top legend.
Printable shortcuts enter text when an input has focus. Pasted text is a single
input operation, never a sequence of navigation commands.

Palette groups: Navigate, Selected record, Filters, Appearance, Monitor. Every
operation needed by a keyboard user is available there; unavailable commands
explain why. The existing privacy-preview export flow remains an explicit action.

Mouse selection, wheel scrolling and clickable actions complement the keyboard.
Clicking a row selects it; opening detail uses a visible action or double-click.
Text selection stays anchored while live data arrives. Ctrl+C copies selected
text where supported; otherwise the existing interruption/cleanup behavior applies.

### Investigation navigation state

Each history entry carries view, scope, filters, selected stable record ID,
pagination cursor, scroll anchor, focus target and expansion state. Record the
origin before navigating to related evidence. Returning restores it even if new
events arrived. If a record has expired, explain its absence and keep the closest
valid context rather than silently selecting an unrelated event.

### Live updates and loading

- Paint selection immediately, then load detail in a background worker.
- Associate each request with selection/scope generation; ignore stale replies.
- Never display the previous session's evidence beneath a new session's title.
- Keep existing evidence visible during same-record refresh; use a small
  “Refreshing” label only for perceptible delays.
- When reading older activity, show `N new events · Latest` without jumping.
- Pin selected text/evidence while copying; explain when display is held.
- Distinguish `Loading`, `No sessions discovered`, `No matches`, `Source
  unavailable`, `Metadata-only capture`, `History truncated`, and `Disconnected`.
- Empty discovery offers Diagnostics and supported-source guidance. Empty search
  offers Clear search. Neither state is populated with demo sessions.
- Quiet success feedback uses the status line; persistent problems stay inspectable.

## 8. Status and trust contract

Present **agent state** and **monitor freshness** as separate concepts. Use the
existing classifier as the authority. The UI may shorten labels but cannot
reclassify lifecycle evidence based on appearance or elapsed time alone.

| Evidence/classifier state | Full presentation |
| --- | --- |
| Advancing records, unfinished turn | Working |
| Exact associated process/children show CPU or I/O progress | Processing |
| Pending tool response during active work | Tool running |
| Unfinished turn without recent progress, below stall criteria | Working quietly |
| Explicit approval/user input wait | Waiting for approval / Waiting for input; generic “Waiting” if subtype is unavailable |
| Live exact process, no recorded/observed progress for classifier threshold | Possibly stalled; “Check the agent terminal” |
| Old unfinished turn, no exact live-process association | Unconfirmed |
| Backlog | Catching up; agent health unconfirmed |
| Collection too old | Monitor delayed; agent health unconfirmed |
| Recorded completion/interruption/failure/idle/incomplete | Corresponding explicit label |
| Insufficient lifecycle evidence | Unknown |

Current classifier thresholds are 35 seconds for recent activity, 15 seconds for
process/collector freshness, and five minutes for possible-stall consideration.
These are existing evidence rules, not a new UX proposal to infer stalls at a
fixed timer. Keep their implementation centralized and expose the rationale in
detail. A separate collector-health badge can accompany a terminal lifecycle
state without rewriting that recorded outcome.

Working counts count observed active turns, not open processes. “Live” describes
the monitor connection and must never imply that every session is progressing.
Keep recorded-event time and successful-collection time distinct. Hidden internal
reviews stay excluded consistently from session rows, connections and summaries.

## 9. Implementation architecture proposal

Organize the future frontend around reusable components with semantic inputs:

- Application shell and responsive workspace.
- Session row / evidence row.
- Status badge with a full evidence explanation.
- Inspector with preview, expanded reading and copy behavior.
- Investigation breadcrumb and history controller.
- Search/filter bar and command registry.
- Monitor-health strip and diagnostic overlay.

Separate data acquisition, presentation models, navigation state and rendering.
Views should consume immutable snapshots with stable IDs. State computation,
file reads, database queries, network requests and expensive formatting stay off
the UI event loop. Bounded caches use record revision, theme and content width;
hidden views do no rendering work.

Keep collection/status semantics behind the existing Python receiver/feed
boundary. If a different renderer wins, add an explicit versioned frontend data
contract over the existing local API. Include provenance, evidence links,
collection health, pagination and cancellation semantics. Avoid importing
runtime-specific UI assumptions into collectors.

A separate frontend would also need ownership-aware receiver startup/teardown,
credential handling, packaged runtime artifacts and terminal restoration. The
current clean `q`/repeated-SIGINT behavior is a parity requirement.

## 10. Performance and compatibility acceptance

These are **proposed targets**, not measurements of the current application.
Record hardware, OS, terminal, Python/runtime version and workload with results.

| Interaction | Acceptance target on reference machine |
| --- | --- |
| Keypress to visible selection | p95 ≤50 ms; p99 ≤100 ms |
| Selection to cached inspector | p95 ≤100 ms |
| Search feedback | Immediate typed input; matching results ≤250 ms after debounce for loaded session inventory |
| Resize to stable layout | ≤150 ms after the final resize event |
| Initial interactive shell | ≤1 second; collection/import can continue afterward |
| Warm local receiver to populated view | ≤2 seconds under baseline workload |
| Settled idle CPU | Average ≤1% of one logical CPU over 60 seconds for small workload |
| Normal owned-receiver exit | ≤2 seconds, clean terminal restoration and no traceback |

Collection-to-display freshness is measured separately from UI input latency.
Document the existing one-second tail, ten-second discovery, two-second feed and
five-second inventory-cache intervals in diagnostics; fast rendering cannot make
an undiscovered session immediately visible. Do not promise subsecond live status
without changing and measuring that pipeline.

Retain deterministic budgets in `docs/performance.md`. Measure whole-process RSS
on its small, large and payload-heavy workloads; a frontend release must explain
any >10% same-machine regression. Exercise a synthetic 1,000-session presentation
fixture and long stored histories without increasing production discovery limits
just to satisfy the visual stress test.

Use at least 200 measured interactions after warmup for latency distributions.
Combine headless/Pilot tests with real terminal or PTY measurements; SVG snapshots
alone do not measure input-to-paint performance or actual clipboard behavior.

### Terminal coverage

- Linux: Kitty or WezTerm, plus a common VTE-based terminal.
- macOS: Terminal.app and iTerm2 or Ghostty.
- Windows: Windows Terminal.
- Multiplexed/remote: tmux locally and over SSH.
- True color, 256-color fallback, monochrome/`NO_COLOR` behavior.
- Mixed-width Unicode, combining marks, long unbroken tokens and plain ASCII.

Use bracketed paste, focus reporting, enhanced keyboard input, synchronized output
and OSC 52 clipboard integration only where framework/terminal support is verified.
Prefer framework negotiation to handwritten escape-sequence logic. Every essential
action has a conventional-key fallback. Clipboard denial yields an honest result.
Terminal restoration is required on normal exit, interruption and handled failure.

Validate keyboard-only operation and screen-reader behavior in actual terminals.
The existing `cockpit --once` offers a linear snapshot alternative; it does not
by itself establish interactive accessibility.

## 11. Design validation and delivery sequence

### Phase A — Approve the experience

Deliver static terminal-cell mockups for 160×48, 120×40, 100×30, 80×24 and 60×18:
default workspace, explicit wait, working quietly, collection delay, linked
failure, expanded evidence, no sessions and no search matches. Use deterministic
synthetic data clearly labeled in design artifacts. Approve hierarchy and colors
before implementation.

### Phase B — Prove one complete investigation

Implement a future Textual vertical slice: Agents → exact failed call → trace →
Back. Include selection/copy, resizing, search and live updates. Compare it with
the current UI using the same fixture, terminal size and benchmark machine.

If that slice exposes a framework limitation, time-box an OpenTUI version of the
same slice to five engineering days. Use identical data and interaction scripts.
Do not broaden the experiment into a second complete application.

**Switch criterion:** the challenger satisfies every correctness/compatibility
gate and either resolves a reproducible blocker that cannot reasonably be fixed
in Textual, or produces at least a 20% reduction in median investigation time
without worse correctness, plus meets the latency/resource targets. Require a
tested packaging/lifecycle plan. A lower-level renderer or prettier sample alone
does not meet this criterion. Small usability samples are directional evidence;
record their size and limitations rather than claiming statistical significance.

### Phase C — Apply the approved system

Extend the component system to all seven views, diagnostics, palette, empty/error
states and preferences. Migrate persisted appearance settings deliberately. Keep
the established keymap and evidence navigation coherent throughout rollout.

### Phase D — Acceptance and release media

Use at least five representative users, including newcomers and keyboard-heavy
users. Have them identify working/attention sessions, explain a quiet turn,
investigate a failure and return, and copy evidence during updates.

Release gates:

- At least 4/5 identify working and attention sessions within five seconds on
  the agreed 120×40 fixture.
- At least 4/5 complete the session → failure → trace investigation within
  thirty seconds without coaching; target no more than three activation steps
  from an already selected session, excluding list navigation.
- No participant confuses monitor delay with confirmed agent failure after
  reading the corresponding inspector.
- Keyboard navigation, search, evidence copy and diagnostics work at 80×24.
- No focus loss, stale-evidence substitution or scroll jumps during scripted
  rapid updates, resizing, theme changes and history navigation.
- Status/lifecycle, ingestion limits and repeated-interrupt cleanup tests pass.
- Accepted screenshots cover each layout and semantic state; actual terminal
  captures verify wrapping and contrast. Regenerate public demos from the
  released TUI with synthetic data so the reference experience stays honest.

## 12. Implementation status

The initial implementation keeps Textual and adds the website-aligned Observation
theme, a compact status header, task-first two-line rows with title fallback,
All/Attention/Working/Finished filters, primary recorded-evidence actions,
four primary tabs with a More picker, and stable row ordering with explicit
refresh. Exact recorded delegations are shown as an expandable tree with a
flat fallback and separate subtree summaries. Responsive list/detail layouts and the existing evidence-preserving
navigation remain in place. The original design phases and release gates below
remain acceptance criteria, not claims that all of them have passed.

Still requiring explicit validation/completion: a complete in-app command
registry for every operation, exact collector freshness reporting in the status
line, systematic terminal/contrast/accessibility checks, a measured 1,000-session
presentation workload and latency distributions, and the multi-user usability
study. The spec's numeric performance and user-study gates have not been measured
by this first implementation.

## 13. Decision summary

The proposed result is a materially redesigned terminal experience: compact
operational chrome, readable task rows, a focused evidence surface, an explicit
investigation thread and reliable live behavior. Textual is the recommended
starting renderer because it already provides the required foundation. OpenTUI
is the preferred migration candidate if a controlled comparison demonstrates a
better delivered experience.

Approval of this specification authorizes planning the implementation phases;
this document itself makes no runtime, dependency or product behavior changes.
