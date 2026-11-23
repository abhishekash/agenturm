# Rust 0.3.8 UI acceptance

Status: opt-in candidate, 23 November 2026. Runtime code is frozen at
`1df22a4`; later changes update documentation and acceptance fixtures only.
The default Rust cutover remains gated. Comparative performance benchmarks
are deferred.

## Implemented behavior

The first screen follows the user's command-center reference in Agenturm's
colors: a full-width task table, project headings and counts, All / Needs you /
Working / Ready / Inactive filters, explicit unknown counts, and selectable
Show more. `G` cycles project, source and ungrouped views. Enter opens the
exact selected inspector; Escape restores its group, filter and source/session.
Compact help exposes every control at 60×18.

Task titles use unambiguous exact saved task links or retained structural
facts. Project paths use an explicit operator-saved Git root linked at session
level to the current local history. The UI labels this **Operator task
workspace**. Invalid, conflicting, archived, worker/turn-only and foreign
history candidates remain unavailable. Notes, diffs, prompts and process cwd
are outside this projection.

The candidate integrates Python keyboard navigation, selectable MCP and
Operations tables, and recorded Activity, Traces, Usage, Findings and Host
inspectors. `Ctrl+F` investigates retained failures or grant-verified source
evidence. Search, More and source opening pin logical history identity and
read snapshot; local source proof also checks the held writer's current path.
`X` acknowledges one exact current pending request, with a chooser for
ambiguity; native lifecycle state remains intact and new revisions can alert
again. Authenticated Python receiver attachment supports `v` for captured
internal reviews. Native capture reports its current limitation.

`z` switches the same granted prompt between Readable and original captured
layout, preserving newline/tab layout and independent scroll positions. It
does not extend the 25-second display lifetime. Escape, quit, interrupt and
expiry redact the modal before acknowledging the private delivery fence.
`c` requests terminal clipboard copy of bounded loaded metadata; `x` writes
that metadata as Markdown, HTML and JSON into a new private directory. These
exports exclude private modal bodies, never overwrite a destination, and are
explicitly distinct from evidence receipts.

## Local qualification

| Check | Result |
| --- | --- |
| Offline locked full workspace | **634 passed**, 93 suites, zero failed or ignored |
| Strict workspace Clippy, all targets | Passed with `-D warnings` |
| UI library | 39 tests passed, including populated / empty / loading / unavailable / cached-error states at 120×32, 80×24 and 60×18 |
| Actual terminal evidence views | Passed at 120×32, 80×24, 60×18 and 60×18 `NO_COLOR`; selection, expand/back, recorded Host return and terminal restoration |
| Actual task workspace grouping | Exact linked root/title/provenance, duplicate agreement, conflicts, archive, G selection, no note/diff promotion and clean exit passed |
| Actual X request handling | Exact source/revision, ambiguity chooser, Seen persistence, new-revision alerting, preserved Paused state and native resolution passed at both widths |
| Actual clipboard/export | Metadata-only OSC52, query/form ownership, cancel, private 0700 directory / 0600 files, spaces in paths, no overwrite, quit/interrupt passed at three widths |
| Actual history replacement | Independent databases with equal snapshot numbers are rejected before matches or bodies, from both roster and inspector; Ctrl+C restores the terminal |
| Actual prompt layouts | Readable/original, pan, resize, Esc, q, Ctrl+C, denied revoke while visible, revoke after redaction and late-z expiry passed; no clipboard body disclosure |
| Extracted Linux x86-64 archive | Empty startup, rolling collection, both native sources, exact inspector selection, restart/replay, backup, follow discovery and prior 0.3.7 history upgrade passed |
| Offline source installer | Install/update receipt checks, SHA pinning, replay, private prefixes, guarded uninstall and retained history passed on extracted 0.3.8 |
| Distinct-UID KVM package suite | All seven cases passed: native granted investigations, private span payloads, private eval bodies, Host collection/history, authenticated remote history, opt-in remote Host, and bounded eval inventory |
| Actual Python 0.2.8 receiver | Passed in a KVM guest: exact native requests, explicit review visibility, two viewport sizes, stalled-read quit and protocol/history identity denial |

The packaged terminal tests use the release executable and packaged adapters.
The acceptance controllers use Python outside the installed application's
empty `PATH`; the runtime and collector do not require Python or Rust.
Terminal checks reconstruct actual cells instead of assuming complete words
are contiguous in Ratatui's cursor and style deltas.

## Candidate integrity

The final archive is `agenturm-0.3.8-linux-x86_64.tar.gz`.

- Archive SHA-256: `528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657`.
- App SHA-256: `2bf3d1df191ba45ce9723f3daabd76caa106ebff3f411282352119c44e53b93f`.
- All nine runtime executables match the first qualified runtime archive
  byte for byte; the final archive updates its bundled installation guide.
- Native Linux x86-64 and ARM64 qualification passed in [run 45120054721](https://github.com/abhishekash/agenturm/actions/runs/45120054721), including the ARM64 prior-release upgrade and systemd user service. Publication is recorded separately.

CI run [45120054720](https://github.com/abhishekash/agenturm/actions/runs/45120054720)
passed Rust protocol/adapters, OTLP end-to-end, both Node versions, and all
five required SDK jobs: automatic CLI capture, Pydantic AI, LangChain,
OpenAI Agents and Google ADK. That revision uses the same frozen Rust runtime.
The Python 3.11–3.14 application matrix was still running when the qualified
Rust candidate was prepared for merge. Both native bundle jobs and all required
Rust and SDK jobs had completed successfully.

## Remaining acceptance

This is not a claim of complete Python UI parity. The user-selected command
center intentionally replaces the initial split inspector. Remaining UI work
includes configured-but-idle MCP inventory and per-call related traces;
generic OTLP metric point/history/exemplar browsing and the seven-day usage
chart; grouped/generated Findings with occurrence counts; per-process project
labels; and native internal approval review capture. Missing native producer
project metadata remains explicitly unavailable without an operator task link.

The execution plan also retains real Kiro transcript qualification, supported
production connector endpoint evidence, and additional platform qualification.
No production connector delivery is authorized by these local fixtures. These
requirements are not retired. See the [execution plan](../rfcs/rust-only-execution-plan.md)
and [workflow inventory](../rfcs/rust-cutover-workflows.md).
