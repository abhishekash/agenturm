# Agenturm Rust 0.3.6 release validation

This record covers the Linux x86-64 candidate built on November 23, 2026.
Packaged checks use the executable files from the checksum-verified archive.
The source checks cover the frozen build commit and subsequent connector
fixture update identified below.

## Candidate identity

| Item | Commit or SHA-256 |
| --- | --- |
| Runtime and fixture source | `1be53d651dae6bf94e5b0fe006df588651078e32` |
| `agenturm-0.3.6-linux-x86_64.tar.gz` | `5a325704855774dbdaa333daba48dcbe9935a178aacb94c57f72086c3f27a693` |
| `bin/agenturm` | `8819045e3f09fe1669cdcdaa0331d16283f1b8d0579c6ee5fda86762882fba17` |
| `bin/agenturm-storage-service` | `1d0e188bb14f6825bc53d58b86842dc3805140708048e0e1e18848d1e3e888f5` |

The subsequent CI fixture commit `46fb52dd63fbc0080c8393bad800e7d6fe28b8ba`
adds a bounded wait for an inherited connector fixture lease during reopen.
The archive retains the runtime source listed above.
The final broker fixture also distinguishes adjacent replay from an older
event replayed after another checkpoint: both preserve the retained events,
while only adjacent replay must preserve the checkpoint.

## Completed checks

