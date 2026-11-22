# Agenturm Rust 0.3.4

This Linux candidate adds grant-fenced source investigations, optional Git
patches in task handoffs, and explicit native turn and worker links for
OTLP traces. Python applications can export trace metadata directly to
the Rust receiver without running a Python receiver.

## Source investigations and task patches

The instruction, path, command and source commands can investigate bounded
native evidence through the local store or the distinct-principal broker.
Results bind exact source receipts, the retained snapshot and current
content approval. Command detail is available for complete, verified native
Codex command records; incomplete evidence fails closed. Reusing a source
reference requires the same history, scope, snapshot and approval.

Task snapshots and handoffs accept `--include-diff` to retain a bounded Git
patch in the private workspace. Plain captures retain metadata and clear
an earlier patch. Capture avoids repository hooks, external diff tools,
text conversion and configured filters; patch capture has byte, file and
time limits. The ordinary task listing does not reveal patch bodies.

[Investigation and broker commands](../rfcs/rust-broker-commands.md) ·
[Task patch workflow](../rfcs/rust-cutover-workflows.md)

## Exact traces and Python producers

Review can select linked OTLP traces for an explicitly identified native
turn and worker, or resolve the exact native turn from a trace. The native
anchor and trace scope must agree at the selected retained snapshot.
Session-only and ambiguous trace scope cannot establish a turn link.

Both Node and Python OTLP producers accept explicit source, session, turn
and worker identities. The Python client supports direct manual tracing
and automatic instrumentation using the OpenTelemetry SDK. The transport
sends bounded trace metadata to a literal loopback HTTP endpoint. It does
not export arbitrary span attributes, prompts or tool payloads. The Rust
application and receiver continue to run without a Python runtime.

[Python producer](../python.md) ·
[JavaScript producer](../javascript.md)

## Validation and release gates

The combined candidate passed 361 library tests across 22 suites, focused
CLI process checks, three Node producer tests and nine Python producer
tests. The Python checks include the real OpenTelemetry SDK with an
offline instrumentor fixture. Archive and installed workflow qualification
must finish before publishing this candidate.

Ubuntu 24.04 x86-64 remains the qualified platform. Real Kiro data,
production connector qualification, additional platforms and the remaining
paired keyboard journeys retain their separate gates. The default
distribution entry point has not changed.
