# Complete review-notes implementation plan

13 November 2026. Baseline: merged `d75872c` after PRs #67–70. Input: the supplied `agenturm-review-notes.md`, preserved unchanged. Status: reconciliation and staged implementation in progress. The user authorized completing the full findings and recommendations after the controlled monitor validation, including work outside the narrower recovery plan. This is not a claim that all findings are closed.

## Delivery and trust rules

Keep personal telemetry, preferences and source notes untouched. Use generated repositories, native records and receiver state for tests and measurements. Preserve private state/authentication, redaction, non-destructive migration, source ownership, capture-policy revocation, pagination and truthful unknown/partial usage. No provider key or account is required for local monitoring.

Each stage has a coherent code change, regression gates, independent review and a reviewable PR. Follow the repository's admin squash-merge preference and verify the remote merged state and local main; report pending CI separately. Keep changes Unreleased until explicitly asked to release. Supply responsive self-contained light/dark HTML companions for documents.

Use the existing Agents → attention → evidence → continuity surfaces. Avoid additional dashboards, generic Host/Metrics work, themes, AI-generated summaries, extra manual task fields, agent launching, native approval execution or autonomous remediation. Optional integrations must not weaken zero-config collection. Observations and human decisions remain distinct.

## Reconciliation of all 27 findings

“Implemented” means a concrete code path and regression/controlled evidence exists, not universal producer compatibility or user adoption. “Partial” identifies remaining work below. Positive findings are preserved rather than treated as missing features.

| Finding | Current status and evidence | Remaining contract / stage |
| --- | --- | --- |
| F1: CPU/RSS | Partial. PR70 fixed HTTP/TLS overhead. PR72 reduces generated-history idle CPU from 23.63% to 15.90% of one core and cold readiness from 184.38 to 112.96 s. The earlier active/probed hour is a different workload, not an idle baseline. | Measure idle and normal-load budgets; profile readiness/read/render costs; B. |
| F2: retention | Implemented for new managed stores: 14-day age policy; existing stores need explicit opt-in. Migration/retention tests preserve legacy data. | Preserve policy during optimizations; B. Age retention is not a byte cap. |
| F3: launch-directory telemetry | Implemented: private shared state, owner-only files, explicit safe legacy migration; PR67. | Preserve during watcher and hook work; F. |
| F4: one-shot truthfulness | Implemented: readiness wait and distinct ready/error/partial/interrupt exits; PR67. | Include headless regressions in B/F. |
| F5: OpenCode step versus turn | Implemented in reader, Review UI/CLI and provenance rollups; PR67. PR71 preserves full turn membership through the UI saved-link boundary. | Preserve whole-turn scope through C/D. |
| F6: Codex evidence omissions | Implemented: native edits/checks/text/usage reach authenticated API, Review/export and task-link fixtures; PR67. | Extend existing producer-shaped coverage; G. |
| F7: noisy attention | Implemented for explicit requests/unknown separation; shared-daemon and incident-specific gates; PR67/70. | Preserve the same semantics without a cockpit; F. |
| F8: useful information density | Partial. Merged PR72 uses one separator at wide sizes and two-line compact rows, retaining shading, hierarchy and keyboard selection. | Source-backed Overview at wide/floor sizes remains open; B/C/D. |
| F9: old completion backlog | Implemented: recent/history selection, readable ages, frozen-ID bulk review; real terminal R fixed and 294 receipts verified in PR70. | Reuse identical receipt behavior in headless runner; F. |
| F10: no recorded continuity use | External adoption evidence absent. Headless Review-to-task composition works; no participants or voluntary reuse measured. | Reduce manual work via approved drafts; D. Prepare materials in G; participant outcomes stay open. |
| F11: shared-daemon false progress | Implemented: multiplexed process work never proves per-session progress; old session stayed unconfirmed throughout the hour. | Preserve stronger-evidence requirements in F. |
| F12: unknown children as attention | Implemented: unknown/unlinked status is separate from actionable requests; PR67. | No hook or collision heuristic may promote this into an approval. |
| F13: strong MCP view / recommendations | Existing inventory/call inspection is a preserved strength; per-tool interpretation remains incomplete. | Windowed reliability/latency and coverage-qualified recommendations; E. |
| F14: generic failure detail | Partial. Commands/exit/output and expected search exits improved; structured native detail extraction and grouping still diverge. | One operation/failure projection and exact navigation/recovery observations; C. |
| F15: historical JSON expansion | Partial. Source Activity and scalar/run projections exist; remaining session/MCP/readiness costs are material. | Profile and remove remaining repeated historical work; B. |
| F16: local API/redaction | Implemented: local tokens, Host/Origin, redirect refusal and expanded redaction; PR67/70. | Mandatory invariants for indexes, exports, background runner and hooks. |
| F17: native add/update | Implemented and regression tested; PR67. | Retain failed-attempt distinction in C/D/G. |
| F18: wrapped checks | Implemented with conservative shell recognition and masked/compound/no-op negative cases; PR67. | Share command semantics in C. |
| F19: usage ownership | Implemented for proved exact-run/turn allocation, unknown fields and partial costs; PR67. | PR71 closed the UI turn-link downgrade; explain qualified usage in E. |
| F20: assistant placeholders | Implemented for supported native text variants; placeholders suppressed; PR67. | Continue schema/version-shaped fixtures in G. |
| F21: lost initiating intent | Implemented: bounded independent request/update context and explicit coverage; PR67. | Typed instruction investigation in G uses the same source references. |
| F22: complete run shows no work | Implemented ingestion → API → Review/export contracts; 294 turn reconstructions in PR70. | Preserve through all later projections. |
| F23: Review/task mismatch | Implemented in PR71: UI/CLI links preserve turn identity/membership/usage and reject contradictory snapshots. Older ambiguous links stay readable with untrusted totals until explicitly repaired. | Preserve consent, provenance and authored fields through C/D. |
| F24: passing suite / fixture gap | Preserved baseline strength; PR72 passed all 12 final CI jobs: 775 minimal-suite cases per supported Python version with 14 optional skips, plus separate SDK jobs. | Current native-shaped golden journeys remain release gates, not test counts alone. |
| F25: OpenCode prompt exclusion | Implemented via explicit recorded-turn membership; exact-step mode remains explicit. | A/C/D must preserve full turn scope. |
| F26: idealized fixtures | Current Codex/OpenCode-shaped fixtures and end-to-end journeys implemented. | Extend deidentified, version-labelled supported-runtime coverage in G. Live Claude/Pi claims require actual evidence. |
| F27: cockpit-bound delivery / native events / Git | Open outside the first recovery scope. A receiver collects independently but does not share durable attention delivery; Review lacks frozen current Git correlation. | Read-only Git D; shared headless attention and optional lifecycle F. |

