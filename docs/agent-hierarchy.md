# Agent hierarchy

Agenturm observes delegated agents as a tree of **agents**, not as a flat list
of sessions. Hierarchy is built only from runtime metadata that names an exact
parent and child. A nested span, shared project, shared working directory,
similar title, process ancestry, or copied conversation is not evidence of a
delegation.

## Identity

Every node has an opaque `agent_id` derived from:

```
runtime + producer/source namespace + native execution id
```

The resolver is `observatory.agent_hierarchy.canonical_agent_id`. Discovery,
storage, API scope resolution, and the terminal tree use the same resolver.
A spawn result and a later child transcript therefore resolve to one node.

The normalized node also keeps the available native identifiers:

- `native_session_id`: the runtime's session/conversation identifier.
- `native_worker_id`: the worker/agent identifier when a runtime shares one
  session across several agents.
- `source_id`: the existing Agenturm evidence source identifier.
- `parent_agent_id`: canonical parent identity, or null.
- `relationship_kind`: `delegated`, `forked`, or `unknown`.
- `relationship_state`: `confirmed`, `unresolved`, `conflicted`, or `invalid`.
- `relationship_provenance`: bounded adapter/record/time evidence. Never raw
  arguments, environments, or transcript content.
- `discovery_state`: `observed` or `announced`.
- `coverage`: `complete`, `partial`, or `unknown`.

An absent parent field is not proof that a session is a root. Only a runtime's
explicit root marker or a confirmed delegation edge changes that label. Forks
and continuations remain separate from delegation.

## Runtime capability matrix

Validation used synthetic fixtures matching the public transcript/database
shapes below. Live validation against each installed runtime version remains
pending where noted.

| Runtime | Child discovery | Exact delegation linking | Independent child evidence | Known limitations |
| --- | --- | --- | --- | --- |
| Codex | Separate rollout JSONL files in the same date tree | `session_meta.payload.parent_thread_id`; also `source.subagent.thread_spawn.parent_thread_id`. Successful `spawn_agent` output may announce the exact child `agent_id` before its transcript arrives. | Yes, each rollout has its own trace/activity evidence. | Older subagent rollouts without `parent_thread_id` stay unlinked. `forked_from_id` is a fork, not a worker edge. |
| Claude Code | `~/.claude/projects/<project>/<session>/subagents/agent-<agentId>.jsonl` | Transcript path parent session plus optional sidecar `agent-<agentId>.meta.json` (`parentAgentId`, `toolUseId`). A successful parent `Task`/`Agent` tool result carrying `toolUseResult.agentId` may announce the child. | Yes, each sidecar transcript has its own recorded request and tools. | Sidecars are optional on older versions. Without a sidecar `parentAgentId`, the session directory is the parent. Older inline sidechain records are not split into workers. |
| OpenCode | `session` rows with `parent_id` set | `session.parent_id` | Yes, each child session has its own messages/parts. | Recent activity rows without a retained session row remain unlinked observations. |
| Pi | Only explicit parent fields if a Pi version supplies them | `parentAgentId` / `parentSessionId` / `parent_id` when present | Not independently verified | No documented Pi subagent transcript format was confirmed. Unlinked sessions are labelled unknown rather than assumed roots. |
| SDK / OTLP | Only the explicit convention below | `agent.parent_id` with `agent.id` | Yes, if the producer emits child evidence | A nested span alone is not proof of a subagent. Producers that omit the attributes remain unlinked. |

Codex, Claude Code, and OpenCode shapes above were checked against published
format notes and exercised with synthetic fixtures. On the validation host
(2026-10-30), the installed schemas also contained the expected identifiers:
Codex CLI `0.164.0` (`thread_spawn`, `parent_thread_id`, `spawn_agent`),
Claude Code `2.1.304` (`parentAgentId`, `toolUseResult`,
`listSubagentIdsFromDisk`), and OpenCode `2.0.32` (`parent_id` /
`session_parent_idx`). A real model-backed parent/child capture on each runtime
is still pending because the local CLIs require account/provider use. Pi `1.3.0`
exposes subagent concepts in its bundled SDK, but its local JSONL parent fields
were not verified; automatic Pi linking is not claimed.

Codex history forked from a parent is shown with `relationship_kind=forked`.
OpenCode sessions are namespaced by database path, so equal native session IDs
from two databases do not merge.

## SDK / OTLP convention (additive)

Producers may attach these attributes to spans and/or `agent.activity` logs.
All are optional; existing exports remain valid.

