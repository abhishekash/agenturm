# RFC Agenturm attention review and task continuity

| Field | Value |
| --- | --- |
| Status | Engineering implementation complete; external pilot pending |
| Date | 11 November 2026 |
| Scope | Local terminal workflows for monitoring, reviewing and continuing agent work |
| Implementation | [Joint implementation plan and execution record](joint-implementation-plan.html) |
| Basis | [User research](../research/2026-11-11-terminal-agent-research.html) and the current repository checkout |

## 1 Summary

Make Agenturm the place engineers check to answer three questions: **Which agent needs me? What evidence should I review? What must the next session know?**

Build on the existing Today view, Workflow inbox, recorded activity, saved tasks and handoffs. Consolidate their state and evidence rules before adding more features. Ship in three milestones: reliable attention and review; structured task continuity; then bounded diagnostics for repeated failures and wasted work.

The key architectural change is a shared, exactly scoped evidence projection used by the session inspector, Workflow, review exports and saved-task capture. User-owned task decisions remain separate from observed agent activity. Agenturm continues to observe agents; approvals and replies happen in their native terminals.

## 2 Problem and rationale

The research identifies verification, context loss and missed requests as recurring problems. The largest broad problem is the effort of checking plausible output. Agenturm's strongest initial fit is attention management because it can reveal a recorded request and help an engineer return to the relevant terminal. Preserving decisions and assembling review evidence extend that same workflow.

The [research report](../research/2026-11-11-terminal-agent-research.md) contains the public source register and ranking limits. These findings establish a product hypothesis, not measured market prevalence or willingness to pay. Existing alternatives already cover process dashboards, usage reports and cross-tool handoffs, so the proposal connects those jobs around a durable task.

Example: an engineer starts a Codex implementation and a Claude Code investigation. The investigation requests input; Agenturm surfaces the reason and returns the engineer to its verified terminal. The implementation later reports completion. Its review shows recorded checks, an edit made afterward, and a missing check result. The engineer saves the task with a rejected approach and a next step, then previews a brief for a fresh session.

## 3 Goals and boundaries

| Goal | Completion criterion |
| --- | --- |
| Reduce manual status checking | Explicit requests appear with stable identity, reason, freshness and a native-terminal return path where supported |
| Reduce evidence reconstruction | Review shows the selected run's recorded edits, check outcomes, subsequent edits, errors and coverage together |
| Preserve task decisions | A saved task carries intent, constraints, decisions, rejected approaches and next actions across explicitly linked sessions |
| Keep monitoring trustworthy | Unknown, partial, stale and unverified information remain visible; observations never become correctness claims |
| Keep the terminal usable | Stable selection, bounded background work, keyboard access and a usable 60-column layout |

This proposal does not add an execution orchestrator, automatic approvals, agent-to-agent messaging, a cloud team dashboard, semantic correctness scoring, automatic context injection, or provider quota enforcement. It does not expand automatic runtime discovery. AI-based decision extraction and a headless notification daemon are deferred; neither is required for the proposed daily workflow.

## 4 Current implementation and integration constraints

| Area | Current implementation | Proposed change |
| --- | --- | --- |
| Agent status | `session_status.py` separates lifecycle, process evidence, collector delay and possible stalls | Preserve classification; supply structured reasons rather than reinterpreting state in each screen |
| Attention | `ui_workflow.py` contains `workflow_reasons`, `AttentionLedger`, desktop delivery and a report | Separate pure rules, persisted user state and UI rendering; stabilize incident identity |
| Review state | `terminal_ui.py` and `ui_preferences.py` retain bounded reviewed/dismissed hashes | Move new exact-run acknowledgements into a dedicated local workflow store; retain legacy compatibility |
| Review evidence | `workflow_report` scans the loaded activity slice for files, checks and context events | Use selected-run evidence with explicit paging and coverage |
| Saved work | `saved_tasks.py`, `task_cli.py`, `ui_saved_tasks.py`, `task_workspace.py` | Add structured briefs and decisions while preserving notes, link deduplication and optimistic revisions |
| Evidence reads | `session_prompts.py`, `terminal.py`, `feed_requests.py`, existing activity API | Reuse exact scoping and generation guards; separate run evidence from prompt history |
| Findings | `findings.py` and stored derived findings | Reuse recorded errors; add bounded pattern observations later |

