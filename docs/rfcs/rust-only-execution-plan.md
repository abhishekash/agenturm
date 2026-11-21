# Rust-only Agenturm execution plan

Status: scope reset on 21 November 2026 at the user's direction. This document supersedes the Python-bridge, compatibility, and migration portions of `rust-plugin-kernel-plan.md` for all future implementation.

## Product decision

Agenturm is being rebuilt as a Rust application. Python is not part of the target runtime. Do not preserve Python runtime compatibility, create dual-runtime paths, route Python clients through a Rust bridge, or build data-migration tooling. The Rust application starts with its own clean data directory. Existing Python-era state is out of scope for automatic import.

The application keeps three independent extension families:

1. **Harness adapters** discover and decode agent activity, then submit bounded observations to the kernel.
2. **Capabilities** derive optional, typed signals from authorized observations.
3. **UI contributions** provide data-only panes; the Rust UI owns layout, navigation, focus and terminal lifecycle.

Connectors remain a later extension family. The **target** kernel owns extension identity, policy, durable writes, query boundaries and supervision. The current `agenturm-kernel` only registers and refreshes trusted in-process contributions; it does not yet own policy, persistence, queries or process supervision. Rust performance is a user-chosen implementation direction; no gain is claimed without whole-application measurements.

## Milestones and gates

### 1. Rust composition kernel — foundation implemented

Deliver typed extension ports and deterministic registries for harnesses, capabilities and UI contributions. The host stamps source identity, validates bounded snapshot results, records returned errors/unwind panics, and returns a render-neutral snapshot. This seam is synchronous and trusted; it is not a process, time or allocation isolation boundary.

**Current evidence:** `agenturm-kernel` provides these three ports, stable ID-ordered polling, validated result bounds and per-extension error reporting. `agenturm-app` adds the `agenturm` binary and composition root with explicit `help`, `version` and `status` commands. No production harness is registered with the kernel yet; a separate one-shot Codex usage projection now exists. Nine kernel and app tests establish this composition seam only. Trait extensions are trusted in-process code: calls are synchronous, result checks follow extension allocation, and caught panics still run the standard panic hook. Independently installed or untrusted extensions must use the supervised process boundary. This does not establish kernel-integrated harness collection, durable query after restart or the full-screen UI.

### 2. First real Rust vertical slice — partial ingestion path implemented; gate open

Wire one bundled Rust harness through the process protocol into the kernel, persist its observations in a Rust-owned SQLite store, and render/query the same session after restart. Exercise duplicate replay, malformed output, adapter crash, store failure before commit, and process restart. The runtime must not start or import Python.

**Gate:** an installed Rust application can discover actual local harness data, commit one session atomically, restart and render it from durable state. A fixture worker alone does not pass.

### 3. Rust UI and capability composition

Build the Ratatui application over kernel query/pane models. Register one built-in capability and one independently packaged pane without editing the central app's view dispatch. Cover no sources, empty history, partial adapter failure, stale data, terminal resize and narrow terminal behavior. UI remains data-only at the extension boundary.

### 4. Harness and domain parity

Port each supported harness to Rust and map its source identities, lifecycle, usage, tool activity, relationships, coverage gaps and content-grant behavior to typed kernel events. Keep provider-specific parsing and identity rules in the adapter crates. Prove source fixtures against the documented product behavior; no Python differential runtime is required.

### 5. Rust-owned persistence and recovery — primitive implemented, integration open

Implement schema creation, bounded writes, transaction receipts, replay, correction, revocation, retention, backup and crash recovery as native Rust behavior. Start from a new Rust-owned data directory; do not import the legacy Python database. Test kill/restart around each commit boundary and validate recovery from SQLite WAL state.

**Current evidence:** `agenturm-store` is a fresh-database-only observation store. It atomically binds source-scoped event IDs and revisions, typed session/worker/turn/operation identity, nanosecond timestamps, content-bound request receipts and opaque 800-byte expected/next cursors. Exact replay (including retry after later cursor advance), stale-cursor rejection, deterministic request IDs, rollback injection, unrelated-database rejection and close/reopen reads are tested. `agenturm ingest-codex-usage` connects the Rust plugin host, the Codex adapter and this store for one explicit usage projection, committing before sending ACK. It preserves all five counters currently emitted by the Codex adapter, including absent-versus-zero values, and reports the number of other valid facts intentionally skipped; malformed records or unsupported usage payload fields fail closed. Rust CI exercises mixed lifecycle/message/usage rows through the actual host and adapter and polls again after ACK. This is not the full milestone gate: the app path bypasses `agenturm-kernel`, does not provide persistent app configuration or query/UI after restart, and does not yet support all Codex/domain facts. The app still has no plugin discovery/configuration or production service identity, cross-process writer fence, product policy, retention, backup, or full crash-recovery drill.

### 6. Capability SDK, packaging and connectors

Package harnesses and capabilities independently while shipping a useful default bundle. Publish extension contracts only after at least two real harnesses and one capability use them. Add connectors later with isolated fake-source/sink tests, explicit grants and idempotent delivery.

## Current blockers

- There is a minimal Rust app entry point, but no Rust TUI or production composition yet.
- Existing harness crates are standalone processes; a narrow Codex usage command now invokes the plugin host and adapter, but it bypasses the kernel and does not yet compose multiple harnesses.
- The Rust observation store is wired only into that explicit one-shot command; the installed storage service remains probe/scratch oriented and is not production authority.
- Existing Rust admission and query work proves bounded subsets, not product semantics across all evidence and policy cases.
- Cross-platform service identity, packaging, crash recovery and release workflows need separate implementation and review.

Do not call the project Rust-only, end-to-end, production-ready, or faster until the corresponding milestone gate has passed. Completing the vertical slice through kernel composition, persistent configuration, restart query and UI is the next critical path; optional connector and sample-only work must not displace it.
