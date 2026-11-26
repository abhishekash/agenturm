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
The single-file history backup command is supported; workspace backup/restore
and private-content capture remain disabled on macOS.

## Install and update

This direct GitHub Release lane needs no App Store listing or Apple Developer
account. Bootstrap the installer from the project, then have it download the
exact release archive and SHA-256 sidecar over HTTPS:

```sh
curl --fail --location --silent --show-error \
  https://raw.githubusercontent.com/abhishekash/agenturm/main/scripts/install-rust-macos.sh \
  -o /tmp/install-rust-macos.sh
bash /tmp/install-rust-macos.sh install --version VERSION
```

Use `update` with the new exact version to upgrade. The installer checks the
sidecar hash and the archive's target, version, and minimum macOS before it
selects the executable. To verify the checksum separately, download both
`agenturm-VERSION-macos-aarch64.tar.gz` and its `.sha256` sidecar from the same
GitHub Release, then run `shasum -a 256 --check` in their directory.

The binaries are **unsigned and not notarized** because this distribution has
no Apple Developer ID. HTTPS and the release checksum help detect accidental
or in-transit changes; they do not prove the publisher's identity to Gatekeeper.
macOS may block first launch. If it does, try launching `agenturm-rust`, then
approve that app in System Settings → Privacy & Security → Open Anyway. This
creates a per-app exception; keep Gatekeeper enabled and do not remove quarantine
attributes. Apple documents this manual exception for software users trust but
that Apple has not checked ([guide](https://support.apple.com/en-us/102445)).

The installer selects the new version atomically at `~/.local/bin/agenturm-rust`;
data and configuration remain under `~/Library/Application Support/Agenturm/Rust`
and preferences under `~/Library/Preferences/Agenturm`. Explicit
`XDG_STATE_HOME`, `XDG_DATA_HOME`, and `XDG_CONFIG_HOME` overrides remain
honored for managed/testing setups; Agenturm keeps its `agenturm-rust`
subdirectory under those overrides.

Run `agenturm-rust cockpit` (or launch `agenturm-rust` from an interactive
terminal). macOS asks for access to protected folders when the user enables
collection of harness data there. Grant access through the normal macOS
privacy prompt/System Settings.

## Uninstall

```sh
bash /tmp/install-rust-macos.sh uninstall
```

This removes only the installer-owned executable releases and launcher. It
retains collection history, configuration, and UI preferences. To remove that
user data, delete `~/Library/Application Support/Agenturm/Rust` separately
after uninstalling.

On a clean Apple Silicon Mac, run
`scripts/verify-rust-macos-install.sh CURRENT.tar.gz [PREVIOUS.tar.gz]` to
check install, optional upgrade, uninstall, retained history, and launch
without Python or a Rust toolchain. This check does not claim an Apple-verified
publisher identity or replace independent Q10 acceptance.

## Qualification status

`scripts/build-rust-macos-bundle.sh` must run on an Apple Silicon Mac, and the
exact direct-download archive must pass clean-host Q10 acceptance. Linux
cross-compilation does not qualify macOS support. The app does not claim an
Apple-verified publisher identity; users can use macOS's per-app approval if
Gatekeeper blocks the downloaded command. Until a native Q10 record exists,
macOS remains an implementation candidate rather than a supported public
platform.
