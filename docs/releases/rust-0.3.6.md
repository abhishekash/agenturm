# Agenturm Rust 0.3.6

This release adds evaluation drill-down, private evaluation text, failure
investigation, live SDK progress, and more retained metadata to the standalone
Rust cockpit and receiver. The application, collector, receiver and storage
service run without Python. Instrumented applications use their own producer SDKs.

## Evaluation results and private text

Press `E` in Traces to choose an exact OTLP source and trace. The cockpit shows
retained revisions, marks the current one, and displays the evaluation verdict
alongside the span execution status. Press `s` for the source's current outcome
page and its pass and fail counts. Escape returns through the same revision and
trace selections. The metadata view works with local, broker and attached Rust
history.

`import-evals --include-detail` optionally captures bounded result error text
and per-score explanations. Press `d` on an eligible revision in the local or
broker cockpit, or use `eval-detail preview` with the selection returned by
`evals --json`. A separate `eval.result_detail` approval is required for the
exact OTLP source. The reader verifies the capture receipt, the selected history
snapshot and the retained supporting span. The cockpit clears the private view
on close or expiry. HTTP reads return metadata and selection references only.
Pruning the supporting span removes its private text. Replaying the import
preserves the original revision and cannot restore pruned text.

[Evaluation workflows and commands](../rfcs/rust-cutover-workflows.md#import-evaluation-results)

## Native evidence and live progress

`investigate failures` pages retained failure evidence across authorized sources
in local and broker history. Filter by source, session or failure kind. A
coverage record identifies the scanned page and its continuation at the same
snapshot. Eligible Codex command references can reopen the exact native record
with the current command-detail approval.

The Pi and OpenCode adapters retain completed assistant-message cost as USD
nano-units with source provenance. Pi distinguishes explicitly provider-reported
cost from its estimate. OpenCode preserves the reported amount with an unknown
basis. Usage shows each report with its original operation and revision. A
combined total remains open where instrumentation or forks repeat a report.

OpenCode prompt preview can reverify retained text after later SQLite writes
while the original message and part bytes still match the admission proof.
The current source binding and approval remain required.

The Python and Node manual SDKs publish `Run.progress` while a span is still
open. Activity and Traces show the retained stage and optional percentage for
the exact producer scope. The progress event ID is immutable within its source
and the receiver acknowledges it after admission. Progress publishes bounded
structural fields. Span completion and native agent state have separate evidence.

[Failure, cost and progress commands](../rfcs/rust-cutover-workflows.md)

## Remote metadata and recorded Host history

`broker serve-history` now offers authenticated GET routes for sessions,
activity, traces, exact trace spans and progress, MCP, metrics and hierarchy.
Read `/api/health` first and pass the returned history binding on later
requests. Scope filters, bounded pages and opaque cursors preserve the selected
history. The receiver applies its current source allowlist. Precision-sensitive
counters, units, timestamps and snapshot tokens are strings.

Add `--include-host` when starting the listener to expose recorded samples via
`GET /api/host` and the attached cockpit's Host history. Choose a sample, then a
known process to see the loaded history for that process's exact boot and start
identity. The reader fetches approved `host.*` history without sampling procfs.
Ordinary metadata routes exclude Host samples and private content. Run the
reader on trusted loopback or through an active SSH forward. Direct public TLS
and Python receiver compatibility remain open.

[HTTP reader and Host configuration](../rfcs/rust-broker-commands.md#read-source-aware-metadata-with-http-get)

## Upgrade and restore

Rust 0.3.6 writes schema 17. Read-only access accepts supported older Rust
schemas. The first writable open upgrades the history transactionally and
preserves its current numeric cursors. Rust 0.3.5 refuses the upgraded history.
Keep a pre-upgrade backup and the matching older executable for rollback.

A backup and restore establish a new read incarnation while preserving the
logical history identity. Open a newly selected snapshot after restore and
renew any private-content approvals before viewing restored evidence.

[Linux bundle and schema upgrade guide](../rfcs/rust-linux-bundle.md)

## Qualification and remaining gates

The Linux x86-64 archive has SHA-256
`5a325704855774dbdaa333daba48dcbe9935a178aacb94c57f72086c3f27a693`.
The exact package passed Python-free launch, managed install and restart,
uninstall with retained history, and upgrade from 0.3.5. Linux guest checks
covered evaluation disclosure across separate broker identities, metadata-only
HTTP reads, backup and pruning, and keyboard navigation at 60×18 and 120×32.
The cockpit restored terminal settings and cleared private text before exit.
The frozen source passed 406 Rust library tests, 38 CLI process tests and
strict workspace Clippy. Native Linux ARM64 candidate qualification also passed;
the tag build attaches its separately qualified archive and checksum.

[Exact archive and qualification record](../validation/rust-036-release.md)

The standalone Rust installation remains opt-in. The default distribution
entry point remains gated on the retained workflow inventory. Open gates include
real Kiro transcript collection, verified isolation with a supported production
connector endpoint, remaining legacy remote workflows, and additional native
evidence and aggregation coverage.

[Rust-only execution plan](../rfcs/rust-only-execution-plan.md)
