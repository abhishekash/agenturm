# Rust UI parity: continuation and integration

25 November 2026. **Integration qualification passed; not a full-parity acceptance or release.**

## Latest continuation

The second shell and acceptance subagents stopped at a subscription usage limit.
Their unfinished files were preserved without editing their worktrees. The
coordinator continued directly, completed the shell correctness fixes and added
regressions for the independent audit findings. The final frozen runtime
`17206a42ae60709171f9b2f26b7fea0bbd982083` passed the locked workspace build,
991 tests across 96 suites, strict app/UI clippy and 159 fresh-process
`NO_COLOR=1` UI tests. Six empty-history and three populated private-prompt PTY
journeys passed at 120×32, 80×24 and 60×18; the extracted selected-debug QA
archive passed another six smoke journeys. See the
[final qualification and remaining gates](rust-ui-parity-acceptance.md),
[shell integration note](rust-ui-parity-shell-integration-notes.md),
[visual lane note](rust-ui-parity-ui07-lane-notes.md), and
[independent Python acceptance preparation](rust-ui-parity-acceptance-prep.md).

The Python preparation records 47 passing exact-tag tests, 42 reference captures
and fixed-clock policy checks; it is not Rust acceptance. The coordinator also
reran the acceptance harness's four self-tests successfully. Final Rust checks
are coordinator qualification, not independent final acceptance. A subsequent
documentation-only commit records the results without relabelling its PR head
as the tested runtime. This integration does not publish a release or change
the default installation.

## Recovery and isolation

Continues the session exported as
`2026-11-24-200436-use-a-new-worktree-and-not-affect-the-other-agent.txt`.
The earlier implementation merged UI-01 through UI-06 into `a633c68` before
the shell-integration agent exhausted its model quota.

The original shell worktree contained uncommitted changes in 12 files. They
were copied without modifying the original worktree, preserved as `2b162cb`,
and were corrected and validated rather than assumed complete.
The main checkout and pre-existing lane worktrees remain untouched.

| Role | New isolated worktree | Branch |
| --- | --- | --- |
| Integration | `/tmp/opencode/agenturm-ui-parity-resume` | `work/ui-parity-resume` |
| Shell implementation | `/tmp/opencode/agenturm-ui-parity-shell` | `work/ui-parity-shell-resume` |
| Read-only shell audit | `/tmp/opencode/agenturm-ui-parity-audit` | `work/ui-parity-audit-resume` |
| Theme/help/palette visual work | `/tmp/opencode/agenturm-ui-parity-visual` | `work/ui-parity-visual-resume` |
| Independent acceptance | `/tmp/opencode/agenturm-ui-parity-acceptance` | `work/ui-parity-acceptance-resume` |

Shared shell files have one implementation owner. The visual lane owns only
`theme.rs`, `help.rs` and `palette.rs`; the auditor does not change feature
code. Rust builds use one compiler job to limit memory pressure. No other
agent's processes were stopped and no global model or billing settings changed.

## Current upstream compatibility

Upstream main was fetched into the dedicated
`refs/agenturm-parity-resume/upstream-main` ref, without moving another
worktree's branch. Its observed head was
`1b00428f2625d2e65a27c155321b3b72a2091f7b` (#394 OpenCode edit-detail), now
merged into the tested runtime.
Merge `d7c3eed` incorporates the newer native evidence and Claude edit-detail
work. The plan's add/add conflict retained the user's requirement that missing
native names receive a human-understandable contextual fallback, never the
literal `Name unavailable`.

## Validation ledger

The first diagnostic run used:

```sh
cargo test --locked -p agenturm-ui -p agenturm-app -j 1
```

It compiled and exited **101**: 183 app unit tests and 81 app integration
tests passed; the UI suite had 134 passes and two failures:

- `inspector_prompts_open_the_navigator_and_escape_returns_to_the_section`:
  Escape did not finish returning to the roster.
- `review_control_order_is_deterministic_and_escape_returns_to_review`:
  activating the first Review control sent no request.

Log: `/tmp/opencode/agenturm-parity-recovery-tests.log`.
Upstream integration happened during this diagnostic run, so it does **not**
qualify a frozen source SHA or executable. Both failures were fixed and
covered by the final integrated run; this historical failure is retained for
provenance. The final independent acceptance gate remains open, regardless of
the coordinator's successful exact-binary qualification.

## Architecture and acceptance boundaries

The shell must consume provider-neutral typed facts and preserve source,
session, worker, turn, snapshot and version identity. Private readers retain
the existing grant, active-display, redraw-before-acknowledgement and timeout
fences. Missing worker proof is unavailability, not permission to relabel
main-session content. Copy/export must identify the actual selected scope.

The tagged Python oracle and selected Rust exact-debug-binary journeys passed,
including nested navigation, live expiry, post-revocation denial and terminal
restoration. Remaining qualification includes a full identical-fixture paired
matrix, release artifacts, live broker revocation in an isolated root-capable
fixture, and final independent UI-Q sign-off. Mouse parity,
unsupported provider capabilities and intentional disclosure differences
cannot be promoted to accepted parity by passing unit tests.

## Cross-project consistency review

This is a review of local source checkouts, not a deployment or remote-state
attestation. No website/profile files were changed.

| Destination | Result | Evidence and scope |
| --- | --- | --- |
| App README and Python keyboard guide | Published-Python baseline retained | `README.md` and `docs/keyboard-navigation.md` describe the existing Python workflows and their own verification. Do not rewrite their historical claims as qualification of this Rust candidate. |
| Rust workflow documentation | Historical release coverage retained | `docs/rfcs/rust-cockpit-workflow-coverage.md` describes previously published Rust binaries; this continuation is separate, unreleased work. |
| Website | Existing integration-list drift | Local `agenturm-dev/src/pages/index.astro` lists Codex, Claude Code, OpenCode and Pi, while the app README also lists Kiro. Python install instructions and loopback/privacy wording remain appropriate to the published package. Rust parity is not advertised as released. |
| GitHub profile | Existing integration-list drift | Local `github-profile/README.md` also omits Kiro. Its `uvx agenturm` command, local-by-default wording and native-terminal approval boundary agree with the published app. |
| Brand/theme | Tokens agree in inspected sources; full asset review not performed | Website `src/styles/global.css` uses dark-default/light-toggle tokens. Profile `scripts/build_profile_assets.py` uses the matching dark palette and references the site's tower drawing. No brand assets changed in this work. |

Website and profile synchronization is outside this implementation request.
Their pre-existing drift is recorded rather than silently modifying other
repositories or advertising an unqualified Rust cutover.

## Related evidence

- [Implementation plan](../rfcs/rust-ui-parity-plan.md)
- [Shared contracts](../rfcs/rust-ui-parity-shared-contracts.md)
- [Original 234-row reference inventory](rust-ui-parity-inventory.md)

The inventory records its original baseline; its counts are not a current
acceptance score. Final results must identify the candidate actually tested.
