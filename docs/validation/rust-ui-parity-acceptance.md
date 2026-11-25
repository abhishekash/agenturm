# Rust UI parity: integration qualification, not full acceptance

25 November 2026. **This continuation is an implementation/qualification slice. Full Python 0.2.8 UI parity, release readiness and default cutover are not signed off.**

## Candidate and ownership

The tested runtime candidate is `17206a42ae60709171f9b2f26b7fea0bbd982083`
on `work/ui-parity-resume`. It includes upstream main through OpenCode
edit-detail PR #394 (`1b00428`), without changing the main checkout or the
original agents' worktrees.

The subsequent qualification commit changes documentation and evidence manifests
only; its PR head must not be substituted for the tested runtime SHA. The
executable SHA-256 is
`30ee1cb918bb32f7c1c6c50429718cd4a72ca1595cbcc128ee8f9cd50430baa9`.

The independent auditor reviewed the recovered `2b162cb` snapshot and reported
11 concrete findings. The coordinator fixed them and added regression checks.
The independent acceptance agent completed Python-oracle preparation before a
usage limit stopped it. Subsequent Rust qualification is coordinator execution,
**not an independent final UI-Q sign-off**.

The builder manifest and terminal artifacts below identify the exact source,
executable and harness actually run. A debug build privately copied for testing
is not a published optimized release archive or reproducible-build proof.

## Validation results

The following results were reconciled with the completed builder and PTY
manifests. Historical diagnostic results are not combined into a passing score.

| Check | Result and scope |
| --- | --- |
| Locked workspace build | Passed, exit 0, frozen runtime source unchanged |
| Locked full workspace tests | Passed: 991 tests across 96 suites, zero failures or ignored tests |
| Strict app/UI clippy | Passed: all targets, warnings denied, exit 0 |
| Monochrome UI suite | Passed: 159 tests in a fresh `NO_COLOR=1` process |
| Empty retained-history PTY | Passed: six journeys at 120×32, 80×24, 60×18, each with q and interrupt-in-help; root navigation, palette, literal query, help, resize and termios restoration |
| Populated local-private PTY | Passed: three sizes, 19 frames each; metadata navigator, exact latest/older request, granted Original, nested return/help, real display expiry and post-revocation denial |
| Extracted QA archive | Passed: all seven executable hashes match; extracted cockpit passed another six empty-history journeys |
| Tagged Python reference | Passed rerun: 47 exact-tag tests; 42 captures; 15 day-boundary checks and 105 Today bucket assertions; reference stayed clean |
| Harness self-tests | Four passed during preparation: exact copy/hash refusal, environment isolation, stub PTY resize/cleanup; stub is not Rust evidence |

Commands for the frozen runtime:

```sh
cargo build --locked --workspace -j 1
cargo test --locked --workspace -j 1 -- --test-threads=1
cargo clippy --locked -p agenturm-ui -p agenturm-app --all-targets -j 1 -- -D warnings
NO_COLOR=1 cargo test --locked -p agenturm-ui --lib -j 1 -- --test-threads=1
```

The private QA archive contains selected debug executables, not the normal
installer/catalog/service bundle. Its SHA-256 is
`507fb728122e58834f1d0dc295fe784a934fb5ad9767d57be0fc12111ef86225`.

## Important discoveries during qualification

- The original shell compiled but failed navigation tests. Several additional
  audit findings concerned private redaction, wrong evidence identity and form
  reconciliation. See the preserved audit and the integration note's
  finding-to-regression table.
- An actual terminal run found that returning to Agents during a pending
  destination read could be ignored. The key now supersedes that navigation,
  and the late response is discarded. A regression exercises the race.
- The smoke harness initially accepted the word Activity in a navigation strip
  rather than waiting for the loaded pane. It now waits for actual Activity
  content. It also preserves its cell model on a no-op resize: the app need not
  repaint a terminal whose dimensions did not change.
