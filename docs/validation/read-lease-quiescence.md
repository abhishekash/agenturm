# Read-lease quiescence checkpoint

20 November 2026. This is a bounded read-pool shutdown primitive for the future Rust writer cutover. It does not enable a Rust writer or change `Store.close()`.

`ReadConnections.wait_closed(timeout)` returns true only after `close()` has stopped new checkouts, every previously checked-out read context has finished its cursor cleanup and connection disposition, and no cleanup has failed. It accepts a timeout from 0 to 60 seconds; expiration returns false without cancelling a reader. The count is registered under the same lock as admission and released after cleanup, so a concurrent checkout cannot slip between the close decision and the drain check. A caller may begin waiting before `close()`.

The new concurrency test holds two readers across `close()`. Both can finish their admitted reads, new admission fails, and the wait stays false until the second reader exits. It checks that both connections and cursors are closed afterward. Injected idle-close, leased-close, and cursor-cleanup failures leave the drain result false; a failed idle close does not prevent an attempt to close remaining idle handles. Existing replacement, symlink retarget, WAL snapshot, and idle-history tests remain green.

Validation: `python -m unittest tests.test_read_connections tests.test_idle_history_watch -q` — 19 tests passed locally.

This proves only this pool's checked-out reads have drained. It does not prove that `Store` background workers, direct writer connections, separate task/workflow databases, or an old process with an already-open writable handle are quiescent. Those remain independent cutover gates.
