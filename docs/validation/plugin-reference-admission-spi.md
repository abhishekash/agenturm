# Trusted reference admission checkpoint

The receiver now has a host-owned `ReferenceAdmissionRegistry` for the reference portion of `commit_batch`. A trusted `CaptureMode` names its provider. The child can submit bounded references through its existing wire format, but cannot register a provider, choose a different one, or supply proof logic through TOML or a manifest. Default capture remains metadata-only.

## Transaction and authority

1. The receiver validates and snapshots the batch. The selected provider validates the exact reference shape and binds its grant to the active source lease. Mutable Pi content references are snapshotted before digest and proof.
2. For requested references, the receiver checks the source lease and member checkpoint, then the provider independently reads and proves the configured source outside the SQLite writer transaction.
3. Inside one `BEGIN IMMEDIATE` writer transaction, the receiver rechecks receiver identity, source lease, policy epoch, capture enablement, fingerprint, cursor, and member checkpoint. It writes evidence, member cursor, dirty marker, provider rows, and replay receipts together. Any provider failure rolls back the whole batch.
4. Exact replay checks every installed provider receipt before acknowledgement, so a previously acknowledged cursor cannot gain or change reference content.

Pi user text and shell command proof, redaction, orphan/capacity pruning, statuses, and receipts live in Pi-owned provider implementations. The Pi content provider explicitly preserves the existing shared `unanchored_origin` diagnostic when a Pi command source cannot be anchored; unrelated providers cannot set that Pi status. Legacy direct `content_refs` and `command_refs` calls still use the same providers. The synthetic trusted audit provider demonstrates registration and atomic dispatch without reading or persisting private content.

## Scope and limits

This is an admission SPI, not a general third-party private content API. Schema initialization, configuration change and revocation cleanup, and the process wire families remain Pi-specific. The synthetic provider's fixed, nonprivate receipt remains after source revocation even though its metadata is erased and its lease is fenced; no private provider may depend on this incomplete lifecycle. A new private-content harness needs those lifecycle and transport gates before it can be enabled. The fixture provider exists only in tests; the default receiver installs Pi providers only. There is no performance claim or production cutover here.

## Verification

- Broad `test_plugin_*.py` suite: **224 tests passed** before the two diagnostic regression cases. The focused admission, Pi, capture-mode, runtime, supervisor, and SPI suite passed **66 tests** after those cases.
- Installed Pi transport/emitter suite: **31 tests passed** after rebase, using freshly built Pi harness and plugin host binaries.
- The SPI tests cover transaction rollback after provider writes, exact replay mismatch, unavailable provider rejection, legacy/new argument mixing rejection, malformed provider ID, mutable Pi reference snapshot, revocation between external verification and writer transaction, and the synthetic lifecycle limit. The diagnostic regression tests prove Pi command `unanchored_origin` persists in the shared Pi status and an unrelated provider reason does not leak into it.

The [HTML checkpoint](plugin-reference-admission-spi.html) presents the same review record in a responsive light/dark layout.
