# Rust cutover workflow inventory

Baseline is the Python `v0.2.8` tag and the published Rust 0.3.1 Linux bundle. The default switch requires the keyboard journeys and retained commands below to pass on the installed candidate. The Python comparison uses disposable 0.2.8 state. The Rust candidate uses a fresh Rust history directory and equivalent source fixtures.

The current implementation scope retains the remaining workflows. Remote
receiver access, remaining eval views, automatic retention, recorded cost and
historical host views are retained requirements. Their implemented paths and
remaining gaps are listed below. The default entry-point switch
stays gated while they and the paired keyboard journeys are incomplete.
Rust supports explicit structural span imports into Rust history. Automatic
migration of Python-era databases remains outside the Rust-only product scope.

## Command inventory

| Python 0.2.8 workflow | Rust path and cutover disposition |
| --- | --- |
| Bare `agenturm`, `cockpit`, global `--db` | Keep local startup, discovery, one-shot snapshots and explicit history selection. Bare Rust startup and `cockpit --db` exist. `cockpit --once` prints a bounded session report, with JSON and page coverage on request. The same report works through a broker or trusted remote reader. Use `query` for raw machine-readable history. `cockpit --endpoint` attaches through a trusted loopback listener or SSH tunnel. Compatibility with a Python receiver and `--new-window` remain open. [Remote history setup](rust-broker-commands.md#read-history-through-an-ssh-tunnel) |
| `serve` | Keep local OTLP collection and source watch through `serve-traces`, `discover --follow` and `collect --watch` or the Linux user service. `broker serve-history` serves the native broker's retained metadata on loopback through a dedicated reader. The Python `/api/*` wire surface, direct remote TLS and arbitrary legacy JSONL tail remain open. |
| `run` | Keep command launch, exit status, local trace delivery and exact source/session linkage. Rust `run` configures OTLP for an instrumented producer. Python-specific script and module injection requires a product decision. |
| `import-jsonl` | Use `import-jsonl` for supported native span exports and `import-traces` for OTLP JSON. Both retain structural trace metadata with exact session scope. Legacy payload and cost import parity remains open. |
| `import-evals` | Import bounded `agent-evals` result metadata into an exact retained OTLP trace source. Use `evals` to read the current outcome in text or JSON. Aggregate views, historical revision queries, and private result detail remain open. |
| `migrate-db` | Retired by the Rust-only product decision. Rust `backup` and `restore` operate on Rust-owned state. |
| `prune` | Keep explicit age-based retention. Rust `prune --before-unix-ns` and persisted `retention set\|disable\|run\|status` are available. New default cockpit histories use 14 days; existing histories retain their policy. Collector, cockpit, local receiver and broker writer ticks apply it, while receipts and checkpoints survive cleanup. Broker policy changes require maintenance authority. |
| `attention status`, `attention watch` | Rust reads local history or an attached Rust receiver and keeps seen state in the user's workspace. Remote watch emits a snapshot and changes from retained native lifecycle evidence. Python receiver observation and cross-host terminal routing remain open. |
| `review` | Rust cockpit and noninteractive Review use exact source, session, turn, and worker scope. The CLI reads a bounded page and exports metadata receipts in Markdown, HTML, and JSON. It can list the explicitly linked spans of a turn, or resolve a trace to its unique retained turn using producer-provided source, session, turn, and worker identity. |
| `investigate path\|command\|instruction\|source` | Rust local and broker investigations use exact source/session scope, bounded paging, current-grant proofs, and pinned source references. Paths and recorded command invocations currently come from verified Codex occurrences. Receiver-wide search and legacy failure filtering remain open. |
| `task list\|create\|show\|archive` | Rust supports create, exact show, completion, archiving and restoration. The default task list hides archived entries. `list --all` and `show` can still read them. |
| `task link\|unlink\|handoff\|backup` | Rust links use exact native source/session/turn/worker identities. `task handoff` reads one saved task revision and renders Markdown, HTML, or JSON. Its private export writes a Markdown and HTML pair. Workspace backup retains the ledger and supports restore into a new history directory. |
| `task update\|snapshot\|brief\|decide\|decisions\|draft\|approve-draft` | Rust supports revision-checked editing, local Git summaries, opt-in tracked text patches, five structured brief fields, and immutable decisions with supersession. Drafts and decision citations can use one exact retained user prompt. Approval and excerpt reopening require revalidation of the original grant and source receipt. Broader Review evidence projections remain open. |

The CLI maintainer owns implementation and installed scenarios for the retained commands. Any later retirement requires an explicit product decision. [Python command source](https://github.com/abhishekash/agenturm/blob/v0.2.8/observatory/__main__.py) · [Rust workflow coverage](rust-cockpit-workflow-coverage.md)

### Import exported spans

Use `import-jsonl` with one exported native span object per line. The importer accepts a regular file or stdin (`--file -`) up to 4 MiB and 1,024 span records. It validates the complete batch before opening history for admission.

```sh
agenturm import-jsonl --file spans.jsonl --db history.sqlite --source-id otlp.exported
```

Rows carry `trace_id`, `span_id`, optional `parent_span_id`, `start_unix_nano` and `end_unix_nano`, and optional `status` (`UNSET`, `OK` or `ERROR`). Timestamps accept integers or decimal strings. IDs use 32 hexadecimal characters for the trace and 16 for each span. A parent outside the file is valid for a partial export.

A span needs an exact session ID from its `attributes` or the explicit `--session-id` argument. Span-level session aliases must agree, including `session.id`, `gen_ai.conversation.id`, and the legacy session, conversation and thread aliases. The argument fills missing session IDs and must match any IDs carried by the file. A native turn link requires `agenturm.source.id` and `agenturm.turn.id`. A worker link adds `agenturm.worker.id`.

Imports retain trace structure, timing, status and exact scope. Import discards span names, payloads, events, links, token usage, cost, and eval results. Those legacy fields and workflows have separate cutover coverage. Exact structural replays deduplicate across JSONL and OTLP JSON under the same source ID. Conflicting retained identities reject the entire import.

### Import evaluation results

Import the spans first, then select their exact OTLP source for the outcome
import and lookup. These commands also support `--broker-config` in place
of `--db`. A broker collector can import only into an allowlisted source.
Collectors and readers can read the outcome for an allowlisted source.

```sh
agenturm import-evals --file results.json --db history.sqlite --source-id otlp.exported
agenturm evals --db history.sqlite --source-id otlp.exported \
  --trace-id 11111111111111111111111111111111 --json
```

The importer accepts a regular `results.json` file or stdin (`--file -`)
up to 4 MiB. The top-level `results` array accepts up to 1,024 outcomes.
Each outcome requires a 32-character hexadecimal `trace_id` and a boolean
`passed`. Optional fields include `task_id`, `provider_model` and up to 32
scores. Each score supplies a `scorer` plus a boolean `passed`, a numeric
`score`, or both. Metadata labels allow up to 128 ASCII characters. They
start with a letter or digit and can also contain `._:/@+-`. The importer
rejects recognized credential prefixes and discards producer errors, score
detail text, inputs, outputs, and other result fields.

Every result must match a retained trace under the selected source ID.
Duplicate trace IDs within a batch or a missing match reject the whole batch.
Evaluation pass/fail is independent of the execution status of the spans.
Corrected metadata advances the outcome revision. An exact replay of any
previous revision leaves the latest outcome unchanged. Plain `evals`
prints the current outcome and `--json` returns its metadata or `null`.
SQLite backups preserve outcomes and replay receipts. Pruning removes
outcome metadata when the trace loses its last retained span in that source.
Retained digests and revision numbers prevent an old import from reviving
the outcome if a new span arrives under the same trace ID.
Aggregate evaluation views, revision browsing, and private result detail
remain outside this implementation.

### Task Git patches

Save a patch from the Linux checkout attached to a task. The optional revision
checks which version of the task receives the capture. The command reports
the updated task, Git summary, and saved patch byte count.

```sh
agenturm task snapshot 42 --db history.sqlite --expected-revision 3 --include-diff
agenturm task handoff 42 --db history.sqlite --include-diff --format json
agenturm task handoff 42 --db history.sqlite --include-diff --output ./handoff-42
```

The patch has separate staged and unstaged sections for tracked files. Git
records binary changes as notices. Untracked files contribute only to the
summary count. Capture uses detached Git configuration, a sealed index copy,
and bounded regular-file inputs. Git hooks, filters, external diff drivers,
text conversion and submodule traversal are disabled during patch generation.
The patch accepts display-safe UTF-8 up to 64 KiB combined across 64 changed
paths and 8 MiB of file and Git blob inputs. A changed HEAD or index, a split
index, an unsupported path, or a limit failure leaves the task unchanged.
Concurrent changes in the checkout can require another capture.

The patch stays in the private workspace ledger and its portable backups.
Only handoffs with `--include-diff` read the patch for display or export.
`--output` writes a new private directory with Markdown and HTML. A plain
snapshot or a workspace remap clears the previously saved patch. Broker task
commands use the same options with `--broker-config` and capture from the
client's checkout.

## SDK and evidence scope

| Producer or evidence | Required behavior and owner |
| --- | --- |
| Native OTLP producers | B5 starts with an actual OpenTelemetry SDK using the documented HTTP/protobuf launcher. Verify parent/child identity, exit/flush, durable reopen and explicit linkage to the intended source and session. Repeat via `serve-traces` for existing processes. SDK integration maintainer. |
| OpenAI, Anthropic, LangChain and LangGraph | Python 0.2.8 instruments OpenAI and Anthropic calls and LangChain automatically, with explicit callbacks also available for LangChain and LangGraph. Inventory producer-side exporters and test selected integrations against Rust before claiming equivalent coverage. Python Agenturm bootstrap remains a product decision. SDK integration maintainer and release owner. |
| OpenAI Agents SDK, Pydantic AI, Google ADK, Node.js | Python 0.2.8 supplies an Agents SDK adapter and exercises vendor OTLP exporters and a Node.js client. Test native OTLP producers independently. Decide which producer-side integrations stay required and which old Agenturm client ingestion routes to retire. SDK integration maintainer and release owner. |
| Prompt and MCP payloads | Preserve the recorded operation and current scope when opening content. Unavailable, revoked and over-limit evidence needs a visible outcome. B2 and B4 own the prompt and MCP detail gaps. |
| Usage, cost and host history | Preserve presence and units for input, output and cache fields. Show cost only when recorded. Rust currently has token counts, Kiro credits and current same-user Linux host data. Producer-reported cost, historical host metrics and process drilldown remain open. Telemetry maintainer implements the retained fields after the release owner sets scope. |

The vendor fixture matrix records the actual SDK paths and pending checks. [Rust OTLP vendor producer qualification](../validation/rust-vendor-sdk-producers.md)

## Required keyboard journeys

Run each journey at 120×32 and 60×18 in both installed applications. Record the exact archive or wheel digest, fixture, terminal, focused control and visible evidence. After each drilldown verify return to the same selected row or run. Record shortcut and layout changes in the result.

| Journey | Python 0.2.8 reference | Rust candidate evidence |
| --- | --- | --- |
| Discover and select a session | Bare launch, Agents, `/` search, arrow selection and Enter. Expand a native child with Right. | Empty-to-populated discovery, stable search selection, explicit parentage when recorded, and Escape restoring the original selection. |
| Follow interrupted work | Agents → Enter → Details and scoped Activity. Open recorded work, then use Back. | Use Enter and `t` to select a turn. Check recorded reason, scoped tools and edits, missing reason handling, a separate PR result and a later successful turn. Return to the same run. A6 and B3. |
| Read a retained prompt | Enter inspector, choose Prompts, navigate index or dropdown, Enter to read, scroll, page older, and return. | Use `p` and Enter for exact current-grant evidence. Scroll without changing the request, reopen an older prompt, revoke, and confirm denial while navigation remains usable. B2. |
| Inspect MCP and linked trace | Keys `3` and `4`, Enter for details, use related evidence links, and return. | Open MCP and Traces, match native operation or span identity, show only scoped granted payload and distinguish missing data. B4 and B5. |
| Review, decide and hand off | Ctrl+R opens Review, then saved task, Decisions and Handoff. | Use Review and `b` for a task with an exact run link, record an evidence-version note, export Markdown and HTML, and reopen the same decision. B3 and B6. |
| Triage a request | `w`, immediately select and open a request, then use the verified route and come back. | Read a pending request, persist seen state, verify the same run and route or a clear unavailable status, then return with selection intact. |
| Inspect Usage and Host | Global Metrics and Host views, scroll and Escape. | Show exact supported units, unknown counters and host availability. Check recorded cost and historical host evidence against the release disposition. |
| Help, editing and terminal lifecycle | F1 and Escape return to the same control, typed characters remain literal, quit restores termios. | Exercise help, search, note editing, resize in a reader, and exit. Record focus and scroll restoration at both sizes. |

The release verification owner runs the paired journeys. The cockpit maintainer owns focus and return behavior. [Python 0.2.8 keyboard reference](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/keyboard-navigation.md)
