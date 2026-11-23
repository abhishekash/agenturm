# Agenturm Rust 0.3.7 release validation

This draft records source and fixture qualification for the Linux release
candidate. The joined runtime source is
`2c63bb43971b1c21a60a9ecfc83f6d30b7b35275`. Each check below names the source,
fixture, or previously published package it exercised. The exact 0.3.7 archive
and its executable digests remain pending.

## Retained workflows

| Workflow | Source qualification |
| --- | --- |
| Private span JSONL import | The collector admitted explicitly selected, bounded text with its exact telemetry span. Local process tests checked native binding and standalone capture, legacy agent-harness field selection, replay, changed-body conflict, metadata-only reimport, and atomic rejection of oversized text. The separate-principal guest checked private preview, body-free metadata, replay after prune, and restore. `agenturm-037-jsonl-vault-guest.log`, `agenturm-037-telemetry-history-cli.log` |
| Current evaluation inventory | `evals --list`, `broker evals --list`, and authenticated `GET /api/outcomes` listed the latest retained outcome per exact source and trace. The fixture exercised correction, same-ID spans in different sources, mixed allowed and denied source sets, numeric zero and absent scores, and private text exclusion. It checked complete-or-error behavior at the key limit, HTTP 413, prune, and same-port restore. `agenturm-037-eval-inventory-tracked-kvm.log`, `agenturm-037-eval-store.log`, `agenturm-037-eval-process.log` |
| Explicit Python receiver attach | The Rust cockpit and Attention client attached to the unchanged Python `v0.2.8` receiver on loopback with the explicit protocol option. At 120×32 and 60×18, the cockpit scoped Activity, Traces and Pending to the selected session and returned to the original roster selection. The guest exercised complete versus partial lifecycle evidence, exact request resolution, protocol and identity denial, and body-free output. `agenturm-037-legacy-quit-kvm.log` |

The Python receiver client rejects a different lifecycle occurrence that
reuses a sender sequence already seen on another request, even after the
receiver has dropped that request from its inventory. A unit regression checks
that the conflicting refresh leaves pending evidence unchanged. The
stalled-read fixture held an HTTP request open and verified that `q` and
Ctrl-C still exited at 120×32 and 60×18 and restored the original termios.
These checks use source-built executables. `bc1e5fc`, `2c63bb4`,
`agenturm-037-legacy-stalled-quit.log`, `agenturm-037-legacy-quit-kvm.log`

## Private telemetry selection

Rust 0.3.5 and 0.3.6 could interpret a saved telemetry selector from one
fresh history in another history with the same source, span, session and
numeric watermark. A synthetic reproduction opened different text. Each
history required its own current grant.

The candidate fix in `c1d2a28` checks the issuing history identity and
snapshot inside the existing grant fence. A restored branch needs a fresh
snapshot and approval.
Older selection files without `history_identity` fail with refresh guidance.
Run `trace-payload snapshot` against the intended history and copy its
`history_identity` and `snapshot_id` together. The fix uses the history
identity already stored by Rust.

Eleven JSONL CLI process tests and two Store telemetry tests passed. The
distinct-UID guest also rejected a wrong history identity and the older
selector format. It verified restore, replay, retention, and terminal
redaction on Escape, `q`, and Ctrl-C. The imported JSONL fixture belonged to
the collector with mode `0600`, and the reader's direct file open failed with
`PermissionError`. `agenturm-037-telemetry-history-cli.log`,
`agenturm-037-telemetry-history-store.log`, `agenturm-037-telemetry-history-kvm.log`

## Completed source and installer checks

| Check | Result | Qualification host log |
| --- | --- | --- |
| Final joined Rust workspace | At frozen runtime `2c63bb4`, 602 tests passed across 93 suites, with zero failures and zero ignored tests. | `agenturm-037-final-workspace.log` |
| Earlier joined Rust workspace | 600 tests passed across 93 suites, with zero failures and zero ignored tests. | `agenturm-037-joined-workspace.log` |
| Earlier joined strict Clippy | The all-target workspace check passed with warnings denied. | `agenturm-037-joined-clippy.log` |
| Opt-in installer fixture | All 14 offline installer tests passed. Coverage included verified replay, release ownership, preservation of existing user files, and uninstall preflight. | `agenturm-037-installer-final-tests.log` |
| Installer with published archives | The opt-in installer used checksum-verified Linux x86-64 packages for 0.3.5 and 0.3.6. Upgrade, same-release replay, history queries and uninstall passed. The history database and an extra file under the release directory kept their verified bytes. | `agenturm-037-exact-installer-ownership.log`, `agenturm-037-exact-installer-upgrade.log` |

The source and fixture logs are retained under `/tmp` on the qualification
host. The installer archive check above covers the published 0.3.5 and 0.3.6
packages. The default Rust cutover remains gated and the installer is opt-in.

## Pending final gates

| Gate | Status |
| --- | --- |
| Joined runtime lint and binary checks | Pending final formatting, strict Clippy and complete binary build sign-off for the frozen source. |
| Mandatory CI | Pending the final source run, including bridge compatibility and the mandatory vendor SDK jobs. |
| Linux x86-64 release archive | Pending exact archive checksum, executable digests and packaged qualification. |
| Native Linux ARM64 release archive | Pending the native runner results, archive checksum and qualified release attachment. |
| Managed installation | Pending exact 0.3.7 install, prior-release upgrade, restart, backup and uninstall with retained history. |
| Exact-archive D3 keyboard journeys | Pending retained, strict, and MCP follow-up checks at 120×32 and 60×18, including terminal restoration. |

- [Published Rust 0.3.6 validation](rust-036-release.md)
- [Rust distribution and installation](../rfcs/rust-distribution-installation.md)
- [Linux bundle and upgrade guide](../rfcs/rust-linux-bundle.md)
- [Broker operations and private approvals](../rfcs/rust-broker-commands.md)
- [Retained workflow inventory](../rfcs/rust-cutover-workflows.md)
- [Rust-only implementation gates](../rfcs/rust-only-execution-plan.md)
