# Plugin correction compaction checkpoint

19 November 2026. Admission now replaces obsolete revisions of a source event inside the same transaction that inserts its higher revision and advances the cursor. The projector uses only the latest revision of each event ID, so its lifecycle and usage results are unchanged. Immutable kind and native identity checks run before replacement. A same-revision replay stays idempotent, and a stale revision still fails.

This lets a correction through even when the source is at its 50,000-row cap, provided the corrected event already has a row to replace. A later failure in the same batch rolls back the deletion, insertion and cursor together. The latest revision and digest remain for future monotonic and conflict checks. Earlier revision payloads are deliberately no longer retained as an audit history; the current projected state and the source cursor remain durable.

This is **not** full retention. Each distinct event still occupies one row, so a continuously emitting source can still reach the cap and pause. Discarding distinct events needs a separately reviewed correction horizon, durable aggregate checkpoint, late-event policy and recovery tests. No source is silently acknowledged past capacity.

Validation: 37 focused admission, projection, runtime and configuration tests passed. The new cases cover correction at capacity, exact replay, stale and conflicting revisions, usage projection after compaction, and rollback when a later record fails. `compileall` and `git diff --check` passed. The Python Store remains the only writer; the process adapter path remains opt-in.
