# Joint implementation plan: Agents, attention, review and continuity

Started: 11 November 2026. Final validation: 12 November 2026. Source: terminal-agent-workflow RFC and the merged Agents/inspector redesign (PRs 50–51).

## Reconciliation

The inspector prerequisite is landed: exact worker evidence, bounded lazy prompts, readable Overview/Prompts/Details, keyboard navigation, tree navigation and partial loading. Preserve those contracts. The RFC's statement that this work is unmerged is superseded by this plan.

## Delivery sequence

1. Establish persisted receiver identity, additive exact trace filtering and one pure execution-scope/review projection. Preserve legacy endpoint namespaces without guessing identity. Share the four-request concurrency budget.
2. Build lazy review: 200-event pages, 1,000 events/2 MiB per selection, four cached selections/8 MiB total, explicit coverage, observed checks, later edits, frozen preview/copy/export and full keyboard access at 60×18.
3. Extract attention incidents and persist local acknowledgements and notification claims. Keep seen unresolved requests visible, initial inventory silent, notification delivery opt-in and stale capture silent. Store failures must leave monitoring usable.
4. Add task brief and immutable decision versions in additive task tables. Integrate revision conflicts, bounds, CLI, offline editors, handoffs and source references. Existing notes remain user-owned.
5. Add explicitly enabled, bounded deterministic repeated-work observations to Findings. Keep usage exactly scoped and heuristics out of desktop alerts.
6. Validate business contracts first, then integration, rendered minimum layouts, installation, resource bounds and full repository tests. Record actual results and limitations, update user documentation, open a reviewable PR and admin squash merge after local release gates pass.

## Ownership and review

The implementation is split into evidence/review, attention/persistence, task continuity and backend/diagnostics work. The independent reviewer critically assesses scope, identity and compatibility while integrating attention. Each component must have a real UI or CLI consumer, tests of user-visible contracts and explicit coverage. Final integration reviews cross component identities, stale-result guards, schema compatibility and keyboard behavior.

## Acceptance gates

- Same native IDs on different receivers, workers sharing a session, absent identity and parent versus subtree never mix evidence or acknowledgements.
- Assistant claims do not establish a successful check. Later recorded edits and incomplete evidence qualify freshness.
- Resolution requires positive fresh native evidence; missing sources, failed reads and partial snapshots never clear a request. Initial inventory is silent per receiver/execution, including restarts.
- New request identity survives threshold/reason changes; seen approvals remain until native resolution. Claims deduplicate between cockpits.
- Structured updates are transactional, conflict-aware and bounded; decision supersession stays within one task. Older notes-only updates preserve structured tables.
- Disk/network/Git/export/notification work stays off the UI thread; caches and concurrent reads remain bounded.
- Preserve existing keys, native terminal return validation, metadata-only capture, escaping and no-overwrite exports.

## External validation

The proposed 10–15 participant pilot is an external research gate. Prepare its protocol and record it as pending; do not invent participants, comprehension gains, alert precision or willingness to pay. Heuristic observations remain explicitly enabled during this pilot stage.

## Execution record

Engineering implementation and release validation are complete. The plan and implementation were peer reviewed across independent evidence, attention, continuity and backend work. Review findings were corrected before the full-suite gate. The external participant pilot remains pending.

### Implemented behavior

- **Shared identity and review:** persisted receiver UUIDs, additive exact trace filtering, authenticated namespace headers on fresh/cached reads with mid-request replacement rejection, producer/worker/run/session matching, explicit lifecycle bounds, and receiver-change cache invalidation. Shared-trace activity uses each event’s canonical owner and recorded session. Unknown identity/session stays unknown; legacy cache recovery uses the event’s own indexed span and does not rebuild all history. Review loads lazily, shares the four-request budget, deduplicates identical requests, and rejects stale responses. The Overview consumes the same pure check projection. Recorded checks, later edits, usage coverage and captured windows remain distinct from verification of the objective.
- **Attention:** stable per-request and per-outcome identities, positive fresh resolution, retained missing-source requests, silent per-execution inventory/restart (including delayed discovery), seen-but-pending requests, reversible per-outcome review, private workflow storage and atomic delivery claims. Notifications remain opt-in, generic and cockpit-bound. Failure, capacity, retry and cleanup paths are visible.
- **Continuity:** additive structured brief and immutable decision tables, optimistic revisions, supersession, explicit excerpt drafts, bounded file inputs, backup CLI, native lifecycle links and frozen review handoffs. Link identities preserve receiver, producer, agent and session even inside one shared trace. Older namespaced links refresh only against equal recorded scope fields. Shared traces and recorded parent/child links retain individual evidence while withholding potentially overlapping aggregate usage. Native-only links do not invent accounting.
- **Diagnostics:** opt-in repeated-error, equivalent-call, compaction and unobserved-recovery observations. Analysis is bounded to 500 records per run and the 20 most recent active traces, with shell polling exclusions, malformed-metadata protection, exact worker/session ownership and matching group counts. Errors after a completion are not attributed to that completion. No heuristic desktop alerts.
- **Terminal workflow:** queue first on narrow screens, stable incident selection/scroll, Enter/Escape drill-down, at least 11 list rows at 60×20 (the Workflow queue currently provides 14), readable review at 60×18, and structured editing at 60×20. Below the editing floor drafts remain intact. Actual rendered frames are embedded below.

### Decisions reached during review

