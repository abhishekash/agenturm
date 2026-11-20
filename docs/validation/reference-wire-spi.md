# Trusted reference wire checkpoint

20 November 2026. A host-owned `BatchHandlerRegistry` can now register a versioned reference family on a common bounded `reference_refs` process envelope. The receiver chooses the handler from the configured feature and binds it to a trusted capture mode and admission provider before creating the Store. The child cannot name a provider or install a handler in its manifest or configuration.

## Contract

The ordinary batch still rejects extra fields. The common envelope has exactly `schema_version: 1` and `refs`, with at most 48 references and 64 KiB of encoded envelope data inside the existing framed JSON, nesting, duplicate-key and structural-token limits. A trusted family validator checks the reference shape and returns typed `ReferencePollBatch` data. The session checks the returned feature against its selected capture mode and passes only that mode's provider and host-derived grant into the existing atomic admission transaction. Unknown `*-ref/N` features, conflicting envelope handlers, and a reference handler without a matching capture mode fail before process startup. Pi content and command envelopes keep their separate existing validators and semantics.

The synthetic audit family sends only a fixed nonprivate token. Its installed process fixture proves a committed cursor/evidence/audit receipt, exact replay without a new dirty sequence, and source revocation that erases the receipt and fences the old lease. It also proves unknown family rejection before Store creation, unapproved provider rejection, malformed envelope rejection, and that the receiver passes its trusted handler registry into supervision. No test reads private source content through this family.

## Limits

This is a transport and dispatch extension point, not a general permission for private content. A new private-content harness still needs source-range proof against its configured artifact, a bounded redaction/read policy, source-specific replay and retention handling, and a tested revocation path before activation. The default receiver installs only Pi reference families and providers. Python `Store` remains the production writer; no Rust cutover or performance gain is claimed. GitHub CI results are separate from the local tests.

The [HTML checkpoint](reference-wire-spi.html) is the responsive review copy.