| Check | Result | Qualification host log |
| --- | --- | --- |
| Full Rust workspace | 591 tests passed across 93 suites, with zero failures or ignored tests. | `agenturm-036-final-full-workspace.log` |
| Rust workspace libraries | 406 tests passed across 22 suites. All ran with zero failures and zero ignored tests. | `agenturm-036-final-libraries.log` |
| Rust CLI process tests | 38 tests passed across 12 integration targets. The suite covered attention, retention, evaluation import, window launch, prompt verification, read incarnation, review receipts, span import, one-shot summaries, tasks, trace launch, and exact trace review. | `agenturm-036-final-process.log` |
| Strict Rust Clippy | Workspace check passed with all targets enabled and warnings denied. | `agenturm-036-final-clippy.log` |
| Connector fixture rerun | All 12 connector cases passed after the bounded fixture reopen wait. | `agenturm-036-connectors-rerun.log` |
| Packaged schema 16 → 17 | The 0.3.6 binary reopened history created by the published 0.3.5 package. Read-only lookup kept schema 16 and returned the original numeric cursor page. The first writable evaluation correction upgraded to schema 17, issued a detail selection, and preserved that page. The 0.3.5 binary then refused the upgraded history. | `agenturm-036-exact-archive-schema-upgrade.log` |
| Native prompt references | The separate-principal guest created two fresh histories with the same OpenCode occurrence metadata and numeric watermark. Each opened its own prompt with the current grant, and the second rejected the first history's reference. Older retained prompts reopened after unrelated writes and a later sweep. Live WAL mutations, direct source access by reader and administrator, revoked approval, stale restore selections, and pruned evidence were denied. | `agenturm-036-exact-native-telemetry-kvm.log` |
| Private telemetry fence | Authenticated OTLP admitted exact reported cost alongside private span text. Replay held the watermark stable and metadata stayed body-free. Reader-only disclosure cleared on Escape, `q`, and Ctrl-C. Revocation waited for delivery. Restore required a fresh approval and selection, and pruning made the selected text unavailable. | `agenturm-036-exact-native-telemetry-kvm.log` |
| Private evaluation detail | Across five Linux identities, the exact evaluation receipt and snapshot controlled disclosure. The fixture checked content-free HTTP metadata, immutable replay, separate fresh histories at the same numeric watermark, restore and prune. Both protected strings cleared before revocation completed on Escape at 60×18 and 120×32, `q` at 120×32, and Ctrl-C at 60×18. Each run restored its original terminal settings. | `agenturm-036-packaged-cockpit-guest.O9kcONGR/guest.log` |
| Packaged evaluation metadata and remote Host | Evaluation revisions, source summary, broker and remote keyboard return, and live allowlist removal passed. Opt-in Host GET and POST queries respected their scope. Recorded process drill-down returned to the exact sample at both viewport sizes, and restore fenced stale read watermarks. | `agenturm-036-packaged-cockpit-guest.O9kcONGR/guest.log` |
| Packaged HTTP progress | The guest retrieved progress before spans arrived. It checked separate bearer credentials, history binding, exact source, trace, and session scope, sparse pagination at a pinned snapshot, and the native worker link. Replay preserved the original progress, including explicit zero and unknown values. | `agenturm-036-exact-remote-progress-guest.6j8wIiwQ/guest.log` |
| Native assistant cost | Packaged Pi and OpenCode retained reported amounts with explicit rounding markers, provenance, zero, and exact assistant anchors across reopen. Incomplete values were withheld. The 120×32 Usage view displayed the normalized amounts and restored terminal settings. | `agenturm-036-exact-native-cost.log` |
| Managed Linux install | The archive passed foreground collection, restart, backup, prior history and cursor reopen, and systemd user install, restart, inspector read, and uninstall with retained history. | `agenturm-036-final-managed-bundle.log` |
| Packaged keyboard journeys | The retained, strict, and MCP follow-up fixtures passed at 120×32 and 60×18. Retained journeys checked handoff export, verified synthetic routing, and related-evidence return. The strict fixture covered interrupted activity, prompt and MCP views, linked traces, Review, tasks, Usage, and Host. The MCP follow-up exercised revoke, reapproval, and evidence return. All six terminals exited successfully and restored termios. | `agenturm-036-packaged-d3-retained.log`, `agenturm-036-packaged-d3-journeys-strict.log`, `agenturm-036-packaged-d3-mcp-rerun.log` |
| Packaged producer and client contracts | All 24 Python and Node checks passed without skips. The suite exercised a real OpenTelemetry SDK with offline instrumentation, exact ancestry and source scope, progress visible before span flush, bearer endpoint binding, partial failure, bounded batches, and opted-in text projection. | `agenturm-036-packaged-producers.log` |
| Packaged vendor and private capture fixtures | All eight selected checks passed without skips. Real OpenAI, LangChain, and LangGraph exercised structural metadata. The five private capture cases covered the standalone producer, OpenAI, LangChain, LangGraph, and Node. | `agenturm-036-packaged-cached-sdk.log` |
| Broker OTLP and progress | Separate identities checked bearer authorization, native progress before span flush, JSON and protobuf admission, stable event rows across interleaved replay, adjacent replay checkpoints, restart, exact Review, and child exit status. | `agenturm-036-exact-broker-otlp.log` |
| Native Linux ARM64 candidate | The native ARM64 runner built the candidate and prior Rust baseline, then qualified archive installation, upgrade, restart, backup, and systemd user uninstall. Both architecture jobs passed at fixture source `46fb52d`; release attachment follows the tag build. | [Bundle run 45105188688](https://github.com/abhishekash/agenturm/actions/runs/45105188688) |
| Mandatory Rust and vendor CI | The Rust protocol job passed full workspace tests, strict Clippy, producer integration, independent plugins and separate-principal storage checks. All five vendor jobs passed 16 selected SDK metadata and private-capture cases with skips disabled: OpenAI, Anthropic, LangChain, LangGraph, OpenAI Agents, Pydantic AI and Google ADK. | [CI run 45105188826](https://github.com/abhishekash/agenturm/actions/runs/45105188826), jobs `137464734159`, `137466139535`, `137466139566`, `137466139574`, `137466139575`, `137466139642` |

The logs are retained under `/tmp` on the qualification host. The prior
0.3.5 Linux x86-64 archive and its validation are recorded separately.
[Rust 0.3.5 archive validation](rust-035-release.md)

## CI and architecture checks in progress

| Gate | Evidence to record |
| --- | --- |
| Broader compatibility CI | Python-version jobs remain active in the broader run. Rust protocol, vendor jobs, Node and OTLP jobs have passed. [CI run 45105188826](https://github.com/abhishekash/agenturm/actions/runs/45105188826) |
| Linux ARM64 release asset | Verify the tag build's archive and checksum attachment. Native candidate qualification has passed. |

## Rust cutover scope

The standalone Rust installation remains opt-in. The broader execution plan
still requires real Kiro transcript collection, verified isolation for a
supported production connector, remaining legacy remote workflows, and
additional native evidence and aggregation coverage.

- [Linux bundle and schema upgrade guide](../rfcs/rust-linux-bundle.md)
- [Broker operation and private approvals](../rfcs/rust-broker-commands.md)
- [Retained workflow inventory](../rfcs/rust-cutover-workflows.md)
- [Rust-only implementation gates](../rfcs/rust-only-execution-plan.md)
- [Vendor SDK producer qualification](rust-vendor-sdk-producers.md)
- [Paired terminal qualification](rust-034-paired-keyboard.md) and [retained workflow keyboard qualification](rust-035-retained-keyboard.md)