1. Missing sources and partial stored reads cannot resolve requests or announce fresh failures. Receiver replacement invalidates stored caches even when the new receiver reuses native IDs.
2. Request, threshold, failure and completion acknowledgements remain independent. Seeing a request or threshold never marks a later completion reviewed.
3. Resolved unacknowledged incidents and delivery claims expire after 30 days. Minimal execution references supporting a valid acknowledgement remain through its 90-day lifetime. Unresolved incidents are not automatically expired. Persistent capacity is 10,000; overflow is visibly memory-only, with durable acknowledgement and desktop delivery disabled.
4. An exact native start is a durable run anchor, but start-only evidence without an explicit end cannot safely isolate a closed run for review. No end is inferred from wall time or a display timestamp.
5. Review stays exact to the selected execution. Existing Activity provides explicit subtree inspection; this implementation does not merge descendant reviews or usage into a parent result.
6. Saving a frozen review preserves its original receiver, run and projection even if the live selection advances. Metadata-only capture cannot supply an excerpt or reveal cached content.
7. Whole-trace accounting cannot be presented as an individual agent's usage when multiple owners are recorded or ownership capture is incomplete. Cached summaries include indexed span/log owners and event owners; sampled and legacy single-owner counts remain unverified. Traces retains its existing trace-wide accounting. Saved parent and child reviews cannot overwrite each other, and possible accounting overlap cannot become a sum.

The compact-layout acceptance target was revised on 13 November 2026: at least 11 list rows at 60×20, including the column heading, with at most nine rows of surrounding interface chrome. The existing 14-row Workflow queue remains above this minimum.

### Resource investigation

Comparisons used the merged prerequisite commit and the same synthetic small, large and payload scenarios on this machine. The final timing comparison used 101 refresh samples per scenario, with ten seconds of warmed headless idle observation. These are local measurements, not portable performance guarantees.

| Scenario | Baseline warm median | Updated warm median | Baseline RSS | Updated RSS | Baseline idle CPU | Updated idle CPU |
| --- | --- | --- | --- | --- | --- | --- |
| Small | 10.735 ms | 10.406 ms | 71.04 MiB | 70.40 MiB | 4.03% | 5.69% |
| Large, 20,000 spans | 12.191 ms | 10.620 ms | 70.56 MiB | 70.98 MiB | 6.29% | 6.21% |
| Payloads, 2,000 spans × 8 KiB | 13.797 ms | 12.505 ms | 77.41 MiB | 76.80 MiB | 5.64% | 4.39% |

The release comparison above passed the proposed warm-median and RSS review thresholds in all three scenarios. Earlier concurrent and short-run measurements exceeded the 10% timing threshold in some samples, so they triggered investigation: another controlled small comparison was +0.55%, and profiling 100 steady-state refreshes showed the same 600 HTTP reads and 1.968 versus 2.000 seconds aggregate profiled time (about +1.6%). Final measurement followed the background redraw fix and canonical-cache fast-path guard. Millisecond-scale medians varied between runs; these local results do not establish a portable latency guarantee.

Profiling caught an unconditional second monitor redraw after background observation. It now invalidates only on meaningful receipt/delivery/warning changes, with a regression. Profiled idle work then matched the baseline closely (about 1.39 versus 1.40 million calls in fifteen seconds). In the final unprofiled run, small-fixture idle CPU was 1.66 percentage points higher; large and payload observations were slightly lower. These short observations varied across repeats and do not establish sustained idle behavior; the multi-hour soak remains pending. RSS remained within the proposed memory allowance. Functional request, event, payload and cache bounds have explicit regressions.

### Validation

Focused tests cover evidence isolation and receiver replacement; read/capture budgets; concurrent claims, restart, retention and capacity; task revisions, old writers, native anchors, supersession and backups; metadata-only and frozen-source races; actual keyboard, selection and minimum layouts. The final built wheel was installed in a separate Python 3.13 environment; its Python module bytes match the source. Installed CLI checks passed for structured brief/decision writes, immutable supersession, stale-revision exit code 3, no-overwrite exit code 2, Markdown/HTML handoff export, SQLite backups and owner-only files. Installed 60×18 keyboard section navigation and return also passed. The installed wheel passed 60 attention/store/controller, legacy-contract, shared-trace and receiver-response checks, then 10 final usage-allocation and scoped-link regressions. Final independent peer review approved the scope/accounting changes with 31 passing tests and no blocking findings.

The HTML report was rendered and inspected in Chrome at desktop/light (1280 pixels) and mobile/dark (390 pixels); all five embedded images loaded and neither layout overflowed horizontally. Static asset and local-anchor checks passed.

The final code commit `1508a06d493e4953cadfed8cf21c231a126c1ea9` passed all 12 jobs in [CI run 43122425982](https://github.com/abhishekash/agenturm/actions/runs/43122425982): the complete 597-test suite on Python 3.11–3.14, Node.js 22/24 and every SDK interoperability job. The base Python environments skip 14 optional integration cases; their dependencies are exercised in separate SDK jobs. Local final UI integration also passed 18 tests. The final report update changes documentation only; its automatic CI rerun may still be pending when admin merge is used.

The first integration run identified legacy UI fixtures relying on absent producer/run metadata or preference hashes; the fixtures now model the actual API and assert durable incident receipts. Strict scoping and legacy preference-read compatibility were preserved.

### External validation and remaining limits

The [participant pilot protocol](workflow-pilot.html) is prepared but has not been conducted. No measured comprehension, response-delay, alert-precision, adoption or willingness-to-pay benefit is claimed. Diagnostics remain explicitly enabled during the pilot stage. Broader actual-terminal/tmux/SSH verification and sustained multi-hour soak measurement remain further validation, alongside improving benchmark repeatability. Retained decision sources are references, not promises that telemetry is still available. User-authored notes and decisions can contain sensitive content and are not automatically inferred or redacted.
