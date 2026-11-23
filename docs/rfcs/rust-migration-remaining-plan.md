# Rust migration: remaining development and validation

Updated 23 November 2026. This is the detailed remaining-work plan for the
[Rust execution plan](rust-only-execution-plan.md) and
[task board](rust-only-execution-tasks.md). It separates unfinished features,
completed code awaiting integration, and tests still needed for acceptance.
The migration is **not complete**, and the default distribution has not switched.

## Development and test ownership — current user direction

This implementation session owns core feature development. A separate agent
owns broad testing and release qualification. The
[delegated core-feature test plan](../validation/rust-core-feature-test-plan.md)
tracks CF1 Claude/Pi presence, CF2 paged session edit browsing, CF3 adapter
list/show/pin-CAS removal and CF4 body-free OTLP log failure investigation.
CF1/CF3/CF4 are implemented; CF2 source is integrated with focused checks,
and the CF1 unknown-time admission/retention refinement is implemented in
schema 22. The composed app lib/bin compile check passed. CF5 exact Claude
worker-spawn evidence is also implemented. Typed broker receipt and local
Review-ledger identity fixes accompany these features. Broad qualification
remains delegated to the other test agent.
Developers run focused compile/regression checks only. The test agent executes
Q1–Q10 and the new acceptance matrix on the supplied integrated source head.

Codex header discovery and trusted local adapter installation already exist;
these lanes extend uncovered provider presence and lifecycle operations rather
than replacing implemented features. C6/C7 local connector boundaries remain
implemented within their documented profile. Broad tests are delegated, not
removed from acceptance.

## 1. Current progress and evidence

These results belong to different source revisions. A passing earlier bundle
does not qualify later changes. Test totals below are successive baselines,
not quantities to add together. No percentage is reported because the remaining
workflow inventory and acceptance scope are not yet closed.