## Coverage of all eleven recommended value problems

| Recommendation | Implementation contract |
| --- | --- |
| R1: reliable cross-runtime attention | Existing classifier/terminal handoff plus persistent headless delivery with durable dedup; F. |
| R2: truthful turn receipts + Git | Scope repair A; shared facts C; frozen read-only current Git correlation D. No worktree attribution or check certification. |
| R3: actionable failure diagnosis | Structured command/exit/output/arguments, occurrence-preserving fingerprints, expected nonzero distinctions and matching-later-success evidence and occurrence-owned Findings filtering/counts; C. |
| R4: automatic continuity | Editable draft from observed request/edits/checks/errors, existing human decisions and captured Git; explicit approval before saving, no authored-field overwrite; D. |
| R5: parallel collisions | Recorded modifying-path overlap only within proved workspace/branch, with rename/attempt/worktree/unknown handling and cross-agent check/edit chronology; D/E. |
| R6: waste/loops | Conservative progression observations combining repeated failures/calls/compactions and usage with captured edit/check progress; E. No declaration that intentional work is waste. |
| R7: MCP recommendations | Per-tool outcomes/durations with denominators, windows and capture coverage; “unobserved” never means unnecessary; E. |
| R8: task usage explanation | Explicit turn/day/lifetime labels, retry/cache explanations, authoritative context occupancy only, and optional clearly labelled user-configured estimates separate from recorded cost; E. |
| R9: retrospective investigations | Typed path/command/instruction searches over retained indexed evidence, bounded paging, CLI output and exact source navigation; G. |
| R10: safe collection | PR67 defaults remain non-negotiable; new data/projections obey capture policy, retention, redaction and auth; all stages. |
| R11: optional native lifecycle | Versioned opt-in input and supported-runtime recipes researched from primary/native contracts; preserve unknown identities and zero-config discovery; F. |

