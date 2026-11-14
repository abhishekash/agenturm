# Agenturm product-owner review: useful work, not more monitoring

14 November 2026 · Reviewed source: `a601ad2236f22ef75600129e920ff763883bee7c` · Recommendations, not implemented changes.

## Product decision

**Agenturm has a credible primary-user job: help an individual engineer direct their attention across concurrent local coding agents.** Completion review strengthens that job; saved tasks help when the work outlives a session. The product should connect these capabilities into one recognizable loop:

**Which agent needs me → why → return to the correct session → review its result → carry the task forward if necessary.**

My previous review correctly identified completion evidence as useful, but made it the whole positioning. That is too narrow. The “passing test followed by an edit” case is a good demonstration of trust, not sufficient evidence of the most frequent reason to open the app. The repository’s existing qualitative research already makes the stronger distinction: attention is the entry point, review supports the decision, continuity preserves the work. This is a product hypothesis supported by implementation, not measured adoption.

**Recommended promise:** “Know which agent needs you, why, and where to return.” Supporting promise: “Review what happened and carry the task forward.”

## Who the primary user actually is

The primary user is **the engineer personally responsible for several concurrent agent tasks and the resulting code**. They already have agent terminals, review diffs, and decide what to integrate. Their problem is fragmented attention and expensive context reconstruction—not a lack of charts.

| User | Fit | Why |
| --- | --- | --- |
| Engineer repeatedly using multiple local coding sessions, especially across runtimes | Primary | One place to find requests, inspect evidence, and recover task context |
| Same engineer resuming work after interruption or switching agents | Strong secondary moment | Human-authored decisions and linked runs survive a session |
| Developer debugging their own MCP/OTLP agent application | Secondary segment | Tool arguments/results and trace correlation support investigation; setup and needs differ |
| Single short-lived agent session with a sufficient native interface | Weak initial fit | Another app may add more overhead than it removes |
| Team manager needing shared assignment, permissions, approvals, and organization-wide reporting | Not the present product | Local acknowledgements and task notes are not a team control plane |

I would recruit ordinary multi-session engineers first, with a separate power-user cohort. “20–50 agents” is a stress case to validate, not a required definition of the customer or a proven capacity claim. The number of concurrent decisions matters more than a large process count.

## How it becomes useful in a real working day

Consider one illustrative task: a retry fix delegated while other agents handle unrelated work.

1. **A recorded input request arrives.** The attention inbox distinguishes that request from ordinary tool waits and completion. You can prioritize responding instead of repeatedly scanning terminals.
2. **You recover enough context to answer.** Inspect the recorded request and relevant evidence. Where the exact process can be mapped, the existing Linux/tmux return action can select its pane. Otherwise use the disclosed fallback and your native terminal. Agenturm must not pretend it can route every runtime or terminal.
3. **The agent advances.** Fresh positive evidence resolves the pending request; merely marking it Seen does not approve it or claim resolution.
4. **The agent reports completion.** Review its recorded checks, edits, failures, and capture gaps before deciding what to inspect or rerun. A sibling’s success is not this execution’s validation. Your diff review and CI remain authoritative for integration.
5. **Only if the task is interrupted, preserve it.** Save the constraint “do not change the public API,” a rejected approach and rationale, the next action, and linked execution evidence. Preview the handoff before pasting it into another agent. This is explicit context maintenance, not automatic memory restoration.

The potential benefit is **less time choosing a terminal and reconstructing a task before acting**. It is not “more agents running,” guaranteed faster shipping, authoritative provider quota, or independent correctness verification.

## The code contains the right foundations

- **Trustworthy state distinctions:** `session_status.py` distinguishes explicit requests, quiet work, process progress, collector delay, and uncertain outcomes. False “stuck” claims would destroy trust in an attention product.
- **Incident identity, not just badges:** `attention.py` and `ui_workflow_controller.py` track exact requests/outcomes, retain unresolved incidents, suppress initial-inventory alert storms, and keep acknowledgements distinct from native resolution.
- **Scoped evidence:** `workflow_evidence.py` isolates receiver/producer/agent/session/run or turn, separates assistant claims from recorded checks, and discloses partial capture. That makes the inbox more useful than a generic notification bell.
- **Reading continuity:** stable IDs, deferred reordering, background evidence reads, and frozen review exports support reading while agents continue. These are product features, not merely implementation details.
- **Durable human intent:** saved tasks retain briefs, versioned decisions, linked runs, and bounded workspace captures separately from telemetry retention. Revision conflicts refuse to overwrite newer work.
- **Low adoption dependency:** supported local discovery and recorded-progress summaries do not require an Agenturm account or model call. Local capture still creates sensitive copies; best-effort redaction is not a confidentiality guarantee.

