# Rust cutover workflow inventory

Baseline is the Python `v0.2.8` tag and the published Rust 0.3.1 Linux bundle. The default switch requires the keyboard journeys and retained commands below to pass on the installed candidate. The Python comparison uses disposable 0.2.8 state. The Rust candidate uses a fresh Rust history directory and equivalent source fixtures.

## Command inventory

| Python 0.2.8 workflow | Rust path and cutover disposition |
| --- | --- |
| Bare `agenturm`, `cockpit`, global `--db` | Keep local startup, discovery, one-shot snapshots and explicit history selection. Bare Rust startup and `cockpit --db` exist. Use `query` for machine-readable history. Retain a documented answer for attaching to an existing remote receiver, `--new-window` and the old one-shot summary before removing those flags. |
| `serve` | Keep local OTLP collection and source watch through `serve-traces`, `discover --follow` and `collect --watch` or the Linux user service. The old read API, remote bind/authentication and arbitrary legacy JSONL tail are open scope decisions. |
| `run` | Keep command launch, exit status, local trace delivery and exact source/session linkage. Rust `run` configures OTLP for an instrumented producer. Python-specific script and module injection requires a product decision. |
| `import-jsonl` | Keep offline ingest where producers emit supported native recordings or OTLP. `collect` and `import-traces` are available. Existing exported span JSONL needs an explicit format or retirement decision. |
| `import-evals` | Open. Existing `agent-evals` result imports and joins have no matched Rust workflow. |
| `migrate-db` | Retired by the Rust-only product decision. Rust `backup` and `restore` operate on Rust-owned state. |
| `prune` | Keep explicit age-based retention. Rust `prune --before-unix-ns` exists. Verify equivalent retained history and checkpoint behavior. Automatic retention remains a scope decision. |
| `attention status`, `attention watch` | Keep durable pending and seen state and a scriptable status/watch path. Rust reads the local store. Remote receiver watch remains an open scope decision. |
| `review` | Rust cockpit and noninteractive Review use exact source, session, turn, and worker scope. The CLI reads a bounded page and exports metadata receipts in Markdown, HTML, and JSON. Trace-to-turn selection remains open. |
| `investigate path\|command\|instruction\|source` | Keep bounded exact-scope investigation when the evidence and current grant support it. Rust session search and history queries do not cover these search and source-reference workflows. B6 depends on content proofs for protected data. |
| `task list\|create\|show\|archive` | Rust supports create, exact show, completion, archiving and restoration. The default task list hides archived entries. `list --all` and `show` can still read them. |
| `task link\|unlink\|handoff\|backup` | Rust links use exact native source/session/turn/worker identities. `task handoff` reads one saved task revision and renders Markdown, HTML, or JSON. Its private export writes a Markdown and HTML pair. Workspace backup retains the ledger and supports restore into a new history directory. |
| `task update\|snapshot\|brief\|decide\|decisions\|draft\|approve-draft` | Rust supports revision-checked editing, local Git summaries, opt-in tracked text patches, five structured brief fields, and immutable decisions with supersession. Drafts and decision citations can use one exact retained user prompt. Approval and excerpt reopening require revalidation of the original grant and source receipt. Broader Review evidence projections remain open. |

The release owner records the disposition of legacy format support, remote receiver access, launcher injection, eval import, automatic retention, and task or review compatibility. Until there is a recorded disposition, any partially mapped workflow remains open. The CLI maintainer owns the implementation and installed scenarios for retained commands. [Python command source](https://github.com/abhishekash/agenturm/blob/v0.2.8/observatory/__main__.py) · [Rust workflow coverage](rust-cockpit-workflow-coverage.md)

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
