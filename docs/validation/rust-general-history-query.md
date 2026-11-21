# Rust durable history query

This slice connects the fresh Rust observation store to a bounded read API, the `agenturm query` command, and older-history navigation in `agenturm inspect`. It is Rust-only and reads only the Rust-owned observation database. It does not import, bridge, or modify Python-era state.

## Query contract

- Optional exact `harness_id` and `session_id` filters are applied before paging.
- Results use a deterministic descending key `(observed_at_unix_ns, harness_id, event_id, revision)` in storage and are returned oldest-to-newest within each page for the existing capability/UI reducer.
- The exclusive `before` cursor carries all four ordering fields plus a `snapshot_id`.
- `snapshot_id` is the highest committed sequence in a dedicated `AUTOINCREMENT` receipt-sequence table, visible when the first page begins. Every page joins its observations to those sequences and excludes later receipts. This prevents concurrent collection, including backdated observations, from shifting a traversal. The explicit sequence is durable across `VACUUM` and row deletion.
- The request limit defaults to 500 and is bounded to `1..=16,384`. Timestamp and snapshot IDs are decimal strings in returned JSON to avoid loss of integer precision in common JSON consumers.
- `agenturm inspect` reads 500 observations per page and exposes `[` for older history and `]` to return to cached newer pages. The UI retains at most four prior page models; the cursor and durable data remain authoritative.

Example first page:

```sh
agenturm query --db ./state.sqlite --harness-id harness.codex --session-id session-1 --limit 500
```

Use the returned `next_before` fields on the next request, including `snapshot_id`. If `has_more` is false, the current snapshot has no older matching observations.

## Safety and limits

The query opens a v3 store through SQLite read-only mode. It does not create a database for a missing path, initialize an empty file, migrate a schema, change journal settings, or change observation rows/cursors. A v2 store must first be opened through a write-capable operation such as `inspect` or collection; that path performs a transactionally backfilled v2-to-v3 schema upgrade. Filters, limit, cursor identity, timestamp range, and snapshot watermark are validated before SQL execution. A cursor from another snapshot is rejected. A future watermark is rejected. Prepared SQL binds every caller-supplied value. Queries over an unrelated database fail through the store's existing marker checks. A terminal page returns no next cursor.

The page watermark relies on the explicit AUTOINCREMENT sequence remaining durable. The current Rust store does not implement retention. Deleting sequence rows is not supported by the product API; if future retention prunes history, the sequence counter and cursor contract must remain monotonic. A page traversal is stable against concurrent appends, but it is not a multi-minute SQLite read transaction and does not pin the WAL.

This is a bounded observation query, not full domain query parity. It does not yet expose lifecycle summaries, relationships, private content, policy-filtered source hydration, a network API, arbitrary ordering, or text search. The `inspect` back stack is intentionally in-memory and bounded; reopening the app starts from the latest page.

## Verification

Focused tests cover equal-timestamp ordering across harnesses, source/session filtering, stable pagination with no duplicates or skips, a concurrent backdated write excluded by the pinned receipt watermark, v2-to-v3 schema upgrade, missing-path and empty-file no-write behavior, invalid filter/cursor rejection, close/reopen read-only queries, CLI cursor round-trip, terminal-page cursor omission, and UI cursor precision above JavaScript's exact integer range. Workspace formatting, Clippy, and full Rust workspace tests are recorded with the implementation PR.