## Concrete product findings, in priority order

### 1. High: the primary attention path is less prominent than the inventory

**Observed:** a synthetic fixture with 44 running sessions, two waits, one failure, and three completions retained all 50 roster rows at 120×36, 80×24, and 60×18. The first wait was at zero-based position 44: row 45, below the initial viewport. Today deliberately orders running work before attention. Pressing `w` opened a six-item inbox with an input request first. At 80 and 60 columns, the footer omitted `w`, although the shortcut still worked.

This is not dropped data or a failed layout assertion. It is a discovery problem: **the app makes the overview easier to find than the decision queue**. Existing headers, help, and the palette mitigate it, but do not make the queue the obvious next step.

**Recommendation:** expose a persistent, directly activatable “Needs you” entry and retain its discoverability at compact sizes. Reuse the existing inbox; do not add another tab or silently change row order while someone is reading.

### 2. High: returning to the agent is treated like a setting

**Observed:** the existing Terminal and Resume buttons are children of the Workflow Settings panel. In all three fixture sizes, that parent was hidden until Settings was activated. The main inspector’s contextual actions expose Review, Activity, and Trace, not that return action.

**Recommendation:** put a contextual “Go to agent terminal” beside a captured input request when a target is verifiable. Show the supported resume-command fallback for stopped Codex/Claude sessions, and an honest unavailable state otherwise. Preserve the exact-process/PID-reuse protections. Do not guess by shared directory, send keystrokes, approve requests, or launch duplicate live sessions.

This recommendation promotes an existing capability; it does not mean terminal return is unimplemented or universally supported.

### 3. High: an always-on product must earn its resource and coverage claims

Native discovery has bounded recent-source windows: 50 JSONL sources across Codex/Claude/Pi, and separately 50 OpenCode sessions. Delegated and historical records can consume those windows. The app already exposes coverage warnings and diagnostics; that is good, but not a guarantee that 50 running agents will all be observed.

The repository’s retained-history report records 112.96 seconds cold readiness and 15.90% of one core at idle on its generated 35-day fixture. A separate older 20-minute soak did not establish RSS stability. These are previous named-revision measurements, not new measurements of this checkout.

**Recommendation:** treat readiness, idle CPU, sustained memory, event-to-display age, and source omissions as adoption gates before marketing a reliable 20–50-agent cockpit. Separate UI responsiveness from collector freshness. Do not use a 50-row presentation test or a 1,010-node hierarchy microbenchmark as a live-workload benchmark.

### 4. Medium: the end-to-end loop has avoidable seams

Today, Workflow, Review, Findings, and Saved tasks each have legitimate purposes, but the user must learn several nouns and acknowledgement meanings. Workflow’s Review action opens evidence; Reviewed/Seen records an acknowledgement; native approval happens elsewhere. The Review screen offers export/save actions, while acknowledgement is handled outside it.

**Recommendation:** preserve the existing shortcuts but make the primary reading path explicit: **request → native terminal**, **failure → recorded failure**, **completion → inspect evidence**. Clearly distinguish “Inspect evidence,” “Mark reviewed,” and “Seen; still pending.” Keep lower-level MCP/traces/findings as investigation depth, not the initial onboarding curriculum. A review acknowledgement is never a merge-readiness gate.

### 5. Medium: documentation is stronger on contracts than on first value

The README leads with execution attribution and completion evidence. The research document leads with attention. The older terminal UX spec still describes All/Attention/Working/Finished filters, while current code and usage describe Today/Running/History/Errors. The earlier review added another narrow narrative rather than reconciling them.

**Recommendation:** adopt one current product contract, then give users a short first-run path: start with existing agents, find a real request or completion, inspect it, return to the native terminal, and observe the outcome. Keep careful evidence/privacy contracts accessible behind that path. Keep main/Unreleased receipts distinct from the published 0.2.3 experience. This review does not change those files.

### 6. Medium: saved tasks must not become compulsory bookkeeping

Structured decisions and handoffs are implemented, including offline editing and conflict protection. Their value depends on people explicitly recording intent and rationale. Maintaining a full task record for every small run could erase the benefit.

**Recommendation:** position saving as an interruption/restart/handoff action, not a prerequisite for monitoring. Offer a minimal brief—constraint, decision, next action—with source links and editable drafts. Preserve manual consent; do not infer accepted decisions or silently inject context. Validate whether this actually replaces a notes file rather than creating a second task manager.

## What I would prioritize next