Two concrete issues require attention. First, `events_for` currently permits untagged same-session fallback even when an explicit agent identity exists; that can mix worker evidence. Second, the attention ledger uses the entire reason tuple in its key, so an additional threshold can create another notification identity for the same request. The proposed shared contracts address both.

The inspector/readability prerequisite described in [the experience review](../agents-experience-review.html) landed in PRs 50–51 before RFC implementation. The joint plan preserves its overview, prompt selection, hierarchy navigation, keyboard and exact-scope contracts.

## 5 Product workflow

The dominant loop remains **scan Today → inspect a request or completion → examine evidence → return to the terminal or save a handoff**.

| Action | Entry point | Result |
| --- | --- | --- |
| Scan work | Existing Agents Today view | Task, source, state and latest recorded work remain visible |
| Inspect attention | Existing `w` workflow entry and command palette | Actionable queue with reasons and observed age |
| Review completion | Select a reported completion and open its review | Exactly scoped evidence; completion remains agent-reported |
| Return to execution | Existing Terminal action | Select a uniquely verified tmux pane, or explain why unavailable |
| Acknowledge | Existing `r`, with state-specific wording | Mark a completion reviewed, or mark an attention incident seen; never resolve a native approval |
| Save continuity | Existing `b` saved-task entry | Create or update a task, with explicit run association |
| Continue elsewhere | Handoff preview, copy or export | A reviewed brief with source timestamps and coverage |

Use existing keys and palette entries. Do not add global single-letter bindings until checking conflicts with prompt reading, text entry and existing views. Every new action has a visible button or menu path as well as keyboard access.

### Wide layout at 150 by 40

```text
Agents  Today   Running   History                  Collection updated 2s ago
Task and latest work                 State         Selected task
Fix refresh expiry                   Needs reply   Fix refresh expiry
  Claude Code · payments                           Reply requested · observed 4m
Add retry regression tests           Review         Latest agent update: ...
  Codex · payments                                 Evidence: 14:32–14:46 · partial
                                                   2 checks · 1 later edit
                                                   Open review / Return to terminal

Enter Inspect   w Attention   b Saved tasks   r Mark seen/reviewed   q Quit
```

The attention queue replaces the primary list while open; it does not become a third persistent panel. The expanded inspector retains Overview, Prompts and Details from the inspector work. Review is a focused drill-down from Overview, not another copy of the full transcript.

### Narrow layout at 60 by 20

```text
Agents / Today                          Collection fresh
Task and latest work                         State
Fix refresh expiry                     Needs reply
  Claude Code · payments · reply requested
Add retry regression tests                  Review
  Codex · payments · updated 2m ago

Enter Inspect   w Attention   b Saved tasks   q Quit

Review opens as a separate full-screen reading view:
Fix refresh expiry / Review                    Esc Back
Agent reported completion · recorded evidence partial
Checks: 1 passed, 1 result unknown
Later recorded edit: retry.py
Open checks / Open edits / Save task
```

At 120 columns and sufficient height, retain list plus preview. At 80×24 and 60-column splits, the list wins and Enter opens detail. Monitor and review support a 60×18 floor; structured editors use the saved-task floor of 60×20. Below the relevant floor, show the required size and retain unsaved data. Escape returns to the previous screen; resizing never discards edits.

**Clutter budget:** at most one enclosing content border; one text status and optional semantic color per state; no marker on every leaf row; no missing-cost placeholders in task rows; no repeated full IDs or stacked permanent action bars. At 60×20, list header, navigation and hints consume at most six rows, leaving at least fourteen rows for the list body, including its column heading. Validate actual rendered frames rather than relying on the sketch.

### Screen states

| State | Required behavior |
| --- | --- |
| Initial/loading | Show the frame immediately; name the requested scope; distinguish waiting for data from an empty queue |
| Empty | Explain that no actionable requests were observed, or that no evidence was captured for this run |
| Partial | Retain available evidence and name truncation, missing identity, metadata-only capture or failed pages |
| Error | Keep previous evidence; show retry for the failed operation |
| Stale/disconnected | Keep the last snapshot with its age; do not announce fresh agent failures from it |
| Saved | Confirm which task revision was saved; do not silently refresh a different selection |
| Conflict | Keep the user's edits and offer reload/copy; never overwrite a newer revision |

## 6 Shared identity and evidence contract

