# Rust cockpit workflow coverage

This comparison uses the tagged Python cockpit `v0.2.8`, the published
Rust 0.3.7 cockpit, and explicitly identified Rust 0.3.8 UI candidate changes. Python
workflows come from the tagged release documentation. The published Rust
archive and its qualification are recorded below.
The coverage below concerns workflow behavior and recorded evidence.
[Python 0.2.8 release notes](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/releases/0.2.8.md) ·
[Python 0.2.8 keyboard paths](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/keyboard-navigation.md) ·
[Rust 0.1.0 qualification](../validation/rust-standalone-execution.md) ·
[Rust harness evidence](../validation/rust-harness-evidence-coverage.md)

## Workflow comparison

| Job | Python 0.2.8 | Rust cockpit |
| --- | --- | --- |
| Open and discover recent work | Installed `agenturm` starts or attaches to the local receiver. Native discovery includes Codex, Claude Code, OpenCode and Pi. | Bare `agenturm` opens the cockpit and starts bounded Codex, Pi, Claude Code, Kiro, and OpenCode discovery, or reads the shared history while another process owns the writer lease. `cockpit --db <path>` opens existing history. |
| Locate a session or worker | Agents provides search, Today and History filters, and an expandable hierarchy. | The 0.3.8 candidate opens a full-width Agenturm command center. It groups exact session tasks by explicit linked operator workspace, source, or no group; unknown project metadata stays labeled unavailable. Status filters and counts cover the loaded page, including unknown states. Search, selectable Show more, sorting and explicit delegation preserve source/session identity. The newest page refreshes every three seconds unless paused. |
| Read state and reason | Overview and Details keep turn endings, explicit reasons and later activity visible. | Session Overview and turn drilldown show recorded lifecycle and reasons. Turn queries distinguish a worker from another worker or a main turn with the same ID. |
| Investigate Activity, MCP, or a trace | Dedicated Activity, MCP and Traces navigate to linked evidence. | Activity, MCP and Operations show recorded operations, including typed Codex MCP server, tool, duration and outcome where present. Traces shows native OTLP span IDs, parent, timing and status from JSON or protobuf. A validated explicit source/session binding links spans to a native session. `m` in MCP, or `M` from another eligible pane, opens recorded Codex payloads under an explicit `codex.mcp_payload` approval when the exact operation has a retained source proof. |
| Review an agent's work | Review presents recorded edits, checks, errors and context. The workflow includes richer saved decisions. | Review shows recorded tools, checks, errors and work results with lifecycle evidence. `y` saves an acknowledgement for the selected evidence version. `n` opens the decision list, where `e` adds a note tied to that version. From Review or turn activity, `e` opens a Codex edit picker. Enter reveals recorded paths and bounded diffs or file contents under a separate `codex.edit_detail` approval. |
| Read prompts | The Prompts navigator provides recorded requests, readable and original views, and pagination. | Message roles appear in Activity. `P` lists eligible user prompt references for pinned Pi, Codex, Claude Code, Kiro, and OpenCode sources. Enter reveals the exact event under the current grant. `z` switches the same granted text between Readable and original captured layout in the 0.3.8 candidate; it does not extend the fixed display lifetime. Escape clears the view. Retained JSONL batches and supported OpenCode SQLite sweeps carry their own proofs. Older OpenCode prompt text can be revealed when the retained row still matches the current source bytes. |
| Check usage or host health | Metrics include run usage and host and process data when the producer supplies it. | Usage shows recorded tokens, native Kiro credits, and individual OTLP, Pi and OpenCode cost reports with provenance. Host reads recorded Linux samples and exact known-process history; live sampling requires a collector. Remote recorded Host access is explicitly opt-in. Combined cost and usage totals remain open where sources can repeat the same report. |
| Handle requests and preserve follow-up work | Workflow surfaces requests, owner and local seen state. Saved tasks, decisions, linked runs and handoff export support follow-up. | `w` shows recorded Codex and Kiro requests. Local seen state survives restart. The 0.3.8 `X` action checks the selected exact current request, offers a chooser for multiple workers, and never resolves native lifecycle state. Explicit routing can focus a uniquely verified Linux tmux pane. `b` opens saved tasks with exact run links, optional Git context, and Markdown and HTML handoff export. |
| Use the keyboard in a compact terminal | Global shortcuts and focus controls support compact layouts. | Root shortcuts select Activity, MCP, Operations, Findings, Host, Usage and Traces. The 0.3.8 command center uses a full-width task list at 120, 80 and 60 columns. Enter opens the selected inspector; Escape restores the same group, filter and task. The protected investigation overlay owns query typing, and loaded-view copy/export includes metadata only. |

