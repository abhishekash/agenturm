# Rust kernel and pluggable Agenturm: execution plan

Status: independently reviewed execution plan, prepared 19 November 2026 against `ca202c7` (main including Kiro support). Implementation progress is recorded below through the durable inbox checkpoint. The remaining gates are still binding; this is not a completed Rust migration. The previously stopped branch cleanup and UI revert are not resumed.

## Implementation ledger · 19 November 2026

| Merged work | Evidence and limit |
| --- | --- |
| [#120](https://github.com/abhishekash/agenturm/pull/120)–[#125](https://github.com/abhishekash/agenturm/pull/125) | Source/reader registration, native evidence facade, provenance repair, and Pi in-process adapter. Full CI passed on #124 and #125. |
| [#126](https://github.com/abhishekash/agenturm/pull/126) | Explicit, inert plugin manifest validation and deterministic activation planning. Full CI passed. No plugin executable is launched. |
| [#127](https://github.com/abhishekash/agenturm/pull/127) | Bounded Rust/Python frame codec and Rust CI job. Rust job and local codec checks passed; full merged-run CI was still running at this snapshot. No live traffic uses the codec. |
| [#128](https://github.com/abhishekash/agenturm/pull/128)–[#131](https://github.com/abhishekash/agenturm/pull/131) | Claude Code, Codex and Kiro format callbacks extracted behind registered in-process adapters; all registered native JSONL sources now require one. Focused test sets passed locally. Full merged-run CI was pending at this snapshot. |
| [#132](https://github.com/abhishekash/agenturm/pull/132)–[#135](https://github.com/abhishekash/agenturm/pull/135) | Progress ledger, trusted collector composition, OpenCode operation contributor, and opt-in supervised plugin process transport. The transport is not connected to production collection. |
| [#136](https://github.com/abhishekash/agenturm/pull/136) | Store-owned source lease, metadata evidence, cursor and dirty sequence commit atomically; exact crash replay and stale-worker fencing tested. See [checkpoint report](../validation/plugin-durable-admission.md). Production activation and projection remain future gates. |
| UI contribution checkpoint | Trusted in-process capability panes register through a headless, bounded view model and generic Textual shell rendering. A synthetic Prompt signals pane works at 60×18 and provider failures are isolated. See [checkpoint report](../validation/ui-contribution-registry.md). External UI plugin discovery is not enabled. |

The existing OpenCode reader is registered and its legacy operation facts now use a contributor, but more source-specific interpretation remains. The synthetic sixth adapter proves collector injection. The process transport and durable inbox establish an opt-in proof, not a production plugin. The UI pane registry is a trusted constructor hook, not a production plugin host. No Rust production ingestion, production plugin projection/capture policy, external UI discovery, storage cutover, external connector, wheel packaging, or verified performance gain has been delivered. The Python Store remains the only database writer.

A local full Python run on the Pi checkpoint executed 1,436 tests with 14 skips and one failure in the HTTP-tail smoke: the host had about 2.57 GiB free while the test required a 4 GiB disk reserve plus fixture headroom. The same checkpoint passed remote full CI; the local failure is an environment limit, not evidence that the performance gate passed. Continue with production plugin admission/capture policy and projection, capability proof using real scoped data, fresh performance baseline, Rust slice, storage-authority review and packaging. The transport, inbox and trusted UI registry alone do not satisfy these gates.

## Recommendation and intended outcome

Build toward a small Rust host with explicit extension points, while retaining the existing Python/Textual UI during migration. Make harness adapters, capability processors and UI contributions independent product building blocks. Keep durable identity, capture policy, storage transactions and extension supervision under kernel control. Move measured expensive work into Rust in bounded stages, with one authoritative storage writer at each stage.

Success means adding a separately packaged harness without editing core dispatch, adding a metric pane without editing the central app, and running existing workflows with lower measured whole-monitor cost. A directory reshuffle, an enum listing every runtime, or a fast Rust parser attached to the same expensive pipeline is insufficient.

This is an application microkernel, not an operating-system microkernel or a requirement to create a fleet of services. The product remains a local monitor. The default distribution includes the supported adapters and capabilities; users should not need to assemble a usable installation.

## 1. What the repository establishes

### Structural audit at the frozen baseline

There are 90 Python modules and 33,151 lines under `observatory`. Line counts indicate review and coupling scope, not execution cost.

| Area | Current evidence | Consequence for the refactor |
| --- | --- | --- |
| Harness collection | `local_agents.py` is 2,122 lines. `Session` branches on Codex, Claude Code, Pi and Kiro; `LocalAgents.__init__` constructs `OpenCodeReader` directly; `discover` hardcodes source roots. | Extract discovery, parsing, source state and format capabilities behind a source-adapter contract. |
| Storage and interpretation | `core.py` is 4,201 lines. `_tool_activity`, `_publish_native_summaries`, ingest paths and `review_turn` contain native/OpenCode-specific rules. | A reader registry alone does not make harnesses pluggable. Move interpretation to adapters and versioned generic fact contracts. Keep transactions central. |
| UI | `terminal_ui.py` is 5,020 lines. Source coverage text, Kiro credits/permissions and view selection are wired into the app. | Replace runtime checks with typed contribution data, while keeping the layout and focus system owned by the UI shell. |
| Composition | `terminal.py:111–127` and `server.py:500–520` assemble Store, receiver and LocalAgents. | Introduce one composition root and an explicit transport/query interface before replacing runtime components. |
| Existing useful seams | `native_policy.py`, `operation_facts.py`, `source_projection.py`, `read_connections.py`, `workflow_evidence.py`, and `ui_*` modules already separate some responsibilities. | Reuse proven semantics and tests. Do not create a parallel second domain model or discard earlier correctness fixes. |

Historical retained-history measurements report approximately 113 seconds cold readiness and 15.9% whole-monitor idle CPU after an earlier optimization; Sessions/MCP under-write p95 still exceeded 100 ms. Another recorded checkpoint reached native metadata at 8.223 seconds and full history at 125.138 seconds. These are older fixture results, not measurements of `ca202c7` or proof of the user's current slowdown. See [performance evidence](../performance.md) and [qualified validation history](../validation/quality-completion.md).

The earlier experimental Rust importer was merged at `f246fdd` and reverted at `c6f2b4f`. Its recorded 20,000-span example compared about 0.18 seconds Rust import, 0.62 seconds Rust plus Python backfill, and 0.78 seconds Python. It omitted substantial production paths and prohibited concurrent Python/Rust writers. [PR #54](https://github.com/abhishekash/agenturm/pull/54) explicitly records that it was reverted at the user’s request; this is not evidence that Rust technically failed. Recover design lessons and fixtures; do not revive that branch as the architecture.

### Performance hypotheses to distinguish

1. Repeated decoding, sanitization and operation interpretation consume CPU.
2. SQLite query plans, transaction contention, migration/backfill and repeated derived work dominate latency.
3. Polling, payload conversion, serialization and redundant UI refreshes create cost even when parsing is cheap.
4. Growth in native history, adapter count or new format behavior changes the workload.

Rust can reduce compute/allocation costs and improve ownership boundaries. It cannot by itself fix missing indexes, excessive reads, unbounded work, pinned WAL snapshots or repeated processing. Phase 0 determines which hypothesis is material.

## 2. Target architecture and dependency rules

```text
 Codex  Claude  Pi  Kiro  OpenCode       Future connectors
      source-adapter plugins          input / output plugins
                 | bounded, versioned ports |
                 v                         v
 +--------------------------------------------------------+
 | Kernel / host                                          |
 | registry + supervision + budgets + identity + policy    |
 | ingestion + commit/cursor authority + query/snapshot API |
 +------------------------+-------------------------------+
                          |
          canonical evidence and scoped queries
                          v
      capability plugins: review, attention, metrics,
      findings, usage, future prompt-efficacy analysis
                          |
          typed view models / commands / diagnostics
                          v
      UI shell + contributed panes / inspector sections
      CLI and headless clients use the same public ports
```

The diagram shows permitted interfaces, not a requirement that every box be a separate process. Kernel control traffic is bidirectional; data delivery, acknowledgements and queries have distinct channels and budgets.

| Building block | Owns | Must not own |
| --- | --- | --- |
| Harness adapter | Discovery specification, source-format parser, native cursor encoding, explicit lifecycle/usage/relationship evidence and supported source versions | Agenturm SQL writes, global status decisions, capture-policy authority, UI widgets |
| Capability extension | Scoped projections, metric definitions, analysis, review/workflow features and declared commands | Unrestricted database reads, overriding canonical source evidence, registering arbitrary kernel event kinds |
| UI shell and extensions | Navigation, layout, keyboard/focus, generic table/chart/detail renderers; contributions supply pane schemas and scoped view models | Reading harness files, deriving lifecycle from prose, changing ingestion ownership |
| Kernel and owned infrastructure | Contract validation, registry/lifecycle, fairness, identity and policy enforcement, atomic persistence, bounded queries, health and compatibility | Native format switch statements, product-specific metric formulas, harness-specific labels/layout |

Storage is an internal port with a first-party SQLite implementation. It is not an independently replaceable third-party database plugin in v1. Redaction and identity enforcement are similarly non-overridable services. “Everything is a plugin” applies to product integrations and features, not to the integrity rules that make their results trustworthy.

Dependency checks must reject `kernel -> adapters`, `kernel -> UI`, `domain -> sqlite/Textual`, and imports between unrelated adapter implementations. A shipped adapter list belongs in distribution configuration, not conditional logic in the kernel. Legacy translation is allowed only in a named, time-limited compatibility layer with tests and a removal milestone.

## 3. Rust deployment and extension mechanism

### Decisions

| Choice | Decision and rationale |
| --- | --- |
| Immediate full Rust rewrite | Reject. Preserve working ingestion, evidence semantics and UI while proving replacement components. |
| Rust placement | First a bounded worker for the measured compute hotspot; later the host and storage authority after parity gates. The chosen first hotspot remains open until profiling. |
| UI language | Retain Textual initially. A later Ratatui UI is a separate decision requiring evidence that rendering is the bottleneck and a parity plan. |
| Bundled Rust code | Workspace crates with ordinary Rust traits and static linkage; internal APIs may evolve with the product. |
| Separately distributed extensions | Versioned framed protocol over supervised local process pipes. Python and Rust implementations use the same conformance suite. No per-event process launch. |
| Dynamic Rust libraries | No public Rust trait-object/dylib ABI. Rust's native ABI has no stability guarantee; a C ABI would still require explicit memory ownership and compatibility machinery. |
| PyO3 | Alternative for a tightly scoped pure compute accelerator if a measured process boundary erases the gain. It is not the public plugin ABI, and a worker's panic must not be represented as safe crash isolation. |
| WebAssembly | Defer. Consider later for constrained analysis plugins; prove required host imports, resource limits and packaging before adopting it. |
| Hot reload | Disable/replace/restart with durable checkpoints in v1. Arbitrary in-flight code replacement is outside scope. |

Start with a minimal Cargo dependency set: serialization/schema support, error handling, structured tracing and bounded runtime primitives. Tokio is appropriate for process I/O and supervision, not as a reason to make SQLite calls async. Keep long-lived SQLite connections on owned dedicated threads; bound CPU work separately. Final crate versions and MSRV are selected and pinned in the first Rust PR after checking supported deployment targets. Include `Cargo.lock`, license/dependency checks and reproducible release builds.

Native child processes provide crash separation, not an OS security sandbox. Approved plugins run with the user's OS privileges unless a separately implemented sandbox is enabled. Manifest permissions restrict kernel APIs, not all filesystem/network syscalls. Do not claim otherwise. Core installation must not auto-execute code found in arbitrary project directories. Install and activation are explicit, version-pinned operations.

## 4. Contracts that must be designed before extraction

### Manifest and activation

Provided capabilities describe features; grants describe host-authorized access and are not interchangeable. Each plugin declares a stable ID, version, kernel protocol range, provided/required extension-point versions, executable/artifact identity, configuration schema, optional dependencies and grants. Harness format versions are separate from plugin and protocol versions. Unknown required features fail activation with a reason; compatible optional additions may be ignored. Reject duplicate IDs, dependency cycles and incompatible providers before ingestion begins. Discover manifests without importing/executing plugin code.

Example, illustrative rather than a shipped schema:

```toml
id = "agenturm.harness.pi"
version = "0.1.0"
protocol = "1"
entrypoint = ["agenturm-pi-adapter"]
provides = ["source-adapter/1"]
requires = ["evidence-ingest/1"]
grants = ["configured-source-read"]
```

The lifecycle is discovered → validated → starting → ready → draining → stopped, with degraded/failed/quarantined states. Optional plugin failure must not make the receiver or unrelated panes unusable. Required capabilities fail their own readiness explicitly. Disable stops new work, drains accepted work to a deadline, retains historical facts with provenance, and marks resulting coverage gaps. Upgrades change the active plugin only at a checkpoint boundary; keep the prior executable/configuration for rollback.

### Evidence and identity

The canonical envelope includes schema version, receiver namespace, producer, source namespace and generation, native session/worker/turn identity when known, source event ID/revision, observed/source timestamps, capture-policy epoch, provenance and typed payload. Missing identity remains missing. Distinguish immutable occurrence identity from mutable source revisions. Transport/plugin IDs are provenance and routing fields, never substitutes for existing event IDs.

Payload families cover message, tool/command, file operation, lifecycle, usage, relationship and diagnostic observations. Numeric tokens, costs and Kiro credits retain typed units, currency and aggregation provenance; unsupported measurements are unknown, not zero. Source annotations live in a namespaced extension field with size/schema limits. Status reducers accept explicit lifecycle evidence, never infer completion from a PR, prose or unrelated failed check. OpenCode step/turn membership and Codex worker ownership survive migration unchanged.

The plugin SDK exposes small ports: `discover`, `poll(resume_cursor, budget)`, `ack(committed_cursor)`, `health`, `drain`; capability plugins receive scoped revisioned snapshots and submit proposed derived records. Adapters never receive an Agenturm Store handle. Extract legacy Store calls into a host-owned ingestion facade before introducing external implementations.

### Delivery, persistence and recovery

Use at-least-once batch delivery with idempotent persistence, not an exactly-once claim. The authoritative writer validates identity, current policy and generation; commits canonical events, the accepted source cursor, projection invalidations and a durable derivation/outbox marker in one transaction; only then acknowledges the batch. In-memory worker notification follows durable admission. A crash between commit and ack replays the same batch safely. Each record receives a defined disposition: accepted, duplicate, retryable failure, or terminally rejected/quarantined. Retryable failure cannot advance the cursor. A permanent malformed record or revoked content advances only when the host explicitly authorizes a policy-safe terminal disposition and commits that disposition with the cursor; diagnostics retain no rejected sensitive payload. Quarantine is bounded, with explicit source-paused/overflow status if its limit is reached. An oversized-record skip follows the same durable disposition rule. The transaction acknowledges every event kind for that cursor together; chunking cannot acknowledge a prefix that omits another kind. A correction preserves occurrence identity and increments a source revision, whereas a distinct source occurrence gets a new identity. A separately issued activation/lease epoch, in addition to file generation, rejects delayed old-instance output.

The initial protocol uses length-prefixed UTF-8 JSON frames with a pre-allocation byte limit. Identifiers are opaque strings. Nanosecond timestamps use decimal strings for every consumer, with one declared integer range and validation rule in the v1 schema. Every other numeric field also has one fixed wire type; encoding must never vary by the connected client. The host stamps receiver and plugin-instance authority and verifies granted source identity; a plugin cannot choose another receiver or overwrite another adapter’s namespace. A per-source lease and generation reject late output from replaced plugin instances. Negotiate version/features once per connection. Bound records, bytes, nesting, decoded allocations and outstanding requests; chunk oversized poll output into resumable batches. Use a distinct control pipe/transport and a responsive supervisor task for cancellation, policy changes and shutdown; do not multiplex control behind a saturated data FIFO. Parsing runs outside the control loop. A noncooperative worker is terminated by an external watchdog after a registered deadline, with replay from the last durable cursor. Framing/protocol errors terminate that connection and preserve the last durable cursor. Bound stderr, restart frequency and crash-loop backoff separately; diagnostic output cannot consume the data budget or grow without limit.

SQLite maintains one application write authority. Initially that is Python Store; a Rust worker cannot write the database. At cutover it becomes the Rust storage service, with all Python consumers using its API. Use a persisted writer/protocol lease and process lock for cooperating new clients, but do not treat them as fences against legacy binaries that ignore them. The proposed cutover uses a new versioned canonical store path/schema, verifies old managed processes are stopped, and archives the prior database and sidecars read-only. Neither changing permissions nor relocating the store revokes an already-open write handle. Phase 8 must prove that the supported legacy binaries, when deliberately pointed at either location, cannot mutate canonical data through initialization, DDL or DML, and cannot silently create an accepted second authority. A schema/write-guard mechanism and launcher/ownership checks require an independent design review and adversarial tests; they are not considered solved by this document. Refuse cutover for explicit/shared/external-writer stores until equivalent exclusivity is established. This is operational compatibility protection, not a sandbox against arbitrary same-user code that deliberately changes permissions or files. If the legacy-write test cannot pass, keep Python authoritative.

A pre-cutover snapshot is not a lossless rollback after accepting new writes. Source replay cannot recover newly authored tasks, decisions, acknowledgements, configuration, delivery state or policy tombstones. Register the point of no return explicitly. After it, use forward recovery unless a separately validated cross-version journal/export covers every durable write and reconciliation proves no loss. Never silently discard user data or treat a gap warning as sufficient rollback.

Read results carry receiver identity, source/data revision, policy epoch and coverage. Queries and source hydration use one coherent SQLite snapshot. Close cursors/transactions promptly to avoid pinning WAL. Before publishing an async derived result, revalidate the contributing revisions and policy epoch; reject stale work. Derived state is rebuildable, user acknowledgements/tasks/decisions are durable product data and must never be silently reconstructed or deleted.

### Policy, extension state and connectors

Preserve metadata-only capture, redaction, revocation tombstones and receiver/source isolation through parsing, storage, caches, UI, copy and export. Raw inputs are restricted to adapters with the configured source grant; analysis/UI plugins consume sanitized scoped evidence. Capability output includes contributing evidence references so revocation invalidates it. A capability that cannot provide sufficient provenance uses a conservative whole-scope invalidation policy.

Plugin state is namespaced and accessed through versioned kernel storage APIs with byte quotas. State migrations are explicit, transactional and reversible or snapshot-backed; plugins cannot run arbitrary DDL against core tables. Schema-incompatible downgrades fail clearly. On removal, historical derived records retain plugin/version provenance and become unavailable/stale if they cannot be interpreted; source evidence and user-owned decisions remain intact.

Future inbound connectors use the source contract with credentials resolved by the host, bounded retries, durable cursors and rate-limit handling. Outbound connectors are a separate side-effect contract: explicit destination/action grants, idempotency keys, durable delivery state, retry/dead-letter visibility and no automatic send merely because an analysis plugin emitted a finding. Recheck policy just before dispatch; cancellation/revocation blocks queued work but cannot undo a request already accepted remotely. A timed-out in-flight request has an uncertain delivery outcome until reconciled using the destination’s idempotency/status support; do not blindly resend or claim it was cancelled. Avoid a generic connector interface that conflates read-only ingestion with publishing messages or mutating external systems.

## 5. Scheduling and performance budgets

For finite-retention input, stopping polling cannot prevent the producer from rotating or deleting unread data. Where permitted, a bounded host-owned durable staging spool can preserve accepted raw input under the capture policy; otherwise detect and expose an unrecoverable source gap. A full spool pauses admission and reports a gap if retention overtakes it. Never turn backlog, rotation loss or plugin failure into a complete-coverage claim.

Backpressure is a product behavior: when a plugin reaches admission limits, stop polling it, retain the durable cursor, expose lag, and let other sources progress. Use fair per-source scheduling and global ceilings. CPU-heavy preparation stays outside the writer thread/transaction when a complete source/policy revision fence can be proved; publication revalidates that fence. Otherwise preserve the current coherent transaction until a safe replacement exists. Measure queue wait, SQL execution, decoding/derivation, WAL checkpoint stalls and UI rendering separately. Do not queue one blocking job per event or let a slow extension hold a database transaction while making RPC/network calls. Coalesce derivation only where older dirty revisions are provably superseded; never coalesce distinct source evidence.

Provisional defaults to validate in Phase 0: at most four active external plugin processes, 4 MiB framed data per plugin, 16 MiB global queued wire bytes, at most two bounded CPU batches, and one SQLite writer. Hard host-owned bounds cover framed bytes, queue bytes, outstanding requests and a schema-aware decoder’s node/string/allocation budget. A post-decode size check alone is not a hard allocation bound. Register and test the decoder’s worst-case expansion before asserting one. An arbitrary native child’s allocations are not bounded by its protocol frame: RSS monitoring/termination is a soft operational limit. Linux cgroups, Windows job objects or other platform mechanisms may provide separate hard process limits, but their guarantees and support must be explicit. Unsupported platforms do not inherit a hard-limit claim. RSS measurements cover all child processes. Existing source record/read limits remain unless an explicit compatibility test proves a change. An inactive plugin consumes no polling loop. On-demand panes activate their processors only when configured or subscribed.

Shutdown stops admission, cancels cooperative work, drains durable acknowledgements/receipts, checkpoints resumable work, closes readers and then stops the writer. Provide a bounded graceful deadline and surface incomplete shutdown. Started blocking work is not killed by cancelling a Tokio task; use bounded units, SQLite progress/interrupt support where appropriate and process termination only after its replay contract makes recovery safe. Never imply a timed-out wait cancelled a write.

### Measurement and acceptance

Phase 0 records fresh baseline data on the target host before assigning a Rust hotspot. Use isolated synthetic fixtures; never benchmark against or mutate personal telemetry by default. Preserve raw failures, environment, source hashes, workload digest, dependency versions and sample definitions.

| Measurement | Proposed gate, subject to baseline review before coding |
| --- | --- |
| Whole-monitor idle CPU | Long-term target below 5% of one core; report receiver, UI, watcher and all plugin children together. |
| Cold retained-history readiness | Long-term target below 30 seconds; distinguish first visible frame, first trustworthy current evidence and complete historical readiness. |
| Sessions/MCP under-write latency | Target p95 below 100 ms on the registered workload; include failed/time-out requests rather than filtering them out. |
| First Rust slice | At least 20% improvement in its predeclared end-to-end primary metric, with correct results and no material regression elsewhere. Parser-only gains do not pass. |
| Pure architectural extraction | Semantic parity; no reproducible regression above 5% CPU/latency or 10% peak RSS under the same fixture. Investigate measurement noise before declaring a pass. |
| Boundedness | Enforced message/byte/worker budgets; memory and backlog reach a plateau under an overload test and resume fairly after relief. |
| Responsiveness and shutdown | Proposed input p95 below 100 ms while ingesting and graceful shutdown within the existing eight-second target; explicit partial/unavailable state remains usable. |

For latency, pre-register at least 200 attempted requests per measured endpoint per run over at least 120 seconds, offered rate/concurrency, payload mix, timeouts and nearest-rank p95. Report errors/timeouts separately and reject a performance pass with capture/coverage errors; a timeout cannot disappear from the denominator. For idle CPU, use at least 60 seconds after a declared 30-second warmup, count process-tree CPU seconds divided by wall seconds (one core = 100%), and report peak aggregate RSS plus sampling interval. Fresh fixture/process means application-cold, not OS-cache-cold; record OS cache treatment without privileged cache eviction. Include startup time and warm-cache behavior separately. A representative three-pair run is a work cap, not statistical certainty; Phase 0 registers total time and stops as inconclusive if required samples cannot fit that budget. Do not multiply all workloads into an open-ended soak matrix.

These are proposed engineering gates, not achieved results or portable promises. Memory ceiling in MiB and normal ingestion rate must be registered after measuring the representative workload. If the target improvement is smaller than run-to-run variability, the result is inconclusive. Start with one baseline diagnostic and a bounded baseline/candidate comparison (three paired runs maximum); repeat only for a concrete unresolved failure. Do not restart the previously cancelled long-soak/100-run protocol.

Use existing `benchmark_app.py`, `benchmark_discovery.py`, `profile_ingest_writes.py`, session/MCP profiles and retained-history/HTTP-tail harnesses where they fit. Add child-process accounting and extension scheduling visibility rather than inventing a disconnected “Rust benchmark.” Separate profile runs from timed comparisons. Include idle, active native writes, cold/restart history, large payloads, many adapters and a deliberately slow plugin.

## 6. Execution sequence: reviewable pull requests

Each row is a bounded delivery, not permission to begin. Split further if a review cannot establish one coherent invariant. No milestone becomes default until its exit gate passes.

| PR / phase | Concrete work and main locations | Exit evidence | Rollback |
| --- | --- | --- | --- |
| 0: baseline and architecture fitness | Freeze main; capture import/call graph, formats/feature inventory and current workload profile. Read the old Rust experiment. Register hardware, fixtures and budgets in validation docs. | Root-cause ranking; repeatable baseline; all current harness features mapped, including Kiro permissions/credits and OpenCode turn membership. | Documents and diagnostic scripts only. |
| 1: canonical ports | Add `observatory/contracts/` and `application/` ports around ingestion, queries, policy, source checkpoints and projections. Keep legacy implementations behind facades. | Existing golden/source-to-export tests pass; contracts have ownership, unknown/partial and version semantics; no public HTTP change. | Remove facades; unchanged storage remains readable. |
| 2: registry and composition | Add manifest validation, deterministic activation and injected registry. Consolidate `LocalAgents` construction from `terminal.py`/`server.py`. | Empty/disabled/conflicting plugin behavior; no auto-execution on manifest discovery; dependency/import checks in CI. | Registry flag selects existing composition. |
| 3: first complete adapter extraction | Move Pi discovery, parsing, lifecycle, task-prefix/cursor and status metadata into `adapters/pi/`; generic host owns reads/admission/persistence. Choose Pi for a smaller vertical slice, not as a speed claim. | Pi fixture parity plus sibling harness non-regression. A separately packaged synthetic sixth adapter works without editing kernel or UI. | Existing Pi adapter path retained behind a configuration switch. |
| 4a–4d: remaining harnesses | Extract Claude Code, Codex, Kiro, then OpenCode; give OpenCode its own reviewable PR. Move source-specific operation/turn rules out of `core.py` into declared fact contributions. | Per-adapter source-to-Review/Activity/export conformance; runtime-specific branches gone from generic services except documented compatibility translations. | Per-adapter switch; never two active readers for one source owner. |
| 5: process plugin host | Implement versioned framed protocol, SDK, supervision, quotas, handshake and source ownership. Run trusted legacy Python adapters in a bundled worker where appropriate to limit process count; document that its adapters share one crash domain. Independently installed plugins use distinct supervised instances with admission limits. | Crash/timeout/malformed frame/replay/upgrade tests; separately installed Rust and Python fixture plugins; control channel works under saturated data queues. | In-process trusted compatibility implementation with the same ports. |
| 6: Rust performance slice | Add Cargo workspace and one profiled parse/preparation worker. Keep Python as sole persistence and policy authority. Differential output comparison on synthetic/replay fixtures before opt-in execution. | Exact semantic parity; declared end-to-end performance gate; full process/RSS overhead included; packaging works from installed artifacts. | Worker off, previous Python implementation selected. |
| 7: capabilities and UI registry | Extract usage/metrics first, then review/attention/findings incrementally. Add declarative view contributions and shared scope/query service. | New capability pane requires only plugin package/config; unavailable/disabled states, keyboard routes and 60×18 fallback work; headless queries do not import Textual. | Disable individual capability; existing user data preserved. |
| 8: Rust host/query/storage parity | Implement Rust domain/storage services behind existing HTTP/query contracts; port migrations, evidence reducers, policy, checkpoints and durable user data handling. Shadow only isolated stores, never two production write authorities. | Entire compatibility corpus, adversarial race/crash suite and installed-client parity; equivalent projections/identities/coverage after restart and rebuild. | Python remains default; shadow state disposable. |
| 9: controlled authority cutover | Stop old collector/writer; consistent backup; validate schema and ownership; start Rust writer and Python clients through API. Add explicit downgrade rules and operational diagnostics. | No data/receipt loss, one writer, restart/recovery proof, rollback drill, full app perf and release matrix. | Before any post-cutover durable write: restore the verified snapshot. After new durable writes: forward recovery by default; downgrade only after separately proving lossless cross-version migration of all durable data. |
| 10: stabilize SDK and connector proof | Publish v1 contracts only after two real adapters and a capability use them. Add an isolated inbound connector fixture and an explicit outbound delivery fixture. Remove deprecated compatibility paths after a deprecation window. | A plugin author adds integrations without kernel edits; support matrix, packaging, upgrade and failure diagnostics documented. | Pinned previous plugin versions; no forced state downgrade. |

The production process-based Phase 6 depends on the minimum Phase 5 host (handshake, framing, control channel, budgets and crash/replay). It may overlap remaining adapter extraction after those gates. A disposable earlier Rust benchmark is allowed only on isolated synthetic data, cannot ingest production state, and does not count as a delivered plugin. A PyO3 alternative requires its own reviewed interpreter/ownership decision and does not satisfy process isolation; full Rust authority must wait for adapter semantics, protocol and durability parity. UI registry work can proceed independently after contracts stabilize. Do not combine a UI redesign with this refactor. Existing 0.2.8 appearance remains the baseline until a separate user decision changes it.

Indicative effort for one experienced implementer plus reviewer: baseline/contracts 1–2 engineer-weeks, full adapter extraction/host 2–4, Rust slice 1–2, capability/UI extraction 2–3, complete storage parity/cutover 3–6. These are planning ranges, not calendar commitments; some work overlaps. Re-estimate after phases 0 and 3. The Rust host milestone is contingent on measured benefit and parity, not a fixed launch date.

## 7. Extension proofs and non-negotiable tests

### Three demonstrations of real modularity

1. **Harness:** install a fixture adapter from a separate package; discover a session, ingest a message/check/usage/lifecycle event, inspect it and uninstall the adapter. No kernel/UI source edit. Disabling it cannot report the session completed.
2. **Capability + pane:** implement an experimental prompt-efficacy capability using sanitized, scoped prompt/response/check evidence. Report defined observations (for example clarification count or time to first recorded validation) with denominators, coverage and evidence links. Do not present these proxies as proven prompt quality or causal impact. Its optional pane registers through a data schema and works without modifying `terminal_ui.py`. Content capture off yields unavailable results. Remote model analysis is a separately enabled connector with explicit data grants.
3. **Connector:** use a fake paginated external source with rate limits, cursor expiry and replay; then a separate fake output sink to prove idempotent delivery and visible failure. No real account or messages are needed to validate the contract.

### Correctness matrix

- All five native harnesses plus OTLP/SDK ingress; malformed/truncated records, append/rotation/truncation/replacement, resumed old sessions and unsupported format versions.
- Duplicate delivery, reordered/late lifecycle events, repeated same command across distinct owners/turns, incomplete OpenCode membership, Kiro credits/permissions and unknown usage.
- Kill before commit, after commit/before ack, during derivation, during checkpoint/migration and during shutdown; restart without lost evidence or duplicate user receipts.
- Capture denial/revocation during parsing, backfill, paused UI, in-flight analysis, copy/export and connector delivery. No cross-receiver/worker/session leakage.
- Plugin crash, dependency failure, duplicate manifest ID, incompatible protocol, full queue, oversized frame, blocked writer, slow consumer, failed state migration and downgrade.
- UI keyboard/focus stability, selection after updates, evidence ownership, availability, 150×40/80×24/60×18 and too-small state; unchanged CLI/HTTP contracts and installed client behavior.

Cross-language serialization goldens include timestamps above 2^53, signed integer bounds, rejected NaN/Infinity, missing versus null, UTF-8/surrogates, duplicate JSON keys, stable occurrence ordering and redaction parity. Version the policy for each ambiguous input rather than letting Python/Rust decoder defaults decide.

Existing regression anchors to preserve and extend:

| Contract | Existing test modules |
| --- | --- |
| Stateful harness parity | `test_current_native_schema`, `test_native_sessions`, `test_kiro_sessions`, `test_kiro_permissions`, `test_opencode`, `test_opencode_turn_proof`, `test_opencode_turn_review` |
| Atomic publication and source retries | `test_atomic_native_publication`, `test_atomic_native_publication_safety`, `test_native_storage_retry`, `test_native_noop_publication`, `test_native_concurrent_read_certificate` |
| Identity and capture | `test_receiver_response_identity`, `test_receiver_write_precondition`, `test_capture_reference_bindings`, `test_feed_capture_safety`, `test_receipt_export_policy`, `test_mcp_source_ownership` |
| Lifecycle and shutdown | `test_interruption_evidence`, `test_native_lifecycle`, `test_queued_receipt_shutdown`, `test_recovery_shutdown`, `test_shutdown` |
| Availability and resource bounds | `test_native_bootstrap`, `test_startup_receiver_health`, `test_receiver_limits`, `test_native_reader_budgets`, `test_resource_usage` |

These tests are starting points, not a substitute for new protocol/crash/migration cases. Record the exact commands and results for each implementation PR; do not re-label old passing runs as proof of new Rust behavior.

Rust-specific checks: `cargo fmt --check`, Clippy with documented lint policy, unit/integration tests, fuzz framing/parsers and cursor decoding, explicit integer-overflow/time bounds, and concurrency tests around commit/ack/revocation. Avoid unsafe code by default; any introduced unsafe/FFI boundary needs a written ownership contract and targeted validation. A separate expert reviewer must inspect the storage-authority cutover, not only the parser implementation.

## 8. Packaging, operations and release gates

Preserve the `agenturm` command and existing API clients. Distribute the Rust executable inside platform-specific Python wheels initially, so `uv tool install agenturm` remains usable; never download executable code on first launch. Define Linux glibc/musl, macOS and Windows support explicitly before making native code mandatory. Unsupported targets retain a clearly declared Python compatibility path until native packaging is qualified. A standalone Rust distribution is a later packaging option, not a reason to break current installation.

Expose plugin ID/version/state, last successful cursor/commit time, lag, queued bytes, processing duration, restart count, policy epoch and missing capability reasons through Diagnostics and headless health. Keep payloads and credentials out of operational logs. Report feature readiness separately from host connection health. Reuse existing support/export redaction behavior.

Before defaulting to Rust: full supported Python/client CI, Rust target CI, wheels installed in clean environments, source/derived semantic comparison, migration/rollback drills, no unresolved high-severity review findings, and the predeclared performance gates must pass. Do not publish a release merely because a Rust executable builds.

## 9. Decision gates and stop conditions

- **Now:** implementation is authorized. Complete each phase’s evidence and review gates before advancing, starting with the current baseline and adapter boundary. This document alone does not certify later cutover or performance.
- **After Phase 0:** agree on the primary slowdown, concrete baseline, absolute resource ceilings and first Rust target. If SQL/polling dominates, fix algorithm/scheduling in the new boundaries before expecting Rust to help.
- **After adapter/process proofs:** freeze the minimum supported contract; do not stabilize speculative APIs before real adapters exercise them.
- **Before storage cutover:** review exact writer exclusion, schema/downgrade, replay retention and policy equivalence. Any ambiguity blocks cutover.
- **If parity or performance fails:** keep Python authoritative, preserve failure evidence, narrow the Rust scope and revise the plan. No expanding benchmark loops to manufacture a pass.

Open design details for Phase 0/1: exact workload volume and sustained write rate, minimum OS/architecture support, first hotspot, normalized envelope schemas, stable cursor compatibility with external clients, and treatment of pre-existing external writers. Defaults above permit planning; none is claimed as measured or implemented.

## 10. Primary design references

- [Eclipse platform architecture](https://help.eclipse.org/latest/topic/org.eclipse.platform.doc.isv/guide/arch.htm): explicit extension points and separately contributed subsystems inform the plugin registry. Agenturm does not need an OSGi implementation.
- [Pi extension documentation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md): event, command and UI contribution interfaces are useful inspiration. Pi's in-process trust model is not a sandbox and is not copied as Agenturm's isolation guarantee.
- [Rust ABI reference](https://doc.rust-lang.org/reference/items/external-blocks.html#abi): native Rust ABI is unsuitable as an independently distributed stable plugin interface.
- [Tokio blocking work](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html): started blocking tasks cannot be aborted by cancelling their handles; bounded work and shutdown design are required.
- [SQLite WAL](https://www.sqlite.org/wal.html): single-writer concurrency and checkpoint/read-snapshot behavior constrain storage architecture regardless of language.

Independent review findings and resolution are recorded in the [companion Rust review](rust-plugin-kernel-review.md). This plan is a proposed execution contract, not an implementation-complete report.
