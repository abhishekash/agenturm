# Rust harness evidence coverage

This matrix describes the evidence each Rust adapter can emit and the checks
that exercise it. The Linux bundle defaults to Pi and Codex. Claude Code, Kiro
and OpenCode have separate Rust adapter crates and require explicit
configuration. Source continuity diagnostics report specific gaps the adapter
detected.

| Harness | Session, turn and lifecycle | Activity and usage | Coverage and relationships | Verification |
| --- | --- | --- | --- | --- |
| Pi | The JSONL header supplies session and optional worker identity. User row IDs identify turns. An explicit stop records completion or interruption, and a stop error carries `pi_stop_error`. Tool continuation records ongoing activity. | User and assistant roles, token and cache counters, tool calls and explicit results. A result carries its call ID and can omit turn identity. Check outcomes are open. | Shared JSONL continuity diagnostics and unsupported-row or role evidence. The header's `parentSessionId` supplies delegation between distinct sessions. Worker relationships are open. | [Adapter tests](../../rust/agenturm-pi-harness/src/pi.rs) and [commit, retry, ACK and reopen through the real adapter](../../rust/agenturm-runtime/tests/pi_process.rs) cover a fixture with explicit `parentSessionId`. |
| Codex | The rollout header supplies thread identity and can identify a worker. Native turn IDs attach when present. Task completion, explicit final answers and aborts produce bounded lifecycle evidence. The adapter diagnoses late outcomes it cannot attribute to the current turn. | User and assistant roles, token and cache counters, tool activity, a bounded set of explicit test, lint and build results, and a typed work result for an explicitly successful GitHub PR creation. A missing tool outcome produces a diagnostic. | Shared JSONL continuity diagnostics and unsupported, historical or unknown-result diagnostics. Explicit delegation and fork relationships retain child and parent thread IDs. Complete check coverage is open. | [Adapter tests](../../rust/agenturm-codex-harness/src/codex.rs) and [commit, retry, ACK and reopen through the real adapter](../../rust/agenturm-runtime/tests/codex_process.rs). A real local Codex rollout also passed collect, reopen and terminal inspection for delegation. |
| Claude Code | Main transcript session identity and optional agent identity. Observed user messages establish turn context. An explicit end-turn records completion. Unknown stop reasons produce a diagnostic. | User and assistant roles, tool calls and results, and per-model-message usage and cache counters. Check outcomes are open. | Shared JSONL continuity diagnostics and unsupported-row, stop-reason and content-block evidence. Parent/child edges are open. | [Adapter-level tests](../../rust/agenturm-claude-harness/src/claude.rs) cover stream checkpoints, tool outcomes and privacy. The local fixture run below verified completion after reopen. |
| Kiro | Configured JSONL path and native row identities distinguish main and worker streams. Turn-end records preserve explicit completion, cancellation or failure. Bounded interaction tracking records pauses and resumed activity. | User and assistant roles, correlated tool calls and results, and a distinct credit-usage fact from native summaries. Check outcomes are open. | Shared JSONL continuity diagnostics and scoped permission, queued-prompt and unsupported-evidence diagnostics. The spawning transcript's `sub_agent_start.subSessionId` supplies the immediate worker relationship. | [Adapter-level tests](../../rust/agenturm-kiro-harness/src/kiro.rs) cover path identity, permissions, checkpoints, and credit evidence. A [real-process fixture](../../rust/agenturm-runtime/tests/kiro_process.rs) commits, ACKs and reopens an explicit failed turn, two levels of delegation, and fractional, zero and unknown credit evidence. |
| OpenCode | The current SQLite adapter uses session identity and explicit idle outcome when present. The session-level lifecycle scope is limited to the session. | User and assistant roles, token and cache counters, and typed tool status from tool parts. Recognized direct test, lint and build commands emit a check when a native terminal shell exit is recorded. | Database continuity, oversized-record and unknown-outcome diagnostics. `session_v2.parent_id` and `fork_session_id` supply separate delegation and fork facts. Stable turn membership is open. | [Current SQLite adapter](../../rust/agenturm-opencode-harness/src/sqlite.rs) and [standalone check and relationship round trip](../../rust/agenturm-opencode-harness/tests/configured_checks.rs). The fixture run below verified an explicit idle failure after reopen. |

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
OpenCode database exceeded the current 512 MiB adapter limit. The snapshot
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
owning session. Direct Claude worker relationships still require parent
evidence.

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

[Host-owned Pi content grants](rust-content-grant-fence.md) are persisted.
Verified delivery of private content remains open. Pi has a
[separate experiment for bounded references](../../rust/agenturm-pi-harness/src/content.rs)
outside the standalone observation path.

Step 3 still requires actual-record collection, reopen and interactive inspection
for each supported harness, including missing evidence, subsequent turns, pane
navigation and narrow terminals. The fixture paths above establish the listed
adapter or durable behaviors.
