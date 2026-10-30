# Changelog

## Agenturm 0.1.5

- Show exact recorded delegations as an expandable agent tree, with a flat fallback, subtree activity/attention summaries, and canonical child inspection.
- Discover Codex subagent rollouts, Claude Code subagent sidecars, and OpenCode child sessions; keep forks and unlinked observations distinct from delegated workers.
- Persist normalized agent identities and bounded relationship provenance in additive SQLite tables, with `/api/hierarchy` and exact/subtree scope parameters for Activity, MCP, Traces, and Findings.
- Add explicit SDK/OTLP hierarchy attributes and document runtime capabilities and limitations.
- Add an Updated ↓/↑ sort control (and `o` shortcut) that always keeps working sessions first and terminated sessions last, with updated-time ordering inside each group.

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
