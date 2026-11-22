# Rust 0.3.2 validation

Ubuntu 24.04.5 x86-64 exercised the integrated source on the
`feature/rust-cutover-parallel-checkpoint` branch. The full Rust workspace
test suite, all-target Clippy with warnings denied, formatting and diff
checks passed.

The legacy-facing Codex failure was reproduced with the old packaged adapter.
The corrected source passed the 31 affected Python-side harness checks and
the native Rust process reopen scenario. Rust-only typed file edits remain
available on native admission.

## Private evidence

The Codex edit integration covers an edit-only receipt, independent approval,
exact source/event/revision/session/turn/operation selection, bounded paths
and diff bodies, changed bytes, revocation and restored-state denial.
Review notes remain tied to the original evidence version. Existing MCP
integration still passes with its separate approval kind.

## Broker clients

A network-free x86-64 Linux guest ran the broker under UID/GID 45000 and
its CLI under UID/GID 45001. The real app collected a Pi fixture through
the broker, received commit-before-ACK, reopened typed token evidence and
replayed collection without additional observations. The client could not
read the broker-owned SQLite file. A PTY opened the broker inspector at
60×18, selected the retained session and quit with its original terminal
settings restored.

Scoped service tests cover source filtering before pagination, exact worker
identity, stable snapshot watermarks, denial after retention expiry and
bounded partial-frame timeouts. Source-free queries only read the broker
allowlist.

The native broker grants the configured collector principal both admission
and read access to allowed sources. Grant administration, retention commands,
private-content disclosure and workspace mutations over that endpoint are
remaining work. [Broker commands](../rfcs/rust-broker-commands.md)

## Published archive

PR 367 merged as `a1e64afcd60780a63c975817b07390049a128f4c` and
is tagged `rust-v0.3.2`. The published Ubuntu x86-64 archive has SHA-256
`7dc8228578601e138f7face3a4556ead7b48ce7929b37641201a082b23aa11e6`.
The source tree matched the tag exactly. The archive passed installation,
Python-free startup and collection, rolling discovery, restart, inspection,
backup, upgrade from 0.3.1 and systemd restart/uninstall with retained history.
GitHub's uploaded asset digest matched the qualified local archive.
The tagged Linux bundle build also passed.

The earlier integrated archive
`a277953901b7c1106802f3bca6595ba48069cb2cccfd280890fa6025286d1d2a`
passed the packaged MCP/edit PTY and independent capability checks.
In the guest it also ran broker follow collection for 12.12 seconds,
discovered a new Pi source, recovered the same rows and watermark after
SIGKILL, and resumed an appended event after restarting the watcher.

[Published release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.2) ·
[Tagged bundle build](https://github.com/abhishekash/agenturm/actions/runs/45043063446)
