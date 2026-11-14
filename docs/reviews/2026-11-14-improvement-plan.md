# Make Agenturm better by closing the action loop

## Product goal

**Make Agenturm the quickest reliable way to find which agent needs you, understand why, and return to the right place.** Beat the user's existing combination of terminal scanning, native notifications, and notes—not an abstract competitor feature list.

This is a proposed improvement plan based on the reviewed `a601ad2` snapshot. It does not authorize implementation or claim the proposed usability and performance targets have been met.

## 1. First delivery: make attention actionable

This is the smallest coherent product improvement and should come before more integrations.

- Promote the existing `w` inbox through a visible **Needs you** control. Keep it discoverable at 80×24 and 60×18. Do not add a duplicate queue or force a new top-level navigation model.
- Keep **requests**, **failures**, and **completion reviews** distinct. A completion can wait; an explicit input request is blocking work. Do not alert for every ordinary tool wait.
- Show the selected task, project, reason, recorded age, source freshness, and captured request excerpt. If an excerpt or timestamp is unavailable, say so.
- Move verified terminal return out of Settings and into the selected request's actions. Keep Settings for notification and threshold configuration.
- Enable the return action only when an exact target is verifiable. Otherwise explain the limitation and offer an appropriate supported fallback. A stopped Codex/Claude session may offer a copied resume command; a live session must not be duplicated.
- Preserve selection and list order during live updates. A new incident must not move a different terminal under the user's cursor.

**Proposed acceptance:** an unfamiliar user finds Needs you within five seconds at 80×24; from a selected incident, inspecting the request and invoking an available verified terminal route takes no more than three activations. Unsupported routing must be obvious, not a silent no-op. Seen remains different from approved/resolved.

## 2. Second delivery: make completion review decision-ready

Keep the current scoped evidence engine. Improve its presentation rather than adding another dashboard.

Lead with four answers:

1. What did this execution attempt?
2. Which recognized checks and results were captured?
3. Were modifying operations recorded after those checks?
4. What should I inspect or rerun next, and what evidence is missing?

Put source references and raw evidence behind this summary. Preserve separate wording for an agent claim, a recorded exit code, and an unknown result. Make the return to the same inbox selection clear, with an explicit review acknowledgement path. An acknowledgement never certifies code correctness.

**Proposed acceptance:** a user can distinguish a sibling's result, a passing check followed by an edit, and partial capture without reconstructing a transcript. Never display a green “verified” badge merely because no later edit was captured. Metadata-only capture cannot provide content-based review.

## 3. Release gate: earn the always-on promise

Run in parallel with those UX improvements; do not defer it until after a 50-agent marketing claim.

- Benchmark the complete receiver + collectors + terminal under sustained 20- and 50-agent workloads with retained history. Include request transitions, delegated workers, and errors—not just idle rows.
- Measure interactive readiness separately from completed indexing, keypress latency separately from event-to-display age, and whole-monitor CPU separately from Store-only CPU.
- Test beyond the recent-source windows. Make omissions, backlog, and stale capture visible. Do not fix coverage by blindly increasing every limit.
- Profile retained allocations and perform a multi-hour fixed-load memory run. A short functional pass is not a stability result.
- Retain loopback authentication, private storage, capture-policy enforcement, and explicit export consent.

**Candidate gates:** existing repository targets of cold readiness below 30 seconds and settled idle CPU below 5% of one core on a named reference fixture; no unexplained sustained RSS growth under fixed retained load; declared collection coverage and freshness under the nominated active workload. These are targets to validate, not current guarantees. Publish raw results and failed runs as well as improvements.

## 4. After the core loop works: lightweight continuation

Keep saved tasks optional. Offer a small reviewed handoff at an interruption or runtime switch:

- Constraint to preserve.
- Decision and rationale worth keeping.
- Next action.
- Exact linked run and relevant evidence.

Reuse the existing brief, decision, and export mechanisms. Prefill editable source-backed drafts where appropriate; do not infer accepted decisions or silently send context to another agent. The goal is to replace duplicated notes, not introduce obligatory task administration.

## Product hierarchy

| Layer | User-facing job |
| --- | --- |
| Entry | Needs you: choose the next action |
| Selected item | Understand the reason and return to the native agent |
| Completion | Inspect evidence before integration |
| Investigation depth | Activity, MCP, traces, findings |
| Optional continuity | Save a brief and prepare a handoff |

Keep existing keyboard conventions; make the useful path easier to discover before expanding the keymap. Explain collection and privacy boundaries during first use without requiring users to study the integration guide to get their first useful view.

## What not to build next

No renderer rewrite without a demonstrated blocker. No new top-level telemetry tabs. No automatic approval, guessed terminal targets, authoritative quota claims from token totals, or general-purpose swarm orchestration. Do not require AI summaries to understand the default view.

## How we know it is better

Compare against the same users' current terminal/notification/notes workflow. Measure request-to-native-response delay, manual status checks, time to find the correct execution and next action, wrong-worker conclusions, false alerts, missed waits, and voluntary repeat use. Separate collection delay from human delay; missing timestamps remain unavailable. Validate interruption recovery separately.

**Delivery order:** actionable inbox and terminal return first; concise completion review second; performance/coverage as release gates throughout; lightweight continuation after observed demand. Update README, walkthrough, help, and release notes to describe the same shipped workflow. Keep unreleased functionality clearly labeled.

The deciding question is: **Does Agenturm remove attention and reconstruction work, or become another thing the engineer must monitor?**

Related evidence: [product-owner review](2026-11-14-product-owner-review.html) · [earlier developer-value review](2026-11-14-developer-value.html).
