# Agent 4: Apple Silicon macOS support

## Completion update · 26 November 2026

Implementation is available on branch `feature/rust-macos-apple-silicon` in
PR [#412](https://github.com/abhishekash/agenturm/pull/412), based on
`fd398c193d1e1c6f42bb20990a84bbfb75c1674c`. The branch contains the native
`aarch64-apple-darwin` bundle, macOS 13 minimum, foreground collector, direct
HTTPS installer/update/uninstall, and Linux/macOS-separated release metadata.
Intel Macs and Rosetta remain excluded. The M1 macOS 15 workflow built and
verified the 0.3.9 candidate, including launch without Python/Rust on the child
`PATH`, fixture collection, query/reopen, backup, inspector navigation,
terminal restoration, install/update/uninstall, and data retention. PR checks
are green. The bundle is unsigned and not notarized; macOS may require a
per-app Gatekeeper exception. Independent clean-host Q10 acceptance is still
open, so this is not yet a public support claim or release.

## Goal

Implement the macOS distribution path for **Apple Silicon only** alongside
Linux. Intel Macs (`x86_64-apple-darwin`) are out of scope. This is a platform
support lane, not permission to add unrelated product features.

## Own

- Execute the D2-Mac/F5 implementation scope in
  [`rust-migration-remaining-plan.md`](../rfcs/rust-migration-remaining-plan.md#f5-macos-distribution-and-integration):
  Apple Silicon build artifact, native paths, bundle/install/update/uninstall,
  and the documented foreground or `launchd` collection choice.
- Define the minimum supported macOS version and use macOS-native config/data
  locations. Do not assume Linux paths, `systemd`, or Linux-only process
  controls.
- Provide the normal documented install path, including signing/notarization
  and Gatekeeper behavior as required. Do not make disabling platform
  protections the standard installation instruction.
- Keep unsupported architectures and capabilities disabled and clearly
  identify them in release metadata.

## Do not own

- Linux implementation or qualification, Rust feature parity, remote
  connector profiles, or provider evidence work.
- Claim macOS support based on cross-compilation alone. Native acceptance
  requires a clean Apple Silicon macOS host and the exact release artifact.
- Build Intel artifacts or add Rosetta-specific support.

## Parallel boundaries

Keep platform packaging, paths, service integration, and platform-specific
documentation in their own files. Avoid edits to shared core behavior unless a
macOS blocker is demonstrated and coordinated with the integration owner.
Native-runner-dependent checks may be recorded as blocked on runner access;
do not fabricate a pass.

## Deliverable

Report the base SHA, target triple (`aarch64-apple-darwin`), minimum OS,
artifact/install approach, changed files, checks run on native hardware, and
remaining signing, service, or host acceptance gaps.

## Done when

The exact Apple Silicon artifact installs and runs without Python or a Rust
toolchain; supported native collection, restart/reopen, query, inspect,
backup, upgrade, uninstall, cockpit navigation, and terminal restoration pass
on a clean supported Mac. Public macOS support still waits on the independent
Q10 acceptance record.
