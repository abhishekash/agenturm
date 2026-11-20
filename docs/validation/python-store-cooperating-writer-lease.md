# Python Store cooperating writer lease checkpoint

20 November 2026. `Store(path, writer_lease=True)` enables an opt-in Linux fence for cooperating new Agenturm telemetry writers. The default remains `writer_lease=False`; this is not a Rust writer cutover and does not exclude old binaries.

## Lock and admission

The lease is acquired before Python Store authority preflight, directory creation, chmod, WAL changes, schema migration, or worker startup. Opt-in requires an existing private database parent directory owned by the current user. A sibling lock name is derived from the resolved database target, so a database and its symlink alias contend on the same lock. The lock must be a singly linked regular file owned by the current user with mode `0600`; it is opened with no-follow and close-on-exec flags, then acquired with nonblocking exclusive `flock`. The code checks the lock descriptor against its pathname before and after locking.

Repeated Store instances in one process share the lock through a PID-and-path registry with a reference count. Forked children close inherited descriptor copies and clear the registry; `O_CLOEXEC` prevents the descriptor from surviving exec. Each `_connect()` verifies the lease before opening, after opening, and before commit. Pooled `_read()` admissions also verify it. Database path resolution and bound device/inode are rechecked; a database absent when the lease was acquired may only be bound after Store's own exclusive file creation.

## Shutdown evidence

Opt-in `Store.close(timeout=...)` asks the existing Store handoff gate to stop new admissions and wait for that Store's tracked writer connections, pooled read leases, migration worker, derived worker, and direct incoming-status read. It returns `True` and releases one reference only after the drain succeeds. A timeout, close error, or failed identity check returns `False` and keeps the lock in the process registry; a later close can retry. If quiescence cannot be proved, the descriptor remains held until cleanup succeeds or the process exits. Constructor failure follows the same rule after tracking has been initialized; failures before that point have not opened Store connections or workers and can safely release the acquired lease.

The focused fixture covers cross-process contention before Store creates a database or sidecars, symlink aliases, same-process reference counts, fork and exec descriptor behavior, SIGKILL release, lock symlink/mode rejection, lock-path replacement, database replacement before and during a transaction, symlink retargeting, missing-target races, constructor failures before and after worker startup, and close timeouts with active readers/writers. It also verifies that non-Linux opt-in fails clearly while default Store startup remains available.

Run the fixture with:

```sh
python -m unittest tests.test_store_writer_lease -v
```

CI runs this command in the Linux Rust protocol and plugin-adapter job.

## Limits

This is a cooperating process fence. A legacy binary can ignore the lock, another process can use direct SQLite, and an already in-flight operation is not fenced by a pathname check. `sqlite3` does not expose a stable main-file descriptor, so an adversarial swap-and-swap-back race is not excluded. The lease covers only telemetry `Store`; workflow, saved tasks, outbound state, and direct mutation routes remain separate. Windows and macOS have no implementation. A protected service-owned canonical path, complete mutation routing, old-writer exclusion, synchronized migration, and forward recovery remain required before Rust may own production writes. No performance gain is asserted.
