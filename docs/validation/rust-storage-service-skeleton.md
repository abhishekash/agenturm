# Feature-off Linux Rust storage-service skeleton

20 November 2026. `agenturm-storage-service` is an installed probe-only server using the [v1 local contract](rust-storage-service-contract.md). It has no SQLite connection, mutation operation, production receiver caller or database migration. Python Store remains the receiver database writer; WorkflowStore and TaskStore remain separate Python mutation paths.

## Installation boundary

The operator must provision two **existing, separate** absolute directories before launch:

| Path | Required ownership and mode | Purpose |
| --- | --- | --- |
| Data root | Service UID, `0700`, empty | Placeholder for a future protected canonical store. This binary never creates or opens a database there. |
| Transport directory | Service UID, client primary GID, `2710` | Owner can create the socket; client group can traverse a known pathname but cannot list or replace it. |

The service UID and client UID must be distinct non-root identities. The directory paths and their ancestors must resolve directly without symlinks; writable ancestors are refused except root-owned sticky directories such as `/tmp`. A private data root cannot be nested inside the transport directory or vice versa. The service checks both directory identities at startup and before processing each accepted connection. A changed or nonempty data root stops the service before peer-frame parsing.

The transport directory contains a persistent, singly linked, no-follow `0600` instance-lock file. It prevents a second cooperating service even if the socket pathname has been removed while the first process still holds the lock. The socket is created with a temporary restrictive umask and must be service-owned, client-group-owned and `0660`; startup never unlinks an existing socket or symlink. The service uses bounded readiness polling to observe SIGINT/SIGTERM. Before graceful unlink it checks the socket inode and transport-directory binding; a replaced pathname is left alone. A crash leaves a stale socket for explicit operator inspection. The lock pathname remains across restarts so its inode is not recreated during ordinary shutdown.

The CLI accepts `--data-root`, `--transport-dir`, `--receiver-id`, `--client-uid` and `--client-gid`, plus an optional positive `--max-connections` for deterministic fixtures. The socket name is fixed: `agenturm-storage-v1.sock`. Each connection can perform only the v1 protocol probe. Kernel `SO_PEERCRED` admission requires the specified client UID/GID and exact receiver identity before any reply. A probe reply certifies only the transport contract; it never claims a ready storage writer. A client with the service UID is denied even when it can open the socket.

## Verification and limits

One Rust unit test checks direct-root symlink and writable-parent refusal. Ten installed Python/CLI tests run a real built binary and cover socket/lock modes, same-UID denial, graceful SIGTERM cleanup, second-instance contention both with and without the socket pathname, replacement-path preservation, pre-existing socket/lock symlinks, private/empty data-root and transport permissions, direct path proof, and live root/lock drift. Focused Rust tests, build, Clippy with warnings denied, formatting and installed tests passed on the isolated branch. The installed fixture cannot create a second UID, so it does **not** prove a successful authorized probe under a deployed distinct-principal configuration.

This is not a production writer, socket-path sandbox against code running as the service UID, general service supervisor, performance result or cutover certificate. A same-service-UID process can still race a pathname check and unlink. A real deployment still needs protected service ownership and socket ACL verification, typed mutation and durable replay/policy APIs, routing of all Python mutation paths, quiescence of old writable handles, adversarial old-binary exclusion, backup/WAL migration and recovery. See the [writer cutover gate](rust-writer-cutover-gate.md) and [execution plan](../rfcs/rust-plugin-kernel-plan.md).