- An unrelated pre-existing retention test closed the child's stdout pipe after
  reading its address. A later status print could kill the receiver with
  BrokenPipe. The fixture now retains the pipe; no retention assertion was
  removed or weakened.
- Standalone cockpit owns the collector writer lease. An external CLI grant
  revocation while that cockpit is open is refused, not a successful live
  revocation. The local PTY gate therefore proves live expiry, then closes the
  cockpit, revokes and reopens to prove denial. Live broker revocation remains
  its separate root-capable isolated test; this host run does not claim it.
- Broker prompt navigation now shares the local typed page/scope/cursor policy.
  It no longer falls back to an untyped prompt list or silently relabels a main
  session with a requested worker identity.
- The populated harness originally matched denial too early and sent Escape/q
  without awaiting the return frame. It now waits for exact denial and roster
  frames. An unavailable main-session key also used to mutate the private
  reader; it is now inert, with a regression. Both fixes are in `17206a4`.
- Inspected compact expiry frames remove the captured text and show expiry;
  denial is explicit, and the wide navigator exposes request selection. Repeated
  status rows and overall visual equivalence are not qualified by these checks.

## Full-plan gates

| Gate | Disposition |
| --- | --- |
| Reference inventory | Original 234-row map retained as a baseline; not all rows reclassified or qualified |
| Screen/state comparison | Selected headless and terminal cases only; no identical-fixture full paired matrix across every root view, state and theme |
| Keyboard/navigation | Integration regressions and specified PTY subsets; not every form/history/clipboard/mouse/receiver journey |
| Exact installed candidate | Debug executable identity, private-copy tests and extracted selected-debug QA archive; not a published release archive, ARM64 build or all transport/provider modes |
| User acceptance | Intentional differences and capability gaps are not silently accepted |
| Independent UI-Q | Python preparation completed; final independent Rust sign-off remains outstanding after the agent usage limit |

In particular, worker prompt listing without worker proof remains explicitly
unavailable, prompt history remains bounded by its adapter window, readable
normalization requires adapter facts, and private-body copy/export remains
disabled. Review Git/Map needs a verified workspace binding; persistent incident
acknowledgement needs its ledger API. Full mouse parity and the pre-existing
provider-specific resume planner remain follow-up work. The unchanged
Solarized muted-on-panel contrast finding remains documented by UI-07.

**Do not describe this integration as “full parity complete”.** The original
plan's remaining required mismatches and acceptance decisions remain open.

## Artifact and reproduction ledger

- Committed small manifests and artifact hashes: [evidence ledger](rust-ui-parity-evidence-17206a4.json)
- Candidate builder: `/tmp/opencode/agenturm-parity-17206a4-builder.json`
- Workspace/lint/PTY log: `/tmp/opencode/agenturm-parity-17206a4-validation.log`
- Empty-history frames/results: `/tmp/opencode/uiq-rust-parity-smoke-17206a4/`
- Populated local-private frames/results: `/tmp/opencode/uiq-rust-inline-17206a4/`
- Monochrome UI: `/tmp/opencode/agenturm-parity-17206a4-no-color-ui.log`
- Archive identity: `/tmp/opencode/agenturm-ui-parity-qa-archive-17206a4/manifest.json`
- Extracted QA archive smoke: `/tmp/opencode/uiq-rust-extracted-smoke-17206a4/`
- Python oracle rerun: `/tmp/opencode/uiq-python-oracle-resume-20261125/`
- Earlier audit: `docs/validation/rust-ui-parity-shell-audit.md` and HTML
- Finding dispositions: `docs/validation/rust-ui-parity-shell-integration-notes.md` and HTML

These local artifacts are synthetic and are not committed as large binaries,
transcripts or SVG captures. The committed small ledger preserves their
identities, not their availability after local cleanup. Reproduction commands are in the respective harness help and
lane notes; output directories must be new and privately owned.
