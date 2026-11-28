# Agenturm Rust 0.3.16

## Faster history navigation

Recent-history reads now use ordered SQLite indexes rather than sorting the
entire observation store. Exact session reads and older-page cursors seek
directly to their scope. Roster lifecycle and relationship lookups use a sparse
index, including when no parent relationship is recorded.

This removes a measured bottleneck shared by navigation, workflow checks, and
background collection. In a million-record synthetic query benchmark, a
256-record page took about 2 ms with indexes. This is a database-read measurement,
not an end-to-end navigation guarantee. See the [measurement and limits](../navigation-performance.html).

This release also includes the workspace Traces paging fix and reduced cockpit
startup latency shipped on main since 0.3.15.

## Upgrade

**Quit older Agenturm processes before launching 0.3.16.** The first writer open
upgrades native history to schema 28 and builds additional indexes. Large stores
can take longer to open initially. Older versions cannot read the upgraded
database. Existing observation identities, grants, snapshots, retention, and
source authorization retain their semantics. Indexes require extra disk space
and add write work; no history is deleted by this upgrade.

Native archives are available for Linux x86-64, Linux ARM64 (glibc 2.39+), and
Apple Silicon macOS 13+. Each archive has a SHA-256 sidecar. Intel Macs and
Rosetta are unsupported.

After the verified installer is updated:

```sh
curl -fsSL https://raw.githubusercontent.com/abhishekash/agenturm/main/scripts/install-rust-bootstrap.sh | bash
agenturm --version
# agenturm 0.3.16
agenturm
```

[Self-contained HTML notes](rust-0.3.16.html).
