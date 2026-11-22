# Rust decoder installation proposal

B8 covers installing a separately packaged **trusted** adapter process that emits an already supported observation format. The Rust host selects and validates its bundled decoder. Adding support for a new format in the host and activating untrusted executable code have their own trust and isolation gates.

## Installation and selection

1. Accept an explicit local package manifest and a regular executable. The manifest declares its plugin ID, supported platform, protocol version, required source features, observation format and artifact digest. Resolve paths inside the package and enforce the current catalog and executable bounds. Require an explicitly supplied expected digest or a trusted signature configured by the operator. A digest attached by an unknown publisher identifies bytes and carries no trust assertion.
2. Verify the format against the host registry, the platform and protocol before installing. Keep the accepted executable in a private managed artifact directory keyed by the verified digest. Install using a temporary file in that directory and atomically publish the completed file. Reject symlinks, path escape, duplicate logical IDs and incompatible manifests.
3. Publish a version 2 collector catalog entry only after the executable is durable. Update the catalog atomically under a writer lock. The collector keeps the existing explicit source-level plugin selection. Installing an artifact alone starts no process, assigns no discovery root, and grants no content access.
4. Use the existing pre-open and pre-launch pin checks, configured features handshake, and host-side observation validation. A pin, protocol or format mismatch fails closed. Updating the pin requires another explicit install. A previous pin can be selected again if its artifact is still present.
5. Remove a catalog selection before garbage-collecting an artifact. Keep running collectors and follow configurations safe across updates. A reloaded collector either recognizes the same complete catalog or starts from a fully published replacement.

The trust decision owner is the release owner. Approval covers the trust anchor and package origin, code execution permissions, artifact update policy and OS sandbox for untrusted code. Independent trust, runtime decoder loading and remote package resolution stay pending until approved.

## Acceptance

Build an independent adapter separately, install it from a manifest, select it in a source config, and reopen collected facts. Exercise an unsupported format, a mismatched platform or handshake, changed artifact bytes, a conflicting plugin ID, and an interruption before catalog publication. Verify that the active selector never resolves to a partial artifact. Use the same bounded validation, commit-before-ACK and reopen tests as the bundled adapters. [Existing source catalog](rust-collection-config.md) · [Process extension trust boundary](rust-process-extension-plugins.md)
