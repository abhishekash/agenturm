# Rust-only execution tasks

Reviewed against the [execution plan](https://github.com/abhishekash/agenturm/blob/14be2845e4c7520ecbe9d77cac2f087b9b81fb2b/docs/rfcs/rust-only-execution-plan.md) at `14be284`. The immediate work can start on four fronts in parallel. The decision about the default distribution depends on which workflows must survive cutover. Untrusted connectors have their own security gate.

## Review of the existing plan

| Plan area | Scheduling decision |
| --- | --- |
| Kernel, plugin boundary and Linux installation (steps 1–2) | Keep independent adapter admission, malformed-batch rejection, isolation of adapter failures, restart and durable reads in regression coverage. The Linux release evidence supersedes the early open discovery and service milestones. |
| Capabilities and panes (step 3) | The inspector already runs an independent capability. The default Linux cockpit also reads explicitly enabled entries from the pinned user catalog. Exercise that entry point on the packaged build. Automatic decoder installation is a separate feature. [Cockpit configuration](https://github.com/abhishekash/agenturm/blob/14be284/docs/rfcs/rust-linux-bundle.md) |
| Evidence and cockpit (step 3) | The five adapters have different native evidence. Schedule the missing cases by provider. The published Linux archive collected real Codex, Pi, Claude Code and OpenCode data. Kiro still needs an actual transcript. [Harness matrix](https://github.com/abhishekash/agenturm/blob/14be284/docs/validation/rust-harness-evidence-coverage.md) |
| Storage, release and connectors (steps 4–6) | Trusted Linux recovery, the published Ubuntu x86-64 Rust 0.3.1 archive, and pinned trusted connector replay/reconciliation are baselines. A distinct storage principal, untrusted connector controls, and each additional platform require separate proof. |
| Default distribution | Shipping a standalone archive and switching the default entry point are separate decisions. Compare user workflows to Python 0.2.8. Keep the Rust runtime and fresh data directory independent of Python and its database. [Cockpit workflow comparison](https://github.com/abhishekash/agenturm/blob/14be284/docs/rfcs/rust-cockpit-workflow-coverage.md) |

## Current integration status — 23 November 2026

The tables below preserve the original task definitions and diagnosis. Later authorization started implementation. E1–E3 discovery/native metadata work is integrated in the Linux candidate; E4/E5 name fallback and recency ordering are implemented with focused UI tests. The final archive and actual-session rerun remain gates. D2-Mac is planned and unsupported until each advertised native architecture passes. See the [current execution plan](rust-only-execution-plan.md) and [paired cockpit audit](../validation/rust-ui-journey-parity.md) for completed versus pending journeys. No workflow has been retired.

## Task board

Each ID is an independently assignable task. **Start now** means the task can run in parallel with every other start-now task. Dependencies apply to completion unless a narrower prerequisite is named. Record the commit or archive digest, platform, scenario and result for each acceptance run.

### A. Native harness evidence

| ID | Task | Depends on | Done when |
| --- | --- | --- | --- |
| A1 | Pi worker relationships and check outcomes | Start now | Native records establish which parent IDs and explicit check outcomes are available. Supported cases survive collect and reopen. Missing evidence stays unknown. |
| A2 | Codex lifecycle and check gaps | Start now | Previously uncovered native turn or check shapes have positive and ambiguous examples. A later turn or successful PR creation cannot overwrite the recorded ending of an earlier turn. |
| A3 | Claude Code parentage and check outcomes | Start now | Verify whether native records identify the immediate parent and check exit. Persist only attributable facts and cover the worker-with-session-only case. |
| A4 | Kiro actual-session qualification | Start now. Actual run needs a Kiro host with a native transcript. | The installed bundle discovers a real session, commits it, restarts and displays its recorded turn and activity. Record which relationship, credit and check shapes were present. Keep fixture-only cases identified. |
| A5 | OpenCode turn boundaries and activity | Start now | A native SQLite sample demonstrates attribution after an explicit idle boundary. Missing boundaries, missing exits and ambiguous outcomes retain their documented scope after reopen. |
| A6 | Verify the installed cockpit per harness | After the corresponding A1–A5 task, independently per provider | Exercise selection, recorded reason, missing evidence, later activity, view switching and terminal restoration at 60×18. Update that provider's matrix row with the actual and fixture evidence. |

### B. Private evidence, cockpit and cutover scope

| ID | Task | Depends on | Done when |
| --- | --- | --- | --- |
| B1 | Inventory remaining user workflows and decide cutover scope | Start now | Map each Python CLI entry point and cockpit workflow to a Rust command, a missing behavior or an explicit retirement decision. Include SDK instrumentation, payload views, usage/cost and host history. List the required Python 0.2.8 keyboard journeys, their expected evidence and navigation results, and the owner of each open decision. |
| B2 | Extend OpenCode prompt proofs beyond the current sweep | Start now | An older retained SQLite event can be revealed under its current grant after a later sweep and reopen. Changed source bytes, expired evidence, source replacement, revocation and restore all deny reveal. |
| B3 | Add detailed edit evidence to Review | Start now with a source that provides edit evidence | A recorded edit appears on the correct run and evidence version. Existing decision notes survive reopen. Content-bearing detail follows the applicable grant. |
| B4 | Add scoped MCP payload detail | Start now with existing Codex MCP metadata | Opening a supported operation shows only authorized detail for that operation. Missing payload, mismatched identity and revoked access are covered. |
| B5 | Implement required SDK instrumentation | B1 selects SDKs. Parallel per SDK. | Each selected SDK can emit a real trace through the documented launch path and reopen a span linked to the correct source and session. |
| B6 | Close remaining required CLI workflows | B1. Parallel per independent command. | Each retained workflow has a Rust entry point and an installed end-to-end scenario. Record accepted retirements and any unresolved user-facing differences. |
| B7 | Qualify default cockpit capability loading | Start now | Launch the packaged bare cockpit with an explicitly enabled independent capability. Observe its pane and metric, then verify disabled and mismatched-pin cases. |
| B8 | Define and implement automatic decoder installation | Start design now. Activation depends on the approved trust model. | An independent supported decoder can be installed, selected and verified through the declared format and pin. Unsupported formats or changed artifacts fail closed. Keep untrusted activation behind its isolation gate. |

### C. Storage authority and connector isolation

| ID | Task | Depends on | Done when |
| --- | --- | --- | --- |
| C1 | Specify broker operations and caller permissions | Start now | Map admission, read/query, grants, retention and maintenance to the existing storage contracts. Specify principal ownership, authorized callers, request replay and failure behavior. |
| C2 | Obtain a Linux qualification host | Start now | Demonstrate separate broker and application identities and verify filesystem permissions under both. The recorded test host could not supply this setup. |
| C3 | Implement broker admission and replay | C1 | The collector commits observations and checkpoint through the broker before ACK. Retry on either side of commit recovers one durable result. |
| C4 | Implement broker queries, grant checks and retention | C1. Parallel with C3 on the agreed contract. | Scoped reads preserve watermarks, revoked grants deny access and retention preserves cursor and receipt semantics. |
| C5 | Route installed clients through the distinct-principal broker | C2–C4 | Collector and cockpit work through the broker while direct database access by the client identity is denied. Repeat concurrent writer, kill/restart, backup, restore and fresh-grant checks. |
| C6 | Implement connector process isolation | Start now | A supervised child demonstrably cannot read the store or unrelated user files. Failure to establish the configured boundary prevents launch. |
| C7 | Implement connector credential and egress controls | Start now. Agree on the launch interface with C6. | Only the granted credential and destination are usable. Revocation fences subsequent work, and receipts and diagnostics omit secrets. |
| C8 | Qualify a supported production connector | C6 and C7, access to a supported endpoint, and proof that the deployed storage boundary denies child access | Under isolation, verify authorized delivery, denial after revocation and reconciliation after an ambiguous send with the same key. Repeat inbound commit-before-ACK for an inbound connector. Enable only the qualified configuration. |

### D. Release and distribution

| ID | Task | Depends on | Done when |
| --- | --- | --- | --- |
| D1 | Qualify the integrated Ubuntu x86-64 candidate | After the changes selected for that candidate | The checksum-identified archive passes Python-free startup, discovery, collection, service restart, query, inspection, upgrade from the previous supported Rust release, backup and uninstall with history retained. Include B7 and applicable new acceptance cases. |
| D2 | Qualify one additional platform or architecture | Start now if a target and runner are selected. Parallel per target. | The exact target artifact passes install, collect, restart, query, inspect, upgrade and uninstall under its claimed service manager. Add support only for combinations that pass. |
| D2-Mac | Qualify macOS release artifacts and installer | Start when native macOS runner(s) are available; qualify Apple Silicon and Intel as separate targets. | For each advertised architecture, a checksum-identified artifact installs and launches without Python or Rust, discovers and collects supported local harnesses, survives restart and upgrade, supports query/inspect/backup, and uninstalls while retaining history. Define minimum macOS version, data/config paths, foreground or `launchd` support, and Gatekeeper signing/notarization. Run required cockpit journeys at normal and narrow terminal sizes and verify terminal restoration. Do not advertise macOS until every claimed combination passes. |
| D3 | Switch the default distribution entry point | B1 decisions, all workflows designated required there, and D1 for the intended release | A clean default install starts Rust and uses its own data directory. Compare the B1 keyboard journeys on Python 0.2.8 and the installed Rust entry point at normal width and 60×18. Record evidence, focus, return selection and intentional shortcut or layout differences. Capture and compare the Python and Rust screens for the selected journeys, including populated, loading, unavailable and narrow states. Resolve unaccepted visual and interaction differences. All required journeys pass. Publish the verified support matrix and install instructions. |

## Pending issue — running sessions missing from the command center

Recorded 23 November 2026 from the user's Rust 0.3.8 screenshot. Diagnosis and backlog only. Implementation is deferred at the user's direction.

**Confirmed findings from the published source**

- Default discovery selects four newest-modified transcript files per harness, within a 16-source global limit. It uses recent file modification time without an active-session inventory. A running session outside the selected files can be omitted from collection.
- The roster reads sessions from committed history. Working/Ready reflect retained lifecycle evidence. They do not establish current process liveness.
- Session titles use an explicitly linked saved-task title, then structural activity such as MCP tool names or “Recorded file edit”. A missing title displays “Untitled session”. Native conversation titles are absent from this path.
- Project grouping reads explicitly saved task workspace roots. Native session working directories are absent from this path, explaining “Project unavailable” for ordinary sessions without a saved link.
- Explicit database selection opens history without starting the cockpit collector. Failed automatic discovery can also fall back to saved history with a notice.

The screenshot proves that the displayed roster and labels fail the user's expected running-session view. It does not establish which discovery limit, database selection or collection failure excluded a particular session. The locally inspected default history contains 334 distinct sessions and has no observations newer than 22 November 14:56 UTC. Its identity has not been matched to the screenshot.

| ID | Pending task | Dependencies and parallel work | Acceptance |
| --- | --- | --- | --- |
| E1 | Diagnose the selected history and cover running sessions in discovery | Pending. Runtime diagnosis first. Can proceed alongside E2 after implementation is authorized. | Record the actual executable/version, command, selected database, collector ownership and coverage notice. Compare expected session IDs against discovered files, selected adapters, committed cursors and roster rows. Exercise more than four concurrent sessions, idle-but-running sessions, source turnover, cold catch-up, restart and intentionally excluded review files. Show collection coverage and distinguish recorded lifecycle from current liveness. |
| E2 | Show exact native session titles and project context | Pending. Independent of E1 collection changes, with agreed metadata contracts. | Preserve exact source/session identity and provenance when native display titles and project paths are available. Keep authorized content handling explicit. Ordinary native sessions display useful names and project groups. Missing or conflicting context has a visible fallback. Activity summaries remain separately identifiable. |
| E3 | Qualify the command center against actual concurrent agent sessions | Pending. After E1 and E2. | Run the installed cockpit against actual simultaneous sessions, compare exact identities and project groups with the native inventory, verify lifecycle changes and selection after refresh/restart, and capture 120×32, 80×24 and 60×18 screens. Account for every omitted session and show partial coverage. |

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

These requirements were added after native naming was introduced. They remain pending implementation and validation.

| ID | Task | Parallel work | Acceptance |
| --- | --- | --- | --- |
| E4 | Use available human names throughout the roster | Can proceed independently of E5. | Use the task/conversation title when present. Otherwise promote the native agent nickname to the primary label, so the screenshot's rows read Nash, Zeno and Mencius. When neither exists, use concise provider/role/project context such as “Codex session”. Remove repeated “Name unavailable” labels. Keep missing metadata and exact IDs in inspector details. Verify titled sessions, nickname-only subagents, duplicates and genuinely unnamed sessions. |
| E5 | Always order the roster by latest update | Can proceed independently of E4. | Sort newest updates first. Order project groups by their newest member, and sessions within each group by descending update time. Status, alphabetical group labels, insertion order and selected-row position must not override recency. Hierarchy presentation must preserve this ordering requirement. Use deterministic ties and preserve the selected session identity as rows move. Verify initial load, live refresh, grouping/filter changes, pagination and restart. Show Updated ↓ and remove the status-sort toggle from the command center. |

## Integration order

- **Assign immediately** to adapter owners (A1–A5), cockpit and workflow owners (B1–B4, B7 and B8 design), storage owners (C1–C2), and connector owners (C6–C7). Acquiring a real Kiro transcript and a separate-principal host early removes two external waits.
- **Integrate by interface.** Provider changes can land separately. If multiple slices need a new shared observation type, land the format, decoder and store support first, then enable adapter emission. Coordinate schema revisions for B2–B4 and broker work. Keep terminal rendering in the UI and provider parsing in the adapters.
- **Close gates independently.** Run A6 as each provider is ready. Join C3 and C4 at C5, and C6 and C7 at C8. For the default switch, finish B1, its required workflow slices and D1, in that order. Extra platforms, automatic decoder installation and production connectors have independent acceptance gates unless explicitly included in the release scope.
