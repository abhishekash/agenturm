# Plugin evidence identity and correction checkpoint

19 November 2026 · branch `architecture/plugin-evidence-contract`

The opt-in process plugin inbox now accepts an optional typed identity with a native session ID and optional worker and turn IDs. Admission persists those IDs next to the source-owned event, records an immutable event kind and identity across revisions, and requires corrections to advance monotonically. Exact same-revision replay remains idempotent. This is a metadata-only inbox; it is not yet a live Sessions projection.

Each source is capped at 50,000 evidence rows, including revisions. On reaching the cap, the complete batch and cursor update roll back, so the caller can pause rather than acknowledge missing evidence. There is no retention or compaction path yet. This cap therefore makes a long-running production adapter pause permanently until a future bounded retention design is added; it must not be enabled by default as a production source.

The schema migrates existing inbox databases by adding identity columns with empty defaults. Old records remain unattributed and must not be guessed into a session. The plugin controls native identifiers but cannot set the receiver namespace, source owner or lease. Identity values are bounded by the wire contract; capture policy and revocation still need a production gate before any external plugin is activated.

Focused validation: `python -m unittest tests.test_plugin_admission tests.test_plugin_process tests.test_plugin_runtime tests.test_plugin_frames tests.test_rust_plugin_fixture -q` passed 28 tests with one Rust fixture skip because the local Rust toolchain is absent. Tests include atomic rollback on cap and malformed identity, old-schema migration, exact replay, stale revisions, changed identity and changed content at a revision. The CI Rust job remains the cross-language fixture gate.

Next required work: source capture-policy epoch, bounded projection with coherent revision and identity, live status semantics, retention/compaction, a production adapter and installed-plugin packaging. Existing native adapters and Python Store remain authoritative.
