# Agenturm Rust 0.3.7 release validation

This record covers source checks, the exact Linux x86-64 archive, and the
published Linux ARM64 bundle. The frozen runtime source is
`2c63bb43971b1c21a60a9ecfc83f6d30b7b35275`. The pull request bundle CI and
packaged fixtures used `634b850ade6b233f674a5e9ca138c2837a26c10d`, which adds
documentation and qualification changes to that runtime. The release tag
points to merged commit `31faa9fc32224f57bba326d1a93e31cc96a316a3`.

## Published artifact identity

| Artifact | SHA-256 |
| --- | --- |
| `agenturm-0.3.7-linux-x86_64.tar.gz` | `936ad6f90e49bea8acdaf44b2fdc08e11c50df50987d7a5aa53ac8e65bef1791` |
| `agenturm-0.3.7-linux-aarch64.tar.gz` | `56d1dcddc934e9c590c1247063ec78c35532201a04361a5de4c2aee5f40dda28` |
| x86-64 `bin/agenturm` | `05bf22885d4753734f26a7b55af01caa42da577fea5ebbd338c560e041468ef3` |
| x86-64 `bin/agenturm-storage-service` | `5cf1c2589302ecc965b7e7e4d0a3e507f93eed5957a5577290c79d06f9e55155` |

The qualification host verified the x86-64 archive checksum and compared the
extracted binaries with their archive members. The guest rechecked those
hashes before running the fixtures. GitHub's published asset digests and the
published checksum files match both archive digests. The published x86-64
asset is the exact locally qualified archive. `agenturm-037-exact-vault-prepare.log`,
`agenturm-037-exact-vault-kvm.log`. [Published Rust 0.3.7 release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.7)

## Retained workflows

| Workflow | Qualification |
| --- | --- |
| Private span JSONL import | The collector admitted explicitly selected, bounded text with its exact telemetry span. Local process tests checked native binding and standalone capture, legacy agent-harness field selection, replay, changed-body conflict, metadata-only reimport, and atomic rejection of oversized text. The packaged separate-principal guest checked private preview, body-free metadata, replay after prune, and restore. `agenturm-037-telemetry-history-cli.log`, `agenturm-037-exact-vault-kvm.log` |
| Current evaluation inventory | `evals --list`, `broker evals --list`, and authenticated `GET /api/outcomes` listed the latest retained outcome per exact source and trace. The packaged fixture exercised correction, same-ID spans in different sources, mixed allowed and denied source sets, numeric zero and absent scores, and private text exclusion. It checked complete-or-error behavior at the key limit, HTTP 413, prune, and same-port restore. `agenturm-037-exact-vault-kvm.log`, `agenturm-037-eval-store.log`, `agenturm-037-eval-process.log` |
| Explicit Python receiver attach | The packaged Rust cockpit and Attention client attached to the unchanged Python `v0.2.8` receiver on loopback with the explicit protocol option. At 120×32 and 60×18, the cockpit scoped Activity, Traces and Pending to the selected session and returned to the original roster selection. The guest exercised complete and partial lifecycle evidence, exact request resolution, protocol and identity denial, and body-free output. `agenturm-037-exact-legacy-kvm.log` |

The Python receiver client rejects a different lifecycle occurrence that
reuses a sender sequence already seen on another request, even after the
receiver has dropped that request from its inventory. A unit regression checks
that the conflicting refresh leaves pending evidence unchanged. The
stalled-read fixture held an HTTP request open and verified that `q` and
Ctrl-C still exited at 120×32 and 60×18 and restored the original termios.
The exact-archive guest repeated the held-read quit test. `bc1e5fc`, `2c63bb4`,
`agenturm-037-legacy-stalled-quit.log`, `agenturm-037-exact-legacy-kvm.log`

## Private telemetry selection

Rust 0.3.5 and 0.3.6 could interpret a saved telemetry selector from one
fresh history in another history with the same source, span, session and
numeric watermark. A synthetic reproduction opened different text. Each
history required its own current grant.

The 0.3.7 fix in `c1d2a28` checks the issuing history identity and
snapshot inside the existing grant fence. A restored branch needs a fresh
snapshot and approval.
Older selection files without `history_identity` fail with refresh guidance.
Run `trace-payload snapshot` against the intended history and copy its
`history_identity` and `snapshot_id` together. The fix uses the history
identity already stored by Rust.

Eleven JSONL CLI process tests and two Store telemetry tests passed. The
packaged distinct-UID guest rejected a wrong history identity and the older
selector format. It verified restore, replay, retention, and terminal
redaction on Escape, `q`, and Ctrl-C. The imported JSONL fixture belonged to
the collector with mode `0600`, and the reader's direct file open failed with
`PermissionError`. The broker denied reader admission in a separate stdin
test. `agenturm-037-telemetry-history-cli.log`,
`agenturm-037-telemetry-history-store.log`, `agenturm-037-exact-vault-kvm.log`

## Completed source checks

