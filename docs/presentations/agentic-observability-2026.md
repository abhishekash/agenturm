# Agentic AI observability — the five hardest problems

Research and product strategy presentation · 27 November 2026

Open `agentic-observability-2026.html` for the presentation. Arrow keys navigate; O toggles the overview; N shows speaker notes. The HTML is self-contained and supports light/dark themes and printing. The Markdown below is the editable slide source.

## 01 | From traces to trustworthy work
<!-- layout: hero -->

Agentic AI observability

**The five hardest problems — and where Agenturm fits.**

A current-state review for product and engineering leaders.

27 November 2026 · Market research + source-level product assessment

> The opportunity: help engineers decide what needs attention, what actually happened, and what evidence supports “done.”

<!-- notes
This is a strategic synthesis, not a market-share report or a measured vendor benchmark. Agenturm is assessed at upstream main commit 6803e2ad8ffccb12c925cdb8e25f4ab34fee8215. The latest published release observed was rust-v0.3.15; source-level capabilities are not a certification of every packaged release or platform.
-->

## 02 | The category has moved beyond trace collection
<!-- layout: cards -->

**Tracing is increasingly available. Trustworthy operational decisions remain difficult.**

### Capture is established
Framework instrumentation, model/tool spans, sessions, token usage and trace viewers are widely offered.

### Interpretation is advancing
Platforms now combine traces with evaluations, feedback, production monitors and automated issue discovery.

### Evidence still has gaps
Missing context, uncertain outcomes, incomplete permissions history and expensive capture limit what any dashboard can conclude.

> Agenturm’s best-supported position: a local, cross-runtime evidence cockpit for engineers supervising coding agents.

Sources: [S1][s1] · [S2][s2] · [S3][s3] · [S4][s4] · [S5][s5] · [S6][s6] · [R1][r1]

<!-- notes
“Established” describes documented availability, not universal adoption or interoperability. LangSmith documents online evaluation and Engine issue detection; Datadog documents Insights with recurring cost and reliability findings. It would be inaccurate to portray the market as only passive trace storage. The opportunity for Agenturm is the particular operator workflow and evidence semantics.
-->

## 03 | Four overlapping layers define the market
<!-- layout: table -->

| Layer | Representative offerings | What buyers get |
| --- | --- | --- |
| Instrumentation and conventions | OpenTelemetry GenAI; OpenAI Agents SDK | Structured events, spans, context and export hooks |
| AI engineering workbenches | LangSmith; Langfuse; Arize AX / Phoenix | Trace inspection, evaluations, datasets, feedback and experiments |
| Production operations | Datadog Agent Observability; managed AI platforms | Fleet-level monitoring, cost trends, alerts and issue investigation |
| Local coding-agent supervision | Agenturm; native agent interfaces | Session attention, recorded work, run review and task continuity |

**These are overlapping jobs, not exclusive product categories.** Phoenix can run locally; Langfuse can be self-hosted; LangSmith offers cloud, hybrid and self-hosted deployment.

Sources: [S1][s1] · [S3][s3] · [S4][s4] · [S5][s5] · [S6][s6] · [S7][s7] · [R1][r1]

<!-- notes
The sample is deliberately representative rather than exhaustive. OpenTelemetry is an instrumentation standard, not a complete monitoring product. OpenAI Agents SDK tracing supports custom exporters and non-OpenAI model integrations. Local deployment and open source alone do not make Agenturm unique.
-->

## 04 | The top five problems
<!-- layout: ranking -->

| Priority | Problem | The decision it blocks |
| --- | --- | --- |
| 01 | Fragmented execution and ownership | Which agent, tool or handoff caused this? |
| 02 | Success without trustworthy verification | Did the task succeed, or merely finish? |
| 03 | Activity without actionable supervision | Should I wait, investigate or respond? |
| 04 | Spend without outcome economics | What did a successful task really cost? |
| 05 | Evidence without safe, accountable capture | What may we retain, share and prove? |

**Ranking method:** prioritize dependency on reliable evidence, consequence of a wrong decision, and relevance across agent workflows. This ordering is editorial synthesis, not measured industry prevalence.

Sources: [S1][s1] · [S2][s2] · [S3][s3] · [S8][s8]

