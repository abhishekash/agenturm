# Agenturm Rust bundle for Apple Silicon macOS

This distribution lane targets **Apple Silicon only** (`aarch64-apple-darwin`)
and requires **macOS 13 Ventura or newer**. Intel Macs and Rosetta are not
supported. The bundle contains native Rust executables and the five bundled
session harnesses; it does not need Python or a Rust toolchain at runtime.

The supported collector is the foreground cockpit: launch `agenturm-rust` in a
terminal and leave it open while collection is needed. Closing the cockpit
stops collection; reopen it to resume. There is no managed background service
on macOS in this release lane. Linux-only systemd, `/proc`, and process
inspection commands are unavailable. Release metadata lists these limits.

## Install and update

Download the exact version's `agenturm-VERSION-macos-aarch64.tar.gz` and its
`.sha256` sidecar from the project's GitHub release. The distributed release
must be Developer ID signed and notarized before it is treated as a supported
download. Verify the checksum, extract the archive, then install or update
with the installer included in that archive:

```sh
tar -xzf ~/Downloads/agenturm-VERSION-macos-aarch64.tar.gz -C ~/Downloads
sh ~/Downloads/agenturm-VERSION-macos-aarch64/install-rust-macos.sh install --version VERSION \
  --archive ~/Downloads/agenturm-VERSION-macos-aarch64.tar.gz \
  --checksum ~/Downloads/agenturm-VERSION-macos-aarch64.tar.gz.sha256
```

The installer verifies the checksum, target metadata, archive contents, and
ownership of its managed paths. It selects the new version atomically at
`~/.local/bin/agenturm-rust`; data and configuration remain under
`~/Library/Application Support/Agenturm/Rust` and preferences under
`~/Library/Preferences/Agenturm`. Explicit `XDG_STATE_HOME`, `XDG_DATA_HOME`,
and `XDG_CONFIG_HOME` overrides remain honored for managed/testing setups;
Agenturm keeps its `agenturm-rust` subdirectory under those overrides.

Run `agenturm-rust cockpit` (or launch `agenturm-rust` from an interactive
terminal). macOS asks for access to protected folders when the user enables
collection of harness data there. Grant access through the normal macOS
privacy prompt/System Settings. Do not remove quarantine attributes or disable
Gatekeeper. If macOS reports that the developer cannot be verified, stop and
obtain a correctly signed, notarized release from the project; do not bypass
the warning.

## Uninstall

```sh
sh /path/to/extracted/agenturm-VERSION-macos-aarch64/install-rust-macos.sh uninstall
```

This removes only the installer-owned executable releases and launcher. It
retains collection history, configuration, and UI preferences. To remove that
user data, delete `~/Library/Application Support/Agenturm/Rust` separately
after uninstalling.

For release preparation on an Apple Silicon signing host, configure the
Developer ID Application certificate in the login keychain and a notarytool
keychain profile, then run:

```sh
APPLE_DEVELOPER_IDENTITY='Developer ID Application: Example (TEAMID)' \
APPLE_NOTARY_PROFILE='agenturm-notary' \
  scripts/sign-notarize-rust-macos-bundle.sh \
  dist/agenturm-VERSION-macos-aarch64.tar.gz
```

The signing step refreshes the pinned adapter digests after signing, submits
the bundle to Apple's notary service, and only replaces the candidate archive
and checksum after notarization succeeds. Gatekeeper must be able to verify
the signed executables online on first use; do not remove quarantine or disable
Gatekeeper.

## Qualification status

`scripts/build-rust-macos-bundle.sh` must run on an Apple Silicon Mac, and the
exact signed, notarized archive must pass the clean-host Q10 acceptance. Linux
cross-compilation does not qualify macOS support. Until a native Q10 record
exists, macOS remains an implementation candidate rather than a supported
public platform.
