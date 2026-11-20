# Trusted reference provider lifecycle

20 November 2026. Reference providers now own their schema installation and source erasure hooks. This extends the [reference admission checkpoint](plugin-reference-admission-spi.md); it does not change the default capture mode or enable external private-content providers.

## Host contract

The receiver installs each trusted provider's idempotent schema before a process session activates. `Store` initializes the bundled Pi providers, and `PluginSession.start()` installs the explicitly supplied trusted registry. A provider cannot be installed by TOML or a child process. A failed schema hook rolls back its DDL and prevents source activation.

The host records every installed provider whose commit hook ran for a source batch in `plugin_reference_owner_v1`, in the same transaction as the cursor, evidence, and receipts. This includes providers that received no references in that batch; their ownership remains recorded even if their rows later become empty. Existing Pi content or command rows are adopted when an older database opens. On a capture revocation or a changed capture fingerprint, the host requires all recorded providers and invokes their source-erasure hooks inside the host's writer transaction. Missing providers and failing hooks reject the change and roll back all deletion, policy, and cursor effects. A second `Store` instance must receive the same trusted registry for a custom provider's cleanup; the default registry refuses an unknown provider rather than leaving its rows while revoking capture.

Pi user-text and requested-command tables are now initialized and erased by their respective providers. The generic admission host owns transaction order and durable provider ownership. It never reads or persists private reference content.

## Validation and limits

The focused suite covers provider DDL rollback, atomic revoke rollback, unknown-provider denial, fingerprint reset, stale-lease fencing, and adoption of existing Pi status. A synthetic nonprivate audit provider proves that its receipt is erased on revocation and cannot be bypassed through a default `Store` call. The broad plugin suite and installed Pi transport tests are recorded on the PR.

This is a trusted in-process extension point. The existing process wire still has Pi-specific content and command reference families; new private-content harnesses need a bounded transport contract, source proof, read gate, and an end-to-end revocation test. Older binaries do not know these hooks or the owner table, so this is not an old-writer exclusion or Rust storage cutover proof. Python `Store` remains the production writer.

The [HTML checkpoint](reference-provider-lifecycle.html) is a self-contained review copy.
