# Independent Rust architecture review

Reviewed 19 November 2026 by a separate review agent, against the first draft of `rust-plugin-kernel-plan.md` on baseline `ca202c7`. This is an architecture review, not implementation verification or authorization to develop. No production files, dependencies or runtime behavior were changed for this review.

Initial draft recommendation: **revise the execution plan before approval**. The author subsequently revised the plan and I reread the changed contracts and execution sequence. **Final verdict: suitable for user review as an execution plan; no remaining plan-level blockers.** The direction is sound: preserve one persistence authority, isolate the public plugin contract from Rust's unstable ABI, retain Textual, and require measured end-to-end benefit. The following gaps matter because an implementation can appear correct in ordinary tests while losing durable decisions, allowing stale writes or hanging under overload.

## Severity-ranked findings

### R1 — High: rollback can discard durable user work

**Location:** Phase 9 rollback and the storage cutover gate.

Restoring the pre-cutover database and replaying retained source files does not recover tasks, acknowledgements, decisions, manual edits, plugin configuration or delivery receipts accepted after cutover. These are explicitly durable product data elsewhere in the plan. A source gap warning cannot repair their loss.

**Required change:** choose a lossless rollback policy. Either maintain a verified cross-version journal/export for every accepted durable mutation and replay it transactionally, or restrict automatic rollback to the pre-write cutover window and use forward recovery once Rust has accepted new writes. Keep a consistent backup for recovery without describing it as a complete post-write rollback. Do not promise restoration of external side effects.

**Acceptance:** create source events, a task edit, a review acknowledgement and an outbound delivery receipt after cutover; inject failure; recover all accepted records with unchanged identities and no duplicate delivery. If recovery cannot be lossless, the candidate must not be made default.

### R2 — High: a new lease does not exclude an old writer

**Location:** Delivery/persistence, Phase 8 and Phase 9.

A persisted lease, process lock and version check only exclude binaries that cooperate with those mechanisms. The existing Python Store writes SQLite directly and does not know the proposed kernel lease. SQLite serializes transactions but does not reject a semantically obsolete writer. Using a Rust writer with the same writable schema can therefore recreate the unsupported dual-writer situation from the earlier experiment.

**Required change:** make writer exclusion a specific cutover design and proof, rather than a claim established by a new lease. Evaluate an incompatible writable schema with guarded legacy mutation paths, or an isolated new storage path plus an explicit managed-process takeover. Define the remaining support boundary for user-started or external writers. The plan must not claim that advisory locking alone fences an arbitrary old binary.

**Acceptance:** start a supported old receiver against the cutover store and exercise source ingestion, outcomes, receipts and migration attempts. It must fail clearly before changing canonical or durable user state. Verify restart, crash cleanup and file/symlink replacement. Run the existing external-writer compatibility cases against an explicitly documented supported or rejected outcome.

### R3 — High: rejection and activation generations need explicit cursor semantics

**Location:** Evidence/identity, plugin lifecycle and delivery/persistence contracts.

The draft correctly commits a batch before acknowledgement, but “a cursor may not advance over rejected/retryable records” merges permanent policy rejection with retryable storage failure. A revoked or permanently malformed record could stall one source forever, or a later implementation could silently skip it. Source generation alone also does not explicitly distinguish a delayed result from a replaced plugin instance reading the same file generation.

**Required change:** define outcomes such as accepted, duplicate, retryable, and terminally rejected/quarantined. Durable policy-safe disposition plus the cursor must commit together when the host explicitly permits skipping a permanent rejection; retryable failures cannot advance it. The host issues an activation/lease epoch in addition to source/file generation and rejects results from retired instances. State whether a correction reuses occurrence identity with a new source revision or creates a new occurrence.

**Acceptance:** replay after commit-before-ack; mixed event types with a partial failure; permanent malformed input followed by valid input; policy revocation during an in-flight batch; a late old-instance result after restart/upgrade; corrected source records; repeated commands with identical text across distinct turns/owners. Assert source cursor, policy-safe disposition, events and derivation admission agree after restart.

### R4 — Medium: distinguish hard admission budgets from native-process limits

**Location:** Scheduling/budgets and framed process protocol.

A bounded channel counts messages, not heap bytes. A bounded input frame also does not bound memory used by an arbitrary native plugin. RSS sampling and termination are soft operational controls, not a strict allocator or security bound. Separate logical control capacity is insufficient if control traffic shares a pipe blocked behind data, or if the worker is synchronously parsing and cannot service control messages.

**Required change:** separate guaranteed host-owned bounds (wire frame, decoded representation, queued bytes, requests) from measured/soft native child budgets and any platform-specific hard process controls. Specify a control path that remains serviceable during data backpressure and a watchdog termination/replay fallback for a noncooperative worker. Account for bundled Python workers: their crash and OS-resource boundary covers the entire bundle, not each logical adapter.

