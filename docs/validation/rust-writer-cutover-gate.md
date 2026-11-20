# Rust writer cutover: ownership gate

20 November 2026, reviewed against Agenturm main through scratch-admission PR #243. This is an architecture and acceptance gate, not a storage cutover or performance result. Python `Store` remains the only production writer.

## What currently writes

`Store.__init__` runs authority preflight before its own mutations, then creates or opens the SQLite file, changes permissions and WAL mode, performs migrations, and may start workers. Every later `Store._connect()` opens a new writable connection. `server.serve`, the cockpit's `LocalReceiver`, and direct CLI commands each construct a `Store`; more than one process can point to the same database. The cockpit normally uses an existing endpoint or starts a `LocalReceiver` if no receiver responds. `outbound_delivery` writes through `Store`. `WorkflowStore` and saved tasks use separate SQLite files and need separate migration inventories.

The current `storage_authority_v1` preflight makes this version of Python Store reject a foreign marker before startup writes. It is point-in-time: it has no cross-process lock, does not fence an already-open connection, and is invisible to older binaries. An explicit `--db` symlink is supported and can be retargeted. `ReadConnections` checks file identity on checkout, but writer connections do not have an equivalent lifetime fence.

## Next bounded implementation

Add a **cooperating writer lease** behind an opt-in activation path and test it before making it a default. A launcher acquires the lease before constructing `Store` or causing a database mutation. For the managed database, use a stable lock file in the private state directory, `observatory.writer.lock`. Open it with `O_NOFOLLOW | O_CLOEXEC | O_CREAT` and mode `0600`; require a regular file owned by the current user with private permissions and compare its descriptor identity with the pathname before and after acquiring a nonblocking exclusive `flock`. Keep the descriptor open until Store background workers and read leases have closed. On constructor failure, release in `finally`.

For explicit `--db`, derive a sibling lock from the resolved target path (`.<database-name>.agenturm-writer.lock`) so aliases to one target contend. Preserve the lexical path and its resolved target identity; a symlink retarget or atomic database replacement must make later writer admission fail closed. A registry keyed by process ID and lock-file identity can share one lease across multiple `Store` instances in one process, with a reference count and one release after the final close. This preserves existing same-process uses while establishing **one cooperating writer process**, not one Store object or one SQLite connection. Reset the registry after `fork`; do not leak the descriptor through `exec`.

A subsequent read/write connection can compare the bound database target before and after open and check the authority marker inside the transaction before writes. Python's `sqlite3` API does not expose a stable main-file descriptor for this connection, so pathname checks cannot prove an adversarially stable inode through a swap-and-swap-back race. The check is a cooperative operational fence, not a security sandbox. A Rust service should own a long-lived writer connection and fail if its published canonical pathname no longer names the bound store.

Do not enable this lease for the ordinary receiver until compatibility tests establish how a second cockpit, standalone `serve`, and direct CLI command connect to the active owner or fail clearly before any startup mutation. The endpoint/API path must carry all writes before Python clients become read-only at Rust cutover.

## Tests for the lease slice

1. Two new processes aimed at one managed path, and then at a target plus explicit symlink alias: exactly one acquires; the loser changes no main database, WAL, SHM, mode, or marker.
2. Two `Store` instances in one process share a lease; closing one leaves the other protected. A different process remains excluded until the last closes.
3. SIGKILL of the owner releases the OS lock; restart recovers its SQLite WAL and cursor/receipt without accepting a second concurrent owner.
4. Constructor failure releases its lease. A symlinked, nonregular, or replaced lock-file pathname is rejected without database mutation.
5. Retarget an explicit `--db` symlink and atomically replace the database while the owner lives; the next write fails and cannot recreate or adopt another target. Reopen only through a fresh, verified activation.
6. Exercise actual cockpit fallback, standalone receiver on another port, and mutating CLI paths; capture their readiness/error behavior before enabling the lease by default.

## Legacy writer exclusion is a separate cutover gate

An advisory lock and `storage_authority_v1` marker bind only cooperating binaries. A legacy Python process can ignore both. SQLite triggers could block selected DML, but cannot prevent the old process from changing schema, dropping triggers, or using an already-open handle; they are not a complete DDL guard. Moving or chmodding a file does not revoke a handle that is already open. Therefore a canonical SQLite file writable by the old process's same operating-system identity **cannot pass** the required adversarial old-binary exclusion test.

Use a new versioned canonical path owned by a distinct service principal, with directory and file ACLs that prevent supported legacy identities from opening it for write. New Python clients use the Rust service API only. The exact service identity and ACL implementation must be proved on each supported platform; explicit/shared/external-writer stores remain on Python authority until equivalent isolation exists. If deployment cannot provide that boundary, keep Python Store authoritative.

Before cutover, stop and identify old managed writers, take a consistent SQLite backup including WAL state, archive old main file and sidecars under read-only ownership, and verify the candidate Rust schema and protocol. Then run each supported old wheel/CLI and a direct old `Store._connect()` script deliberately pointed at both canonical and archived paths. Attempt initialization, DDL, DML and creation of a second accepted receiver identity. Compare full file/sidecar state and verify the new service's receiver identity and receipts remain unchanged. A failure blocks cutover.

The **point of no return** is the first post-cutover Rust durable write, including tasks, decisions, acknowledgements, configuration, policy tombstones and delivery state. Before that write, a verified snapshot can restore the Python store. After it, use forward recovery unless a separately validated cross-version journal/export reconciles every durable write. Source replay alone is insufficient.

## Open decisions

- Which supported platforms can run a distinct storage service principal with enforceable local ACLs? Linux `flock` is only the proposed cooperative lease primitive; Windows/macOS need their own qualification.
- Which current CLI and UI writes must become service API operations before cutover? Inventory those contracts, including separate task/workflow databases and outbound state.
- Which older released binaries are in the supported migration fleet, and what executable artifacts will the adversarial tests pin?
- Does the managed launcher promise one receiver per database, or support multiple processes that must attach to one service? The current code permits multiple `Store` constructors and requires an explicit compatibility choice.

See the [Rust kernel plan](../rfcs/rust-plugin-kernel-plan.md) for the full phase 8/9 gates. No code path in this report enables a Rust production writer.
