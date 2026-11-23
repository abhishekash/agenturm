# Agenturm Rust 0.3.5

This release extends native Rust history with authenticated broker trace
collection, bounded span and evaluation imports, persisted retention,
recorded cost, Host history, and opt-in private SDK message views. The
application, collector, receiver and storage service run without Python.
Producer SDKs remain part of the application being observed.

## Collection and retained history

`broker serve-traces` and `broker run` admit traces through the separate
storage authority. Producer bearer credentials are pinned to the receiver
endpoint. `import-jsonl` accepts bounded structural span exports;
`import-evals` attaches result metadata to exact retained trace identities.
`evals --history` pages retained revisions and `evals --summary` counts the
current page's pass/fail results. Evaluation and execution status remain
independent. Expired result bodies stay unavailable even if a trace ID is
later reused; replay digests and revision numbers survive retention.

`retention status|set|disable|run` persists the age policy. New default
cockpit histories enroll in a 14-day policy. Existing histories keep their
existing policy. Active writers apply the policy periodically, and broker
policy changes require maintenance authority. Cleanup retains checkpoints
and replay receipts. Backups and original harness files have their own
lifecycle.

OTLP JSON, protobuf and span JSONL imports retain exact per-span input,
output, total, cache read/write and reasoning token counters when reported.
Missing counters remain unknown and explicit zero survives. Conflicting or
invalid aliases are withheld independently. Each usage record binds its
source, exact span, session and optional turn/worker. SDK Usage shows the
individual records without adding overlapping wrapper spans.

OTLP and span JSONL imports retain explicitly reported cost as integer
nano-units and a currency. Zero is preserved; missing, invalid or conflicting
amounts remain unknown. Usage presents individual source/span reports.
It does not calculate a bill from token counts, infer prices, or add
potentially overlapping span reports. Kiro credits retain their native unit.

## Private SDK message views

Content capture is off by default. Producer capture, authenticated receiver
capture and a separate `telemetry.span_payload` approval must all be enabled
before viewing producer-captured text. A native prompt approval cannot grant
SDK text access. Selection binds the exact source, session, trace, span,
optional turn/worker, retained receipt and history incarnation.

Known SDK message shapes project bounded input/output text. Headers,
arbitrary attributes, tool arguments, raw provider objects and errors are
excluded. Invalid control or credential-like text is withheld. The metadata
query, one-shot report and HTTP history gateway never return these bodies.
The cockpit's trace payload action uses the existing delivery fence and
clears text when the modal closes or its deadline expires. This is
producer-captured text; it is labeled separately from verified native bytes.

[Broker capture and approval commands](../rfcs/rust-broker-commands.md) ·
[Vendor producer qualification](../validation/rust-vendor-sdk-producers.md)

## Cockpit and Host history

`cockpit --once` emits a bounded session report in text or JSON and works
with local, broker and remote metadata history. Global `--db`, `--once`
and `--new-window` select the cockpit. On Linux, new-window launch uses
an existing tmux session or an available GNOME Terminal, Konsole or xterm,
preserving the working directory and arguments without shell interpolation.

Managed collection config v2 can opt into periodic capture with
`"host":{"source_id":"host.workstation","interval_ms":5000}`. This works
with native sources, follow discovery, or a Host-only configuration through
local, broker and systemd-managed collectors. Existing configurations stay
unchanged. Reader authority is checked before procfs is sampled.

`host record` and `host history` retain and query explicit Linux source
samples. The cockpit opens recorded samples, their process list and the
history of an exact process identity, including boot ID and process start
ticks. PID reuse does not combine histories. Samples retain bounded gauges
and known same-user process categories; command lines, environment and
arbitrary process names are excluded. A broker reader or HTTP client never
samples the receiving machine's procfs.

`broker serve-history` exposes a dedicated metadata reader on a trusted
loopback endpoint. Cockpit and attention attach with receiver-scoped local
workspace state. Use an active SSH forward for remote access. This listener
is not a direct public TLS endpoint and does not implement Python `/api/*`
wire compatibility.

[Retained workflow inventory](../rfcs/rust-cutover-workflows.md)

## Upgrade and restore

The store uses schema 16. Read-only access accepts supported older Rust
schemas; the first writable open upgrades them transactionally. Older Rust
executables reject a schema-16 history. Keep a pre-upgrade backup with its
older executable if rollback is required.

A normal upgrade preserves legacy numeric read cursors. Managed backup and
restore create a new read incarnation while keeping logical history identity.
Old cursors and pinned private selections then fail rather than attach to
the replacement branch. Review notes and marks retain their original
lineage. Private approvals are disabled in the restored history and require
fresh selection and approval.

## Qualification and remaining gates

The joined candidate has passed workspace library and strict Clippy checks,
actual local OpenAI/LangChain/LangGraph and Node producer checks, and
separate-principal Linux fixtures for private delivery and Host navigation.
The final archive and mandatory vendor CI qualification are recorded before
publication.

The default distribution entry point remains gated by the retained workflow
inventory. Real Kiro transcripts, supported production connector delivery,
remaining legacy remote APIs, richer evaluation details and remaining native
evidence coverage keep their own open requirements.
