# Rust exact query from an isolated Store backup

Recorded 20 November 2026. `agenturm-backup-query` is an opt-in diagnostic composition layer. A caller supplies a completed, isolated SQLite Store backup, one source ID and one exact native session ID or opaque Activity ID. It returns a typed historical result with the full receiver, lease, policy, dirty and projected revision certificate. Python Store remains the sole writer and live freshness authority. No production query default or performance claim changes.

## Contract and dependency direction

The existing `agenturm-domain-query` takes a v3 admitted-evidence snapshot and runs `agenturm-domain-reducer`. The backup reader instead returns already-published session and Activity projections. Feeding those projection rows back through the reducer would invent a different input contract. This change extracts the pure exact-row selector from `agenturm-domain-query` and uses it in both paths. The dependency graph is `agenturm-backup-query → agenturm-storage-shadow + agenturm-domain-query → agenturm-domain-reducer`; the storage reader remains independent of query and reducer code.

The bridge asks `agenturm-storage-shadow` to validate only the selected domain's bounded rows in the same short read transaction as its revision certificate. A session query does not load Activity, and an Activity query does not load session rows. The reader requires an absolute regular backup file, refuses SQLite sidecars, and opens with read-only `immutable=1` and `query_only`. The caller must supply a **trusted**, completed and quiescent backup. The bridge never opens the live Store or publishes a result.

For a ready source, `coverage: "partial"` and a `session` or `activity` field contain the selected row or JSON `null` for an exact miss. A pending, unknown or disabled source returns `coverage: "unavailable"`, the historical revision if present, and **omits** the result field. An absent row in partial coverage is not proof that the source is complete. The exact query is source-scoped and rejects malformed identities or unexpected request fields.

## Live fence

The response certifies only the backup's internal revision; it does **not authenticate the backup or its rows**. A structurally valid altered backup can contain a forged projection row with the same revision. For live publication, a Python caller must match the receiver and full revision **and compare the selected row or exact absence** with the corresponding entry in `rust_shadow_snapshot.capture(store, source_id).expected`. It must then call `rust_shadow_snapshot.current(store, snapshot)` after the Rust result and before publication. A write, capture revocation, policy change or projection change makes the old response historical. A row mismatch must be discarded even when `current()` is true. The bridge does not perform these live checks itself.

Example request:

```json
{"schema_version":1,"sqlite_path":"/absolute/path/to/completed-backup.db","source_id":"source-one","query":{"kind":"session","source_id":"source-one","native_session_id":"native-one"}}
```

## Verification and limits

Focused Rust tests and Clippy cover the shared selector and bridge. Installed differential tests compare exact session and Activity rows against Python `capture(...).expected`, including a large usage sum above JSON's safe integer range, pending and disabled capture, source-gap invalidation, exact misses, and an old ready backup whose live Python fence has become false. A separate regression changes a completed backup's session state to another valid state: the revision still matches and `current()` stays true, but the selected row differs from Python's expected projection. Corrupting the unrequested projection does not block a query of the other validated projection; querying the corrupt projection fails closed. The existing installed domain-query suite still passes after selector extraction. The default storage-shadow response is unchanged.

This is a query over published projection, not an independent reduction of admitted evidence. It does not prove live cutover, exactly-one-writer behavior, downgrade handling or crash recovery. `immutable=1` cannot verify that another process is not mutating the file and does not establish file authenticity.