Introduce a pure evidence module, proposed as `observatory/workflow_evidence.py`, consumed by UI and CLI renderers. No Textual widgets, network requests or database writes belong in its projection functions.

| Record | Required fields and semantics |
| --- | --- |
| Execution scope | Receiver namespace, producer namespace, canonical agent identity when available, native session, exact trace/run when available; scope kind is `run`, `agent` or `legacy_session` |
| Evidence reference | Native/stored event or span ID, producer, timestamp, exact available scope, content availability and capture revision |
| Evidence window | Earliest/latest loaded timestamps, paging cursor, loaded count, scope revision, truncation reasons and coverage state |
| Check observation | Source reference, command, recognized check kind, recorded result, exit code when present, start/end times when recorded |
| Review projection | Reported outcome, edits, checks, unresolved recorded errors, context events, usage and the evidence window |

An explicit agent identity constrains every read and projection. Do not substitute another agent's untagged session records. With no exact agent, legacy session evidence is allowed only under the same producer and receiver namespace. If the scope remains ambiguous, show unavailable evidence rather than widening to a service. A selected parent and its subtree are different scopes; subtree inclusion is explicit and labeled.

Receiver namespaces must distinguish two receivers containing the same native IDs. Prefer a persisted opaque receiver identifier exposed additively through health metadata. Older receivers use an endpoint-scoped fallback labeled as such; do not reconcile acknowledgements between fallback and stable namespaces without exact evidence. Credentials never enter the namespace.

A durable run key uses a recorded run/trace identity or exact native lifecycle start reference. Do not derive it from a title, working directory, latest timestamp or cumulative usage. If no stable run anchor exists, persistent review is unavailable and the UI explains that limitation. Exact agent identity alone is not proof that two turns are the same run.

## 7 Evidence loading and review semantics

Reuse the activity endpoint and its existing scoped paging wherever possible. Add an exact trace filter to activity reads only if required by the current server contract; implement it additively in `server.py` and `core.py`. Do not filter a global recent slice in the client and call it complete run history.

Proposed selected-run loading defaults are pages of 200 events, at most 1,000 loaded events and a 2 MiB decoded-payload budget per selection. Fetch only when review opens; additional pages require explicit loading. The first page must show its own window as partial whenever earlier or omitted records may exist. Prompt history remains independently lazy and bounded. Cache at most four selections and 8 MiB total; evicted cached evidence can be reloaded.

Cache keys include receiver, execution scope, scope revision, capture revision and request generation. Deduplicate concurrent identical reads. On a selection change, cancel where supported and discard stale results regardless of cancellation. Existing feed concurrency remains capped at four workers across these reads; new review requests must share that budget. No SQLite, Git, HTTP or terminal subprocess work runs on the UI thread.

Check classification recognizes a bounded allowlist of common validation commands. An unrecognized shell command remains available in activity rather than being declared a test. A check has `passed`, `failed` or `unknown` based on recorded process/tool results. Assistant prose saying tests passed is a reported statement, not a passing check.

Compare a check's recorded completion with later recorded file operations within the same scope. Show “later recorded edits” or “freshness unknown” rather than “verified.” Missing timestamps, unfinished commands, incomplete history and concurrent workspace writers weaken freshness. Even a fully loaded captured interval does not establish that the runtime recorded every edit or that the current worktree matches it.

The first release preserves the existing preview limits for Git capture and does not execute validation commands. Review exports include their scope, capture time and coverage. Copy/export use the exact previewed projection; a refresh changes the preview explicitly before a different export is produced.

## 8 Attention state and notifications

Extract pure reason classification to proposed `observatory/attention.py`. Keep `session_status.py` authoritative for observed lifecycle and health. Model each incident separately from its presentation text.

| Incident kind | Identity anchor | Queue and alert behavior |
| --- | --- | --- |
| Approval or reply request | Exact request event, scoped to agent/run | Highest priority; notify once per observed request transition |
| Reported failure/interruption | Exact run outcome | Queue below explicit requests; a new run is a new incident |
| Possible stall | Run anchor and contiguous period of fresh observations meeting stall criteria | Advisory; suppress while the collector is delayed; reset when progress returns |
| Usage/time threshold | Run anchor plus threshold kind and policy revision | Advisory; once on crossing, rather than once per refresh |
| Reported completion | Exact run outcome | Review queue; no desktop alert by default |

