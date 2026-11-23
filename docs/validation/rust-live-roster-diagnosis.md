# Rust 0.3.8 live roster diagnosis

Historical diagnosis of published Rust 0.3.8, recorded on 23 November 2026.
The user initially requested diagnosis only, then authorized implementation
with “do it”. The sections below describe 0.3.8; the isolated 0.3.9 candidate
is implementing the resulting requirements. Published 0.3.8 assets and the
user’s native source files remain unchanged.

The reported screen shows three loaded sessions, one Working and two Ready,
under Project unavailable. Its labels include MCP · send_message_to_thread,
Recorded file edit and Untitled session. These labels do not establish which
particular running sessions were excluded.

## Confirmed source behavior

| Area | Current behavior | Consequence |
| --- | --- | --- |
| Discovery | Default discovery selects four transcript files per harness by filesystem modification time. The supported per-harness maximum is eight, with sixteen configured sources globally. | Discovery is a bounded recent-file selection rather than an inventory of every running session. A running session can be outside that selection. The exact reported exclusions require runtime comparison. |
| Explicit history inspection | `--db` inspection opens history read-only and does not start a collector. | This mode needs a separate active writer to receive new observations; it is not equivalent to bare startup. |
| Live collection | Bare startup uses follow discovery. If the history has another writer, the cockpit opens that history without starting its own collector. | Effective data coverage depends on the actual writer, selected roots, discovery results, progress and failures. A roster refresh alone cannot collect an undiscovered source. |
| Session roster | The catalog reads up to 200 retained source/session summaries. Status comes from the latest recorded lifecycle fact. | All and Working describe the loaded retained evidence; they are not an operating-system process census or proof that every live agent is represented. |
| Session titles | Exact saved task titles take priority. Remaining labels are selected from a bounded retained structural page: PR, file edit, MCP, check, tool or trace. | The screenshot's generic labels are expected from this implementation. Native display/session titles are not projected into the roster. |
| Project headings | Paths are read from explicit operator-saved Git snapshots linked to a current local session-level task. | Native session cwd/project metadata is absent. Project unavailable is expected without a qualifying task link, even if the harness knows its project directory. |

Source references in the released code:

- `rust/agenturm-app/src/discovery.rs:13` — discovery limits and default;
  `:97` — source budgeting; `:236` — newest-modification-time selection.
- `rust/agenturm-app/src/cockpit_startup.rs:215` — default follow discovery;
  `:250` — shared-history writer fallback.
- `rust/agenturm-app/src/lib.rs:112` — configured-source limit;
  `:1903` — retained roster page; `:1959` — lifecycle projection.
- `rust/agenturm-app/src/session_titles.rs:27` — operator task title priority;
  `:36` and `:118` — bounded structural fallbacks.
- `rust/agenturm-app/src/session_projects.rs:23` — saved Git-root projection.
- `rust/agenturm-store/src/session_catalog.rs:61` — recorded lifecycle semantics.

## Confirmed native display-metadata sources

Read-only inspection by the coordinating investigation confirmed these local
sources. They provide explicit metadata for the pending native-context work;
the current Rust roster does not consume them.

| Harness | Source | Confirmed fields |
| --- | --- | --- |
| Codex | `~/.codex/state_5.sqlite`, `threads` table | `id`, `title`, `name`, `cwd`, `agent_nickname`, `agent_role`, `agent_path` |
| Codex | `~/.codex/session_index.jsonl` | `id`, `thread_name`, `updated_at` |
| Claude Code | Native transcript records of type `custom-title` | `customTitle`, `sessionId`, `type` |
| Claude Code | Native transcript records of type `agent-name` | `agentName`, `sessionId`, `type` |
| Claude Code | Native transcript context rows | `cwd`, `slug` |

Task/session titles, agent names/roles and project directories are distinct
metadata and should remain distinct in the future projection. Match the exact
native session and available agent identity, resolve freshness and precedence,
retain provenance, and preserve operator overrides. Do not treat a slug or
role as a task title or fall back to raw prompt/command extraction. These are
verified local fields, not a guarantee that every installed harness version
uses the same database filename or record shape.

## Requirements arising from this diagnosis