The seven recommended recovery steps map to A/C/G (native correctness), A/F (turn model), B (economics), F (persistent accurate attention), inherited safety gates, D (drafts), and G (pilot materials/external outcomes).

## Stage A — scope consistency and UI continuity repair

First fix `capture_selected_run()` and `_link()` so a whole-turn snapshot cannot be accepted under a step key. Carry receiver/producer/agent/session/turn identity and the full bounded step membership from the same frozen projection. Reject contradictory wrapper/snapshot scope kind, identity or membership. Preserve readable legacy task/decision content while making previously ambiguous link accounting unavailable until explicitly repaired; do not silently reinterpret stored human data.

Gate: actual Review → saved-task UI → handoff preserves turn identity, checks and usage; UI and headless imports agree; turn plus contained step cannot double-count; contradictory snapshots fail; legacy incomplete links remain inspectable without trusted totals; capture-policy and size limits remain intact.

## Stage B — measured economics and useful existing views

Profile the retained fixture before choosing queries or caches. Investigate connection-count JSON grouping, repeated session scalar/label lookups, MCP aggregation and per-trace migration connection/transaction work. Any cache must distinguish raw/derived/ownership/capture revisions and cross-process writes, and must not serve text under another owner or namespace. Migrations remain resumable and readiness truthful.

Use public framework lifecycle patterns to investigate repeated modal/render retention; avoid an unmeasured private-framework patch. Improve useful roster density while preserving one clear separation between roots, deeper child indentation, alternate shading and stable keyboard/scroll position. Existing Overview should show readable source-backed work, not generic placeholder summaries.

Targets on the same generated corpus/reference host: substantially shorten cold readiness (target under 30 seconds), steady idle complete-monitor CPU under 5% of one core, and session/MCP p95 under 100 ms under the controlled write workload. Record achieved values and limitations; do not trade away correctness to meet targets or call changed workloads a before/after result. Repeat normal-load and active-probe comparisons with identical conditions. If a target is missed, keep it explicitly open.

Gate: indexed query plans, source/derived invalidation, concurrent ownership/content snapshots, pruning, file replacement, partial/unknown usage and interrupted migration remain correct. Render 120×36, 80×24 and 60×18. Keep packet/disk/subprocess work off the UI event thread.

## Stage C — shared actionable operation and failure facts

Extract structured native command/result/output once for Findings, Review, diagnostics and later search/drafts. Group by a safe operation/error fingerprint while retaining every occurrence's actor/session/turn/source ID. Different arguments/errors and redacted-ambiguous inputs must not collapse into one supposed failure or recovery.

Expected nonzero search/comparison results require actual command and exit evidence; unknown wrappers/compound failures cannot be declared informational. A passing retry is “later matching success observed,” not proof all consequences were repaired. Show bounded useful stderr/result excerpts and an exact event action. Keep current/previous-conversation unseen error reminders and incident receipts independent from archival findings and native approvals.

Peer-reviewed acceptance details: command/arguments and terminal results keep typed field provenance; arbitrary JSON in output must not become command or exit evidence. Conflicting exits/results remain uncertain, and combined output is never renamed stderr. Input equivalence includes captured cwd and tool/server identity, preserving types and list order; shell whitespace and quoting are not broadly normalized. Input and output clipping qualify matching independently. Redacted, missing or contradictory inputs and incomplete error signatures cannot prove equivalence. Only authoritative simple-shell terminal evidence can make exit code 1 informational; blocked/denied/MCP error evidence and compound failures remain visible. A matching success needs a strictly later comparable timestamp and the same receiver/actor/session/execution scope.

Correlation fingerprints are versioned and computed after capture/redaction policy, never from raw secrets. Durable viewed-error receipts keep their occurrence identity. Findings page rows, counts and hydration must use each occurrence's actual producer/actor/session in one source snapshot, including shared parent/child traces; trace-summary ownership is insufficient. Turn references and capture revocation must survive every projection/export.

