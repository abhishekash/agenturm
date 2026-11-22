# Rust-only Agenturm execution plan

Status: scope reset on 21 November 2026 at the user's direction. This document supersedes the Python-bridge, compatibility, and migration portions of `rust-plugin-kernel-plan.md` for all future implementation.

## Product decision

Agenturm is being rebuilt as a Rust application. Python is not part of the target runtime. Do not preserve Python runtime compatibility, create dual-runtime paths, route Python clients through a Rust bridge, or build data-migration tooling. The Rust application starts with its own clean data directory. Existing Python-era state is out of scope for automatic import.

The application keeps three independent extension families:

1. **Harness adapters** discover and decode agent activity, then submit bounded observations to the kernel.
2. **Capabilities** derive optional, typed signals from authorized observations.
3. **UI contributions** provide data-only panes; the Rust UI owns layout, navigation, focus and terminal lifecycle.

Connectors remain a later runtime extension family. The new `agenturm-connector-api` crate defines preliminary, versioned inbound and outbound contracts; it does not yet register, supervise, authorize, or persist connector work. The **target** kernel owns extension identity, policy, durable writes, query boundaries and supervision. The current `agenturm-kernel` owns supervised admission and a bounded durable observation read port, but not product policy, app configuration (which lives in CLI input), or process supervision. Rust performance is a user-chosen implementation direction; no gain is claimed without whole-application measurements.

## Milestones and gates

### 1. Rust composition kernel — foundation implemented

Deliver typed extension ports and deterministic registries for harnesses, capabilities and UI contributions. The host stamps source identity, validates bounded snapshot results, records returned errors/unwind panics, and returns a render-neutral snapshot. This seam is synchronous and trusted; it is not a process, time or allocation isolation boundary.

**Current evidence:** `agenturm-kernel` provides these three ports, stable ID-ordered polling, validated result bounds and per-extension error reporting. Supervised source adapters register by extension ID; the kernel validates facts, commits the durable cursor and observations through a storage port, then ACKs. In-process extensions can register individually or as a build-time `ExtensionPlugin` package; package registration stages harnesses, source adapters, capabilities, combined observation extensions, and UI contributions and commits them atomically after duplicate and capacity checks. The app exposes registration seams for all three in-process extension families. A combined `ObservationExtension` port now evaluates trusted capability and pane output once, validates both before publishing either, preserves durable query scope in its input without changing the public `Snapshot` shape, rejects ID collisions with capabilities/panes, and is skipped when durable history reads fail. The process-plugin follow-on is now in progress in [the process extension RFC](rust-process-extension-plugins.md): explicit catalog selection, strict DTO projection, and one-shot process supervision are wired in the current worktree; the standalone Rust example is packaged; end-to-end tests and independent Rust/security reviews pass locally. `refresh_with_store` reloads a bounded newest page of committed observations before evaluating capabilities and panes; the Rust store supports close/reopen reads and reports when older rows are outside the page. Versioned source configuration now composes up to 16 registered process adapters in one collection; version 2 selects user-managed, SHA-256-pinned adapters from an explicit catalog and verifies catalog artifacts before opening the store and again before host launches. Linux systemd user-service installation is now available; automatic discovery, dynamic capability/decoder loading, and cross-platform service managers remain open. Catalog integrity checks do not sandbox adapters; children retain the Agenturm user's OS privileges. Trait extensions are trusted in-process code: calls are synchronous, result checks follow extension allocation, and caught panics still run the standard panic hook. The full-screen UI consumes the kernel snapshot through the app composition described below.

### 2. First real Rust vertical slice — durable admission/read primitives implemented; gate open

Wire one bundled Rust harness through the process protocol into the kernel, persist its observations in a Rust-owned SQLite store, and render/query the same session after restart. Exercise duplicate replay, malformed output, adapter crash, store failure before commit, and process restart. The runtime must not start or import Python.

