# Rust-only execution tasks

Progress updated 23 November 2026 against merged native PR #379 (`52ec13b`), Findings PR #380 (`ef3d647`) and the joined Host/Metrics development baseline. See the [detailed remaining-work plan](rust-migration-remaining-plan.md): **F = feature development, I = integration, Q = tests/qualification**. The status column below supersedes old scheduling labels such as “Start now”; dependencies and original acceptance criteria remain for traceability.

- **Merged native baseline:** 813 tests / 96 suites, strict Clippy, exact optimized archive install/upgrade and 12 separate-UID KVM cases passed.
- **Findings follow-up:** 819 tests / 96 suites, actual tagged development/optimized journeys and all applicable PR CI passed; PR #380 merged as `39474dd`; final joined-artifact regression remains.
- **Host/Metrics joined baseline:** 833 tests / 96 suites, strict Clippy and all binaries; real metric HTTP and three-size terminal checks passed. The final Host refresh/context correction passed its 14 focused tests and the joined 833-test / 96-suite run with strict workspace Clippy/all binaries; the updated separate-UID Host KVM/three-size fixture also passed. Final optimized artifact qualification remains.
- **Full migration:** incomplete. Do not add successive test totals or equate implemented code with final artifact acceptance. Default cutover and one-release-per-24-hours rules remain in force.

## Core feature development and delegated testing

Current user direction: this implementation session owns feature development;
a separate agent owns broad test execution. Follow the
[core feature test handoff](../validation/rust-core-feature-test-plan.md).

| Feature lane | Development status | Testing responsibility |
| --- | --- | --- |
| CF1 Claude/Pi neutral native presence | In progress; Codex header discovery already exists. | Separate test agent: actual collection/replay/roster journeys. |
| CF2 Whole-session paged Review edits | In progress; exact existing Codex preview reused with history/snapshot/ReviewVersion binding. | Separate test agent: terminal/private/broker and selection-return acceptance. |
| CF3 Trusted adapter list/show/remove | In progress; local install/replacement already exists; removal uses exact-pin CAS and preserves artifacts. | Separate test agent: CLI/atomicity/security/collection regression. |
| CF4 OTLP log failure investigation | Implemented; one focused process regression passed. Metadata only, explicit log kind, scope/cursor binding retained. | Separate test agent: broad failure inventory/remote/terminal privacy regression. |

Feature developers run focused checks needed for implementation. Q1–Q10 and
release qualification remain delegated; no broad campaign is duplicated here.

## Review of the existing plan

