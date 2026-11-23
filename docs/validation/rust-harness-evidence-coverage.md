# Rust harness evidence coverage

This matrix describes the evidence each Rust adapter can emit and the checks
that exercise it. The Linux bundle discovers Pi, Codex, Claude Code, Kiro, and OpenCode
through separate Rust adapter executables. Source continuity diagnostics report specific gaps the adapter
detected.

| Harness | Session, turn and lifecycle | Activity and usage | Coverage and relationships | Verification |
| --- | --- | --- | --- | --- |
| Pi | The JSONL header supplies session and optional worker identity. User row IDs identify turns. Native stop, error and abort reasons supply terminal evidence. Tool continuation records ongoing activity. | User and assistant roles, token and cache counters, tool calls and explicit results. A result carries its call ID and can omit turn identity. Recognized user shell checks with a noncancelled native exit produce session-scoped outcomes. | Shared JSONL continuity diagnostics and unsupported-row or role evidence. The header's `parentSessionId` supplies delegation between distinct sessions. A native sibling `parentSession` with both matching session headers and a copied first entry supplies a fork. Worker relationships are open. | [Adapter fixture](../../rust/agenturm-pi-harness/src/main.rs) covers native fork attribution and absence, and [commit, retry, ACK and reopen](../../rust/agenturm-runtime/tests/pi_process.rs) covers explicit `parentSessionId`. Real Pi RPC shell rows supplied passing and failing checks. A real Pi fork reopened with both exact native session IDs. |
| Codex | The rollout header supplies thread identity and can identify a worker. Native turn IDs attach when present. Task completion, structured task errors, explicit final answers and aborts produce bounded lifecycle evidence. The adapter diagnoses late outcomes it cannot attribute to the current turn. | User and assistant roles, token and cache counters, tool activity, a bounded set of explicit test, lint and build results, and a typed work result for an explicitly successful GitHub PR creation. Checks require a recognized command, its operation ID, and a consistent terminal status and exit code. Formatting and whitespace checks require an explicit check mode. Informational, dry-run and optional invocations supply tool evidence. | Shared JSONL continuity diagnostics and unsupported, historical or unknown-result diagnostics. Explicit delegation and fork relationships retain child and parent thread IDs. Complete check coverage is open. | [Adapter tests](../../rust/agenturm-codex-harness/src/codex.rs) and [commit, retry, ACK and reopen through the real adapter](../../rust/agenturm-runtime/tests/codex_process.rs). Actual rollouts retained delegation, interruptions, a task failure, a later completion, and typed unittest results. |
| Claude Code | Main transcript session identity and optional agent identity. Observed user messages establish turn context. An explicit end-turn records completion. Unknown stop reasons produce a diagnostic. | User and assistant roles, tool calls and results, and per-model-message usage and cache counters. Check outcomes are open. | Shared JSONL continuity diagnostics and unsupported-row, stop-reason and content-block evidence. Parent/child edges are open. | [Adapter-level tests](../../rust/agenturm-claude-harness/src/claude.rs) cover stream checkpoints, tool outcomes and privacy. The local fixture run below verified completion after reopen. |
| Kiro | Configured JSONL path and native row identities distinguish main and worker streams. Turn-end records preserve explicit completion, cancellation or failure. Bounded interaction tracking records pauses and resumed activity. | User and assistant roles, correlated tool calls and results, and a distinct credit-usage fact from native summaries. Check outcomes are open. | Shared JSONL continuity diagnostics and scoped permission, queued-prompt and unsupported-evidence diagnostics. The spawning transcript's `sub_agent_start.subSessionId` supplies the immediate worker relationship. | [Adapter-level tests](../../rust/agenturm-kiro-harness/src/kiro.rs) cover path identity, permissions, checkpoints, and credit evidence. A [real-process fixture](../../rust/agenturm-runtime/tests/kiro_process.rs) commits, ACKs and reopens an explicit failed turn, two levels of delegation, and fractional, zero and unknown credit evidence. |
| OpenCode | The current SQLite adapter uses session identity and explicit idle outcomes. A retained preceding idle boundary scopes later native messages, tool parts, and the closing outcome to one turn. | User and assistant roles, token and cache counters, and typed tool status from tool parts. Recognized direct test, lint and build commands emit a check when a native terminal shell exit is recorded. | Database continuity, oversized-record and unknown-outcome diagnostics. `session_v2.parent_id` and `fork_session_id` supply separate delegation and fork facts. A missing prior idle boundary leaves evidence scoped to the session. | [Current SQLite adapter](../../rust/agenturm-opencode-harness/src/sqlite.rs) and [standalone check and relationship round trip](../../rust/agenturm-opencode-harness/tests/configured_checks.rs). The fixture run below verified an explicit idle failure after reopen. |