<!-- notes
Problem 1 supplies the evidence foundation for the others. Problem 2 connects observations to value. Problem 3 closes the human response loop. Problems 4 and 5 constrain whether that loop is affordable and acceptable. In regulated deployments, governance may reasonably rank first; for a high-volume agent service, cost may move higher.
-->

## 05 | 01 — Reconstruct the execution, not just the calls
<!-- layout: problem -->

**An agent task can cross turns, workers, tools, processes and memory stores. Those boundaries lose context.**

### Why this stays hard
Trace IDs, session IDs and worker IDs serve different purposes. A nested span does not by itself establish delegation. A missing event may mean missing capture.

### Agenturm today · direct local fit
Native readers and OTLP bring records together. Reviews retain source, session, turn/trace and worker scope. Discovery explicitly reports partial scans and queued sources.

### Remaining gap
Source-dependent coverage is not a complete distributed causal graph. Enterprise identity, memory-change provenance and every external side effect need additional producer evidence.

> Measure: correct owner identification, reconstruction time, and the share of runs with disclosed capture gaps.

Sources: [S1][s1] · [S2][s2] · [S3][s3] · [R2][r2] · [R3][r3] · [R3T][r3t]

<!-- notes
Current OTel agent conventions are still Development and now include a plan span. Do not repeat older claims that planning has no representation. The remaining issue is whether producers emit compatible and sufficient records. PAI’s October study assessed four frameworks, not the whole market, and distinguishes execution identifiers from verifiable enterprise identity. Agenturm's exact local identities help attribution but do not establish IAM authorization.
-->

## 06 | 02 — “Completed” does not mean “correct”
<!-- layout: problem -->

**A successful API call, an agent’s completion message and a verified task outcome are different facts.**

### Why this stays hard
Correctness depends on the final artifact or downstream state. Judges and test suites can be wrong; retries can hide inconsistent performance.

### Agenturm today · partial solution
Review separates recorded checks, edits, errors and agent updates. Existing evaluation-result imports attach pass/fail and scores to retained traces, with result history.

### Remaining gap
Agenturm does not independently rerun checks or certify current files. Imported evaluations are external judgments, not an in-product evaluation runner or proof of grader validity.

> Measure: false acceptance of “done,” artifact-linked verification coverage, and consistency across repeated tasks.

Sources: [S3][s3] · [S8][s8] · [R1][r1] · [R3][r3] · [R4][r4] · [R4T][r4t]

<!-- notes
Hugging Face’s November 23 guide explicitly recommends grading produced artifacts rather than completion claims and testing the verifier with no-op and shortcut solutions. Agenturm's Rust receipt is metadata-oriented: it exposes source references and classifications; it does not reconstruct uncaptured prompt text. Review code and tests establish implementation structure, not a measured reduction in engineering mistakes. No native automatic later-edit invalidation is assumed here.
-->

## 07 | 03 — Turn activity into the right intervention
<!-- layout: problem -->

**The operator needs a reason and an owner, not another stream of events.**

### Why this stays hard
Silence can mean thinking, a long tool call, a hidden request or a broken collector. Repetition can be useful recovery or a loop. Bad alerts interrupt healthy work.

### Agenturm today · direct local fit
Workflow attention retains exact request ownership and delivery state. Failure, interruption, completion, token, USD and elapsed-time incidents have evidence and coverage fields.

### Remaining gap
Thresholds are not semantic loop detection. Response and approval stay in the native agent. Verified terminal routing depends on the environment; live routing is Linux-specific in the reviewed code.

> Measure: actionable-alert precision, missed waits, response time and successful return to the correct session.

Sources: [S2][s2] · [S3][s3] · [S4][s4] · [S6][s6] · [R5][r5] · [R5S][r5s] · [R10][r10]

<!-- notes
Agenturm already has bounded workflow alerts and notification machinery: “no alerting” would be an inaccurate comparison. Its state and delivery evidence should be evaluated against observable requests, not against a silence timer. A seen request is not an approval. Market alternatives already provide production alerts and automated issue discovery; Agenturm's distinction is local coding-session supervision and routing.
-->

## 08 | 04 — Measure cost per verified outcome
<!-- layout: problem -->

**Token totals explain consumption. They do not establish return on effort.**

### Why this stays hard
Retries, caches, worker fan-out, tools, evaluators and human review all affect economics. Incomplete usage and mixed units make precise-looking totals misleading.

