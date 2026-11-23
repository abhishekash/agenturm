# Core feature development: delegated test plan

Updated 23 November 2026. The user assigned this implementation session to
**core feature development** and a separate agent to test execution. This plan
is the handoff contract. It supplements Q1–Q10 in the
[remaining-work plan](../rfcs/rust-migration-remaining-plan.md).

## Synchronized prior execution and current handoff

The separate test agent's [Q1–Q10 execution report](rust-migration-test-plan-results.md)
uses frozen `18aa702` (PR #381, schema 21). Its 833-test/optimized baseline
passes are preserved and reviewed, but do not qualify the newer CF1–CF5 source
`3135060` (PR #382, schema 22).

Current PR #382 CI stopped before Rust tests/SDK follow-ups on an unused
`ObservationStore` import in a Store test. The synchronization fix removes it;
scoped Store lib/test Clippy and a genuine metric-point identity upgrade
regression pass. The reference Review fixture's `--help` now documents and can
show the required import-path setup without loading the reference first.

For the next broad execution use the corrective PR's merged head, record its
exact SHA and freeze artifacts. Keep this matrix pending until those actual
CF1–CF5 and related Review identity scenarios run. Add an explicit shell loop
(`for script in scripts/*.sh; do bash -n "$script" || exit; done`) rather than
`bash -n scripts/*.sh`, which checks only one input. The separate-UID executor
context and known staging helpers are documented in the results report.

## Ownership and execution

- **Feature developers:** implement the feature slices below; run only focused
  compile/parser/state checks that protect the changed behavior; provide source
  head, exact patch scope, focused result and known limitations.
- **Test execution agent:** own joined workspace/Clippy, actual native collection,
  broad terminal/privacy journeys, schema 21-to-22 receipt-clock upgrade, SDK matrix and artifact/KVM
  qualification. Record executable hashes and results in this plan or linked
  validation records. Return defects with exact scope and reproduction.
- **Integration owner:** combine reviewed changes, publish the exact source head
  for testing and fix returned defects. Do not duplicate the test agent's
  campaigns or call an implemented feature release-qualified prematurely.

The base is merged `18aa7026be1041b93a0b8517874f5b518da44777` (PR #381).
Its existing 833-test and development terminal/KVM results are historical
baselines, not results for the new feature slices. Every new row starts
**not run** until the test agent records evidence against the integrated head.

## Feature handoff board

| ID | Developer scope | Implementation worktree | Test owner/status |
| --- | --- | --- | --- |
| CF1 | Exact native Claude/Pi presence before ordinary activity; existing SessionDiscovered contract. Codex already implements header discovery. | `/tmp/agenturm-native-presence` | Separate test agent; not run on new source. |
| CF2 | Whole-session paged Review edit browser pinned to selected history, source/session, ReviewVersion and snapshot; reuse existing private edit proof. | `/tmp/agenturm-review-session-edits` | Separate test agent; not run on new source. |
| CF3 | Trusted installed-adapter list/show/remove with pin-checked removal and atomic catalog publication. Install already exists. | `/tmp/agenturm-adapter-lifecycle` | Separate test agent; not run on new source. |
| CF4 | Body-free OTLP error/fatal logs in failure investigation, including explicit `--kind log`; existing Findings private log view unchanged. | `/tmp/agenturm-core-development` | Separate test agent; not run on new source. |
| CF5 | Implemented: exact native Claude worker-spawn facts and declared-child metadata in session evidence. | `/tmp/agenturm-native-presence` + `/tmp/agenturm-review-session-edits` | Separate test agent; execute on the supplied integrated source head. |
| CF6 | Implemented: combined deduplicated usage/cost totals that exclude and count reports overlapping at exact `(target source, session)` identity; unattributed telemetry contributes with a disclosed boundary. | Development branch `feature/combined-usage-dedup` | Separate test agent; not run on new source. |
| CF7 | Implemented: cross-host terminal routing through an explicit operator-authored attach-time route file; fixed read-only tmux and `/proc` probes resolve one exact remote pane or report unavailable. | Development branch (see PR) | Separate test agent; not run on new source. |

Worktree paths identify current ownership, not release artifacts. The developer
must supply the final integrated commit before the test owner freezes a binary.
Do not build different sources concurrently into one Cargo target directory.

## CF1 — native session presence

**Developer handoff:** Claude/Pi implementation integrated in the development
branch. Forty-one focused adapter tests and scoped strict Clippy passed. The composed app lib/bin compile check passed with all four slices and schema
22. A focused prefix-race regression and unknown-time Claude checks passed;
25 focused Store upgrade/retention/backup checks passed. These are developer
checks, not a final artifact qualification. These checks do not close the delegated rows below.

**Expected product behavior.** Supported exact native identity can register a
session before ordinary activity. Pi uses its native timestamp; recognized
Claude title/name metadata without a timestamp uses explicit unknown time 0,
never file mtime or a fabricated producer timestamp. Discovery
is neutral recorded evidence; it must not create Working, completed, approval
or current process-liveness claims. Existing native name/project provenance and
newest-updated selection rules continue to apply.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Pi exact session header without messages | One neutral presence fact; correct source/session/native timestamp; roster after commit/reopen. | Not run |
| Claude supported timestamped metadata without activity | One neutral presence fact for exact native session; native names remain separately verified. | Not run |
| Claude snapshot prelude without bound identity | No invented session, identity or execution state. | Not run |
| Missing timestamp on exact Claude title/name metadata | Neutral retained presence with time 0 and Updated Unknown; no execution/liveness or recency claim. | Not run |
| Invalid/null/conflicting timestamp, duplicate/conflicting identity, untimed user stub | Unsupported input remains absent or diagnosed; no wall/mtime fallback. | Not run |
| Managed retention with unknown-time presence | Native time remains 0; schema 22 ages only the exact neutral record by its original batch admission clock; it survives a current default cutoff and expires when that admission age passes the cutoff. | Not run |
| Replay/re-admission/restore with admission aging | Neither exact retry nor same event in a new batch refreshes age; restore preserves admission clock, event digest and unknown native time. Old batches remain default-unknown admission time. | Not run |
| Other zero-time records | Existing diagnostic/activity retirement remains unchanged; no general retention exemption. | Not run |
| Header plus normal facts or relationship | No duplicate session registration/count; ordinary lifecycle remains authoritative. | Not run |
| Poll replay/restart/old cursor | Stable event identity and idempotent admission; only verified original header can backfill. | Not run |
| In-place source rewrite during header backfill | Reject mixed prefix/header proof and preserve prior cursor; repeat existing Codex as well as new Claude/Pi paths. | Not run |
| Guardian/internal-review exclusion and legacy decoder mode | Existing exclusions and legacy wire compatibility retained. | Not run |
| Actual simultaneous Codex and Claude/Pi sessions | Account for selected, queued and absent sources; distinguish actual native witnesses from disposable fixture cases. | Not run |

Exercise cold collection, empty history, reopen, later user/tool/lifecycle
activity and renamed metadata. At 120×32, 80×24 and 60×18 verify human names,
neutral fallback status, recency ordering, selected source/session stability and
correct inspector return. Codex header discovery is a regression, not new work.

## CF2 — complete session edit browsing

**Developer handoff:** implementation integrated. Seventeen focused App/UI
checks and scoped Clippy passed. Independent source review confirmed history,
snapshot, source/turn/worker and current private-grant fences. The composed app
lib/bin compiles with the other core features. No broad terminal campaign ran
in this implementation session.

**Expected product behavior.** Review can find an older supported Codex edit
behind unrelated activity rather than being limited to its loaded overview.
The existing per-turn edit picker and exact `codex.edit_detail` preview continue
to work. Seventeen focused App/UI checks and scoped Clippy passed on the lane
source; final composed source compile and delegated acceptance are separate. A metadata selector is not private-content authority.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Earlier edit beyond initial examined range | Explicit examined/partial coverage, continuation and eventual exact result. Empty matching page can still be partial. | Not run |
| Multiple turns/operations with same labels | Correct source/session/turn/worker/event/revision retained; no sibling edit substitution. | Not run |
| Paging while new writes arrive | Same read snapshot, no shifted/duplicated traversal; stable selection and return. | Not run |
| Review decisions and overlays | Enter/escape through browser/private preview returns to original Review and unchanged decision version. | Not run |
| Local edit reveal | Exact retained-at-snapshot membership, current grant and original source proof required. | Not run |
| Broker edit reveal | Existing verified-edit broker authority retained; no fingerprint/private body exposed as metadata. | Not run |
| Revoked grant, changed source/history, prune/restore | Deny or redact; no stale body publication or silent move to newer evidence. | Not run |
| Unsupported worker/provider/edit shape | Clear bounded unavailable/coverage result; no invented proof. | Not run |

Run actual keyboard journeys at all three terminal sizes. Check query-key
ownership, loading/error/empty states, long paths, terminal restoration and
private output lifetime/ACK fences. Private body stays out of copy/export,
ordinary metadata, diagnostics and saved decision content unless separately
explicitly authorized by the existing workflow.

## CF3 — installed adapter lifecycle

**Developer handoff:** implementation integrated in the development branch.
Four focused management/installation tests and strict app lib/bin Clippy passed.
Independent source review found no publication/pin-CAS compatibility blocker.
The actual packaged command and live collector journeys remain delegated below.

**Expected commands.** `adapter list --root DIR [--json]`,
`adapter show --root DIR --plugin-id ID [--json]`, and
`adapter remove --root DIR --plugin-id ID --sha256 CURRENT [--json]`.
The existing trusted local install/replace flow remains compatible.

| Scenario | Required observation | Status |
| --- | --- | --- |
| List/show populated valid private root | Exact catalog identities/pins and deterministic machine-readable output; no executable starts. | Not run |
| Absent root or entry; invalid flags/ID | Useful error and exit status; reads never create directories/catalogs. | Not run |
| Correct removal pin | Only selected entry unregisters atomically; unrelated entry fields remain unchanged. | Not run |
| Stale pin during replacement | Compare-and-swap fails under lock; newly installed pin remains registered. | Not run |
| Last entry removal | Valid empty versioned catalog, with defined list/show/collector behavior. | Not run |
| Failure before catalog publication or held lock | Original catalog remains usable; no partial publication. | Not run |
| Symlink/ownership/mode/path substitution | Reuse existing private-root/path/pin validation; no traversal or unsafe read/write. | Not run |
| Artifact/history/grant preservation | Removal retains artifacts and all unrelated history/content approvals; no garbage collection or service mutation. | Not run |

After install, remove, list and reinstall, explicitly select the supported
adapter in collection and prove changed/missing pins fail closed. This is
trusted local lifecycle management, not a remote downloader or sandbox claim.

## CF4 — OTLP log failure investigation

**Developer handoff:** implementation complete in the accompanying source.
One focused process regression passed, exercising ordinary error/fatal logs,
explicit activity-error flags, source/session separation, no body/source
reference output, kind filters and cursor/query binding. This is a development
check only; the delegated acceptance rows below remain not run.

**Expected product behavior.** `investigate failures` and its cockpit overlay
include retained ordinary OTLP error/fatal log metadata and activity logs that
explicitly report an error. `--kind log` selects only these log candidates.
Public rows carry closed labels and exact recorded metadata, never log bodies.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Ordinary ERROR/FATAL severity with/without trace | Correct closed error/fatal result and optional exact trace/span IDs; source-only records remain discoverable. | Not run |
| Activity log reported_error true/false | Only explicitly reported errors match; normal usage/cost/activity logs remain absent. | Not run |
| Source/session/turn/worker filters | Exact scope retained; source-only log not attributed to a selected session without evidence. | Not run |
| `--kind log`, another kind, changed cursor filter | Typed kind filtering and query/snapshot-bound cursor denial; no cross-query continuation. | Not run |
| Empty page followed by older error log | Examined/matched counts and continuation remain honest; no false exhausted scope. | Not run |
| Authenticated/broker allowlist and restart | No hidden-source metadata; durable results after reopen. | Not run |
| Captured text, unsafe severity label or secret body | Ordinary output/JSON/copy/export has no body or producer-supplied severity label; no new private source authority. | Not run |
| Shared UI/CLI behavior | Body-free label, exact occurrence and prior selection return; existing failed command source route unchanged. | Not run |

Do not merge evaluation outcome inventory into this test's claim. Evaluation
revisions are a separate current-outcome contract and need their own design if
later added to unified failure investigation.

## CF5 — exact Claude worker-spawn evidence

This feature is integrated with CF1–CF4 in the accompanying development source.
Developer checks passed for three Claude decoder cases, the Runtime contract,
two Store round-trip/projection cases and the scoped presentation case; scoped
Clippy passed on the native/kernel lane. These are not installed qualification. The contract is a closed
`worker_spawn { child_worker_id }` fact; record identity names the exact
spawning parent source/session/optional worker/turn/operation. A child-worker
declaration does not claim a separate session or live process.

Native proof must correlate assistant Agent/Task declaration UUID and tool-use
ID to one non-error user result with exact `sourceToolAssistantUUID`, same
session/worker/turn and structured completed status/agentId. Only five actual
historical completed records were verified initially. Async statuses, text,
message ancestry and directory placement are not positive proof.

Delegate: correct parent/child through commit/reopen; duplicate/replay;
sibling worker and same-session different turn; conflicting/reused tool-use ID;
wrong declaring assistant UUID; malformed/self child; non-completed/error/
multiple results; bounded checkpoint overflow and old cursor continuation;
exact declared-child Activity/inspector details and page coverage. Verify no
prompt/tool-body copy and no roster recency/hierarchy changes. All delegated scenarios
remain **not run** until the test owner records actual results on the frozen head.

## CF6 — combined deduplicated usage totals

Implemented with the FD3 combined slice. The combined per-UTC-day projection
excludes reports whose overlap is established through exact
`(target source, session)` identity and counts the exclusions instead of
summing them: native reports are excluded when same-day usage-bearing telemetry
claims their exact source and session, and attributed telemetry rows are
excluded when same-day native reports exist for the claimed identity.
Unattributed telemetry carries no identity link and therefore contributes;
that boundary is disclosed, not inferred away. Combined costs merge the
per-trace-deduplicated and log-only cohorts per currency; native assistant
cost records that forks can duplicate keep `native_origin` with no span
identity and stay outside combined totals.

**Developer handoff:** six focused Store usage tests (exact-identity overlap
exclusion, unattributed contribution, snapshot-pinned combined equality,
revision correction, and older-receiver pages without the combined block),
76 UI lib tests with the new default Combined chart mode, joined workspace
all-target strict Clippy and fmt passed on the development branch. These are
developer checks, not installed qualification.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Native plus attributed telemetry at one source/session/day | Both sides excluded and counted; combined tokens exclude both; all cohort views unchanged. | Not run |
| Attributed telemetry without same-day native rows for the claimed identity | Contributes its deduplicated tokens; no exclusion is recorded. | Not run |
| Unattributed telemetry and native-only days | Combined equals the nonduplicated evidence; no speculative cross-layer deduction or hidden merging. | Not run |
| Combined costs | Trace and log-only costs merged per currency; fork-copyable native-origin records excluded; distinct currencies never converted. | Not run |
| Terminal disclosure at 120×32, 80×24 and 60×18 | Combined mode renders first; `n` reaches excluded-overlapping counts and per-currency combined cost; `s` returns to the unchanged native/span/trace/log views. | Not run |
| Snapshot, revision and retention | A pinned snapshot keeps its combined block; a native revision correction reclassifies only at the newer watermark; retention/scan flags keep marking partial days. | Not run |
| Older attached-receiver page | A usage page without the combined block deserializes with the block absent; every existing view keeps working. | Not run |
| Scan/cohort limits | Days beyond trace-cohort or raw-scan bounds keep their existing partial flags; combined never claims coverage the cohort did not read. | Not run |

All CF6 scenarios are **not run** until the test owner records actual results
on the frozen integrated head.

## CF7 — cross-host terminal routing

Implemented with the FD4 routing slice. An attached native cockpit accepts an
explicit operator-authored `--route-file` (one command word per line, for
example `ssh` and `user@receiver-host`); the legacy Python receiver protocol
rejects it. `l` verifies the exact current request, then runs three fixed,
read-only, bounded probes through the route: a remote `/proc` snapshot whose
matching processes are proved by exact byte comparison of the resume command
tail (NUL-count, basename and length all checked), the tmux pane inventory,
and a pane-process re-verification. The unique resolved target is presented as
an attach command; Agenturm never selects a remote pane, answers, or approves.

**Developer handoff:** eight focused routing tests including a fake-route
end-to-end that emulates `ssh` argument joining — unique resolution, ambiguous
snapshot rejection, stale pane rejection, base64 round-trip, route-file
validation — plus the attach-option parse test, workspace all-target strict
Clippy and fmt. These are developer checks against synthetic routes, not a
qualification of any real transport.

| Scenario | Required observation | Status |
| --- | --- | --- |
| Attached cockpit with a real route to a receiver host | One unique verified pane resolves; the presented attach command opens the exact recorded owner's terminal. | Not run |
| Route semantics | A route that does not join arguments the way `ssh` does fails closed with unavailable, never a guessed target. | Not run |
| Ambiguous, stale or missing remote state | Two candidate panes, a changed pane process between probes, or no matching process report unavailable with the request selection preserved. | Not run |
| Worker/unsupported provider/missing configuration | Worker requests, non-Codex/Claude/Kiro sources, unsafe session identifiers and absent route files fail closed with exact reasons. | Not run |
| Probe hygiene | Probes are read-only, bounded in output and time, carry no session bytes in script text, and leave no temporary files behind on the receiver host. | Not run |
| Return selection at all three sizes | 120×32, 80×24 and 60×18 preserve the Workflow selection after routing results and errors. | Not run |
| Python receiver protocol | `--route-file` with `--receiver-protocol python-v0.2.8` is rejected at parse time. | Not run |

All CF7 scenarios are **not run** until the test owner records actual results
on the frozen integrated head.

## Related Review identity fixes — delegated regression

Typed broker Review receipts now pin the authenticated history identity for
`--format` and directory export as well as linked traces. In the existing
separate-UID fixture, export one exact turn as JSON, verify its identity,
snapshot and user occurrence, then draft a task from the file. Restore into a
new incarnation and reapprove the prompt: old receipt must reject without
excerpt bytes; a fresh receipt must draft. Repeat directory export and confirm
unformatted plain Review retains its existing output contract. **Not run.**

Local Review marks and decision notes now open their ledger against the selected
logical history and snapshot/incarnation. Replace the database path between
selection and mutation, including equal numeric receipt counts: deny before
creating a sidecar or saving a note against the replacement. Repeat after a
same-path incarnation change. Existing broker workspace-marker behavior remains
separate. Two focused Store regressions passed; actual UI/restore regression is
**not run** and belongs to the test owner.

## Joined and release checks delegated unchanged

After the integrated head is supplied, execute relevant Q1–Q10 from the
[remaining-work plan](../rfcs/rust-migration-remaining-plan.md): joined Rust
checks, actual producer/SDK interoperability, Linux x86-64/ARM64, current exact
artifact install/upgrade/recovery, paired Python-quality UI journeys and
separate-UID privacy/authority checks. Real Kiro, production endpoints and
native macOS retain their external dependencies; do not fabricate a pass.

No public release/default switch is requested by this development handoff.
The one-release-per-rolling-24-hours rule and unsupported-profile limits remain.

## Results and defect handoff template

For each execution record: feature/test ID; source commit; executable/archive
SHA-256; OS/architecture; native producer/version or fixture; terminal size;
expected and actual result; PASS/FAIL/BLOCKED; exact log/artifact path. Do not
combine results from different source heads into one qualified candidate.

For each defect return: minimal reproduction, exact owner/source/session scope,
last expected state, failed invariant, and affected feature paths. Developers
fix that defect and return the new source head. The test owner decides the
necessary regression breadth; developers do not restart the entire campaign.