A Linux run with the real Rust plugin host and adapters collected a synthetic
Claude Code transcript and an OpenCode SQLite fixture in one command. It
committed five Claude Code observations and one OpenCode observation, then
ACKed both. A fresh `agenturm query` process read Claude Code's recorded
completion and OpenCode's recorded failure with reason `opencode_idle_failed`.

A follow-up run collected real Claude Code transcripts, ACKed their batches and
reopened their lifecycle observations. The Codex sample containing internal
automated review was excluded by the adapter’s existing review boundary. A bounded snapshot of one actual
OpenCode session preserved 64 native message rows and yielded 107 committed
observations, including lifecycle, message, usage, and diagnostics. The live
OpenCode database exceeded the former 512 MiB database limit. The current reader accepts up to 1 GiB and retains separate WAL and row budgets. The snapshot
exercise covered native SQLite rows within that limit. The separate native-schema
check fixture supplied explicit tool exits and verified paired check/tool
operation IDs after restart. A separate metadata-only extract of ten native
OpenCode `session_v2` rows retained every exact child and parent ID through
collection, ACK, and reopen. The configured SQLite fixture covers fork and
delegation as separate facts and excludes malformed IDs and self-links.
After an idle cursor, a database change starts the next sweep.

The Kiro process fixture reopened a worker and a nested worker with their
exact spawners. Directory placement identifies a worker's source stream.
The spawn record supplies the immediate parent identity. Native Kiro credit
entries are summed and rounded to nine decimal places. Query reports the
integer number of nanocredits, and the inspector renders decimal credits.
An explicit zero remains a credit fact. Summaries without a valid total
produce a diagnostic. Turn scope comes from the summary's explicit native
identifier when present.
A fresh configured collection and query returned 323456789 nanocredits for
one fixture turn. The reopened 60×18 inspector rendered the same value as
0.323456789 credits in Turn Activity and restored the terminal on quit.

Claude Code's local worker transcripts exposed native agent IDs and their
owning session. The sampled worker rows repeated the shared session ID and
supplied transcript message ancestry. The sampled root transcripts also
contained Task or Agent results with a child agent ID. An exact spawn relation
therefore needs a worker-to-worker edge within a shared session and correlation
with the spawning call. The current durable relationship contract represents
distinct sessions. The sampled Bash results exposed output and interruption
metadata without a native exit field. The adapter retains the exact worker
and owning session. Message ancestry does not establish a worker-parent edge,
and a Bash output excerpt does not establish a check outcome.

## Native qualification on Ubuntu 24.04.5 x86-64

The Rust 1.100.0 development archive had SHA-256
`87a11b63f765d69669152d5bd24455a7b994bb0abf7e612b77e5718378821912`.
Each scenario used the bundle's pinned discovery catalog, a separate fresh
Rust store, an empty executable PATH, restarted collectors and a fresh query
process.