### Agenturm today · partial solution
Usage history retains reported tokens and costs with missing-cost coverage. Workflow supports token/USD/elapsed thresholds; USD alerts exclude native estimates.

### Remaining gap
No end-to-end business-cost model was established in this review. Human effort, infrastructure, tool charges and verified success must be joined to compute fully loaded economics.

> Proposed metric: total cost of all attempts ÷ number of independently accepted outcomes, for the same task cohort. Report unknown coverage separately.

Sources: [S1][s1] · [S3][s3] · [S5][s5] · [S6][s6] · [R5][r5] · [R8][r8]

<!-- notes
The denominator includes accepted outcomes only; the numerator includes failed attempts. If no outcomes succeed, report the metric as undefined rather than zero. The same tokens can appear at more than one span level; OTel explicitly warns about aggregation. Agenturm's workflow alert projection excludes native cost estimates and labels exact facts versus reported lower bounds. Those are useful distinctions, not evidence of complete invoice reconciliation or cheaper operations than another product.
-->

## 09 | 05 — Make evidence useful without overexposing it
<!-- layout: problem -->

**Debugging wants more context. Privacy and accountable action require deliberate limits.**

### Why this stays hard
Prompts and tool results can contain secrets. Run IDs do not prove authority. An approval needs the decision, reviewer and governing context to be auditable.

### Agenturm today · local privacy fit
Local SQLite, loopback defaults, source-bound content grants and private receipt exports provide concrete boundaries. Native content tests check grants against exact committed source bytes.

### Remaining gap
Local storage still holds sensitive evidence. The receiver explicitly lacks multiuser authorization. Tamper-evident enterprise audit, IAM-linked authority and universal safe redaction are not established.

> Measure: unauthorized-content exposure, export-review completion and the fraction of consequential actions with attributable approval evidence.

Sources: [S1][s1] · [S2][s2] · [S7][s7] · [R6][r6] · [R7][r7] · [R3T][r3t]

<!-- notes
Privacy controls and enterprise governance are separate capabilities. OpenAI's SDK documents that sensitive tracing data is enabled by default, provides controls, and says hosted tracing is unavailable under Zero Data Retention. That illustrates a capture-policy tradeoff, not a universal vendor weakness. PAI identifies human authority and permission history gaps. Do not treat visible reasoning summaries as faithful access to a model's hidden decision process.
-->

## 10 | Agenturm against all five problems
<!-- layout: table -->

| Problem | Current fit | Implemented evidence | Boundary |
| --- | --- | --- | --- |
| Execution ownership | Direct, local | Scoped reviews; native discovery; worker relationships | Not universal distributed lineage |
| Verified outcomes | Partial | Checks and edits; external eval imports and history | No independent correctness attestation |
| Supervision | Direct, local | Requests; thresholds; delivery receipts; routing | No control or semantic loop detection |
| Outcome economics | Partial | Usage/cost history; missing coverage; thresholds | No established fully loaded cost per success |
| Privacy / authority | Split | Local storage; content grants; private exports | Enterprise authorization/audit gap |

**“Direct” describes workflow fit, not a measured score. All five remain dependent on source coverage.**

Sources: [R1][r1]–[R10][r10] · evidence index in appendix

<!-- notes
The key distinction is direct local fit versus complete resolution of the industry-wide problem. There is no numeric competitive score because there was no common workload, deployment, independent quality benchmark or user study. The comparison uses the inspected implementation and its explicit contracts.
-->

## 11 | Where the alternatives are already strong
<!-- layout: table -->

| Offering | Documented strength | Agenturm’s more focused job |
| --- | --- | --- |
| LangSmith | Traces, online evals, feedback, monitors, automated issue diagnosis | Inspect supported local coding sessions across runtimes |
| Langfuse | OSS/self-hosting, tracing, scores, experiments, cost dashboards and alerts | A keyboard-first attention and run-review workflow |
| Arize AX / Phoenix | Tracing, trajectory/session analysis, evals; local Phoenix option | Keep native session ownership and captured work close to the operator |
| Datadog | Production traces, operational cost trends, evaluations and Insights | Local engineer supervision alongside production monitoring |
| OpenAI Agents SDK | Built-in model/tool/handoff/guardrail tracing; custom processors | Observe supported existing coding-agent records without modifying their application code |