**Current evidence:** the kernel now reconstructs committed typed observations after reopening the SQLite store, and a generic capability/pane can evaluate that durable history. Real-process tests cover Codex ingestion, commit retry, follow-up polling, and kernel rehydration from a reopened database. An injected `SourceBatchDecoder` boundary lets the same supervised source handle different harness IDs; the Pi process test covers decoder reset, durable commit retry, ACK, and reopened-store rehydration. `agenturm collect --config <json> --db <path>` runs several configured adapters through one kernel and store transaction path; version 2 selects user-managed catalog entries with protocol/features declarations and SHA-256 pins; `--watch` retains adapters between repeated polls and closes them on handled shutdown. CI proves actual Pi and Codex processes in the same run and reads both sessions after reopen. The store, kernel, and app now expose a bounded source/session-filtered keyset query. Query pages pin to a durable receipt watermark, so later collection writes cannot shift a traversal; `agenturm query` returns a reusable cursor, and `inspect` can page to older history with `[` and return through a four-page in-memory back stack with `]`. See the [history query validation record](../validation/rust-general-history-query.md) and [plugin catalog validation record](../validation/rust-plugin-catalog.md). Linux systemd user-service install/status/uninstall is implemented for the foreground collector; automatic discovery and dynamic capability/decoder loading remain open.

See [configured harness collection](rust-collection-config.md) for the versioned JSON schema, path rules, and current runtime limits.

**Gate:** an installed Rust application can discover actual local harness data, commit one session atomically, restart and render it from durable state. A fixture worker alone does not pass.

### 3. Rust UI and capability composition — generic viewer and crate-level composition implemented; runtime discovery open

Build the Ratatui application over kernel query/pane models. Register one built-in capability and one independently packaged pane without editing the central app's view dispatch. Cover no sources, empty history, partial adapter failure, stale data, terminal resize and narrow terminal behavior. UI remains data-only at the extension boundary.

**Current evidence:** `agenturm-ui` is a separate Ratatui crate that renders contribution panes, capability metrics and failures through generic model data; it does not dispatch on harness or capability IDs. A separately packaged example implements a capability and data-only pane through the public kernel traits and an `ExtensionPlugin` package; the app installs both alongside the built-in Overview through atomic build-time composition, rejects duplicate package IDs, and leaves no partial contributions after a failed package install. TestBackend tests render the pane at 80×24 and 60×18. This proves crate-level static composition, not dynamic binary discovery or process isolation. See the [independent UI extension validation record](../validation/rust-independent-ui-extension.md) and [example package](../../examples/rust_lifecycle_extension/README.md). `agenturm inspect --db <path>` reopens the Rust store and displays the bounded durable snapshot, with keyset-backed older-page navigation. `agenturm query --db <path>` supports bounded read-only source/session filtering and emits the next stable cursor as JSON. The Overview now reports the latest safe diagnostic code, severity, source and source/session scope alongside the diagnostic count. The UI has explicit small-terminal handling, pane navigation, row scrolling, and a managed terminal lifecycle. State and rendering tests cover selection and wide, narrow and too-small dimensions. The Linux inspector has been exercised in a PTY. Cross-platform service managers and automatic runtime plugin discovery remain open.

### 4. Harness and domain parity — typed tool, lifecycle, check, and message slices implemented; gate open

Port each supported harness to Rust and map its source identities, lifecycle, usage, tool activity, relationships, coverage gaps and content-grant behavior to typed kernel events. Keep provider-specific parsing and identity rules in the adapter crates. Prove source fixtures against the documented product behavior; no Python differential runtime is required.

**Current evidence:** the shared bundled decoder preserves source-neutral tool observations (`category`, `result`, and native `operation_id`), lifecycle observations (`state` and optional bounded `reason_code`), check observations (`category` and `result`), message role observations (`user` or `assistant`), and privacy-safe diagnostics (`code` and `severity`) alongside usage facts. Diagnostics use explicit source/session scope; only closed-catalog codes are retained, while unknown codes remain count-only. The Overview displays the latest diagnostic and lifecycle evidence with source/session identity. Pi and Codex process fixtures prove interrupted lifecycle state, source reason codes, and both message roles survive commit, ACK and reopen. Codex process fixtures prove lint check category/result survives the same path. The app query JSON exposes typed fields. Message body/content, commands, outputs and raw error messages are not retained. See the [tool observation validation record](../validation/rust-tool-observations.md), [lifecycle observation validation record](../validation/rust-lifecycle-observations.md), [check observation validation record](../validation/rust-check-observations.md), [message observation validation record](../validation/rust-message-observations.md), [source-scoped diagnostics record](../validation/rust-source-scoped-diagnostics.md), and [diagnostic inspector record](../validation/rust-diagnostic-inspector.md). This is a bounded domain slice; relationships, check coverage gaps, content grants and complete harness parity remain incomplete.