| Task | Native scenario | Durable result |
| --- | --- | --- |
| A1 | Pi 1.3.0 RPC appended two explicit shell checks to a temporary copy of a real session. A separate Pi 1.3.0 `--fork` capture supplied a native parent pointer and copied entry. | Both checks reopened with their paired tool operations and session identity. Their native rows lacked a model turn key. The real fork reopened with the exact parent and child session IDs. Native worker relationship evidence remains open. |
| A2 | Three local Codex rollouts covered a structured completion error, explicit interruptions followed by later completion, and direct unittest execution. | The failed turn reopened with `codex_task_failed`. The interrupted rollout retained two interruptions and two explicit successful completions. An exit of `-1` produced tool evidence with no check. Direct unittest exits produced one passed and one failed check with exact operation IDs. |
| A3 | An unmodified 43-row Claude Code worker transcript carried an agent ID and a shared session ID. | The worker identity and its explicit completion reopened through the supervised adapter. No durable worker-parent or shell check evidence was present. |
| A4 | No Kiro executable, configured transcript root, or default transcript directory was available on the qualification host. | The real-session gate remains open. Existing fixture runs cover native-format credit, failure, and delegation. |
| A5 | Two bounded samples of actual OpenCode SQLite rows covered an earlier idle boundary, subsequent inline tool activity, terminal idle outcomes, explicit shell exits and missing exits. | The 36-row sample retained first activity at session scope and later tools at turn scope, with both successful and interrupted endings. A 26-row window reopened one direct unittest pass and one failure, each paired to its scoped tool operation. |