**Selection lens:** choose for the workflow. This is not a feature-absence checklist or a vendor ranking.

Sources: [S3][s3] · [S4][s4] · [S5][s5] · [S6][s6] · [S7][s7] · [R1][r1]

<!-- notes
The right-hand column describes Agenturm's chosen focus; it does not claim competitors cannot support those jobs or integrate with coding tools. Arize explicitly documents coding-assistant access to traces. None of the reviewed products was installed and benchmarked for this presentation. Product availability and packaging can vary by deployment and plan.
-->

## 12 | Agenturm’s operating model
<!-- layout: flow -->

### 1. Observe
Read supported native records; receive OTLP from already-instrumented processes.

### 2. Preserve
Retain source and execution identities, bounded history and content-access rules in local storage.

### 3. Inspect
Navigate sessions, requests, tools, checks, usage, traces and imported evaluation results.

### 4. Continue
Review the evidence, return to the native agent, or save a brief, decision and handoff.

> The core promise is evidence continuity across an engineer’s work. `agenturm run` configures telemetry for a process; it does not inject instrumentation or steer the agent.

Sources: [R1][r1] · [R2][r2] · [R3][r3] · [R4][r4] · [R7][r7] · [R9][r9]

<!-- notes
The Rust workspace separates applications, core contracts, domain logic, plugin readers and storage. Supported readers in discovery include Codex, Claude Code, OpenCode, Pi and Kiro. The README's source table names the first four, while usage and discovery include Kiro; broad feature parity across adapters and release artifacts is not assumed. Task continuity is user-authored engineering context, not a claim that Agenturm records all agent memory changes.
-->

## 13 | The review moment that makes this concrete
<!-- layout: scenario -->

**Illustrative scenario · invented events, not a captured run or product benchmark.**

| Time | Recorded event | What an engineer can conclude |
| --- | --- | --- |
| 10:02 | Worker B requests approval | A particular worker needs a response |
| 10:06 | A recognized test command reports success | That recorded check passed in that scope |
| 10:08 | A subsequent file edit is recorded | The prior check alone cannot verify the later state |
| 10:09 | Parent A reports “done” | The parent claims completion; inspect its own evidence |

**Agenturm’s role:** preserve ownership and expose available work records. **Engineer’s action:** inspect the edit and verify the current artifact before acceptance.

Sources for capability boundaries: [R1][r1] · [R3][r3] · [R3T][r3t] · [R5S][r5s]

<!-- notes
This is a conceptual walkthrough, not a screenshot, live demo or promise that every adapter captures all four events. In the Rust receipt reviewed here, checks and edits are classified separately. Chronological invalidation is the engineer's conclusion in this scenario; an automatic native later-edit warning was not established. Legacy compatibility code contains richer later-edit projection, which is not treated as proof of native parity.
-->

## 14 | A credible position — and its competitive risk
<!-- layout: cards -->

### Lead with this
“Inspect your agents’ work, run by run.” Multi-runtime local attention, precise ownership, evidence review and deliberate handoff.

### Prove this next
Less time reconstructing work. Fewer mistaken acceptance decisions. More requests answered in the correct session. Repeat voluntary use.

### Expect this pressure
Native agent interfaces can absorb local supervision. Established platforms already add AI diagnosis. Adapter maintenance and capture gaps can erode trust.

> The defensible asset would be reliable cross-runtime evidence and an effective operator workflow. A terminal UI or local deployment alone is easy to match.

Sources: [S3][s3] · [S4][s4] · [S5][s5] · [S6][s6] · [R1][r1] · [R2][r2]

<!-- notes
This slide is analysis, not a claim of a proven moat or competitive exclusivity. Do not market full autonomous governance, universal compatibility, independently verified correctness or measured productivity gains based on this review. Those require separate evidence. The current implementation already supports many useful primitives; the product question is whether engineers consistently use them to make better decisions.
-->

## 15 | Recommended priorities
<!-- layout: table -->

