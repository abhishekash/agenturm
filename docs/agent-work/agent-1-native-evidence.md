# Agent 1: Native harness evidence gaps

## Goal

Close only source-confirmed native evidence gaps needed to preserve supported
Python workflows in the Rust product. Do not add guessed provider behavior or
broaden provider support without native evidence.

## Own

- Audit the remaining F1 cases in
  [`rust-migration-remaining-plan.md`](../rfcs/rust-migration-remaining-plan.md#f1-complete-native-harness-evidence-and-roster-coverage):
  Pi same-session worker relationships, Claude asynchronous/ambiguous worker
  shapes, and specifically evidenced Codex/OpenCode/Kiro check, turn, or
  relationship shapes.
- For each case, capture the native source fields and exact identity chain
  needed to attribute the fact. Implement a typed fact only when the source
  supplies enough evidence.
- Keep missing parent IDs, exits, timestamps, and liveness explicitly unknown.
  Add focused fixtures/regressions for any newly supported source shape.

## Do not own

- Provider private-content reveal or MCP payload authorization; Agent 2 owns
  that work.
- Generic SDK/CLI parity inventory, UI redesign, release qualification, or
  broad test campaigns.
- A shared event/schema redesign without first proposing the smallest typed
  contract to the integration owner. Avoid editing shared dispatch/navigation.

## Parallel boundaries

Keep changes within the relevant provider adapter and its focused decoder or
domain tests. If multiple providers need a shared contract, submit the proposed
fields and evidence examples before changing shared DTOs so the other lanes can
continue independently.

## Deliverable

Report the base SHA and branch, provider/source shape, changed files, focused
checks, and remaining unsupported shapes. Mark each audited case as implemented,
confirmed gap, unavailable native evidence, or intentionally unsupported.

## Done when

Every worked case has source-backed attribution through persistence/reopen and
an evidence query, or a documented reason it cannot safely be supported. Broad
installed acceptance remains with the independent test owner.

## Audit result at base `fd398c193d1e1c6f42bb20990a84bbfb75c1674c`

Branch: `agent/1-native-evidence`. The source and evidence records at this base
already contain the supported typed facts below. This audit found no additional
native record that justifies widening an adapter, so it makes no decoder or
schema change.

| Case | Native fields and identity chain established by the source record | Status |
| --- | --- | --- |
| Pi same-session worker relationship | The observed native `parentSessionId` and `parentSession` path support a relationship between distinct session headers. Pi 1.3.0 samples and the real fork record established distinct parent/child session IDs, not a worker identity or parent turn within one session. | **Unavailable native evidence** for same-session workers. Keep the existing distinct-session fork path implemented; do not synthesize a same-session edge. |
| Claude completed worker spawn | `Agent`/`Task` declaration UUID and tool-use ID, followed by a user result whose `sourceToolAssistantUUID` and `tool_result.tool_use_id` match; the same session, worker and opening turn must own both records; `toolUseResult.status = completed` and `toolUseResult.agentId` provide the child. | **Implemented** for this exact completed shape in `rust/agenturm-claude-harness/src/claude/worker_spawns.rs` and `src/claude.rs`. |
| Claude async or ambiguous worker result | The current native evidence does not establish a completed child identity for asynchronous results, cross-worker results, or results without an exact declaration/result join. The decoder has negative regressions that withhold those edges. | **Intentionally unsupported** until a positive native record establishes the same exact parent and child identity chain. Sampled Bash output/interruption fields do not establish a shell exit or check result. |
| Codex explicit turns and direct checks | Rollout `event_msg` terminal payloads identify `task_complete`, `task_aborted` or `turn_aborted`, with `turn_id` when supplied. A recognized direct command in a completed item is a check only with the exact tool operation and consistent native status/exit evidence; formatting checks additionally need explicit check mode. Actual captures established direct unittest pass/fail and structured completion/interruption cases. | **Implemented** for the recorded shapes in `rust/agenturm-codex-harness/src/codex.rs`. |
| Additional Codex check/turn shapes | Existing records do not establish every native check mode or terminal event. The coverage inventory itself remains open; unsupported command forms and conflicting/missing exits intentionally remain tool evidence or unknown outcomes. | **Unavailable native evidence** for further shapes. Extend only after a positive rollout identifies the exact item, command/check mode, operation, turn and terminal fields. |
| OpenCode direct checks and relationships | Native SQLite session rows expose `session_v2.id`, `parent_id` and `fork_session_id`; native messages/tool parts and shell metadata can carry turn boundary, command, status and exit fields. Samples established explicit idle boundaries and direct unittest exits. | **Implemented** for observed fork/delegation and direct test/lint/build checks in `rust/agenturm-opencode-harness/src/sqlite.rs`. Missing idle boundaries or exits stay session-scoped/unknown. |
| Further OpenCode shapes | No additional sampled row establishes a safe check or relationship contract beyond the above. | **Unavailable native evidence**; keep unknown boundaries and outcomes explicit. |
| Kiro checks or further transcript shapes | No actual Kiro executable, configured transcript root, or real transcript was available in the recorded qualification. Existing fixtures exercise `sub_agent_start.subSessionId`, `parentExecutionId`, turn outcomes and credit summaries, but fixtures alone do not establish a newly supported native shape. | **Unavailable native evidence** for further Kiro shapes. Check outcomes remain open; do not infer them from tool text or fixture-only fields. |

The detailed source anchors and process/reopen evidence are in the
[harness evidence matrix](../validation/rust-harness-evidence-coverage.md),
especially its A1–A5 native qualification and the Claude worker-spawn follow-up.
This work changed only this assignment record. No focused code checks were
needed because no code or decoder behavior changed; broad installed acceptance
remains with the independent test owner.
