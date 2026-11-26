# Apple Silicon macOS candidate validation

Recorded 26 November 2026 for source commit
`c1e2e2109f153eec087cd6ce1bff052c20c69c10` (base
`fd398c193d1e1c6f42bb20990a84bbfb75c1674c`). The native Apple Silicon
workflow completed successfully as [run 45876593406](https://github.com/abhishekash/agenturm/actions/runs/45876593406).
It built `agenturm-0.3.9-macos-aarch64.tar.gz` on an M1 `macos-15` runner,
verified the archive checksum, `arm64` executable slices, and the macOS 13
deployment target, ran candidate and install verification, then uploaded the
artifact.

The candidate verifier ran the exact extracted bundle with an empty child
`PATH` and passed native CLI version, Codex/Pi fixture collection, query,
reopen/replay, single-file database backup, inspector navigation, and terminal
restoration. The installer verifier passed first install over an existing
user-owned history database, launch with an empty `PATH`, update, uninstall,
history retention, and preservation of an existing `agenturm` command. Python
was used only by runner-side validation helpers; Agenturm child processes had
no Python or Rust toolchain on `PATH`.

Distribution uses a direct HTTPS GitHub Releases download and SHA-256 sidecar;
the install script supports `curl` bootstrap, exact-version install/update, and
uninstall without Python, Rust, Homebrew, or an Apple account. The archive is
unsigned and not notarized, so macOS may require the user to approve Agenturm
in Privacy & Security on first launch. The runner verified the install flow,
but does not simulate Gatekeeper's quarantine prompt for a downloaded file.
The SHA-256 sidecar checks transfer integrity and is not a publisher signature.
Independent Q10 acceptance remains open. No Gatekeeper protections were
bypassed.
