# Rust cutover workflow inventory

Baseline is the Python `v0.2.8` tag and the published Rust 0.3.8 Linux bundle. The 0.3.8 baseline passed integrated runtime, terminal, private-content and native package qualification. The later 0.3.9 development candidate is in final qualification; its [current journey record](../validation/rust-ui-journey-parity.md) supersedes the older screen descriptions below. The default switch requires the keyboard journeys and retained commands below to pass on the installed candidate. The Python comparison uses disposable 0.2.8 state. The Rust candidate uses a fresh Rust history directory and equivalent source fixtures.

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
| Bare `agenturm`, `cockpit`, global `--db` | Keep local startup, discovery, one-shot snapshots and explicit history selection. Bare Rust startup, global `--db` and `cockpit --db` select local history. `cockpit --once` prints a bounded session report, with JSON and page coverage on request. The same report works through a broker or trusted remote reader. Use `query` for raw machine-readable history. `cockpit --endpoint` attaches through a trusted loopback listener or SSH tunnel. On Linux, `cockpit --new-window` opens the same cockpit with the selected history. The explicit Python 0.2.8 mode attaches to an existing tagged receiver for bounded live metadata. [Python receiver mode](#attach-to-an-existing-python-028-receiver) · [Remote history setup](rust-broker-commands.md#read-history-through-an-ssh-tunnel) |
| `serve` | Keep local OTLP collection and source watch through `serve-traces`, `discover --follow` and `collect --watch` or the Linux user service. `broker serve-history` serves the native broker's retained metadata on loopback through a dedicated reader. The Python `/api/*` wire surface, direct remote TLS and arbitrary legacy JSONL tail remain open. |
| `run` | Keep command launch, exit status, local trace delivery and exact source/session linkage. Rust `run` configures OTLP for an instrumented producer. The candidate can also send an explicitly instrumented Python script/module to an existing authenticated native loopback receiver using `--endpoint` and an owner-only producer `--token-file`, without a local database or new listener. The candidate adds explicit installed Python SDK auto instrumentation; plain launch remains unchanged. Actual selected vendor SDK jobs remain a final candidate gate. |
| `import-jsonl` | Use `import-jsonl` for supported native span exports and `import-traces` for OTLP JSON. Both retain structural trace metadata with exact session scope. JSONL can also capture supported private text with explicit opt-in. Legacy amounts without a currency remain unknown. |
| `import-evals` | Import bounded `agent-evals` result metadata into an exact retained OTLP trace source. Use `evals` to read the current outcome in text or JSON. `evals --history` pages retained revisions, `evals --summary` reports current source outcomes with page counts, and `evals --list` inventories the latest retained outcome per trace across explicit OTLP sources. In the cockpit, `E` opens evaluations and `d` opens approved private result text. |
| `migrate-db` | Retired by the Rust-only product decision. Rust `backup` and `restore` operate on Rust-owned state. |
| `prune` | Keep explicit age-based retention. Rust `prune --before-unix-ns` and persisted `retention set\|disable\|run\|status` are available. New default cockpit histories use 14 days; existing histories retain their policy. Collector, cockpit, local receiver and broker writer ticks apply it, while receipts and checkpoints survive cleanup. Broker policy changes require maintenance authority. |
| `attention status`, `attention watch` | Rust reads local history or an attached Rust receiver and keeps seen state in the user's workspace. Remote watch emits a snapshot and changes from retained native lifecycle evidence. Python 0.2.8 mode reads the exact native request inventory. Local `status --workflow` or `watch --once --workflow` adds body-free current incidents, saved policy and matched delivery receipts with exact attempts, without notification or state mutation. Legacy state migration and cross-host terminal routing remain open. |
| `review` | Rust cockpit and noninteractive Review use exact source, session, turn, and worker scope. The CLI reads a bounded page and exports metadata receipts in Markdown, HTML, and JSON. It can list the explicitly linked spans of a turn, or resolve a trace to its unique retained turn using producer-provided source, session, turn, and worker identity. |
| `investigate failures\|path\|command\|instruction\|source` | Rust local and broker investigations page retained failure metadata across authorized sources. Exact-scope searches for instructions, paths and commands use current grants and pinned source references. Codex command search accepts `--failed` for source-confirmed failures. See [failure investigation](#investigate-retained-failures) for the supported kinds and native evidence limits. |
| `task list\|create\|show\|archive` | Rust supports create, exact show, completion, archiving and restoration. The default task list hides archived entries. `list --all` and `show` can still read them. |
| `task link\|unlink\|handoff\|backup` | Rust links use exact native source/session/turn/worker identities. `task handoff` reads one saved task revision and renders Markdown, HTML, or JSON. Its private export writes a Markdown and HTML pair. Workspace backup retains the ledger and supports restore into a new history directory. |
| `task update\|snapshot\|brief\|decide\|decisions\|draft\|approve-draft` | Rust supports revision-checked editing, local Git summaries, opt-in tracked text patches, five structured brief fields, and immutable decisions with supersession. Drafts and decision citations can use one exact retained user prompt. Approval and excerpt reopening require revalidation of the original grant and source receipt. The candidate also accepts exact native and explicit Python receiver Review receipts, revalidating current source and private capture authority before draft publication. |

The CLI maintainer owns implementation and installed scenarios for the retained commands. Any later retirement requires an explicit product decision. [Python command source](https://github.com/abhishekash/agenturm/blob/v0.2.8/observatory/__main__.py) · [Rust workflow coverage](rust-cockpit-workflow-coverage.md)

### Session task labels

The current candidate uses an exact saved task title or safe native conversation
title, then an agent nickname, then concise provider/role context. Prompt-derived
title copies are excluded. IDs remain stable identity keys in details. Native
Codex and Claude name/project metadata has explicit provenance and exact
source/session matching. Renames preserve selection. The command center sorts
project groups by their newest member and sessions by descending update time;
status and hierarchy do not override recency. E4/E5 tests cover missing names,
duplicates, ties and selection movement. See the [native-session validation
record](../validation/rust-real-session-roster.md).

Prompt and payload text stays in explicit private views. Python receiver
attachment uses typed activity and trace evidence tied to the exact service,
session and actor; captured prompt fields do not label its live roster.

### Command-center first screen

Rust 0.3.8 opens with a full-width task table, aligned status
and updated columns, and project path headings with counts. `Tab` and
`Shift+Tab` select All, Needs you, Working, Ready or Inactive; counts apply to
the loaded evidence page and unknown states remain explicit. `G` cycles
project, source and ungrouped views. Project headings use the exact linked
operator task workspace when available; missing paths stay **Project unavailable**.
No path is inferred from prompt text or process cwd. `Enter` opens the
selected inspector; `Esc` preserves the group, filter and exact source/session.
Use `Show more` or `]` for the next recorded page.

`c` requests clipboard copy of bounded loaded metadata. Terminal support and
permission determine whether it reaches the clipboard. `x` opens a directory
form, then writes Markdown, HTML and JSON into a new private directory. It
never overwrites a destination, reads additional source content or exports a
private modal body. Query and form inputs own printable keys.

### Cockpit keyboard navigation

`1` Agents · `2` Activity · `3` MCP · `4` Traces · `5` Host · `6` Metrics · `7` Findings. `9` opens Operations, `U` opens workspace Usage, and `H` opens live local Host information. `8` remains a trace shortcut. On a selected session, `t` opens its exact linked Traces and `T` selects a recorded turn when available.

`Ctrl+R` opens Review for the selected session. `m` cycles the session overview, prompt index, and recorded Activity details. `P` opens the grant-controlled prompt index directly. In MCP, `m` opens a recorded payload selection when available. `M` opens that selection from another eligible pane. In Traces, `d` opens captured text selection when available. Outside that context `d` opens recorded diagnostics, which are also available with `D`.

`Ctrl+F` opens Investigate from the Agents roster or a retained history view. Use `Tab` to choose Failures, Instruction, Path, Command, or Failed command. From a selected session or turn, `s` lets Failures search all authorized sources. `Enter` searches and `PgDn` continues at the same snapshot. The results show occurrence metadata and page coverage. `Enter` on a result with source evidence opens a protected view after checking the current grant and exact retained source. `Esc` closes that view, then returns from Investigate to the same selection. An attached Rust HTTP receiver supports Failures metadata. Protected searches and evidence require local or broker history. The legacy Python receiver attachment supplies live metadata and has no retained investigation view.

`z` switches an open user prompt between Readable and original captured layout; both use the same already granted text and fixed 25-second display lifetime. Original layout preserves captured newlines and tab stops and allows horizontal scrolling. This does not reveal uncaptured source envelopes or affect other private viewers.

`p` pauses or resumes automatic refresh. `R` refreshes on demand. `[` moves to a loaded newer page, `]` reads an older page, and `l` returns to the latest page within the current scope. `a` resets the roster filters. `f` switches between tree and flat views, and `!` shows sessions needing attention. The current candidate keeps Updated ↓ order rather than exposing status sorting. `Esc` closes the current overlay or returns to the previous view. `F1` or `?` opens keyboard help.

On the Rust roster, `X` marks one exact pending request seen. When several current worker or turn requests match the session, the chooser lets you select one. The request stays pending until its source records a state change. A newly recorded request revision can need attention again. `Ctrl+R` or `V` opens Review for the selected session.

The authenticated Python receiver view uses `v` to include or hide captured internal approval reviews. The candidate separates explicitly typed native internal approval reviews behind a default-off visibility preference; ordinary sessions cannot be promoted into that inventory by a name. Python receiver attachment uses `1`, `2`, and `4` for its live metadata pages. `w` or `!` opens Pending, where the receiver's recorded requests can be inspected.

### Attach to an existing Python 0.2.8 receiver

Use an authenticated Python 0.2.8 receiver already listening on loopback, or
forward its port over SSH. Supply its URL-safe bearer token in an owner-only
regular file. The Rust client opens the configured loopback socket directly.
It neither discovers a receiver nor starts one. The protocol option must be
explicit to attach to Python.

```sh
agenturm cockpit --endpoint http://127.0.0.1:14318 --token-file receiver.token \
  --receiver-protocol python-v0.2.8
agenturm cockpit --once --json --endpoint http://127.0.0.1:14318 \
  --token-file receiver.token --receiver-protocol python-v0.2.8
agenturm attention watch --once --endpoint http://127.0.0.1:14318 \
  --token-file receiver.token --receiver-protocol python-v0.2.8
```

The attached cockpit shows bounded live Agents, Activity, Traces, Pending and
Findings pages. Press `7` for Findings, Enter to inspect a selected ID and `s`
for the exact supported source under current receiver/capture authority. Escape
redacts and returns to the same finding. [Tagged Findings qualification](../validation/rust-python-findings.md). Enter on a session scopes the loaded evidence to the exact recorded
service, session and actor where supplied. Escape restores the roster selection.
The one-shot JSON response uses `agenturm-python-receiver-live/1` and labels
readiness and partial coverage. Reads check the receiver header and any body
identity, then confirm the same health identity after the batch. The Python
protocol provides live pages without a Rust history watermark. The candidate adds explicit gated private Review and Review-backed task receipts. It verifies current capture policy, exact receiver/owner/member identity and a fixed display lifetime; private viewers have no copy/export. Cross-host terminal routing remains unavailable. Ordinary client output excludes prompt, log and arbitrary provider bodies.

Attention requires a ready, complete native lifecycle inventory. A partial or
unavailable read leaves request state unconfirmed. A watch preserves the last
exact pending evidence within its process and clears a request only when the same
owner, sender and request record an ordered resolution. A missing request is
insufficient. `attention watch --once` exits unsuccessfully if inventory cannot
be confirmed. This mode creates no Rust history or durable workflow state.

### Investigate retained failures

```sh
agenturm investigate failures --db history.sqlite --limit 50
agenturm broker investigate failures --broker-config broker.json --kind tool
agenturm investigate command 'cargo check' --failed --source-id harness.codex \
  --session-id session-id --db history.sqlite --config sources.json
```

`failures` scans authorized observation metadata, with optional exact
`--source-id`, `--session-id` and `--kind` filters. Supported kinds are
`lifecycle`, `tool`, `mcp`, `check` and `trace`. It reports explicit failed
lifecycle and check states, failed or denied tool and MCP calls, and error
span statuses. Eval outcomes are available separately through `evals`.

Each newline-delimited JSON page ends in a coverage record. `--limit` sets
the number of observations examined, from 1 to 100. Pass `next_cursor` as
`--cursor` with the same filters to continue at the pinned read snapshot.
A page can contain zero matches and a continuation cursor.

Each candidate carries a reported result and exact occurrence metadata.
Eligible Codex shell candidates include a `source_reference`. Open the
reference with `investigate source EVENT_ID --reference TOKEN` and the
appropriate local or broker configuration. Successful source inspection
requires a current Codex command-detail grant and verifies the same retained
occurrence and its terminal evidence. `command --failed` applies that same
proof before emitting a matching invocation. Known informational exit-one
commands, such as a plain `rg` with no matches, and contradictory terminal
records are excluded from confirmed results. Receiver-wide discovery reads
metadata. Phrase search of private evidence requires an exact source and session.

### Open the cockpit in a new window

Run `agenturm cockpit --new-window` on Linux. From a tmux pane, the cockpit
opens a window in that session. From a Linux desktop, it can launch GNOME
Terminal, Konsole, or xterm when a display is available.

```sh
agenturm cockpit --new-window --db ./history.sqlite
agenturm broker cockpit --new-window --broker-config ./broker.json
```

The launcher keeps relative paths based on the original working directory.
Local history, broker history, and `cockpit --endpoint` use the same
options as in the current terminal. The command reports terminal launcher
errors and returns the launcher's status. tmux returns after creating the
window. The cockpit exits in the new window.

Use `cockpit --once` for a report in the current shell.

### Import exported spans

Use `import-jsonl` with one exported native span object per line. The importer accepts a regular file or stdin (`--file -`) up to 4 MiB and 1,024 span records. It validates the complete batch before opening history for admission.

```sh
agenturm import-jsonl --file spans.jsonl --db history.sqlite --source-id otlp.exported
```

Rows carry `trace_id`, `span_id`, optional `parent_span_id`, `start_unix_nano` and `end_unix_nano`, and optional `status` (`UNSET`, `OK` or `ERROR`). Timestamps accept integers or decimal strings. IDs use 32 hexadecimal characters for the trace and 16 for each span. A parent outside the file is valid for a partial export.

A span needs an exact session ID from its `attributes` or the explicit `--session-id` argument. Span-level session aliases must agree, including `session.id`, `gen_ai.conversation.id`, and the legacy session, conversation and thread aliases. The argument fills missing session IDs and must match any IDs carried by the file. A native turn link requires `agenturm.source.id` and `agenturm.turn.id`. A worker link adds `agenturm.worker.id`.

The default import retains trace structure, timing, status, exact scope, and explicitly reported cost and token usage. Span names, events, links, and eval results are outside this import. Exact structural replays deduplicate across JSONL and OTLP JSON under the same source ID. Conflicting retained identities reject the entire import.

Use `--payload-scope standalone` to opt in to the two private text attributes `agenturm.payload.input` and `agenturm.payload.output`. Use `--payload-scope harness.codex` for rows that explicitly carry that `agenturm.source.id`. The session comes from an exact row attribute or the operator's `--session-id`. The scope flag must match the source of every captured span. Each slot is limited to 2 KiB and the captured text in a batch to 64 KiB. Invalid or credential-like selected text rejects the batch with an error that omits its contents.

```sh
agenturm import-jsonl --file spans.jsonl --db history.sqlite --source-id otlp.exported \
  --session-id demo --payload-scope standalone
```

Add `--payload-format agent-harness` to select the legacy agent-harness fields instead. For `agent.run`, `agent.task` and `agent.summary` fill Input and Output. For `tool.call`, the exported executed `tool.arguments` and `tool.result` fill those slots. Empty legacy slots are omitted. The format excludes proposed arguments, approval rationales, exceptions, other attributes, and event bodies. Agent-harness exports without a session ID need the explicit `--session-id` option.

Text is stored with its exact span occurrence in the private telemetry vault. A [telemetry text grant](rust-broker-commands.md#preview-opted-in-telemetry-text) is required to view it. Select capture on the first import. Replaying the same text deduplicates, changing the text or adding it to a metadata-only occurrence conflicts, and reimporting pruned text leaves it retired. Metadata queries and import output omit text.

The input JSONL remains a caller-managed file. Keep private exports in a
directory accessible to the collector and use owner-only read permissions
on the file, such as mode `0600`. The vault grant controls access to the
imported copy.

### Publish live progress

The Python and Node manual SDKs publish `Run.progress` immediately to the
Rust trace receiver. The progress route uses the same loopback authority
and producer bearer as spans. The direct JSON route is
`POST /native/v1/progress` with one event per request and a 4 KiB limit.

Each event carries a nonzero 32-digit hexadecimal `event_id` and `trace_id`,
a decimal-string `observed_at_unix_ns`, and a public `stage` of 1 to 64 ASCII
letters, digits, periods, underscores, or hyphens beginning with a letter.
Send optional integer `percent_basis_points` from 0 to 10000. Each basis
point is 0.01 percent. Zero is an explicit value and omission means unknown.
The SDK sends no progress message or arbitrary attributes.

The receiver selects the storage source. A standalone receiver requires
`session_id`. The launcher may supply its existing local run identity
when the event targets no native harness. Native linkage uses an explicit
`session_id` and `target_source_id`, plus `turn_id` and `worker_id` when
applicable. Linked reads require a retained native anchor. The event ID is
immutable within its storage source, including after retention expires
the event.

Activity shows the retained events. Traces shows the latest report per
exact scope on the loaded page, even while the parent span is still open.
Progress records express reported stages and percentages. Span completion
and native lifecycle state have their own evidence.

### Reported token usage

OTLP JSON, OTLP protobuf and native JSONL retain explicit input, output, total,
cache-read input, cache-write input, and reasoning-output token counts. The
projector accepts supported `gen_ai.usage.*`, `llm.usage.*` and OpenInference
`llm.token_count.*` fields, including the legacy explicit cache and reasoning
aliases. The Python and Node producers forward the supported scalar counters.

Each counter accepts a nonnegative integer through the signed 64-bit limit.
Zero is a reported value. Missing or invalid values and conflicting aliases
leave that counter absent. Each breakdown keeps its originally reported units.
The projector preserves the producer's input and output counts and records total
only when supplied. A batch can contain up to 1,024 spans with cost and token
usage evidence recorded beside each span in the same commit.

Workspace Usage and the originating OTLP session's Usage show the individual
reports. `query` includes the source, exact session, trace and span, and
any reported turn, worker and target harness. Aggregate usage views remain
open for traces where multiple instrumentation layers report the same usage.

### Reported cost

OTLP and native JSONL spans can carry `agenturm.reported_cost.amount` and
`agenturm.reported_cost.currency`. The Rust-compatible Python and Node
producers, including the Python OpenTelemetry metadata exporter, forward
these fields as bounded scalars. The amount accepts a decimal string or
number. The currency is a three-letter uppercase code. The receiver also
accepts `llm.cost_usd`, `llm.cost.total` with `llm.cost.currency`, and
`gen_ai.usage.cost` with `gen_ai.usage.cost.currency`.
The agent-harness JSONL exporter calculates its leaf `llm.cost_usd` from
its own price table. The importer keeps that producer-reported amount in
USD. It omits `agent.cost_usd` and `agent.usage.*` run roll-ups, which can
repeat the measurements on leaf spans.

Amounts use integer nano-units, with nine decimal places. Explicit zero
is a cost record. Missing or invalid amounts, conflicting values, or an
explicit `llm.cost_known` value other than true leave the span without a
cost record. Query exposes each record's source, exact span, currency and
decimal-string `nanounits`. Workspace Usage and the originating OTLP
session's Usage show the amount and currency beside the source evidence.
Multiple spans can repeat a report, so Usage shows individual records and
leaves totals open. Kiro credits use their native credit unit. Agenturm
does not derive an amount from tokens, model prices, or credits.

The native Pi and OpenCode adapters also retain terminal assistant-message
cost reports in USD. Pi reads `message.usage.cost.total` with basis
`estimate` unless Pi explicitly marks `totalFromProvider: true`. Those
records carry basis `provider_reported`. OpenCode reads the completed
assistant message's `cost` with basis `unknown`, since the stored amount can
come from either endpoint accounting or OpenCode's rate estimate.
[Pi cost contract](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md#quick-start) ·
[OpenCode assistant schema](https://github.com/anomalyco/opencode/blob/v2.2.5/packages/schema/src/session-message.ts) ·
[OpenCode cost selection](https://github.com/anomalyco/opencode/blob/v2.2.5/packages/core/src/session/usage.ts)

Native records expose `native_origin.basis`, `native_origin.rounded`,
the retained revision and an operation ID that identifies the associated
assistant event. Amounts round to the nearest nano-unit. Positive amounts
that would round to zero are omitted, preserving the distinction from
an explicit zero. Usage shows the individual reports in the selected
history page. Forks can copy assistant histories and telemetry can repeat
the same provider report, so a combined cost total remains unknown.

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
rejects recognized credential prefixes in metadata labels. Metadata is the
default import. Optional private capture is described below.

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
Read retained revisions with `evals --trace-id <id> --history`. Use
`--limit 1..256` and the returned `--before-revision` cursor to continue.
`evals --summary` lists the latest outcome per retained trace in the selected
source and counts passes and failures on that page. Its `--after-trace-id`
cursor continues in trace-ID order. JSON includes the page and continuation
fields. These views read current state on each invocation; concurrent changes
can change a later page. Counts describe the displayed page, not a whole-history
total. Pruned bodies stay absent from both views even if a trace ID reappears.
The same options work through the broker and keep its source allowlist.

Use `evals --list` with one to 16 distinct `--source-id` flags for a complete,
bounded inventory of current outcomes. Each result identifies its exact source,
trace and latest retained revision. The same trace ID can appear independently
in different sources. All results share the transaction's trace evidence
snapshot, and evaluation metadata reflects current revisions at that read.

```sh
agenturm evals --list --db history.sqlite --source-id otlp.exported \
  --source-id otlp.ci --json
```

The inventory checks up to 1,024 candidate source/trace keys, including retired
keys kept for replay protection, and allows up to 2 MiB of serialized rows. If
either limit is exceeded, the command fails without a partial inventory. The
list includes neither private text nor private reveal selections.
[Broker CLI and authenticated HTTP inventory](rust-broker-commands.md#read-source-aware-metadata-with-http-get)

### Private evaluation text

Use `--include-detail` to capture the `error` string on a result and the
`detail` string on each score. The capture records the original score position.
The allowed fields accept up to 2 KiB each, 4 KiB per result and 64 KiB per
import. Scalar text is the supported format. A credential-like or unsafe
string rejects the batch before any result or detail commits.

```sh
agenturm import-evals --include-detail --file results.json --db history.sqlite --source-id otlp.exported
agenturm eval-detail grant --db history.sqlite --source-id otlp.exported
agenturm eval-detail preview --db history.sqlite --selection selection.json
agenturm eval-detail revoke --db history.sqlite --source-id otlp.exported
```

Copy the `detail_selection` object from `evals --json` for the exact revision
into `selection.json`. It contains the source, trace, revision, receipt ID
and snapshot. The receipt is issued with the new result revision and preserves
the original capture across retries. Reusing the same metadata with supplied
details requires the exact original capture. Capturing detail for an existing
metadata-only result requires a distinct corrected metadata revision. Replay
preserves the current version and leaves expired text unavailable.

Disclosure requires an `eval.result_detail` approval for the exact OTLP
source. The viewer verifies the capture receipt and its supporting retained
trace occurrence at the supplied snapshot. Pruning that supporting occurrence
removes the text and preserves the outcome's revision and detail digest.
Restored backups require fresh approval and a fresh snapshot. Metadata views
only return result metadata and the selection reference. See
[broker evaluation detail commands](rust-broker-commands.md#private-evaluation-text).

In the cockpit, press `E` from global or session Traces to choose a trace on
the loaded page. The picker preserves its selection by exact OTLP source and
trace ID, including when two sources report the same trace ID. Enter opens
the retained revisions and marks the current one. The detail shows the
execution status counts for the loaded spans alongside the separate evaluation
verdict. Page Up and Page Down scroll task, model and scorer metadata. Missing
score values appear as unknown.

Press `s` for the source's current outcomes, or `]` to continue when another
page is available. Escape returns to the revision, trace selection and Traces
views in order. The loaded trace snapshot establishes its identity. Each
evaluation request reads current retained metadata from that history and
fails when the selected snapshot has expired or the receiver has restored a
different branch. This metadata view also works through the broker and the
authenticated HTTP reader for allowed OTLP sources.

In the local or broker cockpit, select a revision marked `d private` to open
its imported evaluation text. Escape clears the private view and returns to
the same selection. The private view uses the current approval and expires
after 25 seconds.

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
| Usage, cost and host history | Rust shows recorded token counts, Kiro credits and individually reported OTLP costs. `host record` and `broker host record` capture optional Linux load, memory and CPU counters with known same-user processes. `5` opens recorded samples, then Enter opens the process list and exact process history. A v2 collection `host` block enables scheduled capture for local, broker, and managed Linux collectors. The HTTP reader can expose recorded Host history when started with `--include-host`. Aggregate cost views remain open. [Host history](rust-broker-commands.md#record-and-read-host-history) |

The vendor fixture matrix records the actual SDK paths and pending checks. [Rust OTLP vendor producer qualification](../validation/rust-vendor-sdk-producers.md)

## Required keyboard journeys

Run each journey at 120×32 and 60×18 in both installed applications. Record the exact archive or wheel digest, fixture, terminal, focused control and visible evidence. After each drilldown verify return to the same selected row or run. Record shortcut and layout changes in the result.

| Journey | Python 0.2.8 reference | Rust candidate evidence |
| --- | --- | --- |
| Discover and select a session | Bare launch, Agents, `/` search, arrow selection and Enter. Expand a native child with Right. | Empty-to-populated discovery, stable search selection, explicit parentage when recorded, and Escape restoring the original selection. |
| Follow interrupted work | Agents → Enter → Details and scoped Activity. Open recorded work, then use Back. | Use Enter and `T` to select a turn. Check recorded reason, scoped tools and edits, missing reason handling, a separate PR result and a later successful turn. Return to the same run. A6 and B3. |
| Read a retained prompt | Enter inspector, choose Prompts, navigate index or dropdown, Enter to read, scroll, page older, and return. | Use `P` and Enter for exact current-grant evidence. Scroll without changing the request, reopen an older prompt, revoke, and confirm denial while navigation remains usable. B2. |
| Inspect MCP and linked trace | Keys `3` and `4`, Enter for details, use related evidence links, and return. | Open MCP and Traces, match native operation or span identity, show only scoped granted payload and distinguish missing data. B4 and B5. |
| Review, decide and hand off | Ctrl+R opens Review, then saved task, Decisions and Handoff. | Use Review and `b` for a task with an exact run link, record an evidence-version note, export Markdown and HTML, and reopen the same decision. B3 and B6. |
| Triage a request | `w`, immediately select and open a request, then use the verified route and come back. | Read a pending request, persist seen state, verify the same run and route or a clear unavailable status, then return with selection intact. |
| Inspect Usage and Host | Global Metrics and Host views, scroll and Escape. | Show recorded cost, exact units, missing counters and host availability. Open two samples of the same process using its boot, PID and start identity, then return to the selected sample. |
| Help, editing and terminal lifecycle | F1 and Escape return to the same control, typed characters remain literal, quit restores termios. | Exercise help, search, note editing, resize in a reader, and exit. Record focus and scroll restoration at both sizes. |

The release verification owner runs the paired journeys. The cockpit maintainer owns focus and return behavior. [Python 0.2.8 keyboard reference](https://github.com/abhishekash/agenturm/blob/v0.2.8/docs/keyboard-navigation.md)
