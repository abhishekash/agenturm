# Trusted local Rust adapter installation

`agenturm adapter install` installs an independently packaged trusted adapter
from local files into a private catalog. The adapter emits an observation
format supported by the running Rust host. Installing does not start the
executable, add a source, or establish a content grant.

## Package and install

The package contains a regular executable and a version 1 JSON manifest.
The executable path is relative to the manifest's directory.

```json
{
  "version": 1,
  "plugin_id": "example.my-adapter",
  "platform": "linux-x86_64",
  "adapter": "bin/my-adapter",
  "sha256": "<64 lowercase hexadecimal characters>",
  "protocol": 1,
  "features": ["source-adapter/1", "configured-root/1"],
  "observation_format": "agenturm-observations/1"
}
```

Run the installer with an expected executable SHA-256 obtained through a
trusted channel. The manifest's digest and the actual executable bytes must
match that pin.

```sh
agenturm adapter install --manifest ./package/manifest.json \
  --sha256 "$expected_sha256" --root "$HOME/.local/state/agenturm-adapters"
```

The root's parent must exist. The installer creates the managed root and
artifact directory as mode `0700`. Existing managed directories must have
that mode and belong to the current user. It places the verified executable
at `artifacts/<sha256>` with mode `0500`. It refuses symlinks in package and
managed paths and parent traversal. The artifact limit is 256 MiB, and
manifests and catalogs are limited to 64 KiB.

The installer verifies the manifest platform against the current operating
system and architecture. It validates the plugin ID, protocol, declared
features and observation format against the collector's contract. Platform
declarations are package metadata. The collector verifies the executable's
actual handshake when explicitly selected.

## Select or replace an adapter

On success the installer prints the path to its version 1 `catalog.json`.
Set that path as `plugin_catalog` in a version 2 collection configuration,
and set the desired entry's `plugin_id` in a source with its own source ID
and root. The collector verifies catalog artifacts before opening history
and before spawning the chosen adapter. An incompatible handshake or changed
artifact cannot be admitted. [Collection configuration](rust-collection-config.md)

Catalog writers take an exclusive lock. The installer synchronizes and
publishes the full artifact before atomically replacing the catalog. A
concurrent reader sees either complete catalog. An interrupted installation
may leave an unselected artifact, which the existing catalog cannot launch.
Reinstalling the same manifest and bytes is idempotent. To replace a plugin's
pin, supply both the expected new digest and the current digest using
`--replace-sha256`. Keeping prior artifact files lets an already running
collector continue with a previous pin. Restart the collector to use the
updated selection.

The package executable inherits the collector user's OS permissions when
launched. The operator decides which package and digest to trust. Remote
package downloads, signature provisioning, new decoder formats, garbage
collection, and untrusted activation remain separate. [Process extension trust boundary](rust-process-extension-plugins.md)

## Inspect and unregister installed adapters

Use the same explicit private root to inspect the catalog. These commands do
not run an executable or create a missing root:

```sh
agenturm adapter list --root "$HOME/.local/state/agenturm-adapters"
agenturm adapter show --root "$HOME/.local/state/agenturm-adapters" \
  --plugin-id example.my-adapter --json
agenturm adapter remove --root "$HOME/.local/state/agenturm-adapters" \
  --plugin-id example.my-adapter --sha256 "$current_sha256"
```

`list` is ordered by plugin ID. `show` reports the registered pin, artifact,
protocol, features and format. Add `--json` to any management command for
machine-readable output.

Removal compares the supplied current pin while holding the installation lock.
A stale pin fails without unregistering a concurrent replacement. The catalog
is published atomically and unrelated registrations remain. Removing the last
entry leaves a valid empty managed catalog; new collection still requires a
nonempty catalog with the selected plugin registered. Reinstall into that root
to add entries again.

Unregistering preserves artifacts, retained history and content approvals. It
changes future configuration selection; it does not stop an already running
collector or revoke its grants. Restart collection to apply a new selection.
Symlink, ownership and private-directory checks still apply to management.

## Verification

On Ubuntu 24.04.5 x86-64 with Rust 1.100.0, the independently built fixture
adapter installed successfully. It collected, ACKed and reopened evidence
after the package executable was removed. A second poll added no duplicate
facts. A manifest with incompatible features failed the collector handshake.
Changing the managed executable failed its pin check before a history file
was opened. Installer tests cover manifest bounds, symlinks, a held catalog
lock, staged partial files, and pin replacement with the original artifact
retained.

The list/show/remove follow-up passed four focused development tests and strict
app library/binary Clippy. These checks include current-pin replacement races,
held locks, path substitution, empty-catalog reinstall and artifact/history
preservation. Shipped-binary and live collection acceptance is delegated in the
[core feature test plan](../validation/rust-core-feature-test-plan.md#cf3--installed-adapter-lifecycle).