### 5. Rust-owned persistence and recovery — primitive implemented, integration open

Implement schema creation, bounded writes, transaction receipts, replay, correction, revocation, retention, backup and crash recovery as native Rust behavior. Start from a new Rust-owned data directory; do not import the legacy Python database. Test kill/restart around each commit boundary and validate recovery from SQLite WAL state.

**Current evidence:** `agenturm-store` is a Rust-owned observation store and implements the kernel storage port. It atomically binds source-scoped event IDs and revisions, explicit source/session scope, typed session/worker/turn/operation identity, nanosecond timestamps, content-bound request receipts and opaque 800-byte expected/next cursors. Exact replay (including retry after later cursor advance), stale-cursor rejection, deterministic request IDs, rollback injection, unrelated-database rejection and close/reopen reads are tested. `agenturm collect --config <json> --db <path>` loads a bounded versioned config, registers up to 16 process adapters together, and admits them through one kernel and store; version 2 resolves adapters through a user-managed catalog, pins their artifact hashes, and checks protocol features at handshake. `--watch` retains process adapters between cycles until shutdown. `ingest-usage --plugin-id ...` and `ingest-codex-usage` remain one-source shortcuts. The kernel commits before sending ACK. The shared observation decoder preserves partial and cached usage counters (including absent-versus-zero), typed tool category/result and opaque operation identity, typed lifecycle state/reason evidence, typed check category/result, typed message role and safe diagnostic code/severity; malformed or unsupported facts fail closed. Local real-process tests exercise Pi and Codex host/adapter processes through this decoder and reopen usage, tool, lifecycle, check, message and diagnostic observations from the Rust store. The shared store schema is v11, with transactional upgrades from supported
earlier formats. It adds durable private-content receipt proofs and retains
the existing observations, cursors, and receipt sequence. Lifecycle facts collected before v5, check facts collected before v6, and message facts collected before v7 cannot be reconstructed because those facts were not previously stored and the source cursor has advanced. This remains short of the full gate: automatic discovery and dynamic capability/decoder loading, and support for all domain facts are open, and service support currently targets Linux systemd user managers. Subsequent work added cooperative Linux writer ownership, native pruning, online backup, and commit-boundary crash drills. A separate production storage principal, comprehensive product policy, and fault coverage beyond those drills remain open.

### 6. Capability SDK, packaging and connectors

Package harnesses and capabilities independently while shipping a useful default bundle. Publish extension contracts only after at least two real harnesses and one capability use them. A preliminary connector API now has separate inbound and outbound ports, bounded DTO validation, stable delivery request digests, and fake source/sink contract tests. Inbound work must commit with its checkpoint before ACK. Outbound attempts are at-least-once; ambiguous outcomes are reconciled by the same idempotency key, never assumed to be exactly-once. See [the Rust connector API RFC](rust-connector-api.md). The bundled trusted driver adds explicit runtime registration and grant fencing, supervised process transport, durable inbound checkpoints and outbound receipts, and persisted retry and reconciliation. Production credential provisioning and OS isolation for untrusted connectors remain open.

## Implementation order

Finish the standalone Rust application from a fresh data directory. Use the same acceptance checks for built-in and independently packaged plugins. Record the commit, platform, scenarios exercised, and result for each milestone.

The [standalone execution validation record](../validation/rust-standalone-execution.md) tracks the Linux bundle, independent adapter, and recovery checks.

### Step 1 — Finish the harness plugin boundary

Start on `feature/rust-harness-decoder-api`. Keep harness-specific parsing in adapter processes. Use a shared, versioned observation format with host validation and select supported formats through the configured plugin catalog. Keep identity, admission, cursor commits and ACK ordering in the host.

**Acceptance.** Build a harness separately, add it through configuration alone, and show that its observations survive reopen. Cover an unsupported format, a malformed batch, and a failing adapter alongside a healthy one. Keep existing Pi and Codex configurations working.