| Check | Result | Qualification host log |
| --- | --- | --- |
| Final joined Rust workspace | At frozen runtime `2c63bb4`, 602 tests passed across 93 suites, with zero failures and zero ignored tests. | `agenturm-037-final-workspace.log` |
| Final strict Clippy and builds | All-target workspace Clippy passed with warnings denied. The debug executables and optimized Linux bundle built successfully. | `agenturm-037-final-clippy.log`, `agenturm-037-final-binaries.log`, `agenturm-037-final-build.log` |
| Earlier joined Rust workspace | 600 tests passed across 93 suites, with zero failures and zero ignored tests. | `agenturm-037-joined-workspace.log` |
| Earlier joined strict Clippy | The all-target workspace check passed with warnings denied. | `agenturm-037-joined-clippy.log` |
| Opt-in installer fixture | All 14 offline installer tests passed. Coverage included verified replay, release ownership, preservation of existing user files, and uninstall preflight. | `agenturm-037-installer-final-tests.log` |
| Installer with published archives | The opt-in installer used checksum-verified Linux x86-64 packages for 0.3.5 and 0.3.6. Upgrade, same-release replay, history queries and uninstall passed. The history database and an extra file under the release directory kept their verified bytes. | `agenturm-037-exact-installer-ownership.log`, `agenturm-037-exact-installer-upgrade.log` |

## Completed exact-archive checks

| Check | Result | Qualification host log |
| --- | --- | --- |
| Published 0.3.5 → 0.3.7 schema upgrade | The 0.3.7 reader preserved the schema 16 database bytes and numeric cursor. The writer upgraded to schema 17, recorded the evaluation correction and its detail receipt, and preserved the original cursor page. The 0.3.5 binary refused the upgraded history. | `agenturm-037-exact-vault-prepare.log` |
| Native HTTP history and live progress | The authenticated gateway respected reader scope and pinned history. Offline restore and receipt regrowth rejected stale reads on the same socket and port. Progress was readable before the first span, with separate bearers, history binding, exact scope, sparse pagination, native worker linkage and immutable replay. Explicit zero and unknown remained distinct. | `agenturm-037-exact-native-http-kvm.log` |
| Evaluation and Host cockpit | The guest checked evaluation revision history, scoped summary, live allowlist removal, broker and remote keyboard return, and private detail with distinct roles. Escape, `q` and Ctrl-C cleared private text and restored the terminal. Recorded Host views checked exact sample and process return at 120×32 and 60×18, opt-in GET and POST, and restored watermark fencing. The successful run used a fixture permission correction. | `agenturm-037-packaged-cockpit-guest.ofjbfw4G/guest.log` |
| Managed installation | Foreground collection, restart, backup, prior history and cursor reopen, and systemd user installation, restart, inspector access and uninstall preserved history. | `agenturm-037-final-managed-bundle.log` |
| Verified offline installer | The `agenturm-rust` installer verified and installed the 0.3.7 archive, accepted same-release replay, and removed its launcher and executables on uninstall. History, configuration and extra user files were retained. | `agenturm-037-final-installer-bundle.log` |
| Python and Node producers | All 24 producer and client cases passed without skips. Eight further cases passed without skips for cached actual OpenAI, LangChain and LangGraph metadata, plus standalone, OpenAI, LangChain, LangGraph and Node private text. | `agenturm-037-packaged-producers.log`, `agenturm-037-packaged-cached-sdk.log` |
| D3 keyboard journeys | Retained, strict and MCP follow-up journeys passed at 120×32 and 60×18. Each of the six terminals exited successfully and restored termios. | `agenturm-037-packaged-d3-retained.log`, `agenturm-037-packaged-d3-journeys-strict.log`, `agenturm-037-packaged-d3-mcp-rerun.log` |

The source and fixture logs are retained under `/tmp` on the qualification host.

## CI qualification

Both native Linux bundle jobs passed at `634b850`, including the offline
installer checks on x86-64 and ARM64. [Native bundle CI run 45109270284](https://github.com/abhishekash/agenturm/actions/runs/45109270284)

The pull request Rust protocol job and all five mandatory vendor jobs passed.
The vendor jobs ran 16 selected cases with skips disabled across OpenAI,
Anthropic, LangChain, LangGraph, OpenAI Agents, Pydantic AI and Google ADK.
[Rust protocol and vendor CI run 45109270165](https://github.com/abhishekash/agenturm/actions/runs/45109270165)

The tagged bundle workflow passed on native x86-64 and ARM64, including
the offline installer. The native ARM64 job qualified the archive, verified
a prior-bundle upgrade, and exercised the user service. The attachment job
published the ARM64 archive and its checksum. [Tagged bundle run 45110581824](https://github.com/abhishekash/agenturm/actions/runs/45110581824) · [Attachment job 137482811086](https://github.com/abhishekash/agenturm/actions/runs/45110581824/job/137482811086)

The merged main-branch CI run was still in progress at publication.
[Main-branch CI run 45110429996](https://github.com/abhishekash/agenturm/actions/runs/45110429996)

## Rust cutover scope

The default Rust cutover remains gated and the installer is opt-in.

- [Published Rust 0.3.6 validation](rust-036-release.md)
- [Rust distribution and installation](../rfcs/rust-distribution-installation.md)
- [Linux bundle and upgrade guide](../rfcs/rust-linux-bundle.md)
- [Broker operations and private approvals](../rfcs/rust-broker-commands.md)
- [Retained workflow inventory](../rfcs/rust-cutover-workflows.md)
- [Rust-only implementation gates](../rfcs/rust-only-execution-plan.md)