1. **Live-session discovery and coverage.** Define and implement the expected
   active-session inventory, separate live membership from recorded lifecycle,
   avoid silent omission under capacity limits, and expose discovery/admission
   coverage and catch-up state. Confirm whether filtered internal-review files
   consume selection slots before normal sources are admitted.
2. **Native display context (E2).** Use the confirmed Codex and Claude metadata
   sources above. Carry explicit native session titles and project
   or cwd metadata through exact source/session identity and history proof.
   Preserve explicit operator-title overrides. Keep raw prompts, commands,
   outputs and unrelated private bodies outside this metadata path. Generic
   last-activity labels should remain visibly distinct from task titles.
3. **Real concurrent-session acceptance.** Verify actual concurrent sessions,
   including more than four per harness, idle-but-running sessions older than
   newer inactive files, internal review/subagent files, large-transcript cold
   catch-up, restart and newly created sessions. Compare expected native IDs
   with discovery selections, admitted observations, roster membership, titles,
   projects and statuses. Cover 120×32, 80×24 and 60×18.

These requirements drove the 0.3.9 candidate. Their validation and remaining
bounds are recorded separately; this historical diagnosis alone is not proof
that any implementation gate has passed. Increasing a numeric cap alone would
not establish complete live discovery or correct status/title semantics.

## Reported read-only runtime findings

The coordinating investigation read the primary default history without
modifying it and reported schema version 11, 273,102 observations, 334 distinct
sessions, and a newest observation timestamp of 22 November 2026 at 14:56 UTC.
Those figures are evidence about that inspected history only. They do not
establish that the screenshot's running application selected the same database.

The screenshot's database identity, actual invocation and collector ownership
remain unconfirmed. In particular, an explicit `--db` inspection is read-only
and does not create a live collector. Its freshness depends on another writer.
Do not use the primary history statistics to claim the exact missing-row cause.

## Human-readable naming acceptance (E2)

The user explicitly requires names that humans can recognize. Session IDs
alone do not satisfy the roster's naming requirement.

- The primary row label must be the native task/conversation title or an
  explicit operator-saved task name. Generic MCP, file-edit, tool or trace
  activity labels do not pass named-session acceptance.
- Agent nickname, provider and project provide supporting context. Keep task
  identity separate from agent identity and latest activity.
- Session/source IDs remain available in details and copy actions; they must
  not displace the human-readable primary label.
- Renaming a task or native conversation must retain stable selection, links,
  history scope and references. Display names must not become identity keys.
- Handle duplicate titles, subagents and missing names explicitly. Duplicate
  names need supporting context; absent names need an honest unavailable-name
  state rather than invented names or a generic activity presented as a title.
- Acceptance must compare actual Codex and Claude native names with their
  roster labels, including rename, duplicate-name and subagent scenarios.

These requirements remain the acceptance contract for the authorized
implementation. The historical diagnosis itself did not change runtime behavior.

## Runtime evidence still needed

Record the actual launched executable and arguments; HOME and harness root
selection; selected history path and writer ownership; expected missing native
session IDs and paths; discovery's selected source paths; collection failures,
committed cursors and catch-up progress; and the resulting source/session
catalog. Compare identities and safe metadata without copying private bodies.

The screenshot plus static code supports the behavior gaps above. It does not
prove whether a particular missing session was excluded by the cap, filtered
source handling, a different root/history, an inactive collector, admission
failure, catch-up latency, or a lifecycle/status interpretation mismatch.


## Implementation follow-up

The 0.3.9 candidate adds bounded follow-source rotation, priority for exact
same-user native transcript handles, exclusion of confirmed guardian reviews
before adapter admission, and visible discovery coverage. Open handles are
selection evidence; roster status remains the recorded native lifecycle.

Native display context uses exact retained source cursors, native file identity,
and the selected history incarnation. Explicit operator names and project links
win. Safe native Codex names and Claude custom titles follow; source/session IDs
remain stable keys in inspector and copy actions. Structural tool activity is
supporting activity, never a substitute name. Prompt-like Codex title fields are
suppressed instead of copying the initial user prompt into the roster.

The inspection mode remains read-only and now states that discovery is disabled.
Large or unavailable metadata sources fail closed and report bounded coverage.
Actual native root and process-witness acceptance is required before claiming
that the original running-session complaint is fixed.