Sort by priority, then earliest observed incident time, then stable ID. Preserve selected incident and viewport during refresh. An acknowledgement changes local user state only. Marking an approval incident seen may silence its alert, but the unresolved request remains visible until native evidence resolves it. A new request in the same run must surface again. Marking a completion reviewed remains reversible.

Retain separate `source_time` and `first_observed_time`. If native request time is absent, label age as observed wait; never reconstruct historical wait from file modification time.

Use a dedicated local `workflow.sqlite3` with versioned tables for acknowledgements, observed incidents and notification claims. Keep display settings in `ui.json`; do not add another growing list of event hashes there. Include namespace and stable scope in primary keys. Use owner-only file permissions and bounded transactions.

Initial inventory remains silent. On restart, unresolved requests reappear in the queue, but no backlog of desktop notifications is sent. After initialization, an exact new request may notify; missing intervals never imply a resolution followed by a new request. Persisted notification claims prevent two open cockpits from sending duplicates for the same incident. Prefer at-most-once attempted delivery for a claimed incident; record delivery failure visibly and offer an explicit retry. A crash between claim and delivery may miss an alert, while the queue still retains the request.

Desktop alerts remain opt-in, coalesced and snoozable. They contain project and generic reason, with commands and prompts omitted. No notification runs while capture is known stale. Preserve the current Linux/macOS delivery paths and report unsupported systems. Alerts still require an open cockpit in this RFC.

## 9 Structured task continuity

Keep a saved task independent from native sessions. Add a structured brief alongside existing freeform notes:

| Field | Meaning |
| --- | --- |
| Intent | User-owned statement of the desired outcome |
| Acceptance criteria | Observable requirements for calling the task complete |
| Constraints | Current boundaries and their scope |
| Decisions | Accepted approaches with rationale and source references |
| Rejected approaches | Options tried or ruled out, including why |
| Open questions | Explicit unresolved decisions |
| Next action | A concrete starting point for the next session |

A decision has an ID, kind, text, rationale, scope, creation time, author kind, optional source reference and optional `supersedes_id`. Only decisions in the same saved task may supersede one another. Reject cycles. Record a changed decision as a new version with a supersession link; do not silently rewrite history. The active handoff includes current decisions and references superseded history on request.

Use additive tables in `tasks.sqlite3`, proposed as `task_briefs_v1` and `task_decisions_v1`, leaving the columns of `saved_tasks_v1` intact. Structured updates and task revision increments commit in one `BEGIN IMMEDIATE` transaction. Preserve `TaskConflict` and CLI exit code 3 for stale revisions. Freeform notes remain readable and editable; migration must not attempt to infer structured decisions from them.

Use a maximum of 128 decision versions per task and 4,000 characters per text/rationale field, with the existing 512 KiB aggregate task limit extended to cover structured fields. Keep the existing 32,000-character notes and 64-link limits. Refuse an over-budget edit with a recovery message; never silently evict user decisions. These proposed limits are adjustable after fixture and pilot review.

The UI adds focused Brief and Decisions screens inside Saved tasks, following its existing drill-down layout. Manual entry is the default. A user can explicitly copy a selected recorded excerpt into a draft decision; it remains unsaved until reviewed and saved. Agent statements do not automatically become accepted decisions.

Extend the existing task CLI with proposed commands such as `task brief show/update` and `task decision add/list/supersede`, supporting `--json`, `--expected-revision` and bounded file input. Retain argparse and the current stdout/stderr and exit-code conventions. Exact subcommand syntax is finalized in the implementation PR; introduce no new CLI framework.

Handoffs lead with intent, current constraints and decisions, rejected approaches and next action, followed by linked-run evidence and workspace capture. Show missing or stale references explicitly. No automatic launch, prompt injection or cross-provider replay is added. Handoff content is useful task context; it is not a guarantee that the next agent will follow it.

## 10 Diagnostics and usage after the first release

Add deterministic observations only after shared scoping and review are stable. Reuse existing recorded-error findings before introducing new rule kinds.