**Verified.** The independent Cargo adapter and the bundled Pi and Codex adapters passed configured collection. Host validation rejects unsupported formats and malformed batches, and healthy adapters retain their commits when another adapter fails.

### Step 2 — Prove the installed Linux workflow

Build a standalone bundle containing the Rust app, plugin host and default harness adapters. Wire discovery for supported local harness data and exercise the collector as a systemd user service on a managed Linux host.

**Acceptance.** Install the bundle on a clean host without Python. Discover real Pi and Codex data, collect one session from each, stop and restart collection, and open both sessions in the inspector. Verify that uninstall leaves collected data available.

**Verified on Ubuntu 24.04.5 x86-64.** The extracted bundle discovered and collected actual local Pi and Codex sessions under a user service, reopened both after restart, rendered them in a terminal, and retained history after uninstall. Discovery, collection and reopen also passed in an isolated filesystem without Python. Discovery can write a fixed configuration or configure watch to follow new files in the bounded selection.

### Step 3 — Complete harness evidence and inspector behavior

Use a coverage matrix for each supported harness. Finish the typed lifecycle, activity, relationships, usage, coverage gaps and content grant paths. Keep terminal layout, navigation and focus in the UI. Prove an independently packaged capability can add a metric and pane using the existing extension catalog.

**Acceptance.** Exercise actual harness records through collect, reopen and inspect. The inspector shows which turn ended, the recorded reason when present, and the trace or activity users can open. A raised PR appears as a work result while the recorded turn state remains visible. Cover missing evidence, a later completed turn, view switching and narrow terminals.

**In progress.** The [harness evidence matrix](../validation/rust-harness-evidence-coverage.md) tracks coverage per adapter. The inspector lists identifiable turns on the loaded page and opens activity for the selected source, session, and turn at the same durable snapshot. A 60×18 PTY test selected an interrupted turn and a later active turn and verified terminal restoration. The durable PR fact has a separate source-native operation and turn identity. Codex, Pi, Kiro, and OpenCode preserve explicit relationship evidence where their source records provide it. Kiro credit usage keeps its native unit. The OpenCode adapter records check outcomes for recognized direct commands with a native shell exit. A separately built Prompt Signals executable added a pane and metrics through the SHA-256-pinned inspector extension catalog. An explicit [Pi user-text preview](../validation/rust-content-grant-fence.md) binds the current grant and source to a retained event in the latest committed batch and verifies its source bytes before delivery.

The [Rust cockpit workflow coverage](rust-cockpit-workflow-coverage.md) records
the interactive Rust implementation and gaps against Python 0.2.8. The
cockpit adds saved tasks, exact run links, handoff exports, recorded requests
with persistent seen state, and explicit routing to a verified Linux tmux
pane. It displays grant-backed user prompts for Pi, Codex, Claude Code, and
Kiro, and typed Codex MCP activity. The loopback OTLP/HTTP receiver accepts
JSON and protobuf. The bare cockpit starts a receiver when it owns the
collector. Explicit source and session identity links spans to retained
native sessions, and `agenturm run` sets up the exporter for an already
instrumented command. Review stores decision notes by evidence version. The Linux bundle discovers
Codex, Pi, Claude Code, Kiro, and OpenCode. General prompt history and
full native MCP payloads still require more coverage.

### Step 4 — Complete storage and recovery

Give the Rust storage authority control over concurrent writers, replay, corrections, revocation, retention and backups. Keep the observation and checkpoint commit atomic.

**Acceptance.** Terminate the collector before commit and again after commit but before ACK. On restart, verify that the cursor and observations agree and records are neither lost nor duplicated. Verify concurrent writer rejection, stale grant rejection and restore from a Rust backup.

**Verified for the trusted Linux collector.** Every persistent writable Linux store open holds a cooperative writer lease, including during schema initialization and migration. Read-only queries and backups can run while collection owns the lease. Backups preserve committed WAL history and receipts, and require fresh private-content approval. Crash, replay, retention, and grant tests cover both commit boundaries, cursor consistency, stale tokens, and restore. Workspace backup and restore preserves saved tasks, handoffs, request state, and review decisions alongside the observation history, with fresh private-content approval required.