| Order | Investment | Acceptance evidence |
| --- | --- | --- |
| P0 | Publish a current adapter/release capability matrix | Versioned live-capture cases; explicit unsupported and partial states |
| P1 | Tighten the native verification workflow | Exact artifact/revision linkage; clearly disclosed stale or missing checks |
| P1 | Validate attention and return-to-session reliability | Missed-request rate, false alerts and verified routing success |
| P2 | Build on evaluation imports and reported usage | Join accepted outcomes with attempts; disclose missing costs and overlapping scope |
| P2 | Make receipts easier to reuse in existing team tools | Reviewed portable exports with provenance and clear retention limits |

**Strategy:** deepen the local coding workflow first; integrate with production evaluation and monitoring systems as demand is demonstrated.

<!-- notes
These are proposed priorities, not shipped features or delivery commitments. Existing eval import, content grants, usage charts, alerts and handoff exports are starting points; the recommendation is not to rebuild them. Artifact/revision-linked verification would need careful design because recorded command success alone cannot attest the current worktree. Enterprise control-plane expansion should follow validated customer demand.
-->

## 16 | Test the product thesis with real operators
<!-- layout: cards -->

**Proposed pilot, not completed research. Compare Agenturm with each participant’s normal native-terminal workflow.**

### Ownership and attention
Find a blocked worker among concurrent sessions. Measure correct owner, missed requests and time to the right terminal.

### Evidence and acceptance
Review a completed task containing a failed check or later edit. Measure false acceptance and time to the relevant evidence.

### Continuity and economics
Resume interrupted work with a saved brief. Measure reconstruction time, task success, voluntary return use and reported-cost coverage.

> Use matched tasks and counterbalanced order. Record experience and runtime versions. Report failures, sample size and uncertainty alongside any improvement.

<!-- notes
Recruit engineers who actually supervise concurrent coding agents. Include both healthy and problematic runs so the product cannot win by simply flagging everything. Use independently reviewed expected outcomes and avoid reusing the exact same task in both conditions for one participant. Measure CPU, memory and startup on the same machines if resource overhead becomes part of the product claim. Do not invent a target lift or conclude causality from a few enthusiastic anecdotes.
-->

## 17 | The recommendation
<!-- layout: closing -->

**Own the moment between an agent’s claim and an engineer’s decision.**

Agenturm has a credible foundation for local supervision: scoped evidence, recorded requests, checks, usage, evaluation imports and task continuity.

Its next milestone should be demonstrated operator value: **faster reconstruction, more accurate review and reliable intervention.**

> Position it as a focused companion to the wider AI observability stack. Earn broader claims with measured outcomes.

<!-- notes
The answer to “does Agenturm solve the top five?” is nuanced: it directly addresses local attribution and attention, partially addresses verified outcomes and economics, and offers useful local privacy boundaries without establishing enterprise governance. That is a meaningful, focused position. It is not yet evidence that it should replace a production AI engineering platform.
-->

## 18 | Method and confidence
<!-- layout: cards -->

### Market evidence
Official standards and product docs; PAI’s October framework assessment; Hugging Face’s November evaluation guide. Accessed 27 November 2026.

### Product evidence
Rust-only upstream `6803e2a`: implementation and regression cases reviewed. Latest release observed: `rust-v0.3.15`. Source capabilities do not establish release parity.

### Interpretation limits
No vendor benchmark or participant study. Fit ratings are qualitative. Implementation, effectiveness and proposed improvements are distinguished throughout.

**Documentation drift:** some docs describe legacy behavior. This review prioritizes native Rust evidence and excludes historical performance claims.

<!-- notes
The original local checkout had divergent commits and older documentation. Research was anchored to the fetched upstream main in a separate worktree. This presentation does not inspect private agent databases or use real session content. Existing tests were read as evidence of intended contracts; their existence is distinct from independently qualifying every supported runtime version. Market sources are living documents unless pinned below. The five-problem ranking is a synthesis, not PAI's six-gap taxonomy reproduced under another name.
-->

## 19 | Sources — market and research
<!-- layout: sources -->