**Acceptance:** an extension floods data while cancellation/revocation arrives; another extension blocks forever or allocates excessive memory. Unrelated sources and UI remain usable, the host stays within its registered allocation bounds, and the worker is stopped or marked failed within a defined deadline. Run these tests on every supported platform; do not silently downgrade a hard limit to sampling.

### R5 — Medium: the Rust shortcut bypasses its host dependency

**Location:** Phase 5, Phase 6 and the paragraph allowing Phase 6 after ports/one adapter.

The first Rust worker is planned as a process, while supervised framing, quotas, handshake and replay are delivered in Phase 5. Allowing Phase 6 immediately after Phase 3 risks inventing a second private worker protocol with weaker behavior, or treating an experiment as a production extension.

**Required change:** make the production process Rust slice depend on the minimum Phase 5 host contract, even if remaining harness extraction proceeds independently. An earlier disposable benchmark may use a private process driver, but cannot ingest production state or count as completion of the plugin host. If the first accelerator uses PyO3, record that separate decision and test interpreter/ownership behavior; it does not satisfy external-plugin isolation.

**Acceptance:** the installed Rust candidate passes the same handshake, boundedness, crash/replay and shutdown cases as the external Python fixture. No one-off lifecycle bypass remains in the application composition root.

### R6 — Medium: performance pass rules need sample definitions and a no-loss overload rule

**Location:** Measurement/acceptance and Phase 0.

The proposed gates appropriately distinguish component and whole-monitor cost. Their validity still depends on how p95, CPU interval, warm/cold state and failed requests are defined. Three paired runs can be sufficient for a bounded engineering decision when effects are large, but cannot establish a useful tail percentile with too few requests. Source cursor retention alone also cannot guarantee recovery when producers rotate/delete unread input under prolonged backpressure.

**Required change:** register minimum requests per run, duration, warmup, cold-cache definition, quantile method and failure treatment before timing. The three-pair maximum is a work bound, not a statistical guarantee; an inconclusive result does not pass. Define overflow behavior for finite-retention sources: durable spool where permitted or explicit unrecoverable coverage gap, never quiet completeness. Include queue wait, SQLite transaction/checkpoint time, transformation time and UI event latency separately.

**Acceptance:** retain every timed request and failure; publish whole-process-tree CPU/RSS; reject a run with capture errors or missing source coverage. A slow-plugin/source-rotation case must report a gap or recover from bounded durable staging. A repeatable component speedup that regresses whole-monitor cost does not pass the Rust milestone.

### R7 — Low: correct the historical explanation and ABI citation

**Location:** Historical Rust paragraph and primary references.

PR #54 explicitly states the experimental Rust port was reverted **at the user's request**. It does not document a technical failure as the reason. The draft's statement that the history does not establish why should be corrected to that narrower, directly supported explanation. The exact Rust ABI stability statement is in the external-blocks ABI reference, rather than the function syntax section currently linked.

