# Rust activation against a detached real Store clone

20 November 2026. This is a feature-off Linux storage-service slice. Python `Store` remains the production writer. The service accepts no canonical database path from a request and does not start from the receiver.

## Boundary

`--experimental-real-clone-activation enabled` requires one service-owned `0700` data root containing a fixed `telemetry-clone.db` and only its known SQLite sidecars and private writer lock. The file must be a singly linked `0600` regular file with `schema_meta.storage_activation_clone_v1=enabled`, `storage_authority_v1=python-v1`, and `plugin_sessions_read_schema=1`. The marker is added only to a detached test clone; it is absent from ordinary production databases. A distinct client UID/GID connects through the existing peer-checked Unix socket. A v2 frame contains no database path. The service pins the clone inode, verifies it across requests, and enforces a private umask.

Only `ActivateSource` is accepted. In one `BEGIN IMMEDIATE` transaction, Rust checks receiver identity, source ownership, artifact generation, capture state and existing metadata-only history. It issues a fresh lease epoch, retains cursor, dirty and policy epochs, and writes a request-bound durable replay receipt. Exact retry returns the same lease even after a lost reply; a changed request with the same ID is rejected. New sources and existing sources with no private-reference ownership are supported. Private content fingerprints, recorded reference providers, revisions and other unsupported source history fail closed. The request cannot select a new mode, run a migration, publish a path or activate a canonical writer.

## Evidence and remaining gates

The disposable two-UID fixture creates a real Python `Store`, makes independent SQLite backups, activates matching sources through Python and the Rust service, and compares the relevant durable fields. It checks first activation, existing-source lease refresh, exact replay, a discarded reply followed by retry, conflicting generation, wrong receiver and UID, rejected non-activation/revoked/private-fingerprint/private-owner sources, receipt count, unchanged original database, service restart with WAL recovery, and refusal of a same-byte replacement clone inode. The Rust storage service and admission suites run separately. GitHub CI is a separate gate and must not be inferred from local results.

The marker and fixed private path constrain this opt-in fixture; they do not prove that a future operator cannot place a canonical database there. No production client routes activation to Rust. The service does not admit lifecycle batches against this clone, clear private reference data, change capture policy, or migrate WorkflowStore, TaskStore or outbound state. Other live Store instances, older held-open writers, coordinated backup publication, full write-route parity and forward-only recovery remain cutover gates. See the [writer cutover gate](rust-writer-cutover-gate.md).

The [HTML checkpoint](rust-real-schema-activation-clone.html) is a self-contained review copy.