| Pattern | Proposed initial rule | Qualification |
| --- | --- | --- |
| Repeated tool failures | Three failures with the same scoped tool and sanitized error fingerprint in five minutes | A retry may be intentional; show all linked evidence |
| Repeated compaction | Three recorded compaction events within ten minutes in the same run | Do not infer that context was lost or that no useful work occurred |
| Repeated equivalent calls | Five consecutive calls with the same canonical tool/argument fingerprint | Exclude known streaming/polling patterns; keep in Findings during the pilot |
| Completion with recorded unresolved errors | Reported completion and an error without a later matching successful operation in the loaded window | Label recovery as unobserved, not disproven |

Do not add a generic “agent is wrong” detector. Canonicalize bounded arguments without storing additional raw payloads; persist safe fingerprints and source references. Rules operate incrementally on at most 500 recent scoped events per run and a bounded active-run set. Mark analysis partial when the budget excludes data. New heuristic findings do not trigger desktop alerts by default.

Continue existing exact-run usage totals and saved-task link deduplication. Separate task-linked runs, session totals, current context and provider quota. Publish context occupancy only with an authoritative numerator, denominator, source time and adapter capability; otherwise show unavailable. Reported USD stays distinct from subscription billing. Do not sum parent and child usage unless adapter semantics establish non-overlapping accounting; default to separate values.

## 11 Storage compatibility and recovery

| Data | Retention and recovery |
| --- | --- |
| User brief and decisions | User-owned; retained with saved tasks independently of telemetry retention; archive does not delete |
| Review acknowledgements | Retain 90 days by default; expose cleanup; expiry may make an old run unreviewed again and must be documented |
| Incident and notification records | Retain resolved unacknowledged records and claims for 30 days; retain minimal execution references supporting a valid acknowledgement for 90 days; preserve open incidents; cap stored incidents at 10,000 with a visible capacity warning |
| Cached review projection | Memory only, bounded; reload from retained evidence |
| Evidence references | Survive as identifiers even if source telemetry is removed; show source unavailable |

At workflow-store capacity, retain existing unresolved records and stop persisting new incidents rather than dropping them silently. Show new incidents in memory with a notice that acknowledgement and cross-cockpit notification deduplication are unavailable. Suppress desktop delivery for those unpersisted incidents.

Migrations are idempotent and transactional, with a local schema version and explicit unsupported-version errors in new builds. Test opening an existing task database, interrupted initialization and a read-only state directory. A workflow-store failure must not prevent basic monitoring; show its impact on acknowledgements and delivery.

Legacy reviewed/dismissed hashes remain recognized only through their existing exact matching rules; do not expand them into guessed run mappings. New records use the new namespace contract. Leave the legacy preference keys readable during transition.

Older binaries will ignore the new task side tables and cannot display structured decisions. Retaining the original columns avoids breaking their positional inserts. Structured data must survive an older notes-only update, with the new client exposing the changed task revision. Mixed-version editing is not recommended and must be documented. Before migration, provide an explicit backup procedure using SQLite's backup API; restoration requires stopped writers. Do not promise that a schema marker can prevent an already released binary from writing.

## 12 Privacy and operational behavior

No new cloud service or model call is required. Metadata-only capture cannot be bypassed by a review fetch, excerpt action or handoff. New exports preserve explicit previews, escaped HTML, owner-only files and existing no-overwrite behavior. User notes and Git snapshots can contain sensitive information; they are not implicitly redacted by a structured schema.

Use bounded background workers for storage, exports, Git capture and notifications. Stop scheduling work on shutdown, bound subprocess duration and discard results after the owning screen closes. Preserve framework-managed terminal cleanup and existing suspend/interrupt behavior. No interactive subprocess is introduced by this RFC.

## 13 Implementation plan

Each change below should be independently reviewable and keep existing workflows working. Estimates are planning ranges for one engineer familiar with this repository, excluding participant recruitment, review queues and unexpected adapter changes.

