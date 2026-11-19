# Plugin capture-policy fence checkpoint

19 November 2026 · branch `architecture/plugin-policy-fence`

An explicitly activated plugin source now has a persisted capture-enabled flag and policy epoch. Admission checks both the source lease and policy epoch inside the Store write transaction, and stamps accepted evidence with that epoch. Changing the policy increments the epoch and changes the lease, so a batch produced by the prior worker cannot advance the durable cursor after revocation.

Disabling capture atomically deletes that source's metadata evidence, preserves its last acknowledged cursor, and increments the dirty sequence for downstream invalidation. Re-enabling capture requires a fresh activation and resumes from that cursor. Repeating the same policy setting has no write or fencing effect. This is a host-owned control; the plugin wire protocol cannot change it.

Validation includes source-ownership checks, old-schema migration, revocation after a committed batch, stale-worker rejection, refusal to activate while revoked, and fresh-lease resume. The focused plugin suite passed locally. Existing native collection and UI remain on the previous path. The projection consumer must honor the dirty sequence and policy epoch before this source can appear in live Sessions.

Limits: the current opt-in process host does not yet receive an asynchronous policy-change control message. Revocation prevents the next Store commit and future activation, but the host must separately terminate or drain its already running child. Content capture remains unavailable through this inbox; only the admitted typed metadata can be stored.