| Plan area | Scheduling decision |
| --- | --- |
| Kernel, plugin boundary and Linux installation (steps 1–2) | Keep independent adapter admission, malformed-batch rejection, isolation of adapter failures, restart and durable reads in regression coverage. The Linux release evidence supersedes the early open discovery and service milestones. |
| Capabilities and panes (step 3) | The inspector already runs an independent capability. The default Linux cockpit also reads explicitly enabled entries from the pinned user catalog. Exercise that entry point on the packaged build. Automatic decoder installation is a separate feature. [Cockpit configuration](https://github.com/abhishekash/agenturm/blob/14be284/docs/rfcs/rust-linux-bundle.md) |
| Evidence and cockpit (step 3) | The five adapters have different native evidence. Schedule the missing cases by provider. The published Linux archive collected real Codex, Pi, Claude Code and OpenCode data. Kiro still needs an actual transcript. [Harness matrix](https://github.com/abhishekash/agenturm/blob/14be284/docs/validation/rust-harness-evidence-coverage.md) |
| Storage, release and connectors (steps 4–6) | Trusted Linux recovery, published Rust 0.3.8 Linux x86-64/ARM64, separate-principal KVM broker checks and trusted connector replay/reconciliation are baselines. The static x86-64 isolated_linux / unix_status_v1 controls are locally qualified; additional profiles, deployed production acceptance, the final combined artifact and macOS remain open. |
| Default distribution | Shipping a standalone archive and switching the default entry point are separate decisions. Compare user workflows to Python 0.2.8. Keep the Rust runtime and fresh data directory independent of Python and its database. [Cockpit workflow comparison](https://github.com/abhishekash/agenturm/blob/14be284/docs/rfcs/rust-cockpit-workflow-coverage.md) |

## Task board

Each ID retains its original acceptance requirement. Assign only unfinished work using the current status and F/I/Q plan. Completed baseline work needs final-artifact regression where indicated, not a new implementation from scratch. Record source/artifact digest, platform, scenario and result for each acceptance run.

### A. Native harness evidence

| ID | Current status | Task | Depends on | Done when |
| --- | --- | --- | --- | --- |
| A1 | Partial — native fork/checks verified; same-session worker relationships open (F1/Q8). | Pi worker relationships and check outcomes | Start now | Native records establish which parent IDs and explicit check outcomes are available. Supported cases survive collect and reopen. Missing evidence stays unknown. |
| A2 | Partial — structured lifecycle/delegation/direct checks verified; remaining shapes and full check coverage open (F1/Q8). | Codex lifecycle and check gaps | Start now | Previously uncovered native turn or check shapes have positive and ambiguous examples. A later turn or successful PR creation cannot overwrite the recorded ending of an earlier turn. |
| A3 | Partial — exact worker/session and completion verified; attributable parent/check evidence open (F1). | Claude Code parentage and check outcomes | Start now | Verify whether native records identify the immediate parent and check exit. Persist only attributable facts and cover the worker-with-session-only case. |
| A4 | External input — native-format fixtures pass; real Kiro transcript unavailable (Q8). | Kiro actual-session qualification | Start now. Actual run needs a Kiro host with a native transcript. | The installed bundle discovers a real session, commits it, restarts and displays its recorded turn and activity. Record which relationship, credit and check shapes were present. Keep fixture-only cases identified. |
| A5 | Implemented supported shapes — native idle boundaries/relationships/direct checks verified; final installed coverage Q8. | OpenCode turn boundaries and activity | Start now | A native SQLite sample demonstrates attribution after an explicit idle boundary. Missing boundaries, missing exits and ambiguous outcomes retain their documented scope after reopen. |
| A6 | Partially qualified — provider checks exist; final installed per-provider acceptance remains (Q8). | Verify the installed cockpit per harness | After the corresponding A1–A5 task, independently per provider | Exercise selection, recorded reason, missing evidence, later activity, view switching and terminal restoration at 60×18. Update that provider's matrix row with the actual and fixture evidence. |

### B. Private evidence, cockpit and cutover scope

| ID | Current status | Task | Depends on | Done when |
| --- | --- | --- | --- | --- |
| B1 | Inventory exists — required differences and cutover scope still need explicit closure (F2). | Inventory remaining user workflows and decide cutover scope | Start now | Map each Python CLI entry point and cockpit workflow to a Rust command, a missing behavior or an explicit retirement decision. Include SDK instrumentation, payload views, usage/cost and host history. List the required Python 0.2.8 keyboard journeys, their expected evidence and navigation results, and the owner of each open decision. |
| B2 | Implemented/verified — older exact retained OpenCode proof path; final artifact regression Q5/Q6. | Extend OpenCode prompt proofs beyond the current sweep | Start now | An older retained SQLite event can be revealed under its current grant after a later sweep and reopen. Changed source bytes, expired evidence, source replacement, revocation and restore all deny reveal. |
| B3 | Partial — exact Codex e/edit preview, codex.edit_detail and version-linked notes implemented; other source/shape coverage F2/Q6. | Add detailed edit evidence to Review | Start now with a source that provides edit evidence | A recorded edit appears on the correct run and evidence version. Existing decision notes survive reopen. Content-bearing detail follows the applicable grant. |
| B4 | Partial — current-grant exact Codex MCP preview/trace paths implemented; remaining source-specific detail F2/Q6. | Add scoped MCP payload detail | Start now with existing Codex MCP metadata | Opening a supported operation shows only authorized detail for that operation. Missing payload, mismatched identity and revoked access are covered. |
| B5 | Selected jobs green — five SDK jobs passed on ef3d647; broader producer inventory F2 and final-head Q7. | Implement required SDK instrumentation | B1 selects SDKs. Parallel per SDK. | Each selected SDK can emit a real trace through the documented launch path and reopen a span linked to the correct source and session. |
| B6 | Selected commands implemented — remaining workflow inventory F2 and installed Q5/Q6. | Close remaining required CLI workflows | B1. Parallel per independent command. | Each retained workflow has a Rust entry point and an installed end-to-end scenario. Record accepted retirements and any unresolved user-facing differences. |
| B7 | Implemented/verified native baseline — packaged final combined artifact rerun Q5. | Qualify default cockpit capability loading | Start now | Launch the packaged bare cockpit with an explicitly enabled independent capability. Observe its pane and metric, then verify disabled and mismatched-pin cases. |
| B8 | Local trusted install/replace already implemented; CF3 list/show/pin-CAS removal in progress. Remote package discovery and new formats remain separate. | Define and implement automatic decoder installation | Start design now. Activation depends on the approved trust model. | An independent supported decoder can be installed, selected and verified through the declared format and pin. Unsupported formats or changed artifacts fail closed. Keep untrusted activation behind its isolation gate. |

### C. Storage authority and connector isolation

| ID | Current status | Task | Depends on | Done when |
| --- | --- | --- | --- | --- |
| C1 | Implemented baseline — broker operations and caller roles exercised in distinct-UID KVM. | Specify broker operations and caller permissions | Start now | Map admission, read/query, grants, retention and maintenance to the existing storage contracts. Specify principal ownership, authorized callers, request replay and failure behavior. |
| C2 | Available/verified — Linux KVM supplies distinct broker/application identities; old host limitation superseded. | Obtain a Linux qualification host | Start now | Demonstrate separate broker and application identities and verify filesystem permissions under both. Separate-UID Linux KVM now supplies this setup; retain it in final archive acceptance. |
| C3 | Implemented/verified baseline — broker admission/replay; final artifact Q5. | Implement broker admission and replay | C1 | The collector commits observations and checkpoint through the broker before ACK. Retry on either side of commit recovers one durable result. |
| C4 | Implemented/verified baseline — scoped reads/grants/retention; final artifact Q5. | Implement broker queries, grant checks and retention | C1. Parallel with C3 on the agreed contract. | Scoped reads preserve watermarks, revoked grants deny access and retention preserves cursor and receipt semantics. |
| C5 | Implemented/verified baseline — separate-UID broker/client denial/recovery; final combined archive Q5. | Route installed clients through the distinct-principal broker | C2–C4 | Collector and cockpit work through the broker while direct database access by the client identity is denied. Repeat concurrent writer, kill/restart, backup, restore and fresh-grant checks. |
| C6 | Implemented/locally qualified — static x86-64 isolated_linux boundary; additional profiles F4 and deployed acceptance Q9 remain. | Implement connector process isolation | Start now | A supervised child demonstrably cannot read the store or unrelated user files. Failure to establish the configured boundary prevents launch. |
| C7 | Implemented/locally qualified — host-mediated unix_status_v1 grants/credentials/endpoints; other transports/provisioning F4 and production Q9 remain. | Implement connector credential and egress controls | Start now. Agree on the launch interface with C6. | Only the granted credential and destination are usable. Revocation fences subsequent work, and receipts and diagnostics omit secrets. |
| C8 | External/dependent — approved endpoint plus F4 boundary proof required (Q9). | Qualify a supported production connector | C6 and C7, access to a supported endpoint, and proof that the deployed storage boundary denies child access | Under isolation, verify authorized delivery, denial after revocation and reconciliation after an ambiguous send with the same key. Repeat inbound commit-before-ACK for an inbound connector. Enable only the qualified configuration. |

### D. Release and distribution

| ID | Current status | Task | Depends on | Done when |
| --- | --- | --- | --- | --- |
| D1 | Native archive qualified — final Host/Metrics/Findings combined archive not yet qualified (Q5/Q6). | Qualify the integrated Ubuntu x86-64 candidate | After the changes selected for that candidate | The checksum-identified archive passes Python-free startup, discovery, collection, service restart, query, inspection, upgrade from the previous supported Rust release, backup and uninstall with history retained. Include B7 and applicable new acceptance cases. |
| D2 | ARM64 native CI qualified on ef3d647 — final joined head/artifact Q7; additional OS targets separate. | Qualify one additional platform or architecture | Start now if a target and runner are selected. Parallel per target. | The exact target artifact passes install, collect, restart, query, inspect, upgrade and uninstall under its claimed service manager. Add support only for combinations that pass. |
| D2-Mac | Open — implementation F5 and native per-architecture qualification Q10; unsupported. | Qualify macOS release artifacts and installer | Start when native macOS runner(s) are available; qualify Apple Silicon and Intel as separate targets. | For each advertised architecture, a checksum-identified artifact installs and launches without Python or Rust, discovers and collects supported local harnesses, survives restart and upgrade, supports query/inspect/backup, and uninstalls while retaining history. Define minimum macOS version, data/config paths, foreground or `launchd` support, and Gatekeeper signing/notarization. Run required cockpit journeys at normal and narrow terminal sizes and verify terminal restoration. Do not advertise macOS until every claimed combination passes. |
| D3 | Gated — required feature inventory and final installed paired journeys not yet closed. | Switch the default distribution entry point | B1 decisions, all workflows designated required there, and D1 for the intended release | A clean default install starts Rust and uses its own data directory. Compare the B1 keyboard journeys on Python 0.2.8 and the installed Rust entry point at normal width and 60×18. Record evidence, focus, return selection and intentional shortcut or layout differences. Capture and compare the Python and Rust screens for the selected journeys, including populated, loading, unavailable and narrow states. Resolve unaccepted visual and interaction differences. All required journeys pass. Publish the verified support matrix and install instructions. |

## Pending issue — running sessions missing from the command center

Historical diagnosis recorded from the user's Rust 0.3.8 screenshot. Later authorization superseded the original diagnosis-only restriction. E1–E5 have an implemented, checked native baseline; header-only roster behavior, actual simultaneous Claude coverage and final combined-artifact acceptance remain open. The findings below describe the published source at diagnosis time, not the current native implementation.

**Confirmed findings from the published source**

- Default discovery selects four newest-modified transcript files per harness, within a 16-source global limit. It uses recent file modification time without an active-session inventory. A running session outside the selected files can be omitted from collection.
- The roster reads sessions from committed history. Working/Ready reflect retained lifecycle evidence. They do not establish current process liveness.
- Session titles use an explicitly linked saved-task title, then structural activity such as MCP tool names or “Recorded file edit”. A missing title displays “Untitled session”. Native conversation titles are absent from this path.
- Project grouping reads explicitly saved task workspace roots. Native session working directories are absent from this path, explaining “Project unavailable” for ordinary sessions without a saved link.
- Explicit database selection opens history without starting the cockpit collector. Failed automatic discovery can also fall back to saved history with a notice.

The screenshot proves that the displayed roster and labels fail the user's expected running-session view. It does not establish which discovery limit, database selection or collection failure excluded a particular session. The locally inspected default history contains 334 distinct sessions and has no observations newer than 22 November 14:56 UTC. Its identity has not been matched to the screenshot.

| ID | Current status | Pending task | Dependencies and parallel work | Acceptance |
| --- | --- | --- | --- | --- |
| E1 | Partial — Codex actual cohort passed; header-only roster behavior F1, simultaneous Claude and installed Q6 remain. | Diagnose the selected history and cover running sessions in discovery | Implemented; retain runtime diagnosis/coverage criteria for Q6 installed regression. | Record the actual executable/version, command, selected database, collector ownership and coverage notice. Compare expected session IDs against discovered files, selected adapters, committed cursors and roster rows. Exercise more than four concurrent sessions, idle-but-running sessions, source turnover, cold catch-up, restart and intentionally excluded review files. Show collection coverage and distinguish recorded lifecycle from current liveness. |
| E2 | Implemented/actual native naming/context checked — final installed rerun Q6. | Show exact native session titles and project context | Implemented; final joined-artifact native metadata checks Q6. | Preserve exact source/session identity and provenance when native display titles and project paths are available. Keep authorized content handling explicit. Ordinary native sessions display useful names and project groups. Missing or conflicting context has a visible fallback. Activity summaries remain separately identifiable. |
| E3 | Partial — actual Codex cohort at all three sizes; historical Claude separate; simultaneous Claude and final installed Q6 remain. | Qualify the command center against actual concurrent agent sessions | Actual-transcript checks passed after E1/E2; final installed Q6 remains. | Run the installed cockpit against actual simultaneous sessions, compare exact identities and project groups with the native inventory, verify lifecycle changes and selection after refresh/restart, and capture 120×32, 80×24 and 60×18 screens. Account for every omitted session and show partial coverage. |

**E2 human-readable naming acceptance**

The command center must identify work by a human-readable task or conversation name. Show the native conversation title or an explicitly saved task name as the primary label, with the agent name/provider and project as supporting context. Session IDs are available in inspection details and copy actions.

Keep explicit agent nicknames and roles distinct from task titles. Reflect native renames while preserving selection and exact links. Disambiguate duplicate titles through agent/project context. When a title is unavailable, show an honest readable fallback and make the missing name apparent. Generic activity descriptions such as “MCP · send_message_to_thread” or “Recorded file edit” do not satisfy named-session acceptance.

Verify the first screen with real named Codex and Claude Code sessions, including renamed sessions, duplicate names, subagents and missing names. A technically correct session-ID match alone does not complete E2.

**E2 native metadata sources verified locally**

- Codex stores `id`, `title`, `name`, `cwd`, `agent_nickname`, `agent_role` and `agent_path` in `~/.codex/state_5.sqlite`'s `threads` table. Its `session_index.jsonl` contains `id`, `thread_name` and `updated_at`. Match on the exact native thread ID and verify field availability for the installed version.
- Claude Code transcripts contain explicit `custom-title` records with `customTitle`, `agent-name` records with `agentName`, and `sessionId`. Native rows can supply `cwd` and `slug`. These record shapes were found in local transcripts. Use the latest applicable explicit name within the same session.
- Keep task title, agent nickname, provider and model as separate fields. A display name can change without changing session identity. Define source precedence, missing-field handling and metadata disclosure before implementing the projection.

Source anchors are `rust/agenturm-app/src/discovery.rs`, `cockpit_startup.rs`, `session_titles.rs`, `session_projects.rs`, `load_session_catalog` in `lib.rs`, and `Filter::accepts` in `rust/agenturm-ui/src/workspace.rs`, as inspected in the Rust 0.3.8 integration worktree.

## Command-center corrections from the latest screenshot

These requirements were added after native naming was introduced. E4/E5 are now implemented and tested in the native batch; retain the acceptance criteria for final installed regression.

| ID | Current status | Task | Parallel work | Acceptance |
| --- | --- | --- | --- | --- |
| E4 | Implemented/tested — exact title, nickname then concise fallback; final installed rerun Q6. | Use available human names throughout the roster | Can proceed independently of E5. | Use the task/conversation title when present. Otherwise promote the native agent nickname to the primary label, so the screenshot's rows read Nash, Zeno and Mencius. When neither exists, use concise provider/role/project context such as “Codex session”. Remove repeated “Name unavailable” labels. Keep missing metadata and exact IDs in inspector details. Verify titled sessions, nickname-only subagents, duplicates and genuinely unnamed sessions. |
| E5 | Implemented/tested — newest-updated group/row order and stable selection; final installed rerun Q6. | Always order the roster by latest update | Can proceed independently of E4. | Sort newest updates first. Order project groups by their newest member, and sessions within each group by descending update time. Status, alphabetical group labels, insertion order and selected-row position must not override recency. Hierarchy presentation must preserve this ordering requirement. Use deterministic ties and preserve the selected session identity as rows move. Verify initial load, live refresh, grouping/filter changes, pagination and restart. Show Updated ↓ and remove the status-sort toggle from the command center. |

## Integration order

- **Prioritize unfinished slices:** I1–I3 integration and Q1–Q7 final Linux qualification; F1/F2 remaining harness/workflow gaps can proceed independently. C1–C5 and E1–E5 have implemented/verified baselines and must not be reassigned as wholly missing. A real Kiro transcript and approved production endpoint remain external inputs; the distinct-principal Linux test host is already available.
- **Integrate by interface.** Provider changes can land separately. If multiple slices need a new shared observation type, land the format, decoder and store support first, then enable adapter emission. Coordinate schema revisions for B2–B4 and broker work. Keep terminal rendering in the UI and provider parsing in the adapters.
- **Close gates independently.** Run A6 as each provider is ready. Join C3 and C4 at C5, and C6 and C7 at C8. For the default switch, finish B1, its required workflow slices and D1, in that order. Extra platforms, automatic decoder installation and production connectors have independent acceptance gates unless explicitly included in the release scope.