Gate: wrapped/nested native shapes, masked failures, grep/diff versus validation errors, secrets, different workers/directories, retries, stale history, equal timestamps, clipped arguments, legacy references and later successes; exact source navigation preserves filters and selection.

## Stage D — frozen Git correlation and approved handoff drafts

Reuse `capture_workspace()` and its bounded read-only Git safeguards. Require a proven local workspace binding or an explicit user-selected remote-to-local mapping; a remote receiver's cwd string is not proof that a local directory is its checkout. Freeze current branch/HEAD/status/diff, observation time, workspace/base provenance and clipping with the evidence receipt. Explicitly distinguish recorded agent paths from all current worktree changes, pre-existing work and concurrent edits. An earlier check does not validate the current worktree. Capture only on deliberate review/draft actions in a worker; no Git polling storm. Metadata-only or revoked source capture must not expose derived Git diff content through Review, drafts or exports; separately authored saved-task material keeps its own explicit ownership and consent boundary.

Generate a clearly labelled editable draft from the original request, scoped evidence, captured Git and saved accepted/rejected human decisions. Drafting must remove repeated identifier entry. Preserve authored fields and require an explicit approve/save action for durable task updates; do not manufacture decisions, constraints or intent from assistant claims. Path-overlap observations use recorded modifying events within proved workspace/branch; handle rename pairs, failed attempts, separate worktrees and missing identity. Same-directory occupancy alone is not a collision.

Gate: real temporary Git repositories with dirty/staged/committed/untracked/renamed states, moved HEAD, partial output, filters/hooks/lazy-fetch defense and timeouts; overlapping writers and check-before-other-agent-edit chronology; draft → edit → approve → saved task/handoff, with no mutation before approval and no authored-field loss.

## Stage E — evidence-qualified MCP, progression and usage explanations

Add per-tool reliability/latency windows with known/unknown outcome and duration counts. Explain small samples, scope, capture boundaries and configured-but-unobserved servers. Recommendations link to observed calls/configuration context; never suggest deleting configuration from absence alone.

Extend opt-in repeated-work observations with source-linked progress context, failures/successes/edits/checks, compaction and usage growth. Exclude routine polling and unprovable equivalence. Keep heuristics out of explicit request counts and desktop alerts. Explain turn/day/lifetime usage, retry contributions and cache semantics. Show context occupancy only from explicit authoritative runtime fields. Optional estimates use explicit rates/units/accounting assumptions, are separate from recorded cost, and become unavailable when allocations are ambiguous.

Gate: mixed runtimes/workers, window boundaries, unknown durations/outcomes, redacted arguments, intentional retries/polls, new progress, missing context/usage and partial model/rate coverage. No hypothetical provider cost is presented as a bill.

## Stage F — persistent attention and optional lifecycle integration

Factor scope preparation, classification, persistence and delivery into one UI-independent runner. A long-running local receiver/watcher can deliver without an open full-screen client; the TUI remains an optional client. Reuse the existing durable claims and receipts so multiple cockpits/watchers do not duplicate alerts. First-install inventory is a quiet baseline. An established watcher must deliver once a still-pending, never-delivered request that arose while it was offline; already-delivered or acknowledged incidents must not repeat. Reconnect must not turn baseline silence into permanent loss of new requests. A fresh explicit request remains until a matching positive resolution. Storage failure is visible and cannot produce optimistic acknowledgements.

Define and fixture a bounded versioned optional native input before wiring recipes. Research current supported runtime contracts from primary documentation/local schemas; do not invent a hook or infer missing turn/request identity from time proximity. Validate sender/session/worker/turn relationships, request IDs, event ordering, replay and source precedence; preserve unscoped/contradictory evidence as uncertain. Installation/configuration is explicit. Collection and notifications never approve a native request, launch an agent or run a remediation command.

Gate: watcher/receiver restarts, two cockpits plus watcher, duplicate delivery claims, offline catch-up, same-text new request, acknowledgement, stale/reordered/duplicate events, missing/foreign parents, storage failure and bounded graceful drain. Auth, Host/Origin, local-token discovery, redaction and retention apply to the new path.

## Stage G — retrospective investigations and participant-ready pilot kit

