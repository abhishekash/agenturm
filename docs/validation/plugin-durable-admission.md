# Durable plugin admission checkpoint

Date: 19 November 2026. Scope: opt-in process adapter inbox. The Python Store remains the sole database writer.

This checkpoint adds three Store tables. `plugin_source_cursor_v1` owns a source ID, plugin ID, exact artifact SHA-256, receiver identity, lease epoch, cursor and last batch digest. `plugin_evidence_v1` holds bounded canonical metadata records with hashed event IDs and signed 64-bit timestamps. `plugin_evidence_dirty_v1` marks the sequence that a future projector must process. One `BEGIN IMMEDIATE` transaction admits all records, advances the cursor and updates the dirty sequence; a conflict rolls the whole batch back. The process transport sends `ack` only after this call returns the accepted cursor.

Activation issues a new lease and preserves the cursor for the same source, plugin and artifact. It refuses silent ownership or artifact replacement. A stale lease cannot write. If the child or host crashes after commit but before `ack`, an exact batch replay returns the committed cursor without duplicate evidence or a new dirty sequence. A different batch at that cursor is rejected. This is a receiver-owned fence, not an OS sandbox or an unforgeable credential for same-process code.

The inbox currently admits only coarse lifecycle state and reason code, nonnegative token counters, and diagnostic code/severity. Message, tool and relationship records fail admission; arbitrary prompts, paths and outputs are not stored through this path. There is no production plugin activation, projection into live UI, policy-epoch reconciliation, connector, public SDK or Rust storage authority yet. The wire schema remains provisional.

Validation: 16 process/admission tests passed locally, including a real executable fixture, durable-ack ordering, crash/replay, source conflict, stale lease, changed event revision, later-record rollback, malformed metadata and nanosecond timestamps above JavaScript's exact integer range. The adjacent receiver/storage suite and full CI are separate checks; passing these tests does not certify the full migration or performance target. See the [execution plan](../rfcs/rust-plugin-kernel-plan.md).
