# Read-only Store client boundary

Recorded 20 November 2026. `StoreReadClient` is an opt-in Python query client for an **already initialized** Agenturm Store. It exposes one bounded `plugin_sessions(limit)` view. The production server and TUI still use `Store`; Python remains the sole writer, and no Rust write path is enabled.

## Contract

The writer installs `schema_meta.plugin_sessions_read_schema = 1` only after the plugin projection columns exist. The client checks that exact marker, the required table and column shapes, and a receiver ID in one read transaction before interpreting rows. An absent database, missing or unknown marker, incompatible table, malformed projection row, or changed database target fails closed. This marker covers **one query contract**; it is not a proof that every Agenturm table or a future writer version is compatible.

The row count is capped at 1,000 and the current trusted Store admission bounds persisted usage fields. A forged future writer could put oversized or misleading text under the same marker; the marker and this client do not authenticate row contents or provide a general byte-bound proof for arbitrary databases.

Shared `plugin_projection.session_rows()` and `render_session_view()` own the SQL and rendering for both `Store.plugin_sessions()` and the new client. Rendering happens after the read transaction closes. The client uses SQLite `mode=ro` and `query_only`, never calls `Store.__init__`, and provides no writer method. It binds the regular database target and receiver on its first successful read. A later file replacement or `--db` symlink retarget requires a new client. A read transaction returns policy-current rows with the same pending, retention-gap, capture-revocation and partial-coverage semantics as the existing Store method.

The file check is point-in-time. It does not authenticate a copied database or detect a swap and restore entirely between observations.

The client does not create the main database, initialize schema, change permissions, run migrations or start background workers. SQLite's read-only WAL handling can still interact with existing `-wal`/`-shm` sidecars; this slice does not claim literal zero filesystem writes on every platform or a writer exclusion fence. It does not use `immutable=1` on live data because that could ignore uncheckpointed WAL changes.

## Verification and limits

Focused tests compare empty, pending, projected, large-usage, bounded-limit, retention-gap, source-gap and revoked results with `Store.plugin_sessions()`. They prove a missing path is not created, an unsupported schema fails, a malformed stored usage row is rejected, main database bytes and modification time are unchanged by the client read in the tested active-WAL fixture, and a bound symlink retarget is rejected. Existing plugin projection and retention tests exercise the shared query extraction.

This is the first read-only seam for a later one-writer architecture. It is not wired into production clients, does not exclude legacy Python writers, and does not establish Rust storage compatibility or a performance gain.
