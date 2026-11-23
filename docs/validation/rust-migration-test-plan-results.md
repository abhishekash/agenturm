# Rust migration remaining test plan — execution results

23 November 2026. This is the single consolidated record for executing
[the remaining test plan](../rfcs/rust-migration-remaining-plan.md) section 4
(Q1–Q10). It contains both the results and every test/feature follow-up found.

- **Source under test:** `origin/main` at
  `18aa7026be1041b93a0b8517874f5b518da44777`
  (`Complete recorded Host inventory and retained metric catalog metadata (#381)`).
- **Worktree/branch:** `/tmp/opencode/agenturm-q-tests`,
  `test/remaining-plan-q1` (detached from `origin/main`, no source changes).
- **Toolchain:** Rust 1.100.0, Linux x86_64, offline locked Cargo cache.
  Python-dependent reference fixtures were rerun under four interpreters —
  **3.11.18, 3.12.3, 3.13.18 and 3.14.10** — against the same optimized binary.
  The upstream CI matrix on the tested head covers 3.11–3.14 and is recorded under Q7.
- **No release, tag, push or default cutover was created.**

## Synchronization review — baseline and current source

Reviewed against merged PR #382, `3135060c79fa5b70ee60ef50e52872a161ec7996`.
The run below used **PR #381 / schema 21**, not the newer five core feature
slices or schema 22. Its passing checks remain valid for that frozen baseline.
They must not be relabeled as acceptance of the current implementation.

| Evidence scope | Review result |
| --- | --- |
| Baseline `18aa702`, optimized App `027f29a…` | Logs reproduce 833 tests / 96 suites and the named HTTP/terminal/reference journeys; all nine executable hashes match the extracted bundle. The archive SHA-256 was independently recomputed and matches the recorded digest. |
| Q1 fixture syntax | Python compileall passed. `bash -n scripts/*.sh` parses only its first file; it does not prove all shell scripts. The test owner must use an explicit per-file loop. |
| Q2 migration identity | The old upgrade test inserts generic Activity, not a MetricPoint. Descriptor-ingestion identity checks within one schema do not close the real point/series identity upgrade case. A dedicated real-point v20/read-only/v22 regression now passes in the synchronization follow-up, comparing full typed records. This closes the narrow assertion gap on the corrective source, not the entire current artifact gate. |
| Q4 interpreter scope | Debug and optimized Findings/Review were checked under 3.12; the four-interpreter table covers the optimized artifact. Do not expand that to debug under every version. |
| Q5/Q6/Q8 limits | No managed service restart/restore or exact optimized guest qualification was demonstrated by this run. Four named bundled harness fixtures were exercised. Reveal/return covers three sizes, while log revocation and passing-eval-correction denial are specifically evidenced at 60×18. |
| Current PR #382 CI | Rust CI stopped at Clippy on an unused test import in `review_ledger.rs:676`; Rust tests and dependent SDK jobs did not run. The synchronization follow-up removes the import and scoped Store lib/test Clippy passes. Current-head CI and CF1–CF5 acceptance remain separate pending evidence. |