The Codex failure mapping follows its producer's structured terminal outcome.
Pi's documented `parentSession` records the parent transcript file path.
An unresolved path remains unknown. To project a fork, the adapter requires
a distinct sibling path, native filenames that identify the two sessions,
matching header IDs and one exact copied entry. The process reads bounded
prefixes without following final symlinks. It records the two IDs.
[Codex terminal event source](https://github.com/openai/codex/blob/main/codex-rs/core/src/tasks/mod.rs) ·
[Pi session format](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)

On Ubuntu 24.04.5 x86-64, a local Pi 1.3.0 `--fork` produced a distinct child
transcript and repeated the parent's first entry. The published Rust 0.3.2
app and plugin host used an independently rebuilt Pi adapter with SHA-256
`8447ea38b0622bd2914ce6d0ee46a7b96b2cab454a4c2d10571c970eba7ba820`. Both
sources committed and ACKed through a pinned catalog. A fresh query reopened
one `forked` edge with the exact child and parent header IDs. A second
collection added zero rows. Previously consumed headers lack this edge unless
collected into a fresh source. Same-session Pi worker parentage remains open.

At 60×18, the installed cockpit opened Activity and a recorded turn for
each available provider. It displayed the Pi stop error, the separate Codex
failure and abort reasons, the Claude Code worker completion, and the
OpenCode idle interruption. Escape returned to the marked turn selection,
then Activity and Agents. Review opened from Agents and quitting restored
the terminal settings. The native Codex and OpenCode queries also verified
separate later or earlier turns. The missing lifecycle and Kiro terminal
scenarios below continue to rely on fixture evidence.

## Inspector acceptance

The Overview pane displays the latest recorded lifecycle state with the source,
session and optional reason. A separate row shows the exact turn ID when present.
An identified turn ending stays visible when a later lifecycle record describes
ongoing activity. The pane uses the currently loaded history page. The rows can
wrap and scroll in a narrow terminal.

A 60×18 Linux PTY showed a failed Kiro fixture turn beside the next ongoing
turn after collection and reopen. Scrolling exposed both IDs, Tab switched to
Metrics, and quitting restored the PTY's terminal settings.

Codex supplies a durable raised pull request when a native completed
`gh pr create` command has an explicit zero exit code, exact turn and operation
IDs, and a single canonical GitHub PR URL as output. The durable fact retains
the repository and PR number. Shell pipelines, command chains, dry runs,
assistant claims and output that needs interpretation fall outside that
evidence rule. An interrupted turn and a raised PR from that turn can both
appear after reopen. Terminal lifecycle state comes from native turn evidence.
A local run with a captured Codex command and terminal lifecycle record
verified via a fresh query that both observations retained the same native
turn ID and the PR reference matched the reported result.

A configured collection of an unmodified local Codex rollout on Ubuntu
24.04.5 x86-64 with Rust 1.100.0 preserved the exact child and parent thread
IDs from `source.subagent.thread_spawn.parent_thread_id`. A separate
`agenturm query` process returned the durable delegated relationship, and
the reopened terminal Overview rendered both IDs at 110×40. Fixtures cover
explicit forks, missing parents, conflicting parent IDs and self-links.
Existing cursors that already passed the rollout header do not backfill the
relationship.

The Pi process fixture preserves `parentSessionId` through a decoder reset,
commit retry, ACK and reopen. Local Pi headers sampled during this run
had no explicit parent session. Both adapters require a valid header timestamp
for relationship evidence, and the host treats each parent ID as meaningful
within the configured source. Deploy an updated host with adapters that emit
the relationship fact, since older hosts reject unknown observation kinds.

Eight unmodified Pi transcripts containing provider composite tool IDs
initially closed the adapter pipe. With bounded `call_…|fc_…` and
`call_…|ctc_…` IDs normalized to opaque operation IDs in the adapter, all
eight collected and reopened with matching call/result identities and
explicit turn endings. The real-process fixture checks the same pairing
through commit retry, ACK and reopen.

Press `t` to list turns with recorded native identities on the current
history page, use the arrow keys to select a turn, and press Enter to open
its durable activity. The list shows a recorded outcome when available and
explicitly identifies turns without lifecycle evidence. Esc returns to the
same selection and then to the history page. Activity queries retain the
history watermark and are scoped to the selected source, session, and turn.
A 60×18 PTY run opened an interrupted Codex fixture turn and its later
running turn, then verified terminal restoration on quit.

The independent Prompt Signals example was built from its own Cargo workspace
and selected through a SHA-256-pinned extension catalog. On a 110×24 PTY, the
inspector showed its pane with user and assistant counts and token totals from
reopened Pi fixture data. BackTab opened Metrics, where the same extension
reported its output-token count.

The [host-owned prompt grant path](rust-content-grant-fence.md) covers
pinned Pi, Codex, Claude Code, and Kiro JSONL sources and a current pinned
OpenCode SQLite sweep. JSONL history requires a retained receipt proof
covering the exact row. The fixture test reopens a failed OpenCode
turn beside later work and checks a scoped prompt and inline tool call.

Step 3 still requires actual-record collection, reopen and interactive inspection
for each supported harness, including missing evidence, subsequent turns, pane
navigation and narrow terminals. The fixture paths above establish the listed
adapter or durable behaviors.

## Core development follow-up: neutral native presence

The current source extends existing Codex header discovery to supported Pi and
Claude Code openings. A Pi exact session header, or supported Claude timestamped
metadata/user stub with an exact native session identity, emits the existing
neutral `session_discovered` fact when ordinary session facts are absent.
A proved existing Pi relationship already supplies that neutral identity and
is not duplicated. Unbound preludes, unsupported metadata and missing/invalid
native timestamps do not produce a row. Native file mtime is not substituted.

Verified header-only cursor backfill is idempotent and preserves the committed
source bytes/offset. Legacy admission omits the new fact. This is presence
evidence, not current liveness or Working state. Real lifecycle facts remain
authoritative. Forty-one focused adapter checks and strict Clippy passed;
actual collection/restart/concurrent roster acceptance remains delegated under
CF1 in the [core feature test plan](rust-core-feature-test-plan.md).

## Core development follow-up: explicit Claude worker spawning

The new `worker_spawn` fact retains the exact spawning parent session, optional
worker, turn and operation, plus one bounded child worker ID. It is separate
from distinct-session parent/fork edges. Claude requires an exact native
Agent/Task declaration UUID and tool-use ID, matching user result with
`sourceToolAssistantUUID`, same verified opening-turn owner/session/worker,
completed status and structured `toolUseResult.agentId`. Asynchronous or
ambiguous results, text and message ancestry do not create an edge. Older
checkpoints lacking an opening owner wait for the next native user turn.

Activity and session Overview expose declared-child details/counts on the
loaded evidence page without inventing child-session rows, liveness or a
recency-changing roster hierarchy. Focused decoder, contract, Store and
presentation checks passed. Actual installed native witness and broad private
scope/regression checks remain delegated under CF5 in the core test plan.
