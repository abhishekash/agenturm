# Rust distribution and installation

Use the published Rust 0.3.8 bundle on Linux x86-64 or ARM64. The opt-in
installer keeps the app, process host, five adapters, connector driver,
storage service and pinned catalog together under **agenturm-rust**. The
default Rust cutover remains gated.

[Published Rust 0.3.8 release](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.8)

The qualified opt-in installer is available in
[scripts/install-rust.sh](../../scripts/install-rust.sh). It does not publish
a package, replace the existing installer, change the default command, or
start a service. Its release-download path uses an exact Rust tag.

## Choose the delivery path

| Path | Decision | Reason |
| --- | --- | --- |
| Verified release-bundle installer | Implemented, explicitly opt-in | Existing x86-64 and ARM64 artifacts already have native install, restart, upgrade and uninstall qualification. Users need neither Python nor a Rust compiler. All companion executables and their catalog remain together. |
| cargo install / crates.io | Future packaging work | The app is named agenturm-app, has publish = false, and depends on workspace path crates. Publishing requires a deliberate crate layout and registry release process. Installing the app binary alone does not install its companion processes and catalog. |
| cargo-binstall | Future developer convenience | It can select CI-built archives using package metadata, but the current bundle contains multiple crates' executables and runtime catalog data. First design and qualify their complete installation; do not advertise a one-binary binstall command that omits them. |
| OS package manager | Future qualified platform package | A Homebrew formula or Linux package can own the complete layout, checksums and uninstall. It adds packaging, upgrade, service-manager and platform checks. There is no qualified macOS or Windows Rust bundle to advertise now. |