| Attribute | Meaning |
| --- | --- |
| `agent.id` | Exact native execution identity, unique within the producer namespace. |
| `agent.parent_id` | Exact native execution identity of the delegating agent, in the same namespace. |
| `agent.namespace` | Producer/source namespace. Defaults to the resource `service.name`. |
| `agent.worker_id` | Optional worker identifier when one session hosts several agents. |
| `agent.runtime` | Runtime label, for example `SDK` or a product name. |
| `agent.relationship.kind` | `delegated`, `forked`, or `unknown`. |
| `agent.relationship.state` | `confirmed`, `unresolved`, `conflicted`, or `invalid`. |
| `agent.coverage` | `complete`, `partial`, or `unknown` for the child evidence. |

`session.id` remains the evidence grouping key. If several agents share it,
supply `agent.worker_id` and use distinct `agent.id` values. A parent is only
linked when `agent.parent_id` names the exact native identity under the same
namespace. Span ancestry alone never creates an edge.

## Storage and reconciliation

SQLite changes are additive and versioned through `schema_meta`:

- `agent_nodes` stores normalized identity, bounded relationship provenance,
  discovery state, coverage, and first/last observation times.
- `agent_identity` indexes runtime/namespace/native identity to canonical
  `agent_id`.
- `spans`, `logs`, and `trace_summaries` gain nullable `agent_id` columns for
  exact scoped evidence queries. Existing rows remain readable.

Upserts are keyed by canonical child identity. Child-before-parent observations
are retained; a later exact parent observation makes the edge available.
Announced spawn nodes are replaced by observed child telemetry without a
duplicate row. Competing confirmed parents retain bounded claims and mark the
node `conflicted`; it is projected as unlinked rather than resolved by arrival
order. Self-links and cycles are marked `invalid` and rejected for projection.
Retention may delete the parent while keeping a newer child. The child remains
visible with unavailable-parent context; a placeholder is not counted as a
working agent.

## API scope

All scope parameters are additive. Legacy calls keep exact-session behavior.

```text
GET /api/hierarchy?limit=2000
GET /api/activity?...&agent_id=ag1_...&subtree=1
GET /api/mcp?...&agent_id=ag1_...&subtree=1
GET /api/traces?...&agent_id=ag1_...&subtree=1
GET /api/findings?...&agent_id=ag1_...&subtree=1
```

`subtree=1` resolves canonical descendant membership in SQLite. Exact mode uses
only that node. Filters, totals, summaries, pagination, and finding occurrence
counts use the same resolved membership before limits are applied. Responses include `agent_scope` and a hierarchy revision. Pass
`scope_revision=<revision>` with continuation requests; a change is rejected
instead of mixing pages from different membership sets. The cockpit also resets
paging when membership changes and rejects responses from a superseded
selection.

## Terminal behavior

The Agents view defaults to a tree when at least one confirmed delegation edge
exists. `f` or the command palette switches between tree and flat presentations.
Up/Down selects visible rows. Right expands a branch, or moves to its first
child when already expanded. Left collapses a branch, or moves to its visible
parent. Enter and `e` keep their existing inspect/expand-evidence behavior.
Clicking the disclosure area toggles children; clicking elsewhere selects the
row as before. Search and Recent/Active filters reveal required ancestor paths
without rewriting the user's saved expansion state.

Every row's State column describes that node only. Descendant work and attention
appear as a separate subtree summary, so a completed parent never becomes
"Working" because a child is active. Announced nodes say “Spawn recorded ·
status unavailable” and never count as working. Expansion state is in-memory and
is not written to UI preferences.

At narrow widths, indentation is capped and deeper rows retain an explicit
`depth N` label instead of dropping the relationship. `AGENTURM_ASCII=1` uses
ASCII branch/disclosure glyphs.

## Limits and privacy

- At most 5,000 relationship observations are accepted in one upsert.
- Graph traversal is bounded to 10,000 nodes; projection is iterative and uses
  one adjacency index per hierarchy revision.
- Provenance strings are capped at 512 bytes; parent claims are capped at four.
- Native discovery keeps its existing source-count, lookback, byte, and cache
  limits. Hierarchy does not add recursive scans.
- Metadata-only capture retains structural relationships from session headers
  and sidecars, but omits task/description text, prompts, arguments, and outputs.
- Diagnostics report node/announcement counts, persistence failures, and the
  hierarchy revision, not prompts or raw spawn payloads.