| Slice | Implementation and integration | Verified progress | Still needed |
| --- | --- | --- | --- |
| Native cockpit, telemetry, Workflow and protected evidence | [PR #379](https://github.com/abhishekash/agenturm/pull/379) merged as `52ec13be1a4caf8b11bec37638affeedf1f56dc3`. | Runtime source `42712dd` passed **813 tests across 96 suites**, strict workspace all-target Clippy and all-binary build. Its optimized x86-64 archive passed install, prior-release upgrade and **12 separate-UID Linux KVM cases**. Linux x86-64/ARM64 and selected SDK CI passed. | Requalify the final combined artifact after the subsequent Findings, Host and Metrics changes. |
| Command-center E1–E5 | Native title/nickname/project provenance, concise fallbacks and newest-updated ordering are implemented in the merged native batch. | Twelve actual Codex transcript handles across two processes passed exact source/session/name/project and return checks at **120×32, 80×24 and 60×18**. Historical native Claude passed separately; rename/collision cases used disposable fixtures. | Final installed rerun and actual simultaneous Claude acceptance. Header-only native files still lack roster rows until session facts arrive; retained lifecycle does not establish OS process liveness. |
| Explicit Python receiver Findings | Implemented in [PR #380](https://github.com/abhishekash/agenturm/pull/380), qualified head `ef3d647309e8cd81ac32ba0ecd3ea0ad14f2c2f0`; now merged as `39474ddabfca3bc448cf8f8a0b71fda42b9560f1`. | **819 tests across 96 suites**, strict Clippy and all binaries. Actual tagged Python 0.2.8 positive and adversarial Findings journeys passed on development and optimized apps; optimized Review regression passed. All applicable CI jobs are green, including Python 3.11–3.14, Node 22/24, OTLP, five selected SDK jobs and both Linux bundle jobs. Release-attachment job skipped as expected. | Merged; include it in final combined-package qualification. |
| Metric descriptions and full-catalog search | Implemented in the joined source accompanying this plan. | Joined Host/Metrics baseline passed **833 tests across 96 suites**, strict Clippy and all binaries. Authenticated JSON/protobuf HTTP proved descriptor replay and unchanged old point records. Real terminal search found a series beyond the first 32 rows, displayed its description and preserved snapshot/selection at all three sizes. | Merged PR #381; test owner qualifies schema 21 and final installed behavior. |
| Remote Host inventory and reported host metadata | Latest-per-authorized-source inventory, hostname/platform metadata and recorded-source labels implemented. Final chooser refresh/context correction is integrated in the joined source. | Baseline included in the 833-test run. Correction passed **14 focused App/UI tests**, joined **833 tests / 96 suites**, strict workspace Clippy and all binaries. Updated separate-UID Host KVM/three-size terminal fixture passed. | Final optimized installed-artifact regression. The corrected typed-POST error assertion passed in the updated KVM fixture. |
| Full cutover and additional platforms | Gated. Rust 0.3.8 is the latest public opt-in Linux release. | Native Linux release and development evidence exists; no macOS qualification is claimed. | Close required feature and installed-journey gaps below. Production endpoint and actual Kiro transcript remain external inputs. |

The [joined Host/Metrics qualification](../validation/rust-host-metric-metadata.md) records the final source, HTTP, terminal and KVM checks.

The detailed source records for the merged batch are
[candidate qualification](https://github.com/abhishekash/agenturm/blob/52ec13be1a4caf8b11bec37638affeedf1f56dc3/docs/validation/rust-039-candidate.md),
[actual-session roster](https://github.com/abhishekash/agenturm/blob/52ec13be1a4caf8b11bec37638affeedf1f56dc3/docs/validation/rust-real-session-roster.md)
and the subsequent
[Findings qualification](https://github.com/abhishekash/agenturm/blob/ef3d647309e8cd81ac32ba0ecd3ea0ad14f2c2f0/docs/validation/rust-python-findings.md).

## Test execution synchronization — 23 November 2026

The separate test agent ran Q1–Q10 on **`18aa702` / PR #381 / schema 21**.
That baseline predates the five CF1–CF5 feature slices merged in PR #382 as
`3135060`. The [consolidated results and review](../validation/rust-migration-test-plan-results.md)
preserve exact source, artifact and interpreter evidence. Do not credit the
baseline's 833 tests to the newer feature source.

| Gate | Verified on baseline `18aa702` | Current feature source and follow-up |
| --- | --- | --- |
| Q1 | 833 tests / 96 suites, Clippy/fmt/build and Python syntax pass. The recorded shell glob checked only its first script. | PR #382 Rust CI stopped on an unused test import. Fixed in sync follow-up; scoped Store test Clippy passes. Whole current Rust run and per-script shell syntax remain with test owner. |
| Q2 | Optimized HTTP/three-size catalog journeys pass. Existing upgrade assertion used generic Activity rather than a metric point. | New real-MetricPoint v20/read-only/v22 full-row identity regression passes. Current optimized metric/schema 22 acceptance still needs execution. |
| Q3 | Executor blocked on separate-UID guest boundary; earlier source-specific development Host proof exists separately. | Exact current artifact Host/authorization/restore guest acceptance remains pending. Existing local KVM staging helpers are recorded in the results. |
| Q4 | Debug/optimized under Python 3.12, and optimized reference suites under 3.11–3.14 pass. | New current-source and Review identity regressions remain delegated. Fixture help/import-path usability fixed and help checked. |
| Q5 | Exact optimized archive checksum, foreground/reopen/upgrade, installer and retained-history uninstall pass. | Managed service/restore and exact current archive/12 guest cases are not certified by that run. |
| Q6 | Named MCP, analytics/usage, Workflow and private reveal/return journeys pass with recorded widths. | Full paired state matrix, Host, CF1–CF5 and Review identity cases remain; denial widths must retain their exact evidence. |
| Q7 | PR #381 and its tested merge are green under the documented event conditions. | Latest Rust job needs retry after lint fix. Dependent SDK jobs skipped on PR #382 are not passes. |
| Q8 | Four named harness fixtures plus actual roster audit and disposable rename/restart checks pass. | Exact current per-provider/native concurrency and Kiro acceptance remain open. |
| Q9/Q10 | Production endpoint and macOS execution were blocked. | External dependencies and support gates remain unchanged. |

The synchronized [core test handoff](../validation/rust-core-feature-test-plan.md)
assigns all broader execution to the other agent. This development session
performed only the narrow corrective regression, Store lint and fixture help
checks. No release/default switch or complete migration claim follows.

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

## 2. Feature development still open

These items concern product behavior or platform support. Passing existing
tests cannot close them. Unknown native evidence must remain unknown; do not
invent check results, parent relationships, current liveness or cost totals.

### F1. Complete native harness evidence and roster coverage

**Existing tasks:** A1–A6 and the remaining E1/E3 coverage cases. **Owner:** harness adapters and shared identity contracts.

- Pi: the native fork and explicit shell-check paths are implemented and have
  real-source evidence. Same-session worker relationships remain open.
- Codex: structured failures, interruptions, explicit completions, delegation
  and recognized direct checks are covered. Complete check coverage and the
  remaining native turn shapes still require an explicit coverage inventory.
- Claude Code: exact worker/session identity and completion are supported.
  Same-session worker-spawn evidence is now a separate typed fact (CF5),
  correlated to exact native Agent/Task declaration and completed result, with
  parent worker/turn/operation and a bounded child ID. Distinct-session
  relationships remain a separate contract; async/ambiguous native shapes
  remain unsupported pending positive evidence.
  Sampled Bash results lack a native exit field; output text alone must not
  become a passed/failed check.
- OpenCode: idle-boundary attribution, parent/fork identities and direct checks
  with explicit exits are implemented. Preserve session-only scope and unknown
  outcomes where boundaries or exits are absent; qualify further native shapes
  rather than treating every missing field as a decoder bug.
- Supported header-only presence is implemented: existing Codex discovery,
  plus CF1 Claude/Pi openings and recognized timestamp-less Claude title/name
  metadata with explicit unknown time 0. Schema 22 uses original admission age
  for retention without inventing source timestamps. This is completed code
  awaiting installed acceptance; unsupported native shapes remain FD1 evidence
  work, and no open handle alone establishes Working or process liveness.
- Kiro: native-format process fixtures cover failure, delegation and fractional
  credits. An actual transcript is required to determine any further decoder
  changes; obtaining it is a validation dependency, not evidence of a known
  missing implementation.

**Development complete when:** the provider matrix identifies supported and
unsupported shapes, attributable missing shapes have typed implementations,
and ambiguous cases remain explicit. Land shared contracts before parallel
adapter changes. Installed acceptance is a separate Q8 test-owner gate.

### F2. Close the remaining required Python 0.2.8 workflow differences

**Existing tasks:** B1, B3–B6 and D3. **Owner:** workflow/cockpit maintainer.

Review every retained workflow against the implemented Rust command and exact
keyboard journey. Much of this work is already implemented: MCP inventory and
linked traces, metric history/exemplars, seven-day usage, Findings source views,
Workflow settings/retry/resume, retention, imports, tasks and scoped private
Review. Do not reopen these as entirely absent features.

Keep the following unresolved coverage explicit:

1. Remaining provider/shape edit evidence beyond Codex. Whole-session paged
   Codex Review edit browsing is implemented in CF2. The exact Codex `e`
   Review/edit preview already supports paths, bounded diffs or contents under
   `codex.edit_detail`, with version-linked decision notes. Preserve those
   implemented guarantees while completing unsupported source/shape cases.
2. Source-specific MCP payload detail beyond the implemented current-grant,
   exact Codex MCP preview and linked-trace paths,
   including exact operation association, missing data and revoked access.
3. SDK-specific automatic instrumentation coverage. Five selected SDK CI jobs
   pass; that does not establish every Python 0.2.8 producer/instrumentation
   combination. Reconcile OpenAI, Anthropic, LangChain/LangGraph, Google ADK
   and Pydantic AI inventory with the actual selected jobs and launch paths.
4. Remaining usage/cost aggregation differences where overlapping producers
   cannot be safely combined. The combined view now deduplicates reports whose
   overlap is established through exact `(target source, session)` identity and
   discloses the exclusions, with fork-copyable native cost records kept out
   of combined totals. Overlap that no producer identity links — for example
   two unattributed instrumentation layers describing the same call — cannot be
   detected; those totals stay unreported rather than summed speculatively.
   Preserve distinct evidence cohorts and currencies in every view.
5. FD4 cross-host terminal routing is a confirmed missing feature. Other CLI,
   SDK and keyboard differences belong to AP1 until a concrete missing code
   path is identified. Visual-state comparisons of implemented paths remain
   qualification with the separate test agent.

**Development complete when:** each confirmed missing code path from AP1 has
its implementation and scope controls, or an explicit product decision records
its disposition. Installed/paired acceptance remains a separate test-owner
gate. No workflow is silently retired. Native approval evidence must retain its exact source and
default-off internal-review visibility; an internal review is not an approval.

### F3. Local adapter lifecycle complete; remote extension work remains

**Existing task:** B8. **Owner:** extension runtime.

Trusted local manifest installation, expected-pin verification, atomic catalog
publication, explicit selection/replacement and independent capability loading
already exist. CF3 now implements installed-adapter list/show and pin-CAS removal; four
focused developer checks passed, with actual installed acceptance delegated. Remote
automatic package discovery/download and new decoder formats remain separate;
do not reimplement the existing supported local installer. Changed artifacts and unsupported formats
must fail closed. Do not equate a checksum with an OS sandbox.

**Current development status:** the supported local manifest install,
replacement, inspection and removal flow is implemented. Installed validation
is delegated. FD5 covers additional remote package/format features; unsupported
untrusted activation still depends on its defined trust/isolation profile.
Keep these independent extension claims separate from Linux qualification.

### F4. Connector profiles beyond the qualified local boundary

**Existing tasks:** C6–C8. **Owner:** connector runtime/security boundary.

C6 and C7 are **implemented and locally qualified** for their documented scope:
`isolated_linux` runs pinned static Linux x86-64 connectors using staged inode
verification, Landlock, seccomp and descriptor isolation, with fail-closed setup.
The child has framed I/O and cannot create files, sockets or processes.
`unix_status_v1` is a host-mediated outbound transport. Its grant binds endpoint,
peer UID/GID, private credential reference, artifact and policy epoch; it checks
peer/inode before opening the private token and rechecks authority before write.
Bounded status operations, redacted receipts and same-key reconciliation pass.
These controls must not be reassigned as wholly missing features.

**Remaining feature scope:** HTTPS/general provider transport, production
credential provisioning and sandbox support on other architectures remain
unimplemented. Implement the profiles actually selected for production; do not
claim arbitrary outbound permissions or portable isolation from the local proof.
Existing C6/C7 behavior needs deployed acceptance, not reinvention.

**Development complete when:** the selected new transport, credential and
architecture profiles are implemented under the declared least-privilege
contract. Deployed acceptance is separately owned by Q9. An approved
production endpoint/configuration is still needed. Keep unsupported profiles
and unqualified production activation disabled. See the
[connector contract and profile limits](rust-connector-api.md).

### F5. macOS distribution and integration

**Existing task:** D2-Mac. **Owner:** platform/distribution.

Provide Apple Silicon and Intel artifacts separately when native runners are
available. Define minimum macOS version, bundle/install/update/uninstall flows,
configuration/state paths, supported harness roots and foreground versus
`launchd` collection. Remove Linux path, `/proc`, process-control and systemd
assumptions from the advertised macOS path. Define signing/notarization and
normal Gatekeeper behavior.

**Development complete when:** the selected macOS artifact, path, installer,
service and signing configuration is implemented for each intended target.
Native acceptance is separately owned by Q10. macOS remains unsupported until
that gate passes; it is separate from Linux release acceptance.

## 3. Implemented code awaiting integration

These are integration tasks, not a claim that the features are absent.

| ID | Work and owner | Sequence | Completion condition |
| --- | --- | --- | --- |
| I1 | **Complete:** Findings PR #380; integration owner | Exact `ef3d647` head and green checks verified; admin squash merged. | Remote MERGED state verified; main commit `39474ddabfca3bc448cf8f8a0b71fda42b9560f1`. Final joined artifact tests remain separate. |
| I2 | **Merged PR #381 (`18aa702`):** Metrics and remote Host; integration owner | Final Host runtime/fixture corrections integrated with existing Findings and Metrics changes. | 833 tests / 96 suites, strict Clippy/all binaries, actual metric HTTP/three-size PTY and separate-UID Host KVM passed. Final optimized archive remains Q5. |
| I3 | Source/plans merged in PR #381; integration owner | Supply each subsequent feature head to the separate test agent. | Current plan, task board, paired journey record, schema/support notes and HTML companions name the same source/artifact. Current-head CI passes. Earlier artifact evidence remains labeled with its original revision. |

The integrated Host correction was handed off at
`/tmp/agenturm-host-mode-runtime.patch` and
`/tmp/agenturm-host-mode-fixture.patch`. These local paths are working-session
handoffs, not durable release artifacts. The final correction has now also passed the joined **833-test / 96-suite**
run, strict all-target workspace Clippy and all binaries. The earlier 14
focused tests are a subset, not an additional aggregate count.

## 4. Tests and qualification still to complete

These checks verify implemented behavior. If a check exposes missing behavior,
record the defect under feature development or integration, fix it, and rerun
the affected acceptance. Do not mark a test complete because a fixture exists.

### Q1. Final joined workspace

**Depends on:** I1/I2 source integration. Run offline locked workspace tests,
strict all-target workspace Clippy and all workspace binaries. Record exact
source head and counts; zero failed or ignored tests. Validate fixture syntax
and diff hygiene. The final Host patch now passes the full 833-test baseline as well as
strict all-target workspace Clippy and all binaries. Record final artifact
qualification separately from this source check.

### Q2. Metric metadata, catalog and schema compatibility

**Depends on:** joined metric source and final artifact.

- Repeat authenticated OTLP JSON/protobuf ingestion, descriptor-only admission,
  exact replay, revision handling and omission of unsafe description text.
- Upgrade supported prior schemas through **21** metric metadata and **22**
  original admission-clock retention, preserving both sets of invariants. Preserve old point IDs, payloads,
  digests, grants and receipts; description companions must not rewrite points.
- Prove filtering occurs before pagination and source authorization before
  grouping/limits. Find an older series beyond the first 32 catalog rows.
- Pin search, source scope and snapshot through paging, description inspection,
  points/exemplar drilldown and return. Exercise blank reset and canceled/failed
  queries without losing the committed selection.
- Record compatibility limits: existing point wire format/cursors remain
  stable; older source-scoped catalog cursors require a fresh catalog page.

The current joined development app already passed real HTTP and all three terminal
sizes; the updated Host guest fixture also passed on its frozen app/service. Repeat on the final optimized installed artifact, not just unit models.

### Q3. Recorded remote Host inventory and chooser

**Depends on:** I2. The updated separate-UID KVM fixture passed on the frozen
joined development bundle at all three terminal sizes. Rerun on the final
optimized archive. Use fresh settled terminal cells, not cumulative output that
can match a prior screen.

Verify local versus remote labels, reported hostname/platform, unreported
metadata, exact source/time/age and stale/unknown handling. A remote read must
not probe the viewer's `/proc` or trigger local capture. Include more than 64
samples for one source plus another source, source allowlist exclusion,
Host-disabled responses and restored-store binding denial.

In the multistream chooser, select the older stream, publish a newer sample
for it, refresh, verify identity through reorder, enter its process list and
return to the same choice. Check boot/PID/start-time identity. A typed history POST with a stale snapshot/incarnation binding must assert
HTTP 200 with the exact `snapshot_replaced` result error. A metadata REST GET
with a stale binding header uses HTTP 409. The previous POST expectation of
409 was a fixture defect; do not change runtime semantics to match it.

### Q4. Explicit receiver Findings and Review regression

**Depends on:** final joined app. The actual immutable Python 0.2.8 reference
commit is `2802d79930728bb331db1d4d718d6dea1c9c374d`.

Retain the already passing development/optimized positive journeys and faults:
exact log/evaluation source, source-only evidence, current policy, owner/member
substitution, cache removal, changed source between reads, unknown/sensitive
fields, same-URL receiver replacement, revocation while visible and terminal
backpressure before publication. Verify no private clipboard/export and exact
selection return. Review must count exact verified step members separately
from the authenticated first-page request prelude.

Rerun affected shared UI/DTO paths on the final artifact. Report the actual
guarantee: bounded, repeatedly checked private display; no remote server-held
authorization transaction or immutable historical evaluation claim.

### Q5. Final Linux archive, install, upgrade and recovery

**Depends on:** Q1 and coherent committed source.

Build a new immutable optimized archive and record its checksum and all nine
executable hashes. On a clean supported host, verify Python-free startup,
discovery, collection, service restart, query, inspector, follow collection,
backup/restore and uninstall with retained history. Upgrade from the published
0.3.8 archive, covering history, cursors, grants, retention and workspace state.
Use the offline installer and qualified support matrix.

Rerun all 12 existing separate-UID guest cases against this exact archive:
native private content, telemetry payloads, private evaluation details, Host
history, remote history, remote Host, evaluation inventory, broker span/usage,
native attention, execution clocks, Finding span payload and log-text broker.
Add Q3's new Host inventory cases. Every case and the aggregate must pass;
successful VM shutdown alone is insufficient.

### Q6. Installed paired UI and workflow journeys

**Depends on:** final extracted artifact. Compare required Python 0.2.8 journeys
and the selected Agenturm command-center design at 120×32, 80×24 and 60×18.

Cover named concurrent sessions and recency; Activity/Review/edit evidence;
MCP inventory/filter/payload/trace return; Traces; metric catalog/history/
exemplars; seven-day usage/cost coverage; Findings source/revocation; Host;
Workflow settings, snooze, exact failed-attempt retry, notification and verified
resume; saved tasks, drafts, decisions and handoff. Include populated, empty,
loading, unavailable/error and narrow states. Verify key ownership, focus,
stable selection, Escape returns and terminal restoration.

Use current recorded evidence for counts and statuses. Explain intentional
layout/shortcut differences; resolve unaccepted visual/interaction gaps before
claiming Python-quality parity. Theme implementation alone is not acceptance.

### Q7. Current-head Linux and SDK CI

**Depends on:** pushed final source. Require native x86-64 and ARM64 bundle
qualification, workspace Rust checks and the five selected SDK jobs. Record
exact job/head results and producer versions. Keep Python/Node interoperability
checks green where retained. PR #380 is fully green now; that status does not
automatically qualify a later Host/Metrics commit.

### Q8. Actual per-harness installed acceptance

**Depends on:** corresponding F1 slice, independently by provider. Verify exact
session/worker/turn attribution, recorded reason, missing evidence, later
activity, restart, view switching and terminal restoration. Exercise native
records as well as explicit fixtures and label them separately.

Actual simultaneous Claude concurrency also remains unverified on the inspected
host; historical Claude and the actual 12-handle Codex cohort are distinct
evidence. Header-only roster behavior is F1 development, not a test-only item.

An actual Kiro transcript/host is unavailable. Keep A4 open until one can be
discovered, collected, reopened and inspected; fixtures alone cannot pass it.

### Q9. Production connector acceptance

**Depends on:** F4 and an approved endpoint/configuration. Prove child filesystem
denial, least credentials/destinations, authorized delivery, denial after
revocation, ambiguous-send reconciliation with the same idempotency key, and
inbound commit-before-ACK. Record deployed identities and configuration without
secrets. Local fake endpoints cannot close this production gate.

### Q10. Native macOS acceptance

**Depends on:** F5 and native runners. For each advertised architecture, install
the checksum-identified artifact without Python or Rust, verify Gatekeeper,
discover supported harness data, collect/restart/query/inspect/backup/upgrade,
and uninstall with history retained. Test advertised `launchd` management
separately. Run normal/narrow cockpit journeys and terminal restoration. Record
minimum OS, architecture, terminal and unsupported harness/process features.

## 5. Execution order and parallel ownership

| Lane | Work that can proceed independently | Join point |
| --- | --- | --- |
| Integration coordinator | I1–I3, exact source/artifact records and Linux packaging | Serialize shared protocol/schema changes and Git integration; Q1 before freezing final artifact. |
| Harness lane | F1 provider slices, actual-source collection and Q8 | Shared worker contract first if needed; each provider can qualify independently. |
| UI/workflow lane | F2 inventory closure and remaining narrow/source journeys | Coordinate shared UI files with Host correction; Q6 uses final joined artifact. |
| Boundary/platform lane | F3/F4 trust contracts; F5 design and available native-runner work | Untrusted activation waits for boundary proof; production and macOS each require their own external inputs. |

Prioritize final Host integration, joined checks, current-source CI and the
combined Linux artifact first. Run HTTP/PTY and guest checks in parallel where
resources and file ownership permit. Do not duplicate builds in one target
directory or install/download tools unnecessarily.

## 6. Completion and release gates

**Feature development complete:** core FD1–FD4 and confirmed AP1 gaps are closed with
evidence and explicit scope decisions. FD5–FD7 retain their own delivery/support
gates; do not advertise them complete merely because Linux ships.

**Linux candidate qualified:** integrated source passes Q1–Q8 as applicable to
its declared support, with external/unsupported cases explicitly recorded.
Earlier native-only qualification is a baseline, not the final combined result.

**Full default cutover complete:** the retained required workflow inventory,
installed paired journeys and applicable production/support gates all pass.
No silent retirement of remote APIs, imports, retention, cost/Host views or
other selected workflows. Rust owns its runtime/data; no automatic import of
the legacy Python database is introduced by this plan.

**Public release:** no more than one in any rolling 24 hours. The last release,
[Rust 0.3.8](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.8),
was published **23 November 2026 at 06:03:24 UTC**. The earliest next slot is
**24 November at 06:03:24 UTC / 07:03:24 Berlin**, subject to rechecking newer
releases and completing qualification. This document creates no release/tag
and does not switch the default. Comparative performance benchmarks remain
deferred; no measured performance gain is claimed.
