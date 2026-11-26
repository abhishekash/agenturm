# FD1 native evidence scan — Pi, Claude and Codex open shapes

Updated 23 November 2026. This record reports what a scan of this machine's
actual native agent records found for the three open FD1 shapes. Prompt and
result text is redacted throughout; only structure and field names are
reported. An unavailable native witness is a dependency, not proof that a
producer records the desired fact — and a positive witness is what allows
typed attribution work to start.

## A — Pi same-session worker relationships: no positive evidence

`$PI_CODING_AGENT_DIR` is unset; the default `~/.pi/agent/sessions` holds 30
`.jsonl` session files (format version 3, October 2026) across 10
project-named directories; every file was scanned.

- Event types present: `message` (13,080), `model_change` (75),
  `context_edit` (52), `thinking_level_change` (38), `session` (30),
  `compaction` (21), `grammar` (9). There is **no fork, worker or subagent
  event type**.
- Recorded tool calls include `bash`, `edit`, `read`, `write`, `codemode`,
  `caveman_retrieve`, and `mcp__ai_shed__*` — no `Agent`/`Task`-style
  delegation invocation anywhere.
- The ubiquitous `parentId` field is ordinary message-chain threading, not a
  worker relationship.
- The fields the Rust adapter looks for — `agentId` (pi.rs:106),
  `parentSessionId` (pi.rs:360), `parentSession` (pi.rs:123) — occur **zero
  times** in any file. The only subagent-adjacent text is an `Agent` tool
  *schema declaration* (never invoked) inside one system prompt, which is
  not a relationship record.

**Disposition:** the Pi fork-parent and worker-identity paths have no native
data to validate against on this machine. FD1-Pi stays an explicit evidence
gate; do not build typed attribution from fixtures.

## B — Claude asynchronous workers: positive native evidence

Source: `~/.claude/projects/-home-abhishekash-abhi-codes-agentic-agenturm/`
(7 `.jsonl` files). Findings are from real Agent launches in live sessions
(Claude Code 2.1.330); the declarations use tool name **`Agent`** — no
`Task`-named tool_use exists on this machine.

### B1 — async launch with structured result: 3 positive records + 1 resolution

An assistant `Agent` tool_use declaration (`input` carries `description`,
`prompt`, `subagent_type`) is answered by exactly one user `tool_result`
whose row-level `sourceToolAssistantUUID` matches the declaring assistant
uuid, with a structured `toolUseResult`:

```json
{"isAsync": true, "status": "async_launched", "agentId": "<16-hex>",
 "description": "<str>", "resolvedModel": "<str>", "prompt": "<str>",
 "outputFile": "<path under /tmp/claude-1000/...>", "canReadOutputFile": true,
 "canContinueAgent": true}
```

All three launches share this exact shape; no `completed` status exists in
any transcript on this machine. The embedded text block is internal metadata
prose, not a structured result.

### B2 — later resolution: the peer-handback row

One resolution record observed: a user row with `isMeta: true`,
`promptSource: "system"`, `turnOrigin: "peer"`, whose `message.content` is a
**bare string** (not a block array) and whose `origin` object is:

```json
{"kind": "peer", "from": "<agentId of the async launch>",
 "senderTaskId": "<same id>", "name": "Explore", "body": "<str>",
 "handback": true, "handbackReportText": "<str>",
 "handbackToolUseId": "<tool_use id from inside the subagent>"}
```

`origin.from` equals the async launch's `toolUseResult.agentId`. Surrounding
mechanics are `queue-operation` rows (`enqueue`/`dequeue`/`remove` with
`reason: "absorbed_mid_turn"` and a `deliveryId`). Only one resolution was
observed — enough to record the shape, not yet enough to implement
resolution attribution.

### B3 — worker transcripts and ancestry metadata

`subagents/agent-<agentId>.jsonl` (filename = the agentId): every row
carries `agentId`, `isSidechain: true`, and the **parent session's**
`sessionId` (workers do not get their own session id), plus normal
`parentUuid` threading. The side file `agent-<agentId>.meta.json` carries:

```json
{"agentType": "<str>", "isBuiltIn": true, "launchedWithDefaults": true,
 "description": "<str>", "toolUseId": "<the parent's Agent tool_use id>",
 "spawnDepth": 1, "requestShape": "background", "requestNonInteractive": true,
 "model": "<str>", "pickedEffortModel": "<str>"}
```

and for worktree-spawned workers additionally `worktreePath`,
`spawnedWithWorktree: true`, `worktreeBranch`. The `meta.json` `toolUseId`
links the worker transcript back to the parent declaration; `origin.from`
links the handback to the worker. This is concrete message-ancestry
evidence: parent session id + worker agentId + `isSidechain` + meta
`toolUseId`.

