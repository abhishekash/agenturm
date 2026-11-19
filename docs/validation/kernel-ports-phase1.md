# Kernel port checkpoint: source scoped native usage

This checkpoint follows [the first native discovery and baseline change](rust-plugin-phase0.html). It extracts a narrow query port while preserving Python Store as the only database authority.

`Session` depends on `NativeEvidencePort` for ingestion, capture-policy changes, connection status and persisted usage. The existing Store implements that port. Claude usage recovery no longer opens a SQLite connection from its format parser: `Store.native_model_usage(source_id, trace_id, span_id)` performs the exact source scoped read and returns an owned usage mapping. A missing or foreign source returns no usage. This is a migration seam, not a new public plugin protocol or a complete harness adapter.

A new test writes a synthetic span and verifies the allowed read plus foreign-source and wrong-trace denials. The native-session and storage-retry suites passed with the change: 51 tests total. `git diff --check` passed. The previous source discovery checkpoint passed its separate 54-test focused run and recorded a synthetic 10,000-file scan at 61.979 ms CPU median per scan. That scan repeats every ten seconds; it does not explain the entire reported performance dip.

The next safe extraction is the rest of the canonical ingestion/policy/checkpoint port. The current span and log writes remain separate transactions. The plan's atomic batch-and-cursor contract is not implemented by this read-port change. Rust work remains gated on a representative end-to-end workload and host supervision.
