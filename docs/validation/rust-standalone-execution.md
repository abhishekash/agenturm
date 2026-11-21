# Standalone Rust execution validation

Initial checkpoint on 21 November 2026, on x86-64 Ubuntu 24.04.5.
Toolchain Rust 1.100.0. The implementation was tested in the
`feature/rust-harness-decoder-api` worktree based on
`6991cb8bbb51117f2eabf02bab82eaa5ad53c1d5`.

## Harness boundary

The separately built [standalone Cargo fixture](../../rust/agenturm-app/tests/fixtures/standalone_harness)
registered through configuration and wrote a usage fact that survived reopening
SQLite. In the same poll, a malformed adapter response and a crashing adapter
left their cursors unset and did not interfere with the healthy source. An
unsupported catalog format failed before database creation. Configured Pi and
Codex tests passed with their real adapter binaries.

## Installed Linux workflow

The extracted versioned archive was installed into a new directory with a fresh
SQLite state directory. On a running systemd user manager, the actual CLI:

1. Discovered one real local Codex and one real local Pi session.
2. Installed and started the collector service, then queried durable observations
   from each source.
3. Rejected a second writer to the same database while read-only queries and
   a live backup continued to work.
4. Stopped and restarted the service, verified a new process, and reopened both
   recorded sessions. The terminal inspector showed two sessions in a real PTY.
5. Uninstalled the unit and queried both sessions from the retained database.

The same archive also discovered, collected and reopened both real sessions
inside an isolated filesystem containing only the bundle, three runtime
libraries, the ELF loader, read-only source roots, and fresh writable state.
The filesystem contained no Python binaries or libraries. The systemd and
isolated filesystem checks ran separately on the same Ubuntu host.

The initial archive `agenturm-0.1.0-linux-x86_64.tar.gz` passed checksum
verification with SHA-256
`f90535ae94e78bbe21516fe663623bdd5c4a85b2c89fe31a3f735e69eb234852`.
See [bundle instructions](../rfcs/rust-linux-bundle.md).

## Storage and recovery

The [configured collection test](../../rust/agenturm-app/tests/fixtures/crash_recovery.rs)
killed a subprocess at the public commit boundary before SQLite committed
and again after a durable commit but before the kernel could send ACK. At the
second boundary, SQLite still held committed frames in the WAL. Restart
recovered exactly one observation with its matching cursor and admitted no
duplicate. The collector held the Linux writer lease during both crash cases.

The Linux store acquires the same process lease for every writable open of a
persistent database, including schema creation and migration. A subprocess
test holds the collector lease on a v9 store. A second process reads committed
history, receives a lease error from both writable entry points, and confirms
the schema remains at v9. After the collector releases the lease, the second
process opens the store with write access and migrates it to the current schema.
Writable connections within one process share the lease. The operator must
keep the database and its lock file in a trusted directory. The lease
coordinates Agenturm processes that use the Rust store.

The store rejects commit, pruning and grant changes if the writable path is
retargeted after opening. Retention moves expired event identities and digests
into tombstones while preserving request receipts and source checkpoints.
Tests cover pinned query expiration, exact replay, conflicting older records,
backup and reopen. Revisions preserve the earlier evidence in history.

The native backup test reopened a detached SQLite snapshot and proved that
committed WAL data, the cursor, and the replay receipt survived. It also proved
the reopened copy could accept a new batch. The running systemd collector
supported the same backup path while both real sources were present.

See [the harness evidence matrix](rust-harness-evidence-coverage.md) for
per-adapter fixture and terminal coverage.

## Follow-up Linux qualification

The integrated candidate passed on Ubuntu 24.04.5 x86-64 with Rust 1.100.0,
from the development worktree based on commit
`6991cb8bbb51117f2eabf02bab82eaa5ad53c1d5`. Its
`agenturm-0.1.0-linux-x86_64.tar.gz` archive had SHA-256
`14364f4aec1d16966edfd63f9472a38674feb75974d3b68b1278c70e0a3808cb`.

The [reusable archive verifier](../../scripts/verify-rust-linux-bundle.sh)
verified the checksum and version, extracted the bundle, discovered fixture
sources with an empty PATH, collected and reopened both sessions, replayed
without duplicates, opened a selected turn in the inspector, and reopened
a backup. The verifier also opened and replayed the database created by an
earlier local candidate of the same version. A separate run exercised the
systemd user service with a fresh database, new PID after restart, PTY
inspection, and retained data after uninstall. CI qualifies `rust-v*` tags and
uploads verified artifacts. The worktree has yet to produce a published
Rust tag or exercise an upgrade from one.

## Rolling discovery and scoped preview

On Ubuntu 24.04.5 x86-64, a fresh archive started collection with empty
discovery roots and admitted new Pi and Codex files in the same watch process.
After SIGTERM, a new CLI process reopened both sessions. A separate managed
run used actual local Pi and Codex records, rejected a second writer,
reopened a live backup, restarted into a new process, and retained history
after uninstall.

The configured Pi content round trip verified default denial, explicit
approval, rejection after revocation or source replacement, exact byte
verification, and denial while another process holds the writer lease.
See the [Pi preview record](rust-content-grant-fence.md) for the proof
window and operating instructions.