Cargo's
[publish field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-publish-field)
controls registry publication.
[cargo-binstall's upstream package support](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md)
documents archive URLs and binary-path metadata. These are possible later
distribution mechanisms; this change does not enable either publication path.

## Install with the checked-out source installer

Use Bash, curl, GNU tar and coreutils on qualified Linux x86-64 or ARM64.

~~~sh
bash scripts/install-rust.sh install --version 0.3.8
agenturm-rust --version
agenturm-rust
~~~

The installer prints the absolute launcher path on stdout and progress and
next steps on stderr. If ~/.local/bin is absent from PATH, it says so without
editing the shell profile. The default managed layout is:

~~~text
~/.local/bin/agenturm-rust
  -> ~/.local/share/agenturm-rust/dist/releases/
       agenturm-0.3.8-linux-ARCH/bin/agenturm
~~~

XDG_DATA_HOME selects the data base. --prefix and --bin-dir accept absolute
paths, including spaces. The release directory has an ownership marker and a
per-release receipt. The existing agenturm command stays selected as before.

The installer rejects unsupported architectures and non-glibc Linux.
Its conservative runtime floor is glibc 2.39, matching the qualified
Ubuntu 24.04 runners. Other Linux distributions have not acquired support
claims merely because the executable can start on them.

Preview without downloading or changing files:

~~~sh
bash scripts/install-rust.sh install --version 0.3.8 --dry-run
~~~

For an already downloaded archive, the offline path uses the same checks:

~~~sh
bash scripts/install-rust.sh install --version 0.3.8 \
  --archive ./agenturm-0.3.8-linux-x86_64.tar.gz \
  --checksum ./agenturm-0.3.8-linux-x86_64.tar.gz.sha256 \
  --sha256 528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657
~~~

Use the aarch64 asset and its matching digest on ARM64.

| Published 0.3.8 archive | SHA-256 |
| --- | --- |
| Linux x86-64 | 528d4c7057d280258788d44e684505b7c552bc5f4dc8f730c9a24d568fc0d657 |
| Linux ARM64 / aarch64 | 95e37c0254f9bcd1650b1a7d9a630a40c180f61dc80f43035e934c732dd556cc |

[Release assets and sidecars](https://github.com/abhishekash/agenturm/releases/tag/rust-v0.3.8) ·
[Archive qualification](../validation/rust-038-ui-acceptance.md)

## Update and recover

Updates are deliberate and versioned:

~~~sh
bash scripts/install-rust.sh update --version 0.3.8
~~~

There is no latest alias, automatic update, or lookup of GitHub's global
latest release, which can refer to the Python distribution. A new complete
bundle is verified and tested with --version before the launcher is switched
atomically. Prior releases remain available. A same-version archive with
different bytes is rejected; modified installed binaries or catalog files
are not silently overwritten.

An installation does not open history, migrate it, rewrite a service, or
restart an existing collector. Managed service units retain their selected
executable until explicitly reinstalled. Before upgrading a managed history,
stop collection and back it up with the old selected version:

~~~sh
state="$HOME/.local/state/agenturm-rust"
agenturm-rust service uninstall
agenturm-rust backup --db "$state/history.sqlite" \
  --bundle "$HOME/agenturm-before-upgrade"
bash scripts/install-rust.sh update --version 0.3.8
agenturm-rust service install --config "$state/sources.json" \
  --db "$state/history.sqlite"
agenturm-rust service status
~~~

Use the actual configured state and source paths if different, and choose a
new backup directory. A foreground cockpit must also be closed before its
history is backed up for an upgrade.

Selecting a retained executable version does not downgrade a database.
Rust 0.3.6–0.3.8 use schema 17, which 0.3.5 refuses. Keep the
pre-upgrade backup and its matching executable for rollback. Preserve the
upgraded history in its existing location. Restored private evidence needs new
approvals and selections. See the
[bundle upgrade guide](rust-linux-bundle.md).

## Uninstall

Stop managed collection first, then remove the program installation:

~~~sh
agenturm-rust service uninstall
bash scripts/install-rust.sh uninstall
~~~

Use the same custom `--prefix` and `--bin-dir` values as the installation.
The installer refuses uninstall while its known collector unit is still
configured. It removes only the matching agenturm-rust launcher and known
managed program files. History, configuration and source transcripts are
retained. Unexpected additional files inside a release directory are kept.
The small ownership directory can remain for a later reinstall.
If additional files keep a former version directory in place after its
program receipt is removed, move those retained files to a data directory
before reinstalling that exact version, or choose another --prefix.

An unrelated regular file or foreign symbolic link named agenturm-rust is
never replaced. An unrecognized --prefix is never adopted or recursively
removed. No sudo or shell profile edits are part of this flow.

## Integrity and failure behavior

- Fetch only exact Rust-version assets from the project release URL, over
  HTTPS with HTTPS-only redirects, bounded transfers and finite timeouts.
- Require one bounded checksum line naming the exact platform archive.
  Verify its SHA-256 before extraction. --sha256 additionally checks a digest
  supplied through an independently trusted channel.
- Accept the closed current bundle inventory only. Reject extra or duplicate
  paths, traversal, symbolic links, hard links and special archive members.
  Bound compressed and declared unpacked sizes.
- Extract into a private staging directory. Verify the executable version
  and platform before publishing a version directory. Serialize installers
  with a local lock and replace the launcher only after verification.
- Refuse foreign launchers, writable-by-others destinations, altered managed
  directories and conflicting same-version receipts.

A checksum fetched with the archive detects transfer corruption and differing
bytes. It is not an independent publisher signature and cannot establish
authenticity after compromise of the same release account. A published
installer should be pinned to a reviewed commit, with its digest available
through release documentation. Signed release manifests or independently
verifiable build attestations are follow-up work.

## Ready now and publication gates

The versioned Linux release bundles are already public and qualified.
The opt-in installer, offline verification mode and accompanying tests are
implemented in the repository. Its exact 0.3.8 archive test installed into
paths with spaces without a network transfer.
The offline suite covers install, update, identical replay, foreign launchers,
invalid digests and archives, platform refusal, data preservation and refusal
before deleting an altered installation. An exact 0.3.5 → 0.3.6 installation
also preserved a schema-16 history byte-for-byte through update and read-only
query. Uninstall retained both the external history and an additional history
file placed inside a release directory.
The Rust 0.3.8 tag run passed native bundle qualification and the offline
installer check on both Linux x86-64 and ARM64. Each job verified all
companion executables and catalog pins, same-version replay, the existing
`agenturm` command, and uninstall with both normal and version-local SQLite
history preserved. The published assets match the verified SHA-256 digests
listed above.
[Native tag qualification](https://github.com/abhishekash/agenturm/actions/runs/45121070134)

A hosted one-command bootstrap is a proposed next distribution step after
the installer review and qualification. It should fetch this script at an
immutable reviewed commit and invoke Bash with an explicit version. This
change does not publish a mutable install endpoint, registry package, OS
package, or new default installer.

Before advertising the bootstrap, qualify its real HTTPS path on both
architectures, including bad-checksum and interrupted-update cases. The
native offline install and uninstall checks have passed. Before making
agenturm select Rust by default, close the retained
workflow and external qualification gates in the
[execution plan](rust-only-execution-plan.md).
