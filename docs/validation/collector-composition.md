# Collector composition checkpoint

The receiver and embedded terminal now use `start_local_collector` with one immutable `CollectorRegistry`. The bundled registry contains Codex, Claude Code, Pi, Kiro and OpenCode. A caller can supply a trusted registry with a sixth adapter or disable a bundled source without changing either application entry point. Duplicate IDs and source/reader ID collisions fail before any collector starts.

This is explicit in-process Python configuration. It does not discover or execute independent plugin packages. Process isolation, manifest activation, durable batch/cursor admission and the Rust host remain separate gates in the [execution plan](../rfcs/rust-plugin-kernel-plan.md).

Local validation: 12 collector, adapter, reader-registry and startup-health tests passed; `git diff --check` passed. Normal CLI and HTTP startup contracts were not changed.
