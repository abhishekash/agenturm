# Native adapter status extension checkpoint

19 November 2026. Active native adapters can now contribute top-level health metadata through an optional `status_extension` hook. The generic collector calls one instance per adapter group, isolates exceptions, rejects duplicate keys and forbids overriding host-owned health fields, and limits serialized extension data to 64 KiB. Kiro's workspace permission scan, ten-second cache and metadata-only folder redaction moved from `LocalAgents` into `KiroAdapter`. The existing `kiro_folders: []` field remains present when no Kiro source is active for health-client compatibility.

The adapter receives only the current capture-details setting and an error-report callback, not a Store handle. Kiro still reports an `OSError` from workspace scanning while retaining its last cached result. The collector continues to own source discovery, bounded polling, persistence and the health response. This is a trusted in-process extension, not the separately supervised process plugin API.

The provenance manifest binds the updated source hashes to commit `41a51fa` and retains prior hashes as history. No fixture bytes or observed producer-version claim changed.

Validation: 80 focused provenance, native ports, Kiro permissions/sessions, native sessions, lifecycle and source projection tests passed. An injected adapter proves metadata-only status contribution, failure isolation and rejection of a `sessions` override. `compileall` and `git diff --check` passed. Remaining adapter extraction, durable distinct-event retention, Rust production authority and the full performance gate remain open.