## Rust 0.3.8 UI candidate

This candidate is not a published release. The first screen follows the user's
command-center reference with Agenturm branding and colors. `Tab` selects All,
Needs you, Working, Ready and Inactive. Unknown state counts remain explicit.
`G` cycles Project, Source and None without changing the exact selected session.
Project paths come only from the exact operator-linked saved task workspace;
that provenance remains explicit. No prompt, edit path or process cwd is used
to invent a project or task title.

`Ctrl+F` searches retained failure metadata and explicitly granted source
evidence. `c` copies bounded loaded metadata through a terminal clipboard
request, and `x` writes a new private Markdown/HTML/JSON export directory.
Export never overwrites a destination and is labeled as a loaded view rather
than an evidence receipt. Private prompt, edit, MCP, span and eval bodies stay
inside their existing grant-fenced viewers.

Final integrated acceptance is tracked in the [execution plan](rust-only-execution-plan.md).
Comparative performance benchmarking remains deferred, and default cutover is gated.

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

A 60×18 terminal exercise created and linked a task, exported a handoff in
both formats, persisted a request’s seen state across reopen, and observed
the request resolve after a new lifecycle event. A separate run used the
request’s exact resumed session identity to focus its tmux pane. The prompt exercise listed
Pi prompt references, revealed and scrolled the exact approved event, cleared
it on Escape, and verified that revocation prevented another reveal. A local
OTLP HTTP test submitted the same export twice and reopened a single durable
span with its native IDs and displayed it in the Traces pane. The receiver
rejected an HTTP request carrying an Origin header. Local discovery and
collection checked eight Codex and Pi sources with zero source failures.

Saved tasks, handoffs, and request seen state live in the adjacent
`.workspace.sqlite` database. See the [Linux usage guide](rust-linux-bundle.md)
for shortcuts and trace collection commands.

## Published Rust 0.2.0

The Ubuntu 24.04 x86-64 release tag `rust-v0.2.0` resolves to
`34345408a170f059e8af096c87c17a2e97d074e8`. The promoted CI archive has
SHA-256 `594b47dd8d2c6035034e57b1dec21ca34445b00017a938a19746b9de3dfafa7a`.
[Release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.2.0) ·
[Tag workflow](https://github.com/abhishekash/agenturm/actions/runs/44961241954)

The downloaded tag archive passed bare startup and rolling discovery, clean
terminal exit and writer lease release, systemd install and restart, and
history retention after uninstall. It reopened a database created by the
published Rust 0.1.0 bundle and resumed both source cursors. A separate
60×18 run saved and reopened a review acknowledgement while the observation
database's checksum stayed unchanged. The published version's history backup covers observations. Its local review
acknowledgements use the adjacent `.reviews.sqlite` sidecar. The workspace
backup added after this release preserves both local sidecars alongside
history and requires fresh private-content approval after restore.


## Published Rust 0.3.1

Ubuntu 24.04.5 x86-64 qualified the tagged archive from
`4661eae53a288d091688597f539c8a1167595e59`. The published
archive has SHA-256
`82c54a46812ee9231050a33d4773e93a0286f4dda487cabcec130a04137f53bb`.
It opened and advanced the previous 0.2.0 history, ran as a systemd user
service through restart and uninstall, and preserved history afterward.

The exact published binary checked 13 real local sources with zero failures
on both initial collection and reopen. The bundled Kiro adapter also
collected and reopened a native-format fixture. An actual OpenCode SQLite
sample reopened bounded inline tool and turn evidence. The 60×18 cockpit
verified grant-fenced prompts and linked OTLP with the active collector.
The OTLP/HTTP receiver also accepted a real SDK's protobuf export and
returned the expected partial-success response.

[Release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.1) ·
[Tagged build](https://github.com/abhishekash/agenturm/actions/runs/45024901558) ·
[Remaining migration acceptance](rust-only-execution-plan.md#remaining-work-and-acceptance)