| Order | Product outcome | Scope |
| --- | --- | --- |
| P0 | Find and act on the right request | Promote the existing inbox and verified terminal-return action; make compact discovery clear |
| P0 | Trust the monitor enough to leave it open | Measure and resolve readiness, idle-resource, capture-coverage, and source-freshness limits |
| P1 | Finish one investigation without losing context | Tighten request/failure/completion actions and return paths; retain exact evidence boundaries |
| P1 | Resume an interrupted task with less repetition | Trial lightweight saved briefs and the existing reviewed handoff |
| Defer | More capabilities without a demonstrated primary-user benefit | Renderer rewrite, more top-level dashboards, auto-approval, autonomous coordination, invented quota/cost estimates, and team-SaaS expansion |

These are proposed priorities, not authorization or an implementation plan. More features are not the missing proof of usefulness.

## How to test whether primary users benefit

Run a small consent-based pilot with multi-session engineers, including different runtimes and a separately reported 20–50-agent cohort. Compare the same jobs with their existing notifications/tmux/notes setup and with Agenturm. Counterbalance the order where practical; do not compare unlike tasks and call the difference a productivity gain.

Measure:

- Time from a recorded input request to a native response/advancement, separating collector detection delay from human delay. If the source lacks a timestamp or response signal, label it unavailable rather than fabricating a duration.
- Time to identify the correct execution and explain one appropriate next action, plus wrong-worker and false-verification conclusions.
- Repeated manual terminal checks, irrelevant alerts, missed waits, and undiscovered sources. A quiet inbox is not proof of zero missed events.
- Time to resume an interrupted task and whether constraints/rejected approaches are actually preserved.
- Voluntary repeat use and which existing script or notes workflow the user can stop maintaining. An attractive screenshot or a reported token total is not this evidence.

**Proposed usability gates, not measured outcomes:** an unfamiliar user finds the actionable queue within five seconds at 80×24; reaches a recorded request and its verified terminal route within three activations excluding list navigation; does not mistake Seen for approval or a completion for validation. Test source omissions and unsupported terminal routing, not just the happy path. Keep pilot records local or explicitly reviewed before sharing.

## Validation performed for this review

- **47 tests passed in 58.866 seconds** across attention, Workflow, Today outcomes, and task continuity. They included headless keyboard/resize journeys and mocked exact tmux routing. SQLite `ResourceWarning`s and asyncio slow-task diagnostics appeared; no assertion failed. Those warnings do not establish a product memory leak.
- The separate synthetic 50-session probe checked roster retention, inbox ordering, compact shortcut hints, and Settings visibility at **120×36, 80×24, and 60×18**. All 50 rows remained available; the inbox held six relevant incidents. It did not ingest 50 live agents or run a performance test.
- The prior review’s **38 passing tests** covered status and execution receipts, including an authenticated loopback receiver. These are separate focused batches, not a full-suite result.
- No participant research, real provider calls, actual notification delivery, live terminal switching, production load benchmark, clipboard validation, or every-emulator visual inspection was performed. No product code, shared worktree, running agent, or remote branch was changed.

Commands for the additional regression batch:

```sh
uv run --frozen python -m unittest tests.test_attention tests.test_workflow tests.test_today_outcomes tests.test_task_continuity -v
```

## Source map

Paths refer to the reviewed commit; older research/specification text is not treated as proof of current behavior.

- [Earlier review](2026-11-14-developer-value.md)
- [Existing product research](../research/2026-11-11-terminal-agent-research.md): audience, attention/review/continuity distinction, alternative workarounds, and evidence limitations. External research sources were not independently revalidated in this review.
- [README](../../README.md), [usage](../usage.md), [older UX spec](../terminal-ux-spec.md), [saved tasks](../saved-tasks.md).
- `observatory/terminal_ui.py:1005–1056,1765–1803,1862–1907`: running-first ordering, compact hints, inspector actions, palette.
- `observatory/ui_workflow.py:62–94,167–192,324–352`: terminal/resume capabilities and their Settings placement.
- `observatory/attention.py` and `observatory/ui_workflow_controller.py`: incident identity, resolution, acknowledgement, and opt-in alert delivery.
- `observatory/workflow_evidence.py`, `observatory/ui_review.py`, `observatory/ui_saved_tasks.py`: execution evidence, frozen review, explicit continuity.
- [Retained-history measurements](../validation/history-read-economics.md), [soak qualifications](../rigorous-validation.md), [privacy](../privacy.md).

**Bottom line:** useful foundations, an actionable primary-user problem, and a plausible daily workflow. The next product win is making that workflow obvious and cheap—not adding another kind of telemetry.