| Step | Work and principal files | Dependency | Acceptance gate | Effort |
| --- | --- | --- | --- | --- |
| 0 | Reconcile inspector work; record current rendered frames and evidence behavior. `terminal_ui.py`, `session_prompts.py`, existing experience review | None | Identify landed versus outstanding scope fixes; agree on shared loader ownership; preserve unrelated edits | 1–2 days |
| 1 | Introduce pure execution scope and evidence projection. New `workflow_evidence.py`; reuse `agent_hierarchy.py` and current readers | 0 | Explicit workers cannot receive sibling or untagged evidence; legacy scope remains labeled | 2–3 days |
| 2 | Add lazy selected-run loader and additive trace filtering if needed. `terminal.py`, `feed_requests.py`, `server.py`, `core.py` | 1 | Paging and byte limits; bounded cache; selection-race and older-receiver behavior verified | 2–4 days |
| 3 | Extract attention rules and incident transitions. New `attention.py`; `session_status.py`, `ui_workflow.py` | 1 | Stable requests survive refresh; threshold changes do not duplicate request alerts; stale capture does not imply a stall | 2–3 days |
| 4 | Persist acknowledgements and notification claims. New `workflow_store.py`; `ui_preferences.py`, notification integration | 3 | Restart remains silent; two cockpits claim one notification; failures and capacity are visible | 2–4 days |
| 5 | Integrate queue and completion review. `ui_workflow.py`, `terminal_ui.py`, `ui_export.py` | 2, 4 | Review matches selected run and export preview; keyboard and narrow layouts pass | 3–4 days |
| 6 | Add structured brief and decision storage plus CLI. `saved_tasks.py`, `task_cli.py` | 1 | Transactional revisions, supersession, limits, old-database compatibility and JSON behavior pass | 3–4 days |
| 7 | Add Brief/Decisions UI and handoff content. `ui_saved_tasks.py`, handoff rendering, `task_workspace.py` where needed | 2, 6 | Offline editing, conflict recovery, source expiry and copy/export parity pass | 3–4 days |
| 8 | Add bounded repeated-work findings and scoped usage presentation. `findings.py`, `storage_derived.py`, `ui_monitoring.py` | 2, 5 | Deterministic patterns, false-positive fixtures, no cross-agent totals; heuristics opt-in for review | 3–5 days |
| 9 | Document, benchmark and run a pilot; update `usage.md`, `saved-tasks.md`, `privacy.md`, `performance.md` | 5, 7; 8 optional | Release checklist, observed workflow benefit and remaining limitations recorded | 2–3 engineering days plus pilot |

The full proposal is approximately **23–36 engineering days** before external waiting time. Milestone 1 consists of steps 0–5 and can ship without structured decisions or heuristic diagnostics. Milestone 2 adds steps 6–7. Milestone 3 adds step 8 only after initial pilot feedback. Step 9 runs at each milestone rather than delaying all feedback until the end.

## 14 Verification and release gates

| Area | Required cases | Existing test homes to extend |
| --- | --- | --- |
| Identity | Workers sharing sessions; same native IDs on different receivers; unknown identity; parent versus subtree | `test_agent_hierarchy_api.py`, `test_session_view.py`, new evidence projection tests |
| Attention | New request, seen-but-unresolved request, new request in same run, progress after stall, delayed collector, threshold change | `test_workflow.py`, `test_session_status.py`, `test_today_outcomes.py` |
| Persistence | Restart, competing cockpits, crash after notification claim, failed delivery, storage full/read-only, legacy preferences | New workflow-store tests; `test_ui_review_polish.py` where relevant |
| Review | Passing command then later edit; missing result; assistant claim only; partial paging; expired telemetry; scope change during read | `test_terminal_review.py`, `test_feed_concurrency.py`, `test_export_coverage.py` |
| Decisions | Supersession, cycles, stale revision, bounds, old task database, older notes-only writer, unlinked/expired sources | `test_saved_tasks.py`, `test_task_cli.py`, `test_saved_tasks_ui.py` |
| Rendering | 150×40, 120×40, 80×24, 60×20 and relevant floor; CJK/combining text, ASCII, monochrome, disconnected state | Existing headless Textual tests and pinned render fixtures |
| Resource behavior | Idle unchanged database, rapid selection, large captured output, bounded request counts and shutdown | `test_resource_usage.py`, `test_shutdown.py`, benchmark scripts |

Test business transitions and scope rules before UI integration. Use Textual's existing headless harness for focus, keys, scroll and rendered layout. Add at most a small PTY smoke path for changed terminal behavior; do not build a parallel testing framework. No paid models or private transcripts belong in regression fixtures.

Run focused tests per change, then the repository suite using `uv run python -m unittest discover -s tests -v` for each releasable milestone. Compare the existing small, large and payload benchmarks on the same machine. Proposed regression gates are no more than 10% increase in warm median refresh latency or 10%/15 MiB increase in steady RSS, whichever memory allowance is larger. These are starting review thresholds, not published performance guarantees. Investigate any sustained idle-work increase; functional bounds on requests and retained payloads are mandatory even when timings are noisy.

