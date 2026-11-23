# Agenturm Rust 0.3.5 release validation

This record covers the Linux x86-64 candidate built on November 23, 2026.
Package checks used the binaries from the archive below. Source checks used
the same Rust code and the fixture updates identified in the table.

## Candidate identity

| Item | Commit or SHA-256 |
| --- | --- |
| Runtime source | `135dc210ce1e04c34e5c8d084632ff79610a2cfe` |
| Candidate checkout with fixture updates | `e7d30f7037fa6a3b73e1249bda84966548cd4383` |
| `agenturm-0.3.5-linux-x86_64.tar.gz` | `76fd77c0c492ac9ae9a458ae196cd64245e39e772d0208d54e4165393874fceb` |
| `bin/agenturm` | `d7824c2e410d95707d20de7a2e949a18d19463a94493c33c08b404128c44091d` |
| `bin/agenturm-storage-service` | `8852b332b7ef749e6efedf32ea4c0f1b8dfb5564304e27bddf2a0880707bff9b` |

The archive matched its checksum sidecar. The two commits after the runtime
source add the broker maximum-span fixture, fix an SDK test loader, and make
the payload fixture count structural spans while checking every returned row
for private text.

## Completed checks

| Check | Result | Qualification host log |
| --- | --- | --- |
| Rust workspace library tests | 386 passed across 22 suites, with zero failures or ignored tests. | `agenturm-035-final-joined-libraries.log` |
| Rust CLI process tests | 36 passed across 12 integration targets. They exercised attention, retention, evaluation import, window launch, prompt delivery, read incarnation, review receipts, span import, summaries, tasks, trace launch, and exact trace review. | `agenturm-035-final-process.log` |
| Strict Rust Clippy | Passed the workspace check with warnings denied, including integration tests. | `agenturm-035-final-joined-clippy.log` |
| Schema 14 → 16 | The packaged 0.3.5 binary read a 0.3.4 history without migration. The first writable open upgraded to schema 16, and the prior numeric cursor still returned the original page. The packaged 0.3.4 binary denied access after migration. | `agenturm-035-exact-schema-upgrade.log` |
| Same-port backup, restore, and receipt regrowth | On the same broker socket and HTTP port, an alternate occurrence reached the old numeric watermark. Stale snapshots and pins were rejected. A fresh reader saw the restored branch, and the same logical history retained its attention state. A replacement logical history received separate state. | `agenturm-035-archive-read-restore.log` |
| Private SDK text, cost, and token usage | Separate Linux principals exercised authenticated admission and reader-only delivery. One retained span carried USD 0.002, zero input tokens, nine output tokens, and private text. Exact replay kept the watermark stable. The remote reader returned the span, cost, and counters without the text, and the OS denied direct SQLite access. The private modal cleared on Escape, `q`, and Ctrl-C. Revocation waited for delivery acknowledgement, restore required fresh approval and selection, and pruning made the selected text unavailable. | `agenturm-035-archive-read-restore.log` |
| Managed x86-64 install | The candidate passed foreground restart, backup, bundled source discovery, and managed systemd user install, restart, inspector read, and uninstall with retained history. | `agenturm-035-final-managed-bundle.log` |
| Packaged producer checks on the qualification host | 16 passed. Python and Node transport, real OpenAI, LangChain, and LangGraph producers, and opt-in private text passed. Eight cases skipped because the host lacked Anthropic, OpenAI Agents SDK, Pydantic AI, and Google ADK dependencies. | `agenturm-035-final-packaged-producers.log` |

The logs in the table were retained under `/tmp` on the qualification host.
The same-port and private text checks used an offline Linux guest with
separate service, collector, and reader credentials. The private approval
and maintenance operations used their configured identities. The fixture SDK
workflows used offline model interfaces or loopback responses.

## Packaged process and terminal qualification

