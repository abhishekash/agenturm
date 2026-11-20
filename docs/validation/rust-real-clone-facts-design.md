# Detached Store clone: generic fact admission design

20 November 2026. This is a feature-off continuation on the fixed private real-Store clone path. Python `Store` remains the production writer; no receiver, canonical path, or production client is routed to Rust.

## Proposed slice

Keep the existing `AdmitLifecycle` wire operation and its guarantees unchanged. Add a separate typed `AdmitFacts` v2 operation for only `message`, `tool`, `check`, and `usage`. Map each fact through the existing guarded `BEGIN IMMEDIATE` transaction, cursor/lease fence, durable request replay receipt, dirty-sequence update, and empty-content receipt/status writes. The service still accepts only activation plus these two admission operations in this explicitly marked clone mode.

The service must validate canonical payload JSON itself. Match Python `plugin_admission._metadata` and `plugin_activity.validate_metadata`: message roles are `user|assistant`; tool categories are `shell|file|mcp|other` and results are `started|succeeded|failed|denied`; check categories are `test|lint|build|review|other` and results are `passed|failed|skipped|unknown`; usage is a nonempty subset of `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, and `total_tokens`, each an integer in the signed 64-bit nonnegative range. Messages, tools, and checks require a native session ID. An operation ID is allowed only for a tool. Lifecycle remains on its existing path. Discovery, diagnostics, relationships, revisions/corrections, content references, retention, and capture-policy changes stay rejected.

## Boundaries and failure behavior

The v2 frame, 48-row prepared batch, 800-byte cursor, identifier, payload, and request limits remain enforced before SQL. Only typed source IDs are accepted; no database path is added to requests. Lease, receiver, source identity, cursor, and fixed-clone marker/inode checks remain mandatory. Any validation, SQLite, trigger, or marker failure aborts the entire transaction. Exact request replay returns the durable receipt; conflicting reuse is rejected. Revocation/lease fencing is tested against an old lease. Existing scratch JSON behavior and its lifecycle-only limit stay intact.

## Evidence required

Use independent Python Store backups and compare cursor, evidence rows, dirty state, and source state for a mixed message/tool/check/usage batch. Test multi-row rollback, lost acknowledgement and restart replay, revoked/stale lease rejection, receiver/source mismatch, marker deletion during writes, replaced clone inode/path refusal, exact cursor and protocol bounds, and that the canonical input remains unchanged. Any Python semantics the Rust slice cannot match must be explicitly gated rather than silently narrowed or approximated.