### Negative findings within B

- **Text-only results** (tool_result without structured `toolUseResult`):
  no positive evidence — all Agent results carry the structured object.
- **Multiple genuine tool_result rows for one tool_use id**: none. Apparent
  duplicates are always `type: "api-request-blob"` rows — recorded outbound
  API request echoes with no uuid and no `sourceToolAssistantUUID` — never a
  second genuine user tool_result.

### What the decoder currently rejects

`rust/agenturm-claude-harness/src/claude/worker_spawns.rs` requires
`status == "completed"` (line ~224) and a structured result (line ~223):
`async_launched` launches with a valid `agentId` are currently invisible;
the peer-handback row (bare-string content, no `sourceToolAssistantUUID`)
fails row deserialization and is ignored; `queue-operation` rows and
`.meta.json` files are never read. First-consumer-wins and the >3
declarations cap remain deliberate ambiguity guards.

**Disposition:** async launches have a positive, exactly-structured native
witness (declaration → structured `async_launched` result with child
`agentId`, plus sidechain transcript and meta linkage). This is sufficient
to implement a **launch-only typed attribution** — a child identity with a
declared launched state and **no completion claim**. Resolution attribution
(single sample) and `.meta.json` ingestion remain recorded shapes, not
implemented facts.

## C — Codex check/turn shapes: exec wraps shell in JavaScript source

Recent rollouts under `~/.codex/sessions/2026/11/*` record command execution
as `custom_tool_call` rows with tool name `exec` whose `input` is a **string
of JavaScript source** wrapping one or many shell commands (`const r=await
<cmd>` with branching on `r.value`; `await Promise.allSettled([...])` running
commands in parallel; `for p in <paths>; do … done` loops; `sed`/`rg`
chains). Three representative files with hundreds of such calls were
recorded (2026-11-21 through 23). The most recent rollouts additionally
contain a Codex collab layer as ordinary `function_call` rows —
`spawn_agent`, `wait_agent`, `followup_task`, `list_agents`,
`interrupt_agent`, `request_user_input_async`, `send_message_to_user_async`
— a worker-orchestration family separate from the exec shell layer.

**Disposition:** recognized direct-check coverage must eventually treat the
exec/JS wrapper as a check candidate; the collab-layer function_calls are a
new relationship evidence family. Neither is implemented by this record;
both are now on file for the check-coverage and relationship inventories.

### C1 — Native wrapper result status remains unproven (26 November)

A follow-up structure-only scan of the local November Codex rollouts found
58,919 `exec` custom-tool calls. Of these, 30,580 used a single literal
`exec_command` call in a JavaScript binding; none had an observed guard that
turns that command's native nonzero `exit_code` into a thrown wrapper error.
The observed `custom_tool_call_output` records expose their output as text or
content arrays and do not carry a structured exit/status field. The scan did
not inspect or retain command text or output content.

**Disposition:** keep wrapper check recognition gated. The current
`CommandExecution` path continues to require its structured command,
completion status and exit code. Do not infer a passed/failed check from
JavaScript output text or from a successful wrapper/tool call. Reopen only
when a real matched native call/output provides structured exit evidence.

### C2 — Collaboration calls do not provide a typed child edge (26 November)

The same local scan found 2,341 `spawn_agent` function-call records. Their
stringified arguments contained `task_name` and `message` (and sometimes
`fork_turns`); those are requested-task inputs, not a child identity. Among
these, 2,089 outputs matched by native call ID, and every matched output was a
string rather than a structured result object. No typed child ID was present
in the call arguments or matched output schema.

**Disposition:** keep this collaboration family inventoried but unsupported
for worker relationship facts. Do not derive child IDs, completion, liveness,
or names from task text or output prose. The independent native Codex state
database remains the supported source for its native conversation/task names.

## Outcome for FD1 scheduling

| Shape | Evidence | Scheduling |
| --- | --- | --- |
| Pi same-session worker relationships | Negative on all local records | Explicit evidence gate; no fixture-based work |
| Claude async launch attribution | Positive (3 records, exact structure, ancestry) | Implement launch-only typed attribution next; no completion claim |
| Claude peer-handback resolution | One observed record | Record the shape; implement only with more witnesses |
| Claude text-only / duplicate results | Negative | Keep the deliberate rejections |
| Codex exec/JS check wrapper; collab layer | Positive shapes recorded | Feed the check-coverage inventory; separate work item |
