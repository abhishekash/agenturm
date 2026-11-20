# Auxiliary SQLite handoff quiescence

20 November 2026. This feature-off gate covers one `WorkflowStore` instance or one `TaskStore` instance. It does not switch either database to Rust, release a writer lease, or run in the normal receiver.

Each store now tracks its own `_transaction()` or `_connect()` handle from admission until the SQLite connection actually closes. `TaskStore.backup()` also counts its source and destination backup work. `quiesce_for_writer_handoff(timeout)` atomically stops new admissions and waits for active operations under one positive deadline of at most 60 seconds. A held operation, expired deadline, or failed connection close returns false; a failed attempt keeps the admission fence in place. Ordinary `TaskStore.close()` remains a no-op, and constructors still initialize the databases as before.

The saved-task backup helper now explicitly closes both SQLite connections on success and failure. Previously, its destination connection was only committed by a context manager and could stay open. A close failure in an instance backup poisons that instance's handoff result. Native lifecycle outbox calls use `WorkflowStore._transaction()` and therefore pass through its instance fence.

Validation: 58 focused tests passed locally across `tests.test_aux_store_handoff`, `tests.test_workflow_store`, `tests.test_saved_tasks`, `tests.test_task_continuity`, and `tests.test_native_lifecycle`. The new tests hold workflow and task handles across handoff, hold a task backup, inject connection and both backup-close failures, reject invalid timeouts, and verify a native lifecycle call is fenced.

This proof is instance-local. Another `WorkflowStore` or `TaskStore` instance can still open and mutate the same database, including constructor schema work. The task CLI calls standalone `backup_task_database()` directly; it has no instance to fence and remains a separate migration inventory path. Direct SQLite users, outbound delivery, the main `Store`, old processes with already-open handles, all-DB migration, and forward recovery remain cutover gates.
