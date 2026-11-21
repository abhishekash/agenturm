# Rust-only Agenturm execution plan

Status: scope reset on 21 November 2026 at the user's direction. This document supersedes the Python-bridge, compatibility, and migration portions of `rust-plugin-kernel-plan.md` for all future implementation.

## Product decision

Agenturm is being rebuilt as a Rust application. Python is not part of the target runtime. Do not preserve Python runtime compatibility, create dual-runtime paths, route Python clients through a Rust bridge, or build data-migration tooling. The Rust application starts with its own clean data directory. Existing Python-era state is out of scope for automatic import.

The application keeps three independent extension families:

1. **Harness adapters** discover and decode agent activity, then submit bounded observations to the kernel.
2. **Capabilities** derive optional, typed signals from authorized observations.
3. **UI contributions** provide data-only panes; the Rust UI owns layout, navigation, focus and terminal lifecycle.

Connectors remain a later extension family. The **target** kernel owns extension identity, policy, durable writes, query boundaries and supervision. The current `agenturm-kernel` owns supervised admission and a bounded durable observation read port, but not product policy, app configuration (which lives in CLI input), or process supervision. Rust performance is a user-chosen implementation direction; no gain is claimed without whole-application measurements.

## Milestones and gates

### 1. Rust composition kernel — foundation implemented

Deliver typed extension ports and deterministic registries for harnesses, capabilities and UI contributions. The host stamps source identity, validates bounded snapshot results, records returned errors/unwind panics, and returns a render-neutral snapshot. This seam is synchronous and trusted; it is not a process, time or allocation isolation boundary.

**Current evidence:** `agenturm-kernel` provides these three ports, stable ID-ordered polling, validated result bounds and per-extension error reporting. Supervised source adapters register by extension ID; the kernel validates facts, commits the durable cursor and observations through a storage port, then ACKs. The app exposes registration seams for all three in-process extension families. `refresh_with_store` reloads a bounded newest page of committed observations before evaluating capabilities and panes; the Rust store supports close/reopen reads and reports when older rows are outside the page. Versioned source configuration now composes up to 16 registered process adapters in one collection; the foreground `--watch` mode polls them repeatedly and shuts down on a handled signal. Linux systemd user-service installation is now available; general plugin discovery and cross-platform service managers remain open. Trait extensions are trusted in-process code: calls are synchronous, result checks follow extension allocation, and caught panics still run the standard panic hook. Independently installed or untrusted extensions must use the supervised process boundary. This does not establish the full-screen UI.

### 2. First real Rust vertical slice — durable admission/read primitives implemented; gate open

Wire one bundled Rust harness through the process protocol into the kernel, persist its observations in a Rust-owned SQLite store, and render/query the same session after restart. Exercise duplicate replay, malformed output, adapter crash, store failure before commit, and process restart. The runtime must not start or import Python.

**Current evidence:** the kernel now reconstructs committed typed observations after reopening the SQLite store, and a generic capability/pane can evaluate that durable history. Real-process tests cover Codex ingestion, commit retry, follow-up polling, and kernel rehydration from a reopened database. An injected `SourceBatchDecoder` boundary lets the same supervised source handle different harness IDs; the Pi process test covers decoder reset, durable commit retry, ACK, and reopened-store rehydration. `agenturm collect --config <json> --db <path>` runs several configured adapters through one kernel and store transaction path; `--watch` retains those adapters between repeated polls and closes them on handled shutdown. CI proves actual Pi and Codex processes in the same run and reads both sessions after reopen. The generic `ingest-usage` and Codex convenience commands remain available. Linux systemd user-service install/status/uninstall is implemented for the foreground collector; general plugin discovery remains open.

See [configured harness collection](rust-collection-config.md) for the versioned JSON schema, path rules, and current runtime limits.

**Gate:** an installed Rust application can discover actual local harness data, commit one session atomically, restart and render it from durable state. A fixture worker alone does not pass.

### 3. Rust UI and capability composition — generic viewer implemented; composition partial

