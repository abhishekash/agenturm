# Cooperating candidate preparation

20 November 2026. This feature-off Python API coordinates a supplied set of live store instances before invoking the [offline candidate producer](rust-migration-candidate-producer-2026-11-20.md). It does not run during receiver startup and does not transfer writer authority. Python `Store` remains the production writer.

## Scope and order

`prepare_cooperating_candidate(telemetry=..., workflow=..., tasks=..., outbound=..., output=..., verifier=..., timeout=...)` requires an `OutboundQueue` bound to the exact supplied `Store`. It performs a read-only input preflight before any one-way fence, rejecting unavailable verifier, invalid source, or invalid output paths while the supplied stores remain usable. The offline producer repeats that preflight after drain. The coordinator first holds the queue's exclusive dispatch handoff guard. While holding it, the API requests and waits for the supplied `WorkflowStore`, `TaskStore`, and `Store` instances to quiesce, using one deadline for the drain. A failed drain produces no candidate. Handoff requests are **one-way**: a failure can leave some supplied instances fenced, and this API never resumes them.

After all three drains succeed, the API checks the dispatch guard's database binding and calls the offline producer while retaining the guard. The producer makes detached telemetry, workflow, and task backups and has the Rust verifier check the trusted manifest. The guard is checked again before returning. If a late guard failure follows a verified backup, the API changes the output marker to `incomplete`; if replacement fails it removes the marker and syncs the directory. If durable invalidation cannot be proved, it reports that the output needs manual quarantine. The timeout is a drain deadline, not a bound on backup or verifier runtime.

## Evidence

Nine integration tests with real stores cover a verified candidate, retained one-way fences, an active dispatch lease blocking before any store is fenced, held workflow and telemetry connections preventing a candidate, rejection of an outbound queue bound to another Store, invalid verifier/output preflight without fencing, dispatch exclusion through backup, and post-backup guard failure and marker-sync recovery. The installed test invokes the Rust verifier. These are local focused checks; queued GitHub CI is not a passing result.

## Open gates

The guard excludes cooperating dispatch and reconciliation for the supplied telemetry path; it does not settle uncertain deliveries. Store drain covers one `Store` instance and its known workers/readers. Workflow and task drains cover one instance each. Other instances, direct SQLite callers, direct CLI backup paths, older held-open binaries, and platform permissions are outside this API. The caller must independently stop and exclude every such writer before treating the candidate as a common stopped-write snapshot. A later Rust service must still route every mutation, protect and publish the canonical target, recover crashes, and handle forward-only writes. See the [writer cutover gate](rust-writer-cutover-gate.md) and the [held-open writer proof](legacy-held-open-writer-gate.md).

The [HTML checkpoint](cooperating-candidate-preparation.html) is a self-contained light/dark review copy.