Build typed path/command/instruction investigations on the same indexed source identities. Preserve global retained-history scope, bounded paging, disclosed clipping/pruning and exact event navigation; add reproducible CLI results. Reuse existing Activity storage/search instead of creating competing ownership or unbounded JSON scans. Measure any extra search indexing/read cost.

Prepare generated scenarios, expected answers, observer timing/error sheets, consent/data-handling instructions and reconstruction/resumption exercises using the existing pilot protocol. Include explicit approval versus unknown status, older check/later edit, turn membership, native terminal handoff, overlapping paths, reconnect and resumed decisions. No personal transcripts or participant identifiers belong in fixtures.

Gate: investigations answer which session changed a path, which command failed and which agent received an instruction across producers, including renames, unicode, missing IDs, pagination and retention. CLI/UI source navigation agrees. Participant materials render at desktop/mobile sizes and clearly state proposed measures and zero observed participant outcomes.

## External validation and completion ledger

### Stage A implementation — merged [PR #71](https://github.com/abhishekash/agenturm/pull/71)

Terminal capture now uses the shared execution preparation and preserves the frozen turn boundary, member trace IDs and each check's source trace. Task import rejects contradictory wrapper/snapshot identity, kind, membership and usage. Existing inconsistent links stay readable and retain authored data; their usage is withheld with a repair warning until an explicit relink or unlink. Turn-plus-contained-step accounting remains ambiguous instead of double-counted.

Validation includes an actual keyboard-driven Review → Save task → edit → Ctrl+S → task → handoff journey, UI/CLI import equality, later-member selection, contradictory imports, legacy reads/edits/explicit repair and metadata-only capture. Peer review exposed additional usage, consent and excerpt-provenance gaps: frozen usage now owns accounting; same-receiver capture revocation is rechecked before capture and task persistence without losing authored fields; decision excerpts retain their actual member trace instead of the turn anchor. Explicitly saving an older frozen receipt after receiver replacement retains only that receipt's facts. The focused gate passed 72 tests in 79.87 seconds; all 12 turn-continuity cases passed again after the final queued-save and frozen-namespace policy guards. HTML companions were rendered at 1280-pixel light and 390-pixel dark widths without document overflow or external assets. Stages B–G remain open; this stage does not complete the full review.

### Stage B first increment — merged [PR #72](https://github.com/abhishekash/agenturm/pull/72)

The frozen source adds indexed MCP/source lookups, bounded original-title reads, fair resumable derived batches, denser roster rows and public-API Workflow reuse. Peer review also closed reproduced cross-worker MCP enrichment, collector retry/replacement and sticky internal-capture races. The 219-test combined gate and 66-test final native delta gate passed; independent final retry review passed 24 tests. The final healthy matched comparison measured cold readiness 184.38 → 112.96 seconds, complete idle CPU 23.63% → 15.90% of one core, session-under-write p95 373.58 → 284.90 ms and MCP-under-write p95 246.09 → 142.60 ms. All three targets remain open. The earlier `ca93` run is invalid for resource claims because native ingestion recorded SQLite lock failures. See the [measurement report](../validation/history-read-economics.md) for source hashes, raw figures and limitations. Stage B remains open until its remaining useful-view and resource work is accounted for; C–G remain open.

### Stage B compatibility watcher increment — validation complete, PR preparation

The empty compatibility-queue probe reuses a read-only lease and releases it before writer acquisition. A fresh matched pair measured whole-monitor idle CPU 17.80% → 14.90% of one core, with unchanged cold readiness/read latency and all health/drain gates passing. Seven new concurrency/lifecycle cases, a 32-test focused gate and independent 18-test review passed. See the [separate measurements](../validation/idle-history-watch.md). Resource targets, static source preparation and source-backed Overview remain open; C–G remain open.

Participant recruitment/consent, live Claude/Pi data not available in this environment, measured human reconstruction/resumption time and voluntary repeat use cannot be manufactured by automated tests. Prepare the materials and report these dependencies with denominators; do not mark F10/adoption evidence complete without participants. Native integration capabilities unsupported by a runtime must be labelled rather than guessed.

After each merged stage, update this reconciliation with concrete files, tests, measurements and PR links. Technical delivery and observed user outcomes are separate statuses. The final full-note record must account for every F1–F27 and R1–R11, preserve the stated limits, and identify any external dependency still open.