Build the Ratatui application over kernel query/pane models. Register one built-in capability and one independently packaged pane without editing the central app's view dispatch. Cover no sources, empty history, partial adapter failure, stale data, terminal resize and narrow terminal behavior. UI remains data-only at the extension boundary.

**Current evidence:** `agenturm-ui` is a separate Ratatui crate that renders contribution panes, capability metrics and failures through generic model data; it does not dispatch on harness or capability IDs. `agenturm inspect --db <path>` reopens the Rust store and displays the bounded durable snapshot. The UI has explicit small-terminal handling, pane navigation, row scrolling, and a managed terminal lifecycle. State and rendering tests cover selection and wide, narrow and too-small dimensions. Cross-platform service managers, an independently packaged pane, and a PTY lifecycle test remain open.

### 4. Harness and domain parity

Port each supported harness to Rust and map its source identities, lifecycle, usage, tool activity, relationships, coverage gaps and content-grant behavior to typed kernel events. Keep provider-specific parsing and identity rules in the adapter crates. Prove source fixtures against the documented product behavior; no Python differential runtime is required.

### 5. Rust-owned persistence and recovery — primitive implemented, integration open

Implement schema creation, bounded writes, transaction receipts, replay, correction, revocation, retention, backup and crash recovery as native Rust behavior. Start from a new Rust-owned data directory; do not import the legacy Python database. Test kill/restart around each commit boundary and validate recovery from SQLite WAL state.

**Current evidence:** `agenturm-store` is a fresh-database-only observation store and implements the kernel storage port. It atomically binds source-scoped event IDs and revisions, typed session/worker/turn/operation identity, nanosecond timestamps, content-bound request receipts and opaque 800-byte expected/next cursors. Exact replay (including retry after later cursor advance), stale-cursor rejection, deterministic request IDs, rollback injection, unrelated-database rejection and close/reopen reads are tested. `agenturm collect --config <json> --db <path>` loads a bounded versioned config, registers up to 16 process adapters together, and admits them through one kernel and store; `--watch` retains those process adapters between cycles until shutdown. `ingest-usage --plugin-id ...` and `ingest-codex-usage` remain one-source shortcuts. The kernel commits before sending ACK. The shared usage projection preserves partial and cached counters (including absent-versus-zero) and reports the number of other validated supported facts intentionally skipped; malformed or unsupported facts fail closed. Rust CI exercises actual Pi and Codex host/adapter processes together through the shared projection and reads both sessions after reopening the store. This remains short of the full gate: general plugin discovery and support for all domain facts are open, and service support currently targets Linux systemd user managers. The app still has no production service identity, cross-process writer fence, product policy, retention, backup, or full crash-recovery drill.

### 6. Capability SDK, packaging and connectors

Package harnesses and capabilities independently while shipping a useful default bundle. Publish extension contracts only after at least two real harnesses and one capability use them. Add connectors later with isolated fake-source/sink tests, explicit grants and idempotent delivery.

## Current blockers

- The Rust TUI can inspect durable snapshots, and the app can poll a bounded configured set in foreground watch mode; Linux systemd user service installation is wired; cross-platform service support and independent plugin discovery remain open.
- Existing harness crates are standalone processes; the app composes configured Pi and Codex adapters through kernel-owned admission, but does not discover plugins or expose all domain facts yet.
- The Rust observation store is wired to kernel admission and bounded history reads; the app lacks a general query API and a separate production storage service identity.
- Existing Rust admission and query work proves bounded subsets, not product semantics across all evidence and policy cases.
- The service unit and plugin descendants run under the same user identity; a separate storage authority or tighter child sandbox is still required for untrusted adapters. Cross-platform managers, plugin discovery, packaging, crash recovery and release workflows also remain open.

Do not call the project Rust-only, end-to-end, production-ready, or faster until the corresponding milestone gate has passed. Validating the Linux service install on a managed host and completing the general restart query/UI path remain on the critical path; optional connector and sample-only work must not displace it.