1. **[S1 · OpenTelemetry GenAI agent conventions][s1]** — Development status; agent, workflow and plan spans; context, usage and sensitive-content semantics. Pinned revision `81b5d438`.
2. **[S2 · Partnership on AI][s2]** — *The Observability Gap in AI Agents*, 7 Oct 2026. Four-framework assessment; identity, permissions, memory and human-intervention gaps.
3. **[S3 · Arize agent observability guide][s3]** — Updated 27 Oct 2026. Outcome verification, context propagation, sessions, evaluation and cost per successful task.
4. **[S4 · LangSmith Observability][s4]** — Traces, integrations, feedback, monitors, online evaluations, Engine and deployment options.
5. **[S5 · Langfuse Observability][s5]** — Tracing, cost tracking, scores, experiments, alerts and self-hosting.
6. **[S6 · Datadog Agent Observability][s6]** — Production operations, evaluations, sensitive-data controls, Insights and span-based pricing.
7. **[S7 · OpenAI Agents SDK tracing][s7]** — Built-in events, custom processors, sensitive-data defaults and hosted-tracing ZDR limitation.
8. **[S8 · Hugging Face agentic evaluation guide][s8]** — 23 Nov 2026. Artifact verification, grader validation, reproducibility and repeated-run reliability.

<!-- notes
Vendor documentation is evidence that a capability is documented, not independent proof of comparative effectiveness. PAI’s conclusions are scoped to the frameworks and methodology it assessed. Hugging Face’s empirical examples are not generalized into Agenturm performance claims. External links require connectivity; the presentation itself does not.
-->

## 20 | Sources — Agenturm implementation
<!-- layout: sources -->

All repository links are pinned to **`6803e2ad8ffccb12c925cdb8e25f4ab34fee8215`**.

1. **[R1 · README][r1] / [R2 · Discovery][r2]** — Product scope, local runtimes, OTLP prerequisites and bounded coverage reporting.
2. **[R3 · Review receipts][r3] / [scope tests][r3t]** — Checks, edits, errors, provenance, explicit worker scope and private exports.
3. **[R4 · Evaluation import][r4] / [import tests][r4t]** — External scores attached to existing traces; history and execution status kept distinct.
4. **[R5 · Workflow alerts][r5] / [attention status][r5s]** — Source-bound incidents, thresholds, coverage and notification delivery state.
5. **[R6 · Native content tests][r6] / [R7 · Receiver contract][r7]** — Grants and source-byte verification; loopback/authentication and multiuser limits.
6. **[R8 · Usage history][r8]** — Reported-cost coverage and currency-aware aggregation.
7. **[R9 · Saved tasks][r9] / [R10 · Terminal routing][r10]** — Briefs, decisions, handoffs and conditional verified native-terminal lookup.

**Read code + tests as capability evidence. Treat effectiveness and broad runtime parity as hypotheses requiring validation.**

<!-- notes
The strongest source anchors are ReviewScope and render/classify in review_receipt.rs; Action::Import and execute_evals in eval_import.rs; ExactTurn::usd and scope-bound projection in workflow_alerts.rs; WorkflowStatus in attention/status.rs; preview_requires_a_current_grant_and_exact_committed_pi_bytes in prompt_content.rs; and the explicit limitations at the top of workflow_route.rs. The receipt scope test demonstrates exclusion of a sibling worker, session, turn and source.
-->

[s1]: https://github.com/open-telemetry/semantic-conventions-genai/blob/81b5d438519f868a186bff088d9de24aefdb2f6b/docs/gen-ai/gen-ai-agent-spans.md
[s2]: https://partnershiponai.org/resource/the-observability-gap-in-ai-agents/
[s3]: https://arize.com/ai-agents/agent-observability/
[s4]: https://docs.langchain.com/langsmith/observability
[s5]: https://langfuse.com/docs/observability/overview
[s6]: https://docs.datadoghq.com/llm_observability/
[s7]: https://openai.github.io/openai-agents-python/tracing/
[s8]: https://huggingface.co/blog/agentic-evals
[r1]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/README.md
[r2]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/discovery.rs
[r3]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/review_receipt.rs
[r3t]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/tests/review_receipt.rs
[r4]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/eval_import.rs
[r4t]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/tests/eval_import.rs
[r5]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/workflow_alerts.rs
[r5s]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/attention/status.rs
[r6]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/tests/prompt_content.rs
[r7]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/docs/receiver.md
[r8]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/storage/agenturm-store/src/usage_history.rs
[r9]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/task_cli.rs
[r10]: https://github.com/abhishekash/agenturm/blob/6803e2ad8ffccb12c925cdb8e25f4ab34fee8215/crates/applications/agenturm-app/src/workflow_route.rs
