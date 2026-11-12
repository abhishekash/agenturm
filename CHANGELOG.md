# Changelog

## Agenturm 0.2.0

- Make prompt browsing work directly with arrow keys, provide persistent F1 help, and preserve navigator/reader focus across section changes and resize.
- Restore terminal settings on external SIGINT/SIGTERM, preserve prior signal handlers, and return meaningful interrupt, termination and failure exit statuses.
- Add a workflow inbox for waiting requests, failures and agent-reported completion, with opt-in local notification delivery and explicit acknowledgement.
- Keep selected work and reading positions stationary during background refreshes, and clarify current work versus agent-reported completion.
- Add exactly scoped, lazy recorded-work review with check outcomes, later edits, bounded paging and frozen copy/export.
- Persist local attention acknowledgements and desktop delivery claims with stable request/run identities, silent restart inventory and seen-but-pending requests.
- Add structured saved-task briefs, immutable decision versions, explicit excerpt drafts, native lifecycle links and continuity CLI commands.
- Keep saved links and usage separate across receivers/producers, preserve old notes-only databases, and provide an explicit pre-migration SQLite backup command.
- Offer opt-in bounded repeated-work observations in Findings, with polling exclusions and captured-window qualifications.
- Reorganize the session inspector into Overview, Prompts, and Details, with recent agent-reported progress before history and a selectable, wrapped prompt reader.
- Separate known runtime context from requests, preserve the original captured text, and keep meaningful task descriptions through short follow-up replies.
- Correct worker evidence and usage attribution, load bounded history for quiet sessions, and preserve prompt selection and reading position during updates.
- Separate agents with alternating row backgrounds and extra spacing on roomy terminals; keep compact rows, prioritize working and attention counts, honor collapsed branches, and reveal hidden children during navigation.
- Support a usable 60×18 inspector with a clear resize message below that minimum.
- Give the inspector a native keyboard tab strip, visible focus, scoped Tab/Shift+Tab navigation, and preserved reading positions between sections.
- Reduce startup history-query work and keep available live agents visible when stored reads are delayed, with explicit missing/stale evidence and safe filter-scoped retention.

Restart Agenturm and any separately running receiver after upgrading. The release remains Python-based; the experimental Rust ingestion change was reverted.

Known limitation: a mixed headless 20-minute validation run showed RSS rising from about 70 to 125 MiB despite bounded review caches and stable sampled descriptors. Memory stability remains under investigation; see the [validation report](docs/rigorous-validation.html).

## Agenturm 0.1.7

- Reduce local monitoring resource usage, load expensive view data on demand, and index source lookups to avoid startup timeouts.
- Show truthful connection/loading progress, retry feedback, and a Retry now action without claiming an empty session list before data arrives.
- Make session inspectors readable with bounded, indented previews and full recorded evidence available in Activity.
- Add working-first, updated-time, and status sorting, red error filters, and clearer failure investigation.
- Show tokens and cost for each task's latest recorded run; keep input/output and cached-input breakdowns in the inspector, with exact values under `u`.
- Improve compact layouts, keyboard navigation, copying, and pausing live updates without losing the selected session.

Restart Agenturm after upgrading. Restart any separately running receiver as well so it serves the new task-usage fields.

## Agenturm 0.1.6

- Show exact recorded delegations as an expandable agent tree, with a flat fallback, subtree activity/attention summaries, and canonical child inspection.
- Discover Codex subagent rollouts, Claude Code subagent sidecars, and OpenCode child sessions; keep forks and unlinked observations distinct from delegated workers.
- Persist normalized agent identities and bounded relationship provenance in additive SQLite tables, with `/api/hierarchy` and exact/subtree scope parameters for Activity, MCP, Traces, and Findings.
- Add explicit SDK/OTLP hierarchy attributes and document runtime capabilities and limitations.
- Add an Updated ↓/↑ sort control (and `o` shortcut) that always keeps working sessions first and terminated sessions last, with updated-time ordering inside each group.

## Agenturm 0.1.5

- Install only the `agenturm` command; remove the legacy `agent-observatory` and `agent-goechala` entry points.
- Show recorded reasons when a session fails.
- Make attention states actionable and evidence-based, and persist dismissed attention failures.
- Avoid caching inspector text at a hidden-pane width.
- Refresh the terminal walkthrough media.

## Agenturm 0.1.4

- Rework the terminal cockpit with a compact status overview and a separate, clearly framed view navigator.
- Keep the agent list uncapped, add Recent / Active / All filtering, and surface recorded prompts and activity in the inspector.
- Capture OpenCode user prompts locally when detail capture is enabled; metadata-only mode continues to omit prompt content.
- Improve MCP evidence, session prioritization, responsive navigation, themes, and graceful shutdown.

## Agenturm 0.1.3

- Rename and brand the terminal monitor as Agenturm, with a tower identity and walkthrough.
- Refresh the TUI with the Alpine Ember theme and tower mark.
- Keep selected Activity filter labels and keyboard shortcuts visible.
- Improve ADK protocol span handling and clarify release tagging for the renamed package.

## Agent Observatory 0.1.2

- See the GitHub releases for earlier release notes.