Before release, verify installation from the built package, existing task commands, metadata-only behavior, missing notification helpers, an older receiver and a fresh state directory. Render documentation HTML in light/dark mode at desktop and mobile widths. Update examples to distinguish reported completion from observed check results.

## 15 Rollout and success measures

Ship reliable evidence loading and manual attention first. Preserve existing preferences and leave desktop notifications opt-in. Add structured tasks next; keep legacy notes fully usable. Introduce heuristic findings behind an explicit setting and keep them out of desktop alerts during the first pilot. Disable new heuristic kinds without removing raw evidence or saved decisions if false positives are high.

Recruit 10–15 engineers, including single-session users as a comparison and users of multiple agent brands. This is a proposed pilot, not completed validation. Gather baseline workflows before showing the changes. Use consent-based observation or participant records; no automatic product telemetry is introduced.

| Measure | Definition | Proposed success signal |
| --- | --- | --- |
| Approval response delay | Native request time to native response time when both are available; otherwise observed wait reported separately | Lower paired median for users with repeated request events |
| Status checking effort | Participant-counted terminal checks during comparable work periods | Fewer checks without more missed requests |
| Review preparation | Time to locate relevant edits, checks and failures before substantive review | Lower paired time; no increase in mistaken verification claims |
| Continuation effort | Time and corrections needed to resume a saved task in another session | Less re-explanation and fewer repeated rejected approaches |
| Alert quality | Participant-rated actionable alerts divided by delivered alerts; missed known requests counted separately | At least 90% actionable explicit-request alerts in the pilot; investigate every known miss |
| Repeat use | Participants using attention/review and saved-task handoffs in a second week | Repeated voluntary use rather than screenshot approval |

Record denominators and individual results; a small self-selected pilot cannot establish market-wide savings. Useful evidence includes a participant retiring a maintained script or regularly choosing Agenturm to reconstruct work. Willingness to pay remains a separate discovery question.

## 16 Alternatives and tradeoffs

| Alternative | Benefit | Reason for the proposed choice |
| --- | --- | --- |
| Keep improving each screen independently | Small local changes | Divergent scope and review semantics would remain; shared pure projections reduce this risk |
| Build an orchestrator with approval forwarding | More control in one interface | Much larger permission, execution and recovery surface than the observed workflow requires |
| Add automatic memory extraction first | Less manual note-taking | Risks promoting stale or incorrect statements into durable decisions; explicit user-owned decisions are easier to validate |
| Focus on usage reports | Concrete and measurable | Established alternatives already serve reporting; task evidence and continuity provide a stronger combined proposition |
| Store all workflow state in telemetry | One database | Telemetry retention and remote receivers should not own local decisions and acknowledgements |
| Start with a notification daemon | Alerts without an open TUI | Requires another lifecycle and migration surface; validate attention quality before making delivery persistent |

## 17 Questions for reviewers

These are design choices for RFC review, not blockers to preparing the plan.

1. **Should an acknowledged request disappear?** Recommended: keep unresolved approvals/replies visible with a seen marker; acknowledgement only silences the local alert.
2. **Should decisions be extracted automatically?** Recommended: manual entry and explicit excerpt capture in the first release; extraction may later propose drafts.
3. **Should Agenturm launch a destination agent?** Recommended: retain preview/copy/export; launch and injection require a separate RFC.
4. **Should review include descendants by default?** Recommended: exact selected agent/run; provide explicit subtree review with independently labeled evidence and usage.
5. **Should heuristics notify?** Recommended: Findings only until measured precision justifies opt-in alerts for specific rule kinds.
6. **What happens without stable run identity?** Recommended: inspect labeled session evidence, but disable persistent exact-run review rather than invent an anchor.
7. **Are the proposed bounds and retention suitable?** Recommended: begin with the stated limits, expose coverage/capacity honestly, and adjust using synthetic workloads and pilot evidence.

## 18 Decision and follow-up

Implementation was authorized by the user on 11 November 2026. Engineering implementation and peer review are complete. The [joint plan](joint-implementation-plan.html) records reconciliation, corrected review findings, actual validation and remaining limits. The proposed participant pilot has not been conducted; its outcome measures remain hypotheses until tested with real participants.