### Step 5 — Ship the standalone Rust release

Build and publish versioned Rust artifacts with checksums, install and uninstall instructions, and a support matrix for platform and service manager coverage. Use the Linux workflow above as the first release gate and verify each additional platform before advertising support.

**Acceptance.** From a tagged artifact, run install, collect, restart, query, inspect and uninstall on each supported platform. Check upgrades using the previous Rust release database when available. Report performance changes only when measured.

**Verified on Ubuntu 24.04.5 x86-64.** The [first standalone Rust release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.1.0) ships the exact archive and checksum from the successful tag build. The archive passed install, rolling discovery, collection, restart, query, inspection, backup, and uninstall. The database upgrade used an earlier local candidate because this is the first tagged Rust release. Additional platforms require their own qualification.

### Step 6 — Add connector runtime support after the Rust release gates

Use the existing inbound admission path and the connector API as the starting point. Wire explicit grants and supervised processes, then add durable outbound delivery and reconciliation.

**Acceptance.** Use isolated fake sources and sinks to prove revocation, replay, stable idempotency keys and recovery after an ambiguous send. Apply OS isolation and credential boundaries before enabling untrusted connector executables.

**Verified for pinned trusted connectors.** Native process fixtures exercise
revocation, replay after commit before ACK, accept-then-timeout recovery
using the same key, and confirmed-absence retry. The bundled driver exposes
collection and delivery commands. Untrusted connector activation remains
gated on OS isolation and credential control.

## Remaining work and acceptance

| Gate | Verified in the Rust path | Remaining acceptance |
| --- | --- | --- |
| Configured process boundary | An independently built adapter and the bundled adapters collect through the same pinned catalog, validation, commit and ACK path. An independent capability adds metrics and a pane. | The default cockpit loads explicitly selected, pinned independent capabilities from its user catalog. Automatic decoder installation and untrusted process activation remain open. |
| Installed Linux collector | Codex, Pi, Claude Code and OpenCode were collected from real local data. Kiro has an end-to-end native fixture. Published Rust history and cursors reopen in the development bundle. | Collect a real Kiro session on a host with a native Kiro transcript. Qualify installer and user service on any additional platform before advertising it. |
| Evidence and cockpit | Native review decisions, exact task handoffs, pending request routing, typed Codex MCP metadata, and explicit OTLP source/session links have terminal and durable checks. Grant-fenced JSONL history and OpenCode's current SQLite sweep have prompt proof tests. | Add source-specific missing lifecycle and relationship evidence, older SQLite prompt proofs, and richer review/edit and MCP detail. Verify more actual native turn shapes in the installed cockpit. |
| Storage and recovery | Atomic records and cursors, crash replay, writer leases, and portable workspace backup/restore are exercised. Restore preserves review and workbench state and disables private-content approvals. | A distinct principal for the standalone observation database remains a separate storage cutover. The distinct-UID Rust storage service fixture currently exercises its own contracts. |
| Release and compatibility | Ubuntu 24.04 x86-64 installations were qualified from published Rust releases. The development archive upgrades real release-format history. | Qualify the new tagged archive before publication. Additional operating systems and architectures remain unqualified. |
| Connector runtime | The pinned driver supervises local processes, checks policy epochs, persists inbound and outbound progress, and reconciles an ambiguous external effect before retrying. Sustained watch is exercised with local fixture executables. | Add verified least-privilege OS isolation and credential/egress control before enabling untrusted connectors. Exercise a supported production connector under that boundary. |
| Full product cutover | The native cockpit, collector, telemetry receiver, and scoped extension paths can run without a Python process. | Account for the remaining Python CLI workflows, SDK-specific automatic instrumentation and payload views before changing the default distribution entry point. |

## Current blockers

- Full harness parity still needs the unsupported native evidence identified in the coverage matrix. Private-content delivery covers bounded retained Pi, Codex, Claude Code, and Kiro JSONL receipts and the current OpenCode SQLite sweep.
- The collector and child adapters share an OS user. A separate storage authority or child isolation remains necessary for untrusted executables.
- Ubuntu 24.04 x86-64 is the qualified release platform. Other platforms require their own release checks.