The same archive passed all six distinct-principal scenarios. Native private
content, three SIGKILL/lost-reply recovery, automatic retention, private
telemetry and Host capture/navigation passed in the combined guest. The
remote fixture initially lacked the archive's plugin catalog in the guest
layout; copying the shipped catalog corrected that fixture, and the remote
scope/restore suite passed separately. The maximum broker request exceeded
the two-second frame deadline under TCG CPU emulation. The unchanged archive
then passed under KVM hardware virtualization with the same deadlines: 1,024
spans, 1,024 cost facts and 1,024 token facts were admitted atomically; a
reader recovered all 3,072 facts across three pinned pages; malformed and
reader writes added no rows; replay was a no-op. The entire KVM guest finished
in 3.16 seconds. No application change or timeout increase was used.

Logs: `agenturm-035-final-six-fixtures-guest.log`,
`agenturm-035-final-remote-catalog-guest.log`, and
`agenturm-035-final-kvm-maximum-guest.log`.

The packaged application, plugin host and Codex adapter completed the retained,
strict keyboard and MCP journeys at 120×32 and 60×18. All 180 recorded steps
passed without missing or false assertions. All six terminal sessions exited
successfully and restored termios. The strict keyboard fixture reflects the
actual turn-picker/back stack and verifies both older and later prompt bodies,
MCP reapproval and truthful missing-route status. The retained fixture proves
an actual verified synthetic route and return. The unchanged Python 0.2.8
baseline remains in the paired records linked below.

Manifests and captures: `/tmp/agenturm-035-final-d3-retained`,
`/tmp/agenturm-035-final-d3-journeys-strict`, and
`/tmp/agenturm-035-final-d3-mcp-rerun`.

## Mandatory vendor and architecture CI

[CI run 45095665794](https://github.com/abhishekash/agenturm/actions/runs/45095665794)
qualified source `e7d30f7037fa6a3b73e1249bda84966548cd4383`.
All 16 selected cases below passed, with skips disabled. They exercise actual
SDK paths through offline model interfaces or loopback fake providers and
verify the raw reported counters against exact retained span identities,
alongside opt-in payload delivery.

| CI job | Selected cases passed |
| --- | --- |
| [OpenAI and Anthropic](https://github.com/abhishekash/agenturm/actions/runs/45095665794/job/137436414614) | OpenAI and Anthropic structural/token cases, both private message cases, standalone private capture and Node capture: 6. |
| [LangChain and LangGraph](https://github.com/abhishekash/agenturm/actions/runs/45095665794/job/137436414650) | Both structural/token cases and both private message cases: 4. |
| [Pydantic AI](https://github.com/abhishekash/agenturm/actions/runs/45095665794/job/137436414613) | Native instrumentation/token and private message cases: 2. |
| [Google ADK](https://github.com/abhishekash/agenturm/actions/runs/45095665794/job/137436414666) | Native instrumentation/token and private message cases: 2. |
| [OpenAI Agents SDK](https://github.com/abhishekash/agenturm/actions/runs/45095665794/job/137436414815) | Real tool workflow/token and model message cases: 2. |

[Bundle run 45095665793](https://github.com/abhishekash/agenturm/actions/runs/45095665793)
passed native x86-64 and arm64 qualification. The arm64 job used Ubuntu 24.04
arm64, installed and restarted its user service, upgraded Rust 0.2.0 history,
backed up and uninstalled while retaining data. Its candidate artifact is
qualified; the release-tag job must attach its own exact native archive
before an arm64 download is advertised. Publication checks the live tag's
peeled commit, checksum sidecar and existing asset digest without overwriting
assets. If a release is absent, automation creates a draft for its owner.

## Rust cutover scope

The default distribution entry point is unchanged. The execution plan still
requires real Kiro transcript collection, verified isolation and a supported
production connector, remaining legacy remote API workflows, and additional
native evidence and evaluation detail. Package qualification applies to the
platforms and artifacts verified in this record.

- [Rust 0.3.5 release notes](../releases/rust-0.3.5.md)
- [Linux bundle and schema upgrade guide](../rfcs/rust-linux-bundle.md)
- [Broker operation and private approval guide](../rfcs/rust-broker-commands.md)
- [Vendor SDK producer qualification](rust-vendor-sdk-producers.md)
- [Paired keyboard qualification](rust-034-paired-keyboard.md) and [retained workflow keyboard qualification](rust-035-retained-keyboard.md)
- [Retained workflow inventory](../rfcs/rust-cutover-workflows.md) and [Rust-only implementation gates](../rfcs/rust-only-execution-plan.md)
