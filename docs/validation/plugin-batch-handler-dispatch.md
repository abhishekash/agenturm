# Trusted batch handler dispatch

20 November 2026. This is a transport boundary refactor, not a new public plugin SDK or a change to capture policy.

`PluginProcess` now resolves one host-owned batch validator from its approved `required_features` before starting a child. The ordinary v1 batch stays strict. `pi-content-ref/1` selects the existing versioned content envelope validator and exact same-batch reference checks. A missing or malformed Pi envelope closes the connection without acknowledgment. An unregistered `*-content-ref/N` feature and two selected envelope handlers fail before process launch. Features merely advertised by the child cannot select a handler.

The process host also closes a child if a trusted handler raises unexpectedly or returns an invalid batch shape or a different source ID. No cursor is acknowledged on that path.

The immutable `BatchHandlerRegistry` can be composed by trusted host code. A synthetic second handler was exercised through a real child process without changing `PluginProcess`. The child cannot register Python handlers or supply a Store handle. Pi content source configuration, filesystem proof, capture policy, lease, durable admission, and redaction remain owned by `PluginSession` and Store. A new wire envelope still needs a reviewed host handler and an admission contract; this refactor does not grant content access to arbitrary extensions.

Validation in the isolated worktree:

- 37 dispatch, transport, and installed Rust Pi emitter/receiver tests passed together (one platform-dependent test skipped).
- `git diff --check` passed.

No performance claim is made. The public UI, default metadata-only capture, and storage writer are unchanged.
