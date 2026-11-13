# Reliability recovery implementation plan

Status: implemented, independently reviewed and merged through [PR #67](https://github.com/abhishekash/agenturm/pull/67) on 13 November 2026 at 14:27 UTC. Merge commit: `10137aaf721cc4c3a543dfeaabe83be1f0e45e62`. Local main was synced to that commit. These changes remain **Unreleased**.

Baseline: main `d36446c`. Input: the consolidated `agenturm-review-notes.md` and the independently reproduced assessment. The work improves the existing monitor, evidence review, attention inbox and task CLI. Persistent notification services, model-generated briefs, collision detection, new dashboards and extra runtime integrations remain outside this plan.

## Delivery status

| Workstream | Status | Delivered behavior |
| --- | --- | --- |
| Native evidence fidelity | Complete | Native checks, edits, assistant content and scoped usage survive ingestion, review and export; unsupported commands retain their original evidence. |
| OpenCode turn identity | Complete | Explicit step/recorded-turn selectors preserve initiating prompts, worker ownership and qualified usage. |
| Local safety and storage | Complete | Private shared state, automatic local authentication, Host/Origin checks, redaction, safe migration and new-store retention. |
| Indexed history reads | Complete; performance follow-up below | Indexed source selection and scalar summaries, snapshot-coherent detail reads, resumable backfill and visible readiness. |
| Attention and durability | Complete | Unknown status separated from action, recent/history completion filtering, frozen-ID bulk review and graceful receipt draining. |
| Existing workflow composition | Complete | Review JSON links directly to saved tasks; command failures are readable; `--once` reports readiness and failure truthfully. |

Completion here means the implemented contracts passed the gates below. It does not establish a full native-reader/cockpit performance budget or prove daily adoption; those remain explicit follow-ups.

## Delivery rules

- Keep personal telemetry and the supplied review notes untouched. Exercise migrations and retention on temporary synthetic copies only. Preserve raw source identities, worker boundaries and existing explicit database configuration.
- Implement in reviewable stages with focused regression gates and a final repository-wide check. Ask an independent reviewer to challenge the plan and final code. Merge completed work using the existing repository preference; do not publish a release without a release request.
- Every failure below must have an adapter-shaped reproduction. Passing idealized projection tests is insufficient. Document unsupported evidence instead of inventing results, ownership, lifecycle or validation.

## 1. Native evidence fidelity

Normalize real Codex file actions (`add`, `update`, existing aliases) through one shared modifying-operation predicate. Decode supported assistant-message variants and suppress placeholder-only updates. Preserve useful command, recorded exit code and bounded error output; avoid duplicate generic errors when an identified detailed event exists.

Recognize bounded known shell wrappers without executing them. Recognize named npm/pnpm/yarn/bun validation scripts and Git whitespace checks. Single commands and all-success AND chains may be described using their recorded shell result. A failed compound chain does not identify which inner command failed; semicolon, OR, pipelines, substitutions and background cases must remain unclassified or explicitly uncertain unless independent command results exist. Preserve unclassified shell activity in Review so “unrecognized” cannot become “no work happened.”

Review should query an indexed evidence selection, with explicit coverage and a separately bounded initiating/latest request and useful assistant update. Bookkeeping should not crowd these out of the first page. Keep the 1,000-event/2 MiB memory budget and disclose selection/clipping. Never silently broaden a run to all session history.

Acceptance: current-shaped Codex records with wrapped `npm run check`, `npm run test:walkthrough`, `git diff --check`, `add`/`update`, exit codes and assistant content survive ingestion → API → UI/CLI projection → export. Include negative cases for masked failures, unrelated workers, producers and turns, metadata-only capture and truncated payloads.

## 2. OpenCode turn identity and usage provenance

Retain original trace IDs for assistant steps. Add authoritative user-turn membership from recorded message parent relationships; correlate the user prompt and all of its assistant/tool steps using a separate turn identity. Expose “step” versus “turn” scope explicitly. Do not attach unparented historical messages using proximity or collapse separate requests into one session scope. Continue to show isolated steps when the producer cannot establish a turn.

Refresh ownership metadata through a versioned derived-cache rebuild, with progress/diagnostics where applicable. Derive producer/agent/session ownership from stored source records; do not substitute single-agent counts for ownership. For turn usage, sum only disjoint recorded step allocations with complete ownership and preserve unknown/partial flags. Usage that is recorded but not allocated must say why; it must not be described as absent.

Acceptance: a fixture with two user requests, several assistant steps, a delegated worker and an unparented step yields distinct scopes, includes the matching prompt and never imports another actor's checks. Old cache migration restores provable ownership and leaves ambiguity unavailable.

## 3. Local safety and predictable storage

Use a common private state directory for the default telemetry database, while honoring `--db`. Discover an existing launch-directory database and explain explicit use/migration; never move or delete it silently. Provide a consistent SQLite-backup migration command that refuses occupied destinations (including sidecars) and preserves WAL-backed committed data. Default migration creates a new receiver namespace and records its source provenance. An identity-preserving cutover requires an explicitly stopped old receiver; a backup is never permission to run two live receivers with one namespace. Keep owner-only database/token files.

Generate an automatic local receiver token for newly launched local receivers; attach local clients using that token. Keep explicit token configuration and existing external receiver connections working. Validate Host and Origin before API reads and ingestion. An existing unauthenticated receiver must be identified as such rather than being silently assumed secure. Same-OS-user agents remain inside the user's trust boundary; a token does not provide same-user sandboxing.

Expand textual and structured redaction fixtures for common assigned secrets, database URL passwords, Stripe tokens and MySQL password arguments without corrupting numeric usage or ordinary command flags.

New managed state stores default to 14 days of telemetry history (an age bound, not a hard byte cap); explicit `--retention-days` configures or opts an existing store in. Only a newly created managed store is auto-enrolled; relocation/reopening never enrolls a legacy store. Existing policy metadata is respected. Migration tests include live WAL committed rows, failures/interruption, symlinks and occupied sidecars. Existing explicit databases retain their policy until the user chooses otherwise. Do not prune any existing personal database while implementing. Explain that retention bounds storage, while indexing bounds read cost.

Acceptance: launch from unrelated directories resolves to one state store; explicit legacy paths still work; migration round-trips committed data without overwrite; foreign Host/Origin and missing-token requests fail before content access; local CLI/API/exporter attachment works; fake-secret fixtures are redacted while benign fields remain intact.

## 4. Indexed read economics and migration

Materialize normalized activity and MCP/event metadata with time, trace/turn, producer, canonical agent and session indexes. Maintain bounded per-run/session/usage projections incrementally at committed ingestion/derivation rather than reparsing all historical JSON on each poll. Preserve raw records as the source of truth. Use versioned, idempotent migrations and resumable/batched backfills; stable pagination and replacement detection must survive backfill and pruning.

Treat activity, sessions, metrics and MCP as separate read paths. Eliminate the documented full-history expansion and grouping from normal live reads, preserve complete history behind explicit paging, and avoid cache invalidation that makes unrelated writes force expensive recomputation. Bound live queries without hiding their time/window coverage. Do not solve latency merely by increasing client timeouts.

Acceptance: query plans use indexed range/identity access; ingest/update/replay/prune/reopen and cross-process retention remain consistent; shared traces retain per-event owners. Exercise a many-short-trace, multi-week synthetic workload comparable in counts and payload distribution to the reported database, both unchanged and under continuous writes. Record endpoint latency, CPU, RSS and backlog on a named reference host; avoid claiming portable idle budgets from one machine.

## 5. Attention and freshness

Shared daemon CPU/I/O is process-level evidence, not per-session progress. Known multiplexed daemons must never supply per-session CPU/I/O proof, even if only one session is currently observed. Attribute other process work only where a process is uniquely tied to one observed session; otherwise use session records or explicit lifecycle evidence. Preserve genuine quiet active turns and pending native approvals without guessing failure from missing records.

Count explicit actionable requests/failures in the attention headline; present unknown/stale collection separately. Default the completion-review backlog to recent work with an explicit history filter. Display ages in appropriate units. Add deliberate bulk review only for a frozen set of explicitly selected completion incident IDs, excluding later arrivals, approvals and budget incidents, with incident-specific acknowledgement and durable receipt writes. Never bulk-resolve approvals or unrelated budget incidents. Drain queued seen-error/acknowledgement writes during graceful shutdown with a bounded deadline and visible failure semantics rather than acknowledging optimistically and dropping them. SIGKILL is outside graceful-drain guarantees.

Acceptance: a shared daemon busy on one session does not keep another old session working; unknown untitled child announcements do not create actionable attention; current replies remain visible until positive resolution; old completions remain accessible without dominating the default inbox; acknowledgements affect only the selected incident and persist after shutdown/restart.

## 6. Existing workflow composition and information quality

Accept a versioned Review JSON export directly in `task link --evidence-file`, preserving receiver/producer/agent/session/run or turn scope, exact usage allocation and the frozen projection. Reconcile size limits and reject malformed or contradictory identities. Keep task briefs and decisions user-owned; do not manufacture a generated brief.

Improve current Overview/Review/Findings output with safely decoded command/result excerpts and useful failure details. Avoid blanket HIGH classification for every non-zero shell exit; retain the original result and distinguish an observation from a confirmed unresolved failure. Existing views should make source lookup and unknowns comprehensible.

Make `--once` wait for bounded initial discovery/index readiness, distinguish empty/partial/delayed/error states, return an appropriate status, and print unknown totals/cost as unavailable rather than false zeros. Preserve stable stdout/stderr behavior for scripting. Exit 0 means a ready snapshot (including a known empty store); exit 2 means fatal/read failure; exit 3 means the bounded readiness deadline expired with partial discovery/index state; interrupt returns 130. Unknown totals remain unavailable on non-ready output.

Acceptance: review JSON → task link → handoff works without a hand-authored wrapper; malformed/cross-scope/oversized imports are explicit; native command output is readable without escaping artefacts; cold/empty/ready/partial/timed-out snapshots have truthful counts, costs and exit statuses.

## Delivery validation

1. Independent plan and code review; resolve identified correctness/privacy/data-loss risks.
2. Adapter-to-export golden journeys, security/migration, cross-process storage and lifecycle/keyboard tests pass.
3. Representative history benchmark, not only unchanged tiny/single-trace benchmarks, is recorded with conditions and limits.
4. Repository test suite, appropriate packaging checks and CI are reviewed. Keep release status truthful and document any optional skips or pending CI.
5. Update usage, privacy/performance documentation and this plan's implementation record. Supply responsive self-contained HTML companions. Preserve the input notes and existing telemetry.

Results for tested head `b2f0b42737c89b4c5911c553dc9b45c0a5c62bbd`:

- [All 12 PR CI jobs passed](https://github.com/abhishekash/agenturm/actions/runs/43440918727): 706 tests on each of Python 3.11, 3.12, 3.13 and 3.14, with 14 optional SDK skips per version; both Node versions and six SDK/interoperability jobs passed separately.
- Independent review approved implementation `ecdf027` after 38 targeted tests and an authenticated older-run usage probe. The final readiness-test amendment was separately approved after all six affected tests passed independently. It waits on advertised indexing readiness without weakening the history assertions.
- The wheel/sdist built successfully. An isolated installed wheel passed automatic token discovery, cold-start snapshot, native request/check projection and authentication boundaries. Real terminal keyboard journeys covered Agents, expanded inspector and Prompts at 120×36, plus compact Agents and Workflow/history at 60×18. HTML companions passed 16 desktop/mobile light/dark render checks.
- After merge, all 19 combined session-filter, local-monitoring and shutdown tests passed on `10137aa`, which also includes separately validated [PR #68](https://github.com/abhishekash/agenturm/pull/68). The [post-merge main CI run](https://github.com/abhishekash/agenturm/actions/runs/43442939551) was still running when this plan was updated; its linked result is authoritative for the merged tree.
- The supplied review notes and personal telemetry were preserved. No release was published.

## Implementation record

- Planning: baseline and supplied feedback inspected; independent reproductions already confirm edit/check omissions, OpenCode prompt exclusion, stale shared-process progress, ownership cache gaps, local request/redaction weaknesses and CLI shape mismatch.
- Plan peer review: approved for stage 1; required migration cutover/retention, multiplexed-daemon, backfill readiness, bounded shutdown, and frozen bulk-ID contracts incorporated before later stages. Backfills must resume after interruption, advertise incomplete coverage and preserve stable event IDs/cursors. Stop older receivers for upgrade; source-change triggers also queue compatible old-writer changes, and reads refuse incomplete indexing instead of returning stale text.
- Native fidelity: schema-derived Codex fixture reaches authenticated HTTP, the real headless CLI, frozen export and task-link input. Safe wrapper/check parsing, failed-operation effects, bounded message context and source ownership have focused regressions. Expanded native/storage batches passed 65 and 85 tests.
- Turn/provenance: explicit mutually exclusive step/turn selectors, same-session user-parent membership, preserved step traces, streaming and late-parent cases, independent scope/usage ownership, missing per-step usage and bounded membership are implemented. Tests include authenticated HTTP and progress without accounting proof.
- Safety/storage defaults: private common state, atomic local tokens, Host/Origin checks, redirect refusal, safe SDK failures, explicit WAL-aware copy and opt-in/persisted retention are implemented. Only newly created managed databases auto-enrol in 14-day retention. Tests use isolated synthetic state.
- Indexed reads: source Activity scalars/pointers, indexed MCP candidates, compact latest-run payloads, narrow per-run/session numeric aggregation and covering service indexes are implemented. Invalidations are transactional; interrupted background rebuilds resume; hydration and ownership share one SQLite snapshot. Readiness/progress is explicit. Benchmark results are recorded separately.
- Attention: shared daemon work does not imply session progress/liveness; unknown remains visible separately from actionable attention. Recent/history completion filtering, frozen-ID bulk review and bounded graceful receipt drain are implemented. A real Textual teardown test verifies queued acknowledgements and seen errors survive a pending publication.
- CLI/information/snapshot: Review JSON imports directly into task links, including bounded turn membership and overlap-safe accounting; command/result excerpts and unclassified shell records remain inspectable; --once has explicit ready/error/partial/interrupt outcomes without false empty snapshots.
- Independent code review: approved after resolving parser, token, native streaming, provenance, readiness, hydration-race and shutdown findings. A 28-test snapshot batch was followed by 38 tests on the implementation commit and six tests on the final readiness amendment. These overlapping batches are not a unique test count. Final CI and merge results are recorded above.

The [validation report](../reliability-recovery-validation.md) records representative history measurements, integration fixes, limitations and final CI status.

## Remaining quality work, in execution order

These improve the delivered features. They are follow-up work, not claims that the measurements or user outcomes already exist.

1. **Measure the complete monitor under sustained native ingestion.** Establish a repeatable, isolated soak of at least one hour with the receiver, supported native readers and interactive cockpit running together. Exercise prompts, delegated workers, approval requests, completion review, reconnect and graceful shutdown. Record per-process CPU/RSS, read latency, ingestion/derivation backlog, database/WAL growth and reconstruction mistakes. Gate: every injected request and accepted acknowledgement is accounted for; no cross-worker evidence or shared-daemon false progress; compare resource use under the same workload. The existing Store probe does not settle the original live-monitor CPU concern. Repeat the existing reconstruction pilot before claiming daily-use value.
2. **Shorten first-history readiness.** The representative fixture needed 212.81 seconds for its initial ownership/source rebuild. Profile batch transactions and query plans before changing the migration. Reuse the same 35-day fixture and record cold/restart timings, foreground read availability and backlog under writes. Gate: repeatable improvement without increased evidence loss, ownership ambiguity or peak memory; interruption/restart preserves stable event IDs and pagination. Keep incomplete readiness visible.
3. **Reduce remaining read and storage costs.** Under writes, the Store probe measured median Sessions at 375.52 ms and MCP at 249.98 ms. The fixture occupied 413.85 MiB, including 60.12 MiB of Activity metadata/pointers plus indexes. Refine scalar lookups and remove redundant index work using query plans and per-table measurements. Gate: controlled before/after results on the same fixture and host, with pagination, concurrent ownership/content snapshots, pruning and compatible-writer invalidation unchanged. Age retention must not be described as a byte cap or automatic compaction.
4. **Maintain producer-shaped regression fixtures.** Add deidentified, version-labelled records as supported producers change, covering streaming text, wrapped checks, failed edits, late parents, unknown usage and clipped output. Gate: ingestion → authenticated API → review/export → task link preserves the expected scope and uncertainty. Extend the existing runtime coverage; do not add integrations as part of this work.

Record each follow-up's measurements and decision in the validation report before marking it complete. Preserve the original acceptance contracts and the boundaries around personal data.