**Acceptance:** cite [PR #54](https://github.com/abhishekash/agenturm/pull/54) and the [Rust ABI reference](https://doc.rust-lang.org/reference/items/external-blocks.html#abi). Preserve the distinction between the historical microbenchmark and current application performance.

## Additional implementation acceptance details

These strengthen the implementation checklist without expanding this session into development:

- Make integer timestamps/identifiers, null versus absent values, non-finite numbers, Unicode/invalid UTF-8 and duplicate JSON-key handling deterministic across Python/Rust. Use one wire representation per field, not a client-dependent timestamp encoding. Differential fixtures must preserve ownership, redaction and unknown values, not merely total counts.
- A separately packaged harness proof must include discovery, persisted cursor/state, restart, lifecycle and source-to-Review/Activity/export inspection. It must require only installation/configuration, not edits to a built-in adapter list or central enum. Stateful Kiro parent reconciliation and OpenCode step/turn membership remain required extraction cases.
- Keep expensive preparation outside the SQLite writer thread where parity permits it; revalidate source/policy generations before committing derived output. If the proof is unavailable, retain the existing in-transaction semantics rather than publishing a stale projection for speed.
- Capability declarations identify provided features; permissions identify granted authority. A plugin ID cannot authorize ownership of another plugin's source or another receiver. Plain native subprocesses remain trusted OS code.
- Connector revocation must distinguish not-yet-dispatched work from an already sent request whose result is uncertain. Record that uncertainty and use destination idempotency where supported; revocation cannot undo an external action already accepted remotely.

## Evidence inspected

The review inspected the plan, native collector/Store boundaries, persistence policy, read leases, measured performance reports and historical Rust code. Useful anchors at `ca202c7`:

- `observatory/local_agents.py:1516`: runtime-specific dispatch; `:1617`: direct policy/storage writes; `:1823`: direct OpenCode construction; `:1835`: fixed discovery roots; `:1898`: stateful Kiro worker reconciliation.
- `observatory/core.py:1739`: source commit is fenced through derived work publication; `:2052`: policy recheck; `:2059`: source generation validation; `:2138`: ingestion begins an immediate transaction.
- `observatory/native_policy.py:53`: durable policy table and mutation triggers; `:79`: native source-position guard.
- `observatory/read_connections.py:14`: cursors close before lease reuse to avoid retained WAL snapshots; `:65`: database identity checks across acquisition/use.
- `docs/validation/history-read-economics.md:11`: coherent derived chunks hold the writer transaction; `:62`: earlier performance improvements depended on connection reuse and avoiding repeated work.
- `docs/validation/quality-completion.md:29` and `docs/performance.md:154`: historical startup/CPU limits remain qualified rather than claimed as current success.
- Historical `f246fdd:rust/README.md`: limited importer, unsupported concurrent writers and the 0.62-second combined versus 0.78-second Python example. No new benchmark was run for this review.

## Primary technical sources

- [Rust external-block ABI reference](https://doc.rust-lang.org/reference/items/external-blocks.html#abi): the native Rust ABI has no stability guarantee; keep independently installed plugins behind a versioned transport.
- [Tokio blocking work](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html): started blocking jobs cannot be aborted through their task handles. Persistent blocking loops belong on dedicated threads; bound CPU concurrency separately.
- [Tokio channels](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html): bounded channels provide message backpressure, and clean shutdown closes then drains the receiver. Byte accounting remains an application responsibility.
- [SQLite WAL](https://sqlite.org/wal.html): concurrency and checkpoint/read-snapshot behavior remain relevant regardless of the host language.
- [PyO3 parallelism](https://pyo3.rs/main/parallelism): owned Rust work can detach from the interpreter; re-entering Python and cross-thread interpreter attachment need deliberate design.
- [Wasmtime security](https://docs.wasmtime.dev/security.html): imported host interfaces provide a sandbox boundary for a possible future Wasm design. This does not imply that ordinary native child processes are sandboxed.

## Resolution and final recommendation

The revised plan was reread after the author addressed the findings. The original findings above are retained so the review records what changed rather than presenting the initial draft as already safe.

| Finding | Resolution in revised plan | Status |
| --- | --- | --- |
| R1: lossful rollback | Defines the first post-cutover durable write as the point beyond snapshot rollback; forward recovery is the default afterward. Any downgrade requires a separately proven complete durable-write migration. | Resolved at plan level |
| R2: old writers ignore new leases | Uses a versioned destination path/schema, verifies old managed processes have stopped, acknowledges already-open handles, and requires adversarial supported-legacy DDL/DML tests. Explicit/shared stores cannot cut over without demonstrated exclusivity. Failure keeps Python authoritative. | Resolved as an explicit no-go implementation gate |
| R3: rejection/cursor and old instances | Adds typed durable dispositions, policy-safe terminal rejection, bounded quarantine, mixed-event atomic acknowledgement, occurrence/source revision distinction and a separate activation epoch. | Resolved at plan level |
| R4: resource/control feasibility | Separates hard host admission/decoder limits from soft native RSS limits and platform-specific controls; uses distinct control transport, responsive supervisor and external watchdog/replay. Bundled-worker crash scope is disclosed. | Resolved at plan level |
| R5: Rust host dependency | Makes a production process Rust slice depend on the minimum Phase 5 host. Earlier disposable experiments cannot ingest production state or count as the plugin architecture. PyO3 requires a separate decision. | Resolved at plan level |
| R6: measurement and finite retention | Registers request counts/duration, quantile and timeout handling, idle warmup/window, process-tree accounting and bounded total work. Defines bounded policy-aware staging or explicit unrecoverable source gaps. | Resolved at plan level |
| R7: historical reason and ABI source | Corrects the reason to the user's request, links PR #54 and cites the Rust external-block ABI reference. | Resolved |

**Final recommendation:** present the revised plan to the user. Begin no development until the user chooses to proceed. The extraction-first sequence, process plugin boundary, preservation of Textual, and measured Rust adoption are a defensible direction for this repository. This review approves a planning artifact, not a runtime design already proved by code or measurements.

The final precision pass also fixes IDs as opaque strings, nanosecond timestamps as decimal strings for every consumer, and numeric field types in the schema. The intentionally open implementation gates remain substantial: the exact source/activation lease and full wire schemas; enforceable legacy-writer exclusion; durable state migration and forward recovery; platform process-limit support; and a fresh representative baseline. These must be designed, tested and independently reviewed in their assigned phases. In particular, neither a new Rust binary nor a successful parser benchmark is evidence that storage authority may switch.

Review verification consisted of source/history inspection, primary-document checks and two reads of revised plan contracts. No performance campaign, runtime tests, dependency installation or production implementation was performed by the reviewer.
