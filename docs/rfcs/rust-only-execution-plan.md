# Rust-only Agenturm execution plan

Status updated 23 November 2026. **Migration and feature development remain incomplete; default cutover is gated.** The Rust-only product decision dates from 21 November. This document supersedes the Python-bridge, compatibility, and migration portions of `rust-plugin-kernel-plan.md` for future implementation.

## Current progress — 23 November 2026

Use this status and the [detailed remaining-work plan](rust-migration-remaining-plan.md) for current scheduling. That separate plan distinguishes **feature development (F1–F5)**, **integration (I1–I3)** and **tests/qualification (Q1–Q10)**. Early milestone paragraphs below describe implementation history and must not override this progress record.

| Area | Current status | Next completion condition |
| --- | --- | --- |
| Native Rust cockpit, telemetry, Workflow and protected evidence | PR [#379](https://github.com/abhishekash/agenturm/pull/379) merged as `52ec13b`. Runtime source `42712dd` passed **813 tests / 96 suites**, strict workspace all-target Clippy and all binaries. Exact optimized x86-64 archive passed installer/0.3.8 upgrade and **12 separate-UID KVM cases**. | Requalify the final combined archive after the newer slices land. |
| Command center E1–E5 | Native human-readable titles, nickname fallback, project provenance, newest-update ordering and stable selection implemented. Twelve actual Codex transcript handles across two processes passed checks at 120×32, 80×24 and 60×18. | Supported header-only presence is implemented in CF1. Final installed/native concurrency acceptance remains with the test owner. Recorded lifecycle is not process liveness. |
| Explicit receiver Findings | Merged PR [#380](https://github.com/abhishekash/agenturm/pull/380), `ef3d647` → `39474dd`; **819 tests / 96 suites**, strict Clippy, optimized actual-tagged Findings/Review journeys passed. All applicable CI jobs are green, including both Linux bundle targets and five selected SDK jobs. | Final combined-artifact regression; verified merge complete. |
| Metric descriptions and full-catalog search | Joined code passed **833 tests / 96 suites**, strict Clippy and all binaries. Real HTTP and all three terminal sizes passed search beyond the first page, descriptions, snapshot binding and exact return. | Current-source CI and final optimized schema 21→22 upgrade/installed acceptance are qualification work. |
| Remote Host inventory | Latest-per-authorized-source query and reported hostname/platform implemented. Final chooser refresh/source-context correction passed **14 focused tests** plus the joined **833-test / 96-suite** run, strict workspace Clippy and all binaries. | Correction integrated; joined checks and updated separate-UID terminal/KVM fixture passed. Final optimized archive remains open. |
| Remaining feature work | Core FD1–FD4: attributable native shapes, private provider details beyond Codex, duplicate-safe usage/cost aggregation and cross-host terminal routing. Separate FD5–FD7: remote extension packages/formats, additional connector profiles and macOS. | Use the FD registry below; AP1 resolves SDK/CLI audit items. Tests remain delegated. No workflow is retired implicitly. |
| Release/default switch | Rust 0.3.8 remains the latest public opt-in Linux release. No 0.3.9 publication or default change. | Final selected workflow acceptance and immutable-artifact checks. At most one public release per rolling 24 hours. |

The 813, 819 and 833 test counts belong to successive source baselines and are **not additive**. The 833-test baseline includes the final Host chooser correction and predates CF1–CF5/schema 22. No completion percentage is asserted while required workflow acceptance remains open. A real Kiro transcript and an approved production connector endpoint remain external inputs; macOS needs native runners and its D2-Mac checks. Comparative benchmarks remain deferred.


## Remaining feature development — current scheduling

The five CF1–CF5 slices are implemented and merged in PR #382 (`3135060`);
PR #383 (`2fea865`) synchronizes baseline test evidence and fixes the reported
lint/upgrade-identity coverage gap. FD3's combined usage slice is merged in PR
#385 (`1541481`). The rows below describe development still open. The SDK/CLI
audit and test qualification are separate work categories.

### Active implementation

**FD4 — cross-host terminal routing is in progress in the shared checkout**
(`main` at `1541481`, with uncommitted changes in
`rust/agenturm-app/src/workflow_route.rs`,
`rust/agenturm-app/src/remote_history/client.rs`, and
`rust/agenturm-app/src/cockpit_workflows.rs`). The current slice adds an
operator-supplied route file to the native remote cockpit, verifies the exact
current request, and probes the receiver host for one unique tmux pane whose
process ancestry matches the provider's exact resume command. Worker requests,
unsupported providers, missing configuration, ambiguous/stale targets and
unavailable routes fail closed. This is implementation-in-progress evidence,
not a completed or merged feature: focused validation, return-selection review,
security review, and confirmation of supported route transport semantics are
still pending. The Python receiver explicitly rejects this option.

### Core Linux features

| ID | Priority / owner | Feature still to implement | Dependency and completion condition |
| --- | --- | --- | --- |
| FD1 | Core / harness adapters | Remaining attributable native evidence: Pi same-session worker relationships, Claude asynchronous workers, and additional supported check/turn shapes. | Obtain positive native records for each shape, then add exact typed attribution through admission/reopen and the evidence UI. Missing exit codes or parent evidence stay unknown; fixtures or output prose cannot invent facts. Completed Claude worker spawning and neutral presence are not reopened. |
| FD2 | Core / private evidence and UI | Provider-specific private edit and MCP detail beyond the implemented Codex routes. | Define each provider's exact retained/source proof and separate content approval. Implement scoped selection/reveal/revocation and clear unavailable cases. Existing whole-session Codex Review edits, MCP preview and linked-trace paths remain implemented. |
| FD3 | Core / usage and cost | Combined usage/cost totals that safely handle overlapping producer reports. | **Combined slice implemented:** per-day combined totals exclude reports whose overlap is established through exact `(target source, session)` identity, count the exclusions, and sum only nonduplicated native reports plus the per-trace-deduplicated trace cohort and disjoint log-only cohort; currencies stay distinct, and native assistant cost records that forks can duplicate stay outside combined totals. Unattributed telemetry carries no identity link and contributes; that boundary is disclosed rather than inferred away. Older attached-receiver pages without the combined block stay readable. Remaining: cross-layer overlap without identity links cannot be detected and is never summed speculatively; installed, paired-state and broad qualification are delegated as CF6 in the [core feature test plan](../validation/rust-core-feature-test-plan.md). Existing seven-day and per-currency views remain implemented. |
| FD4 | Core / remote workflow — **in progress** | Cross-host terminal routing to the exact recorded remote owner/session. | Current working-tree slice: native receiver `--route-file`, exact current-request check, bounded receiver-host process/tmux probes and unique-target verification. Still complete focused validation, return-selection review, security/transport review, and fail-closed acceptance. Resolve one exact target or report unavailable; routing never answers or approves. Existing local routing/resume and native/attached receiver reads are not missing features. |

FD1–FD4 can progress independently after agreeing on any shared DTO, grant or
transport contracts. An unavailable native witness is a dependency, not proof
that the producer records the desired fact.

### Separate platform and extension development

| ID | Owner | Feature still open | Existing baseline and boundary |
| --- | --- | --- | --- |
| FD5 | Extension runtime | Remote adapter package discovery/download and support for additional decoder formats. | Trusted local install/replace/list/show/pin-CAS remove is complete. Define trusted origin, integrity, version/format and update selection before remote acquisition. Activation remains explicit and subject to the supported trust/isolation profile. |
| FD6 | Connector runtime | HTTPS/general provider transport, production credential provisioning and sandbox support beyond the current architecture/profile. | Static Linux x86-64 isolated_linux and host-mediated unix_status_v1 are implemented. Select and implement the additional profile before its deployed acceptance; an approved production endpoint is a qualification dependency, not this transport implementation. |
| FD7 | Platform/distribution | macOS artifacts, installation/update/uninstall, native paths and service integration, signing/notarization and normal Gatekeeper behavior. | Apple Silicon and Intel are separate targets. Define minimum OS and foreground/launchd support. Native testing follows implementation; macOS is unsupported until D2-Mac passes. Linux qualification can proceed under its own declared support. |

These are independent delivery/support tracks. Do not advertise them complete
because a Linux build ships, and do not silently retire them or expand the
Linux release scope without an explicit product decision.

### AP1 — SDK/CLI parity audit, not a blanket missing-feature list

Reconcile every retained Python 0.2.8 CLI/keyboard journey and selected SDK
integration with its actual Rust code path. Cover OpenAI, Anthropic,
LangChain/LangGraph, Google ADK and Pydantic AI as applicable to the retained
inventory. Record each row as **implemented**, **test-only acceptance pending**,
**confirmed missing feature**, **native evidence unavailable**, or **scope
decision needed**. Include the owner, source anchor and acceptance journey.

Only confirmed missing behavior becomes a new FD development task. Passing
selected SDK jobs on an older source is not complete instrumentation coverage;
unexecuted tests are not evidence that implementation is absent. Complete this
audit before claiming feature development or default cutover complete. No
workflow is retired by this documentation update.

### Implemented code and qualification ownership

CF1 neutral Claude/Pi presence and admission-age retention, CF2 whole-session
Review edits, CF3 adapter lifecycle, CF4 log failure investigation and CF5 exact
Claude worker spawning are **implemented**. Their pending current-head tests,
paired UI states, installers/upgrades, artifact/guest checks and CI belong to
the separate test execution agent in
[the core feature test plan](../validation/rust-core-feature-test-plan.md).
Actual Kiro collection, the approved production endpoint and native Mac runs
are evidence/platform gates. Comparative performance benchmarks stay deferred.

## Synchronized test progress

The [independent Q1–Q10 run and review](../validation/rust-migration-test-plan-results.md)
qualifies the recorded **`18aa702` baseline** with its exact artifact; it does
not qualify newer CF1–CF5 source `3135060`. Baseline 833-test Rust checks and
named optimized journeys passed; full shell syntax, genuine point-upgrade
identity, guest/service/restore and paired-state scope require the distinctions
recorded there. The new real-point upgrade regression passes in the sync fix.

PR #382 current-source Rust CI stopped on an unused test import before Rust
execution and dependent SDK jobs. The import is removed and scoped Store test
Clippy passes in the corrective change. Broader current-head, schema 22 and
CF1–CF5 qualification stays with the separate test agent. No release/default
switch is implied by the baseline report.

## Core feature development and delegated testing

Current user direction: this implementation session owns feature development;
a separate agent owns broad test execution. Follow the
[core feature test handoff](../validation/rust-core-feature-test-plan.md).

| Feature lane | Development status | Testing responsibility |
| --- | --- | --- |
| CF1 Claude/Pi neutral native presence | Implemented; timestamped and exact untimed Claude metadata supported, unknown-time admission retention in schema 22. Codex unchanged. | Separate test agent: actual collection/replay/roster journeys. |
| CF2 Whole-session paged Review edits | Implemented; 17 focused checks/Clippy passed. Paged exact-session browser with original ReviewVersion and private proof. | Separate test agent: terminal/private/broker and selection-return acceptance. |
| CF3 Trusted adapter list/show/remove | Implemented; four focused checks/Clippy passed. Exact-pin CAS removal preserves artifacts/history. | Separate test agent: CLI/atomicity/security/collection regression. |
| CF4 OTLP log failure investigation | Implemented; one focused process regression passed. Metadata only, explicit log kind, scope/cursor binding retained. | Separate test agent: broad failure inventory/remote/terminal privacy regression. |
| CF5 Exact Claude worker spawning | Implemented; exact structured declaration/result correlation, typed parent scope/child ID and declared-child evidence presentation. | Separate test agent: native collection, persistence and exact parent/child acceptance. |

Feature developers run focused checks needed for implementation. Q1–Q10 and
release qualification remain delegated; no broad campaign is duplicated here.

## Product decision

Agenturm is being rebuilt as a Rust application. Python is not part of the target runtime. Do not preserve Python runtime compatibility, create dual-runtime paths, route Python clients through a Rust bridge, or build data-migration tooling. The Rust application starts with its own clean data directory. Existing Python-era state is out of scope for automatic import.

The application keeps three independent extension families:

1. **Harness adapters** discover and decode agent activity, then submit bounded observations to the kernel.
2. **Capabilities** derive optional, typed signals from authorized observations.
3. **UI contributions** provide data-only panes; the Rust UI owns layout, navigation, focus and terminal lifecycle.

Connectors are a separate runtime extension family with versioned inbound/outbound contracts and an implemented trusted pinned process driver. Local fixtures verify supervised progress, policy/replay and reconciliation; the locally qualified static Linux x86-64 isolated_linux / unix_status_v1 profile has process/credential/endpoint controls; additional profiles and deployed production acceptance remain open. The kernel owns validated admission and durable read contracts, while application configuration and process supervision have their own runtime boundaries. Typed admission and artifact pins are not OS isolation. Rust performance is a user-chosen implementation direction; no gain is claimed without whole-application measurements.

## Release cadence

Publish at most one release in any rolling 24-hour period. Continue implementation,
qualification and completed PR merges between publications, then batch qualified
work into the next eligible release. This policy does not pause implementation
or change the default Rust cutover gates.

The latest publication is Rust 0.3.8 at 23 November 2026, 06:03:24 UTC.
The next batch is eligible no earlier than 24 November, 06:03:24 UTC
(07:03:24 Europe/Berlin); recheck the latest publication when preparing that batch.
Rust 0.3.9 remains an unpublished candidate.

## Milestones and gates

The following milestone evidence is historical. Consult the current progress table and separate remaining-work plan before assigning work or interpreting an old open gate.

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

### Current UI implementation status — 23 November 2026

Python 0.2.8 remains the visual and interaction acceptance reference. The original `2fa075f` → `06d450c` handoff and its 97 tests are historical, superseded by the joined qualification above. Theme/design polish and the native command-center naming/recency changes are implemented. MCP inventory/related traces, metric history/exemplars, seven-day usage, grouped Findings and protected native sources, and Workflow settings/retry/resume are also implemented in the merged native batch.

Metric descriptions/full-catalog search and recorded remote Host inventory are implemented follow-ups awaiting final integration/qualification. Exact Python-quality parity remains gated on the required installed journeys and visual/error-state checks; theme completion alone does not close it. See F2 and Q6 in the [remaining-work plan](rust-migration-remaining-plan.md).

### Command-center defect follow-up — E1–E5

The original Rust 0.3.8 screenshot and diagnosis are retained in the [task list](rust-only-execution-tasks.md#pending-issue--running-sessions-missing-from-the-command-center) as historical evidence. Later implementation authorization superseded the earlier diagnosis-only instruction. The native batch now implements expanded discovery/context handling, exact titles/projects, nickname fallbacks, recency ordering and stable identity. Actual concurrent Codex-transcript checks passed at all three terminal sizes; historical Claude is separate, simultaneous live Claude remains unverified, and header-only native sources still lack retained rows before session facts arrive. Final installed acceptance remains open; retained lifecycle status must not be described as an OS-level live-process inventory.

### Native roster qualification history

This is the earlier native-batch record; the current progress table and remaining plan govern final combined acceptance.

The later “do it” instruction authorized implementation of E1/E2/E3 from the
[live-roster diagnosis](../validation/rust-live-roster-diagnosis.md). Parallel
lanes implemented discovery, native context, and actual-session acceptance.

- Follow discovery prioritizes exact same-user native transcript handles,
  excludes confirmed guardian reviews before adapter admission, and rotates
  queued sources within the current cap. Coverage distinguishes selected,
  queued and partially scanned sources. Open handles do not invent a Working
  state; explicit history inspection states that discovery is disabled.
- Human names come from explicit operator titles or safe native Codex and Claude
  metadata. Prompt-derived Codex title copies are suppressed. Provider and agent
  context support the name; IDs remain stable keys available in details and
  copy actions. Native recorded cwd supplies project headings with separate
  provenance. Structural activity remains supporting metadata.
- Metadata checks the exact history incarnation, retained checkpoint, native
  inode and authenticated header/prefix. Collector-owned validated membership
  survives rotation. A bounded configured-root lookup recovers exact retained
  source members after restart; it does not cache names or scan ambient homes.
- The verified Claude snapshot-prefixed opening is supported without changing
  the shared cursor envelope. Full-prefix proof is required for its metadata.
  Ordinary large Claude transcripts report partial native-title coverage above
  the bounded scan budget. Header-only transcripts with no session facts do not
  yet create roster rows. Discovery beyond its scan window remains partial.
- All five Python palettes are integrated with preview/apply/cancel and saved
  preferences. Semantic status styles, alternating rows and observed MCP-server
  summaries share the same theme. The command-center first-screen structure is
  preserved. This is not a claim that the remaining view parity below is done.
- PR #377 merged as `ed03815fdfe0a6bfea88ee07ce69e9ff227c2b89`. The final
  runtime passed 652 tests across 93 workspace suites, zero failed or ignored,
  strict all-target Clippy, actual 12-session Codex process-witness acceptance,
  recorded Codex and historical Claude acceptance, and all seven multi-user
  KVM cases. Native x86-64 and ARM64 candidate CI passed. See the
  [native-session record](../validation/rust-real-session-roster.md) for exact
  artifact identity, remaining bounds and evidence categories. Publication is
  held under the rolling 24-hour cadence. Contemporaneous live Claude
  concurrency remains unverified; its actual historical native recordings
  passed the display and collection checks.
- A subsequent visual follow-up keeps task text on the shared row background
  and mutes supporting context. The updated source passed 653 workspace tests
  across 93 suites, all 45 UI tests, strict Clippy and formatting. Its rendered
  background regression covers 120 and 60 columns. The next daily release must
  qualify its selected final batch; the earlier exact archive remains identified
  separately in the validation record.

### First screen — Agenturm command center

The user selected the command-center screenshot supplied on 23 November as the layout reference for the first screen. Use Agenturm branding and the established Agenturm colors, typography and terminal controls.

- Open with an Agenturm command-center header and a project grouping control.
- Show selectable counts for All, Needs you, Working, Ready and Inactive. Counts and statuses come from recorded evidence and current collection state.
- Identify each row primarily by its readable task or conversation name. Show agent name/provider and project context to disambiguate names. Put session IDs in inspection details and copy actions. Generic operation labels do not satisfy named-session acceptance.
- When a task title is absent, use the native agent nickname as the row title. If both are absent, use concise provider/role/project context. Repeated “Name unavailable” labels fail roster acceptance. Track this correction as E4.
- Always sort by latest update. Project groups follow their newest member and rows follow descending update time. Preserve selected identity across refreshes and use deterministic ties. Hierarchy and status sorting must not override recency. Track this correction as E5.
- Place tasks under project path headings, each with its task count. Use aligned Tasks, Status and Updated columns, readable task titles, compact status markers and a full-width selected-row highlight.
- Use the available width for the task list on initial launch. Enter opens the selected task or session inspector, and Escape returns to the same project, filter and task selection.
- Support keyboard selection and additional rows through a Show more action. Preserve the project grouping, status text and updated age in compact terminals, truncating long task titles by display width.
- Compare populated, empty, loading and unavailable screens at 120×32, 80×24 and 60×18. Use the screenshot's structure with Agenturm's visual identity, then verify the resulting first screen with the user.

### Parallel work and dependencies

Use the table below to assign remaining work. Steps 1–6 record milestone history. The [task breakdown](rust-only-execution-tasks.md) gives the acceptance check for each ID.

| Work | Independent remaining work | Join/dependency |
| --- | --- | --- |
| Integration | I1 Findings merge; I2 Host correction plus Metrics; I3 source/docs consistency. | Serialize shared protocol/UI/Git changes; Q1 joined checks before freezing the final artifact. |
| Harnesses | F1 attributable missing provider evidence; Q8 actual native qualification per provider. | Shared worker relationship contract first where needed; actual Kiro transcript remains external. |
| Cockpit/workflows | F2 remaining edit/MCP, instrumentation and usage coverage inventory. | Q6 paired installed journeys after final integration. E1–E5 are implemented, not fresh feature assignments. |
| Boundary/extensions | F3 decoder installation and F4 connector process/credential/egress controls. | Q9 needs boundary proof and approved endpoint. C1–C5 already have a verified separate-UID baseline. |
| Distribution | F5 macOS design/implementation when native runners exist; current-head Linux/SDK CI Q7. | Q5 final optimized Linux archive, upgrade and recovery; Q10 separate native Mac acceptance; D3 remains gated. |

**UI acceptance before switching the default (B1 → D3).** Use the [Python 0.2.8 workflow comparison](rust-cockpit-workflow-coverage.md) to list the required keyboard journeys and expected results. Run the same journeys on Python 0.2.8 and the installed Rust candidate at normal width and 60×18. Verify visible evidence, focus and return selection. For example, open an interrupted turn, inspect its tool activity and edits, then return to the same turn. Record layout or shortcut differences and resolve missing required behavior before cutover. Use the Python 0.2.8 screens as the visual and interaction reference. Compare populated, loading, unavailable and narrow layouts side by side. Check the summary header, horizontal views, session hierarchy, evidence tables, selected-row inspector, keyboard focus and return path. Track each visible difference until accepted or fixed. Defer comparative performance benchmarking to a later phase.

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

**macOS target (D2-Mac).** Qualify Apple Silicon (`aarch64-apple-darwin`) and Intel (`x86_64-apple-darwin`) as separate targets when native runners are available. Define and publish the minimum supported macOS version, bundle format, install/update/uninstall behavior, data/config locations, supported local harness discovery, and whether collection is foreground-only or also managed by `launchd`. The installer must not assume Linux paths, `systemd`, or Linux-only process controls. Verify Gatekeeper behavior and use signed/notarized release artifacts wherever required for a normal documented install; do not instruct users to bypass platform protections as the standard path.

**D2-Mac acceptance.** On each advertised architecture, use the exact checksum-identified release artifact on a clean supported macOS host. Install without Python or a Rust toolchain; launch in a standard terminal; discover and collect supported native harness data; restart and reopen history; exercise query, inspect, backup, upgrade, and uninstall with history retained. If `launchd` collection is advertised, install, restart, and remove that service separately. Run the required cockpit journeys at normal and narrow terminal sizes, verify terminal restoration, and record unsupported harnesses or OS limitations before advertising support.

**Verified on Ubuntu 24.04.5 x86-64.** The [first standalone Rust release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.1.0) ships the exact archive and checksum from the successful tag build. The archive passed install, rolling discovery, collection, restart, query, inspection, backup, and uninstall. The database upgrade used an earlier local candidate because this is the first tagged Rust release. Additional platforms require their own qualification.

**Rust 0.3.1 qualified and published.** The tag points to commit
`4661eae53a288d091688597f539c8a1167595e59`. The exact Ubuntu 24.04.5
x86-64 archive passed bare startup, collection without a Python executable
on PATH, prior 0.2.0 history and cursor upgrade, backup, systemd user
installation and restart, inspection, and retained history after uninstall.
A 60×18 PTY verified the active collector's prompt fence and linked OTLP
view. A real OpenTelemetry SDK verified protobuf ingestion. The published
download matched the qualified archive and its SHA-256.

[Release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.1) ·
[Tagged build](https://github.com/abhishekash/agenturm/actions/runs/45024901558)

### Step 6 — Add connector runtime support after the Rust release gates

Use the existing inbound admission path and the connector API as the starting point. Wire explicit grants and supervised processes, then add durable outbound delivery and reconciliation.

**Acceptance.** Use isolated fake sources and sinks to prove revocation, replay, stable idempotency keys and recovery after an ambiguous send. Apply OS isolation and credential boundaries before enabling untrusted connector executables.

**Verified for pinned trusted connectors.** Native process fixtures exercise
revocation, replay after commit before ACK, accept-then-timeout recovery
using the same key, and confirmed-absence retry. The bundled driver exposes
collection and delivery commands. Untrusted connector activation remains
gated on OS isolation and credential control.

## Native journey integration history — 23 November 2026

Historical source record retained from PR #379. Statements below about running tests, draft status and outstanding archive work describe that earlier source; use the current progress table and separate remaining plan for current status.

The integration branch and [draft PR #379](https://github.com/abhishekash/agenturm/pull/379) implement the following Python 0.2.8 journeys. This is a development candidate; the final checksum-identified archive is still being qualified and the default distribution remains gated. The [paired cockpit audit](../validation/rust-ui-journey-parity.md) distinguishes tested behavior and remaining differences.

| Journey | Implemented and checked | Remaining qualification or work |
| --- | --- | --- |
| Command center E1–E5 | Exact native titles/nicknames/project provenance; concise fallback; newest-updated group and row ordering; stable selection. Twelve actual open native transcripts across two processes passed exact source/session/title/project and return checks at 120×32, 80×24 and 60×18. Rename, distinct sources sharing a native ID, catch-up and restart passed. | Final archive rerun. Header-only installed qualification passed old-cursor one-shot backfill, stable replay, guardian separation, neutral Unknown state and lifecycle authority under timestamp skew. |
| MCP | Authorized configured idle inventory; exact call-to-trace links pinned to source/session/turn/worker/operation and snapshot; retained literal text search before pagination and exact server filter carryover. Beyond-latest-500 search, linked trace, empty-result inventory separation and selection restoration passed at all three sizes. | Final extracted bundle. |
| Metric history and Usage | All five typed OTLP metric kinds via JSON/protobuf; series/points/exemplar browsing; page-local catalog search; seven UTC days, per-currency costs, representative trace counters, run/error/log evidence and overlap suppression. Actual SDK exports, replay, denial and three terminal sizes passed. | Final archive and current SDK matrix. Descriptions are outside retained typed metric fields. |
| Findings and Host | Source-scoped finding groups/counts, exact occurrences and page-local filters. Protected exact failed Codex command sources reuse the current-grant/redraw-ACK viewer. Same-user pinned live and retained processes expose permitted cwd basenames. Live PID/project search, inspector and identity-preserving stopped-process refresh passed at all three sizes. | Final packaged private-source journey. Exact failed-span sources and parent PID/search are implemented; actual terminal checks passed for failed command/span reveal, revocation and return selection. Failed evaluation and captured generic error-log sources are implemented with exact receipt/current failure or log-grant checks. The log route passed source-only terminal checks and separate-account qualification. The explicit Python receiver Findings follow-up passed tagged source, redaction, owner/member and capture-policy checks at all three widths; see its qualification record. Current remote Host inventory and metric descriptions/catalog search are being completed in parallel. |
| Native requests and Workflow | Typed requests and separate default-off guardian reviews; bind/send/hook/retry for native and explicit Python receivers; durable opt-in desktop delivery with current receiver clock and exact request proof. Separate-account guest replay and restore fencing passed. Workflow copies a verified current main-session resume command without launching it. | Final joined guest and installed notifications/resume. Review completion never implies approval. Typed threshold settings, ten-minute snooze, quiet baselines, exact failed-attempt retries and authenticated execution clocks are integrated. Final joined tests, terminal and packaged clock qualification are running. |
| Storage and Review | Schema 20 adds a separate log-text grant while preserving schema 19 span grants and receipts; it upgrades supported prior history and retains idempotent attention receipts. Native remote and standalone trace Review; bounded explicit Python 0.2.8 receiver Review and investigations; a gated private receiver cockpit view. Authenticated native read/pagination regressions passed in the separate-account guest. | Final prior-release archive upgrade. Authoritative turn-membership and bounded private output fencing are implemented; adversarial and actual terminal checks passed. Exact OpenCode first-page request preludes are separately authenticated without expanding step membership. |
| Import, launcher and tasks | Durable checkpointed span-JSONL following; typed activity logs; optional installed Python SDK auto instrumentation with cached real SDK witnesses; exact Rust/Python Review-backed task drafts, approval and reveal; task-only backup. Tagged receiver tests cover revocation, immutable prompt authority and fresh Rust Review after positive capture-safety regrant. | Final packaged and SDK checks. The private Python task path retains current owner/member authority through final publication; tagged receiver positive, ordinary-only and changed-second-read cases passed. Final archive rerun remains. |

The joined `42712dd` source passed 813 tests across 96 suites, with none failed or ignored, strict whole-workspace all-target Clippy and an offline build of every workspace binary. Its frozen nine-binary development bundle passed Workflow settings/retry, concurrent retry CAS, protected Finding command/span and native resume terminal checks. See the [candidate qualification record](../validation/rust-039-candidate.md) for the exact executable hash and remaining archive gates. Linux x86-64 and ARM64 archive jobs passed on the PR source. The Rust CI test job exposed Node fixtures counting only structural spans after summary companions were added; those fixtures now verify each typed cohort and paired identities, and their focused local checks pass. The new head still needs CI. No receiver admission rule is weakened to satisfy those fixtures.

The [Python receiver Findings qualification](../validation/rust-python-findings.md) records the subsequent 819-test client/UI follow-up and exact tagged source authority checks.

The default cutover still requires the remaining required workflow differences, final installed paired journeys and supported production acceptance. Do not retire workflows without explicit user approval. Typed native HTTP is not a claim of full Python 0.2.8 wire compatibility.

A real Kiro transcript and an approved production connector endpoint remain external acceptance inputs. Local implementation and qualification continue. macOS D2-Mac is planned but unsupported until each native architecture, installer and cockpit gate passes. Comparative performance benchmarking remains deferred. Public release cadence remains at most one release in any rolling 24 hours; this candidate record does not create a public release or switch the default.

## Remaining work and acceptance

The [separate remaining-work plan](rust-migration-remaining-plan.md) is the detailed acceptance checklist. Its feature, integration and test IDs keep these categories distinct.

| Gate | Verified progress | Remaining work |
| --- | --- | --- |
| Native harnesses | Real Pi, Codex, Claude Code and OpenCode collection; Kiro native-format process fixtures. | FD1: remaining attributable worker/check/turn evidence shapes. Q8: per-provider installed scenarios and an actual Kiro transcript. |
| Cockpit/workflows | Native command center, MCP inventory/trace navigation, telemetry history, usage, Findings, Workflow and selected tasks/imports/retention implemented. | F2: close remaining detailed edit/MCP and SDK/usage coverage inventory; I1–I3 integration; Q6 installed paired visual/keyboard acceptance. |
| Metrics/Host follow-ups | 833-test joined baseline; real metric HTTP/PTY checks; Host correction has 14 focused passing tests. | Final Host correction and Q1–Q3 development checks passed; repeat on the final optimized artifact. Preserve exact source/snapshot, selection and recorded-source labels. |
| Storage authority/recovery | Separate-UID Linux guests exercised broker admission/queries/grants/retention, backup/restore and disclosure boundaries. A qualification host is available; C1–C5 are no longer wholly missing implementation. | Q5 final schema 21 combined-artifact upgrade/recovery; F4 additional connector profiles and Q9 deployed endpoint acceptance; C6/C7 local static-x86-64 isolation and host-mediated transport are already implemented. |
| Extensions/connectors | Explicit pinned catalog/capability loading and trusted connector replay/reconciliation implemented. | FD5 remote package discovery/new formats; FD6 additional connector profiles; C6/C7 local isolated_linux and unix_status_v1 controls are implemented. Keep unsupported profiles/unqualified production activation disabled. |
| Distribution | Published Linux x86-64 and ARM64 Rust 0.3.8; both architectures passed PR #380 bundle CI. | Q5–Q7 final exact source/artifact qualification. F5/Q10 macOS remains unsupported pending native implementation/testing. |
| Full default cutover | Rust native path can run without a Python process. | Required retained workflow inventory, installed paired journeys and applicable production/support gates must pass. No silent retirement or default change. |

## Current blockers and release constraints

- **Local work can continue:** qualify the integrated Host/Metrics and merged Findings path in the final combined bundle and close the remaining feature/coverage inventory. No command approval is pending.
- **External inputs:** an actual Kiro transcript/host, approved production connector endpoint/configuration and native macOS runners. Local fixtures do not close these acceptance gates.
- **Untrusted activation:** local isolated_linux and unix_status_v1 controls are qualified within their supported scope. HTTPS/general provider transport, production credential provisioning, other-architecture sandboxing and deployed acceptance remain open; keep unsupported paths disabled.
- **Release cadence:** Rust 0.3.8 was published 23 November at 06:03:24 UTC. Earliest next public release is 24 November at 06:03:24 UTC / 07:03:24 Berlin, subject to checking for a newer release and passing qualification. Implementation continues between releases.
- **Scope:** macOS is unsupported until D2-Mac passes. Performance benchmarking remains deferred. No automatic legacy Python database import is introduced.
