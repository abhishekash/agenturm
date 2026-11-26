# Apple Silicon macOS candidate validation

Recorded 26 November 2026 for source commit
`2d8b4c3910b471ebb49c860556a7affb60785d08` (base
`fd398c193d1e1c6f42bb20990a84bbfb75c1674c`). The native Apple Silicon
workflow completed as [run 45868463836](https://github.com/abhishekash/agenturm/actions/runs/45868463836).
It built `agenturm-0.3.9-macos-aarch64.tar.gz` on an M1 `macos-15` runner,
verified the archive checksum, `arm64` executable slices, and the macOS 13
deployment target, then uploaded artifact `15140814477`.

The candidate verifier ran the exact extracted bundle with an empty child
`PATH` and passed native CLI version, Codex/Pi fixture collection, query,
reopen/replay, single-file database backup, inspector navigation, and terminal
restoration. Python was used only by the runner-side validation helper; the
Agenturm child processes had no Python or Rust toolchain on `PATH`.

This is native candidate evidence, not release acceptance. The artifact is
unsigned and the standard installer correctly refuses it because Gatekeeper
cannot accept it. No Apple Developer ID or notarization credentials were
available in repository Actions secrets. Signed installation, upgrade from a
previous signed macOS release, uninstall, and independent Q10 acceptance
remain open. No Gatekeeper protections were bypassed.
