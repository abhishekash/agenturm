# Native evidence facade checkpoint

The collector now constructs a `NativeEvidenceFacade` and passes it to both JSONL `Session` readers and registered native readers. Its public methods cover source-scoped persisted usage, internal-source policy lookup/redaction, connection health, and span/log ingestion. The facade forwards to the existing Store, which remains the sole Agenturm SQLite writer and policy authority. Generic collector projections and hierarchy persistence retain their existing Store path.

This is a dependency boundary, not an in-process security sandbox. A Python reader can still access arbitrary process resources if it is installed and activated in-process. The planned process protocol is required before separately installed plugins can be treated as isolated from the host API. The facade also does not implement atomic span/log/cursor admission; current retry behavior remains in place.

`NativeEvidencePort` now declares the actual ingestion options used by OpenCode (`skip_unchanged_native`) and the receiver precondition used for logs. The existing Store satisfies those calls. A new test verifies that both reader paths receive the facade and cannot call raw SQL or hierarchy methods through its ordinary API. The source-scoped usage test remains in place.

Validation: **79 focused tests passed** across native ports, registry, discovery, storage retries, native sessions and OpenCode. `git diff --check` passed. This follows the [reader registry checkpoint](native-reader-registry-phase2.html) and the [reviewed plan](../rfcs/rust-plugin-kernel-plan.html).
