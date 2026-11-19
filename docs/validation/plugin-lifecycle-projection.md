# Plugin lifecycle projection checkpoint

19 November 2026 · branch `architecture/plugin-live-projection`

The opt-in plugin path now turns admitted, identity-bearing lifecycle metadata into bounded session rows. The source plugin supplies event IDs, revisions, native session identity and typed lifecycle state. The host creates a receiver-scoped session key. A missing native identity remains unattributed and does not become a fabricated session. Usage and diagnostics do not imply task completion.

Projection selects the latest revision of each occurrence, then the latest explicit lifecycle timestamp for each session. A correction to an older occurrence cannot override a later outcome. Conflicting states at the same timestamp yield Unknown with an ambiguity reason. The existing session health reducer maps the result to Completed, Interrupted, Working or Unknown; a PR, low-level tool failure or arbitrary prose does not set status.

Each source projection is derived outside the writer transaction and published only when its source lease, dirty sequence and policy epoch still match. A stale projection fails publication. Revocation deletes both evidence and projected rows in one transaction. Query results omit dirty or revoked projections and expose a pending-source count. A background worker coalesces rebuilds after durable acknowledgment, so the plugin poll path does not wait for the full projection. The worker reports its last projection error on the opt-in `PluginSession` object.

The receiver health response includes policy-current plugin sessions in its existing local session list and exposes pending/omitted projection coverage. Source-scoped identity remains distinct for multiple native sessions under one plugin source. Inactive installations make only a cheap empty-source check; no plugin executable is discovered or started by health.

Validation covers corrections, same-time conflicts, missing identity, two sessions from one source, revocation, a concurrent source change during derivation, health integration, the existing session-health reducer and a coalesced runtime projection. The plugin and projection suites passed locally. The installed production plugin path, async cancellation, usage aggregation, retention/compaction and full Rust authority remain later gates. The Python Store remains the sole writer.

The current projector rebuilds one source from at most 50,000 admitted rows and may need incremental indexing before a busy production adapter is enabled. It runs outside the poll acknowledgment path but still consumes CPU. Pending coverage is explicit until the rebuild succeeds.
