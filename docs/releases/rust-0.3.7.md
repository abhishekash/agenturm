# Agenturm Rust 0.3.7

This release extends retained span imports and evaluation queries, adds an
explicit client for an existing Python 0.2.8 receiver, and binds saved private
telemetry selections to their issuing history. The standalone Rust runtime
continues to run without Python.

## Private span text and history binding

`import-jsonl` and `broker import-jsonl` can capture bounded private span text
when the collector selects `--payload-scope` on first admission. The standard
format selects `agenturm.payload.input` and `agenturm.payload.output`.
`--payload-format agent-harness` selects the supported run, task, summary and executed tool
fields from legacy exports. Capture requires exact source and session scope.
In broker mode, the collector's allowlist and configured telemetry binding
also apply. Keep the source export in a collector-owned directory with
owner-only file permissions such as mode `0600`. The telemetry grant controls
the imported copy.

An exact replay deduplicates. Changed text or an attempt to add text to an
existing metadata-only span is rejected. Pruned text stays retired. Ordinary
history and import output omit the private text.

Saved private telemetry selectors now identify the history that issued them.
Rust 0.3.5 and 0.3.6 could accept a selector from another history when the
scope and numeric watermark matched and the receiving history already had its
own approval. The current reader checks history identity and snapshot under
the grant fence. Refresh older selections with `trace-payload snapshot` against
the intended history and copy `history_identity` and `snapshot_id` together.
The current telemetry grant remains required. A restored history needs a fresh
selection and approval. The database stays on schema 17.

[Private JSONL import and selector refresh](../rfcs/rust-cutover-workflows.md#import-exported-spans)

## Current evaluation inventory

Use `evals --list`, `broker evals --list`, or authenticated
`GET /api/outcomes` to list the latest retained outcome for each exact source
and trace. Select one to 16 distinct `otlp.*` sources. The broker authorizes
the entire set before reading it. Each row identifies its source and revision,
so the same trace ID from two sources remains two distinct results.

The inventory is complete within its bounds. It checks up to 1,024 candidate
source and trace keys, including replay-protection entries, and accepts up to
2 MiB of serialized rows. Exceeding either cap fails the request without
partial results. The evidence snapshot identifies the retained trace evidence.
Evaluation metadata reflects the current read transaction. The inventory's
metadata preserves numeric zero and uses `null` for unknown scores. The list
excludes private text and reveal selections. Use `evals --history` for retained
revisions.

[CLI and HTTP inventory commands](../rfcs/rust-broker-commands.md#read-source-aware-metadata-with-http-get)

## Attach to an existing Python receiver

Use `--receiver-protocol python-v0.2.8` with `cockpit`, `cockpit --once`, or
`attention` to read an authenticated Python 0.2.8 receiver through trusted
loopback or an active SSH forward. The client verifies the receiver identity
across a bounded set of live reads. Enter on a session scopes the visible
activity, traces, and pending requests to its recorded service, session, and
actor. Escape returns to the roster selection.

Attention keeps previously confirmed pending requests during incomplete
inventory reads. It accepts an ordered resolution for the exact sender and
request and rejects reuse of a sender's sequence for another occurrence.
The live view provides metadata only. It creates no Rust history, saved
Review receipt, or private content grant. `q` and Ctrl-C restore terminal
settings promptly even during a stalled receiver read. The Rust receiver
protocol remains the default.

[Python receiver setup and coverage](../rfcs/rust-cutover-workflows.md#attach-to-an-existing-python-028-receiver)

## Opt-in installer source candidate

The checked-out `scripts/install-rust.sh` installs the complete versioned Linux
bundle as `agenturm-rust`. It verifies the archive checksum and layout before
selecting a release, supports offline archives and same-version replay, and
preserves user history and configuration on uninstall. The existing `agenturm`
command retains its current behavior. Both native Linux bundle jobs run the
offline installer verifier before uploading their artifacts. A hosted
bootstrap, signed release manifests, and registry packages remain future work.

[Installer proposal and usage](../rfcs/rust-distribution-installation.md)

## Qualification and remaining gates

The frozen source passed 602 Rust workspace tests across 93 suites and strict
workspace Clippy. The installer passed 14 offline fixture tests and upgrade,
replay, and uninstall checks with the published 0.3.5 and 0.3.6 Linux x86-64
archives. Source-built Linux guests checked private selection isolation,
complete evaluation inventories, and tagged Python receiver navigation at
60×18 and 120×32.

The exact Linux x86-64 archive has SHA-256
936ad6f90e49bea8acdaf44b2fdc08e11c50df50987d7a5aa53ac8e65bef1791.
It passed managed install, restart, backup, upgrade and uninstall, all six
retained keyboard journeys, 24 producer/client cases and eight cached SDK
and private-capture cases without skips. Separate-principal packaged guests
passed the selector, evaluation, legacy receiver, HTTP, Host and disclosure
checks. Both native Linux candidate jobs passed the offline installer flow;
all five mandatory vendor jobs passed 16 selected SDK cases. The tag build
qualifies and attaches the ARM64 archive and checksum separately.

The standalone Rust installation remains opt-in.
The default distribution cutover still depends on real Kiro transcript
collection, qualification with a supported production connector endpoint,
and the remaining legacy remote and native evidence workflows.

[Qualification record](../validation/rust-037-release.md) ·
[Rust-only execution plan](../rfcs/rust-only-execution-plan.md)