[Failed current-source CI job](https://github.com/abhishekash/agenturm/actions/runs/45236649459/job/137910022785).
The source-level changes and independent test-owner acceptance are tracked in
[the delegated core feature test plan](rust-core-feature-test-plan.md).
No new full test campaign is claimed by this review.

## Result summary

| Gate | Result | Basis |
| --- | --- | --- |
| Q1 Final joined workspace | **PASS Rust / PARTIAL shell syntax** | 833 tests / 96 suites, strict Clippy/fmt/build and clean diff; Python syntax checked, full shell-script syntax not established |
| Q2 Metric metadata, catalog and schema | **PASS journeys / PARTIAL migration identity** | JSON/protobuf descriptor journeys and three-size catalog navigation on the optimized archive; schema 20→21 unit coverage |
| Q3 Recorded remote Host inventory | **BLOCKED** | Fixture requires a root guest with separate UIDs; no passwordless sudo and user namespaces are disabled on the test host |
| Q4 Python receiver Findings/Review | **PASS** | Tagged Python 0.2.8 reference, all positive/fault cases and Review oracle, debug and optimized, under Python 3.11/3.12/3.13/3.14 |
| Q5 Linux archive/install/upgrade/recovery | **PARTIAL** | Archive, checksum, nine hashes, 0.3.8 upgrade, installer and uninstall-retained-history pass; 12 separate-UID KVM cases blocked as Q3 |
| Q6 Installed paired UI/workflow journeys | **PARTIAL** | MCP, analytics/usage, Workflow settings/retry, task continuity and investigations pass at the recorded sizes; Host paths blocked; full Python-vs-Rust state matrix not automated |
| Q7 Tested-head Linux and SDK CI | **PASS** | PR #381 head `ebe19a92` fully green; merge commit `18aa702` Rust, Node, OTLP, SDK and Python 3.11–3.14 green; ARM64 passed on the PR head and was skipped on push |
| Q8 Actual per-harness installed acceptance | **PARTIAL** | Four named bundled fixtures collect/reopen; actual roster audit and rename/restart mutation pass; Kiro transcript unavailable and per-provider exact acceptance remains open |
| Q9 Production connector acceptance | **BLOCKED** | No approved production endpoint/configuration |
| Q10 Native macOS acceptance | **BLOCKED** | No macOS runners/artifacts |

No product defect was reproduced on the tested `18aa702` baseline. Every executed check there passed; the later PR #382 Clippy failure is a separate current-source finding. The only
implementation-adjacent gap found is a test-coverage gap in Q2, recorded in
“Follow-up work” below.

## Q1. Baseline Rust workspace — PASS; shell syntax — PARTIAL

- `cargo test --workspace --locked --offline`: **833 passed, 0 failed,
  0 ignored, 96 suites**. This reproduces the joined baseline count on the
  frozen `18aa702` baseline, not the subsequent current main commit.
- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`:
  pass, zero warnings.
- `cargo build --workspace --bins --locked --offline`: all workspace binaries
  built, including the app, plugin host, connector, storage service and all five
  harness adapters.
- Fixture syntax: `python3 -m compileall -q scripts` passed. The recorded
  `bash -n scripts/*.sh` command checks only its first shell input; all-script
  shell syntax validation remains unproven by this command.
- Diff hygiene: clean worktree; `git diff --check` returned zero.
- Log: `/tmp/opencode/q1-tests.log`, `/tmp/opencode/q1-clippy-fmt.log`,
  `/tmp/opencode/q1-binaries.log`.

The Host chooser correction’s focused tests are included in the 833; they are a
subset, not an additional count.

## Q2. Baseline metric journeys — PASS; point-upgrade identity — PARTIAL

Executed on both the debug build and the optimized extracted archive:

- `validate_rust_metric_descriptions.py --app …`: authenticated JSON and
  protobuf ingestion, unauthenticated denial, descriptor-only admission, exact
  replay, revision handling and unsafe description omission. The seven old
  points retained identical IDs, serialized values, digests and receipt
  sequences; description companions never rewrote them.
- `validate_rust_metric_catalog.py --app …`: at **120×32, 80×24 and 60×18**,
  found an older matching series beyond the first 32 catalog rows, opened its
  description, entered exact points, returned to the same series/search/snapshot
  and cleared the filter.
- Schema 20→21: `metric_description_upgrade_preserves_v20_grants_and_receipts`
  passed inside Q1. It drops the description index, marks v20, reopens read-only,
  upgrades and asserts the batch digest, history sequence, event digest, payload
  JSON and content grant are unchanged, and that replayed batches remain replay.
- Optimized app SHA-256: `027f29a15cd871d3dcfbae5b03906c182bb63295a5ce83fc75cb17bce4ed59a7`.
- Logs: `/tmp/opencode/q2-metrics.log`,
  `/tmp/opencode/q2-metrics-optimized.log`.

Compatibility statement confirmed by the fixture: existing point wire format and
cursors are unchanged; older source-scoped catalog cursors need a fresh first
page.

## Q3. Recorded remote Host inventory — BLOCKED

`validate_rust_remote_host_history.py` was not executed. It requires
`os.geteuid() == 0` and allocates UIDs 45000–45999 for broker, collector,
reader, maintainer and viewer. This host has no passwordless `sudo` and
`unshare -r` fails (`/proc/self/uid_map: Operation not permitted`), so the
separate-UID fixture cannot run here.

Static check only (not a pass): the corrected fixture asserts the history restore
as HTTP **200** with the typed `snapshot_replaced` error
(`scripts/validate_rust_remote_host_history.py:320`), while metadata GET with a
stale binding asserts HTTP 409 (lines 239, 305, 313–318). The previous 409
expectation for the restore POST is not present.

The same block applies to the 12 separate-UID Q5 KVM cases. `execution_clocks`,
`native_private_content` and `private_eval_details` also refused to run without
root, consistent with their guest-only contract. They must be run on a
disposable root/KVM guest; nothing in this record contradicts their earlier
passing baseline.

## Q4. Explicit receiver Findings and Review — PASS

Reference: immutable Python 0.2.8 at
`2802d79930728bb331db1d4d718d6dea1c9c374d`.

- `validate_rust_python_findings.py` passed on the debug build and again on the
  optimized archive, using Python 3.12: exact log
  and failed-evaluation source, source-only
  evidence, policy denial before fetch, revocation while visible, same-URL cache
  removal, owner and member substitution, payload drift, unknown and sensitive
  fields, same-URL receiver replacement, terminal backpressure, no private
  clipboard/export, exact selection return and terminal restoration at
  120×32, 80×24 and 60×18.
- `validate_rust_python_receiver_review.py` passed on both: four verified
  step-member events counted separately from the two unspanned first-page user
  messages, with request, assistant, check, usage, file, policy-revocation,
  cursor and bearer assertions.
- Logs: `/tmp/opencode/q4-python-findings.log`,
  `/tmp/opencode/q4-review.log`, `/tmp/opencode/q4-optimized.log`.

Guarantee remains as documented: a bounded, repeatedly checked private display.
No remote server-held authorization transaction and no immutable historical
evaluation claim is made.

### Interpreter matrix

The same four Rust acceptance suites were run one interpreter at a time against
the optimized archive (`027f29a…`). The Python version runs the reference
receiver only; every assertion remains on Rust behavior.

| Interpreter | Findings | Review membership | Task continuity | Investigations |
| --- | --- | --- | --- | --- |
| Python 3.11.18 | PASS | PASS | PASS | PASS |
| Python 3.12.3 | PASS | PASS | PASS | PASS |
| Python 3.13.18 | PASS | PASS | PASS | PASS |
| Python 3.14.10 | PASS | PASS | PASS | PASS |

All runs exited 0 with no traceback and no incompatibility against the pinned
0.2.8 reference. Logs: `/tmp/opencode/py311-*`, `/tmp/opencode/py313-*`,
`/tmp/opencode/py314-*`, plus the earlier 3.12 logs
`/tmp/opencode/q4-optimized.log` and `/tmp/opencode/q6-python-journeys.log`.

## Q5. Linux archive, install, upgrade and recovery — PARTIAL

Built with `scripts/build-rust-linux-bundle.sh` from `18aa702`.

- Archive: `agenturm-0.3.9-linux-x86_64.tar.gz`
- SHA-256:
  `1c5ba29cd726cea42440a9c44d08c9a421d21cb7e9dae06633f71ba9fa1e04d9`

Nine executable hashes:

| Executable | SHA-256 |
| --- | --- |
| `agenturm` | `027f29a15cd871d3dcfbae5b03906c182bb63295a5ce83fc75cb17bce4ed59a7` |
| `agenturm-claude-harness` | `7f4196dea29a852ebe9dc06358486e1997e65605ce3e7043f05d4317ef203070` |
| `agenturm-codex-harness` | `ca87ff23f54f841521743aa99c65e494821d82677286b38a653d3e24e2fe1c50` |
| `agenturm-connector` | `e0a2f1cfc38ed26553a045e3dbe9da316649d7d37788f7bcc0c5cc1a5981c2ee` |
| `agenturm-kiro-harness` | `6122b36580dcd38fdd4398fc48f1a2871c34e9fc7d341ff2c57b6adfcb713822` |
| `agenturm-opencode-harness` | `5cef66045794d05c7102518b596410a23531c3eb5f17b61337d68c701ffd8efe` |
| `agenturm-pi-harness` | `3292f6e2dd3b4736123cb18da43732f1e94a0614f9e086c17f532d2dc9d98c9f` |
| `agenturm-plugin-host` | `807646e23f686fa7cc843072ee20df5230396a61eef97a4396d42e2dbaf6feab` |
| `agenturm-storage-service` | `abd3e0f2a9e2b7a00f2b6b934b89390c214028f88dbc2c2c278d1024ba7f63ac` |

`verify-rust-linux-bundle.sh` passed with the published 0.3.8 x86-64 archive as
`--previous`:

- checksum and archive/executable version agreement;
- Python-free `--version` and startup under an empty `PATH`;
- discovery and collection of Codex and Pi fixtures;
- restart, exact replay and unchanged query output;
- inspector, backup, bare-cockpit empty start/collection/writer release;
- follow discovery without restart;
- bundled Claude Code and Kiro fixture discovery, collection and reopen;
- 0.3.8 history and cursor still valid after opening with the 0.3.9 bundle.

`verify-rust-installer.sh` passed: offline install, checksum, five-plugin
catalog hashes, `agenturm-rust` launcher, pre-existing binary preserved, JSONL
import/replay and retained configuration/history after uninstall. Forbidden
`sudo`, `systemctl`, `curl`, Python and network commands were never invoked.

0.3.8 prior archive checksum:
`528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657`.

Not run: the 12 separate-UID KVM cases (Q3 block).

## Q6. Installed paired UI/workflow journeys — PARTIAL

Run against the optimized extracted archive (app SHA-256
`027f29a…`, same as Q5):

- MCP inventory/filter/trace return: pass at 120×32, 80×24, 60×18.
- OTLP metric points, immutable replay, exemplars, distinct trace counts,
  per-currency reported costs and log overlap: pass at 120×32, 80×24, 60×18.
- Workflow settings, snooze/incidents, exact pending return and terminal
  restoration: pass at all three sizes.
- Exact Workflow retry: failed and unavailable attempts, single delivery,
  resolved-proof rejection and terminal restoration at 120×32 and 60×18.
- Tagged Python task continuity: private approval, replay, reveal and revocation
  pass.
- Error-log and failed-evaluation source: exact protected source and redacted
  return at all three sizes. Explicit log revocation and stale passing-evaluation
  correction denial are recorded at 60×18; wider denial cases are not claimed.
- Generic error logs: metadata-only witness and opt-in text witness pass.
- Tagged Python investigations: exact source, bounded cursor, policy and owner
  rechecks pass.
- Native attention against the tagged 0.2.8 receiver wheel: private binding,
  durable lost reply, replaced-receiver denial, ordered replay and metadata-only
  resolution pass.
- Concurrent Workflow retry compare-and-swap: two actual monitors cannot retry
  the same failed receipt after the attempt changes; three notification attempts
  were exercised and the secret never rendered.
- UI unit suite is included in Q1.

Not run here: Host and remote-Host journeys (Q3 block), the full
populated/empty/loading/unavailable/error matrix for every view, and a fresh
side-by-side Python 0.2.8 capture. The parity document still lists the accepted
differences, so Q6 is not claimed complete.

## Q7. Tested-head Linux and SDK CI — PASS

PR #381 head `ebe19a92ad3fdbbb350b00360877f1bf7cefdca6` (same tree as the merge
commit): every job succeeded; `Attach qualified arm64 release archive` skipped as
expected for a non-release merge.

- Rust protocol and plugin adapters: success.
- Build and verify Linux x86_64 bundle: success.
- Qualify Linux arm64 candidate: success.
- Linux x86_64 Rust host wheel: success.
- Python 3.11, 3.12, 3.13, 3.14: success.
- Node.js 22, Node.js 24: success.
- OpenTelemetry OTLP end-to-end: success.
- Five selected SDK jobs: Pydantic AI, Google ADK, LangChain, OpenAI Agents and
  Automatic SDK capture through CLI: success.

Merge commit `18aa702`: Rust protocol, x86-64 bundle, host wheel, Node, OTLP,
all SDK jobs and Python 3.11–3.14 success; ARM64 candidate/attach skipped on the
push event (the same ARM64 job passed on the PR head). CI for the tested `18aa702` head is green. Current PR #382 is tracked separately above.

## Q8. Actual per-harness installed acceptance — PARTIAL

- The extracted archive discovered, collected and reopened Codex, Pi, Claude
  Code and Kiro fixtures (Q5). Fixtures are labeled as fixtures, not native
  evidence.
- Actual local roster audit: discovered Codex sessions/reviews/subagents and
  Claude sessions/subagents; default retained 14 Codex records, explicit
  retained 4 Codex and 4 Claude records, one duplicate native ID across source
  keys was reported rather than merged.
- Human rename, two independent sources sharing one native ID, catch-up and
  restart passed in disposable state.
- Logs: `/tmp/opencode/q8-roster.log`.

Still open: an actual Kiro transcript/host (A4), and the full per-provider
installed acceptance of exact worker/turn attribution, missing evidence, later
activity and view switching (Q8). Bundled fixtures cannot close those.

## Q9. Production connector acceptance — BLOCKED

No approved production endpoint/configuration is available. Trusted pinned
fixtures and separate-UID broker contracts are not a production boundary proof.
The least-privilege launch, credential/egress restriction, revocation fencing,
idempotent ambiguous-send reconciliation and inbound commit-before-ACK remain
unqualified in production.

## Q10. Native macOS acceptance — BLOCKED

No macOS runner or artifact exists. Apple Silicon/Intel install, Gatekeeper,
`launchd`, discovery/collection and terminal journeys remain unsupported until
qualified.

## Follow-up work from the baseline run

1. **Q2 test gap — point IDs across schema 20→21.** Add an explicit `event_id`
   and exact series-key equality across a genuine MetricPoint upgrade. The
   existing `metric_description_upgrade_preserves_v20_grants_and_receipts`
   test seeds generic Activity and proves generic digests/grants/receipts.
   The synchronization follow-up adds a separate real-point regression. No data loss was observed,
   so this is coverage, not a reproduced defect. Owner: store.
2. **Q3/Q5 environment — separate-UID KVM.** Run
   `validate_rust_remote_host_history.py` and the 12 guest cases on a disposable
   root/KVM host. This machine cannot: no passwordless sudo, user namespaces
   disabled. No code change is implied.
3. **Q6 parity matrix — automate or record the full paired run.** The repeatable
   journey scripts pass, but populated/empty/loading/unavailable/error and
   Python-vs-Rust captures are still manual. Keep the accepted differences in
   `rust-ui-journey-parity.md` explicit.
4. **Q4 fixture usability — document `PYTHONPATH`.** `validate_rust_python_receiver_review.py`
   imports `observatory` but its help does not say to set
   `PYTHONPATH=<python-0.2.8-root>`; CI already does. Add it to the script
   docstring/help. Owner: validation scripts.
5. **Q8 — Kiro actual transcript.** Keep A4 open until a real Kiro host can be
   discovered, collected, reopened and inspected. Fixtures alone cannot pass it.
6. **Q9 — production connector.** Requires an approved endpoint and the F4
   least-privilege boundary; local fakes cannot close it.
7. **Q10 — macOS.** Requires native runners and signed/notarized artifacts.
8. **Q7 baseline — complete.** PR #381 and its merge are green under the
   documented event conditions. Current PR #382 CI must be retried after the
   unused-import fix; skipped SDK jobs are not passes.

## Reproduction commands

```sh
export CARGO_HOME=/tmp/agenturm-cargo RUSTUP_HOME=/home/abhishekash/.rustup
export PATH=/tmp/agenturm-cargo/bin:$PATH
export CARGO_TARGET_DIR=/tmp/opencode/agenturm-q-target

cargo test --workspace --locked --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo build --workspace --bins --locked --offline

APP=/tmp/opencode/agenturm-q-target/debug/agenturm
python3 scripts/validate_rust_metric_descriptions.py --app "$APP"
python3 scripts/validate_rust_metric_catalog.py --app "$APP"
python3 scripts/validate_rust_python_findings.py \
  --bin-dir /tmp/opencode/agenturm-q-target/debug \
  --python-root /tmp/agenturm-python-028-reference
PYTHONPATH=/tmp/agenturm-python-028-reference \
  python3 scripts/validate_rust_python_receiver_review.py "$APP"
```

Q5/Q6 used the built archive in `/tmp/opencode/agenturm-q-bundle` and the
extracted tree in `/tmp/opencode/agenturm-q-extracted`. All `/tmp/opencode`
logs are working-session handles, not durable release artifacts.

## Current test-owner handoff after review

Run CF1–CF5 and the related broker/local Review identity regressions on the
current integrated source, including schema 21→22 admission-time retention and
new typed WorkerSpawn evidence. Preserve all passing baseline results with
source/artifact identifiers rather than repeating their old status on new code.

The separate-UID block describes this executor's privilege context. Earlier
source-specific Host guest results exist in
[Host/Metrics qualification](rust-host-metric-metadata.md), on development App
`a3fd5ae6…`; they do not certify optimized App `027f29a…` or current schema 22.
Existing local guest staging helpers include
`/tmp/agenturm-final-host-merge-kvm.sh` and
`/tmp/agenturm-final-joined-guest.sh`. A test owner with KVM access can adapt
those to its frozen checkout, exact binary hashes and retained image/kernel.
Do not run fixture UID/host mutations directly on the workstation. If the
executor cannot access that guest boundary, keep the case blocked and record it.

Current broad Rust/SDK/installed/private/terminal acceptance remains delegated;
this review performs only the targeted corrective checks recorded in the plan.

## Corrective development checks from synchronization

- `cargo test --offline --locked -p agenturm-store --lib
  v20_metric_point_preserves_event_and_series_identity_through_upgrade`: **1
  passed**. The fixture uses a genuine gauge MetricPoint and compares the full
  `(source, StoredObservation)` before downgrade, during read-only schema 20,
  and after the writer's schema 22 upgrade, including point ID and series.
- `cargo clippy --offline --locked -p agenturm-store --lib --tests -- -D warnings`:
  **PASS**, after removing the unused import that stopped PR #382 CI.
- `python3 scripts/validate_rust_python_receiver_review.py --help`: **PASS**
  without the reference package installed on the import path. Help now shows
  the required `PYTHONPATH` setup; reference imports occur after argument parsing.
  Python syntax compilation also passed. Full tagged receiver behavior remains
  delegated to the test owner.

Log: `/tmp/agenturm-test-sync-focused-checks.log`. These scoped checks are not a
new whole-workspace, SDK, archive, terminal or separate-UID campaign.
