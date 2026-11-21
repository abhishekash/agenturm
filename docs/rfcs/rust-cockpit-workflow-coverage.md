# Rust cockpit workflow coverage

This comparison uses the tagged Python cockpit `v0.2.8` and the published
standalone Rust baseline `rust-v0.1.0`. Python workflows come from the tagged
release documentation. Rust coverage comes from the tagged CLI and UI,
the published archive qualification, and the per-harness evidence record.
The coverage below concerns workflow behavior and recorded evidence.
[Python 0.2.8 release notes](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/releases/0.2.8.md) ·
[Python 0.2.8 keyboard paths](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/keyboard-navigation.md) ·
[Rust 0.1.0 qualification](../validation/rust-standalone-execution.md) ·
[Rust harness evidence](../validation/rust-harness-evidence-coverage.md)

## Workflow comparison

| Job | Python 0.2.8 | Rust cockpit 0.2.0 |
| --- | --- | --- |
| Open and discover recent work | Installed `agenturm` starts or attaches to the local receiver. Native discovery includes Codex, Claude Code, OpenCode and Pi. | Bare `agenturm` opens the cockpit and starts bounded Pi and Codex discovery, or reads the shared history while another process owns the writer lease. `cockpit --db <path>` opens existing history. |
| Locate a session or worker | Agents provides search, Today and History filters, and an expandable hierarchy. | Agents refreshes every three seconds on the newest catalog page. It provides search, paging, recorded-state filters, sorting, and expandable explicit delegation on the current page. |
| Read state and reason | Overview and Details keep turn endings, explicit reasons and later activity visible. | Session Overview and turn drilldown show recorded lifecycle and reasons. Turn queries distinguish a worker from another worker or a main turn with the same ID. |
| Investigate Activity, MCP, or a trace | Dedicated Activity, MCP and Traces navigate to linked evidence. | Global and scoped Activity, MCP and Operations display admitted metadata and exact operation grouping. Native spans, MCP payloads, and deep linked tracing remain open. |
| Review an agent's work | Review presents recorded edits, checks, errors and context. The workflow includes richer saved decisions. | Review shows recorded tools, checks, errors and work results with lifecycle evidence. `y` saves an acknowledgement for the selected evidence version in a separate SQLite file. Detailed edit evidence and decision notes remain open. |
| Read prompts | The Prompts navigator provides recorded requests, readable and original views, and pagination. | Message roles appear in Activity. The explicit Pi preview reads an eligible event under a verified current grant. General prompt browsing remains open. |
| Check usage or host health | Metrics include run usage and host and process data when the producer supplies it. | Usage shows recorded token and cache fields and separate Kiro credit units. Host now shows current Linux load and memory. Cost, historical host metrics and detailed process inventory remain open. |
| Handle requests and preserve follow-up work | Workflow surfaces requests, owner and local seen state. Saved tasks, decisions, linked runs and handoff export support follow-up. | Recorded lifecycle can describe waiting or failure. Request routing, saved tasks, and handoff export remain open. |
| Use the keyboard in a compact terminal | Global shortcuts and focus controls support compact layouts. | Root shortcuts select Activity, MCP, Operations, Findings, Host and Usage. At 60 columns the roster uses one pane and detail opens separately. Wide terminals show a selected-session preview. |

## Cockpit patch acceptance

The local cockpit candidate opened Agents from the bare executable at 60×18
with empty state. Two fixture files added while the cockpit was open produced
durable Pi and Codex sessions. A roster refresh showed both sessions. After
quit, another collector opened the same SQLite store and resumed its cursors.
The archived inspector also opened a recorded Codex turn through cross-source
history and displayed its native completion reason.

The release checks include opening a specific session, reading scoped
activity and review, returning to the same selection, and terminal restoration.
[Linux bundle verifier](../../scripts/verify-rust-linux-bundle.sh)

The cockpit update adds an Agents roster, source-scoped session Overview,
Activity, Review and Usage, and global shortcuts for available evidence.
Review acknowledgement applies to the selected evidence version. The global
MCP pane covers recorded tool outcomes and operation IDs. Workflow request
handling, terminal routing, deep traces and MCP details, general prompt
navigation, and cross-session task handoffs remain distinct workflows.
