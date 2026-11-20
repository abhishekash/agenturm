# Process Pi Activity export provenance

Recorded 20 November 2026. This is a content-free provenance slice for the existing process Pi reader. It does not provide native Pi Review/export content parity. The process protocol still carries only admitted, coarse facts. Prompt text, tool arguments, output, commands and paths are not transported or stored by this reader.

## What is available

For a selected process Activity event, the JSON export now includes the receiver's opaque event, turn and operation IDs, its source type and event source ID, and an explicit `source_provenance` object. The provenance object contains only the configured process source ID, fixed `content-free` / `partial` labels, typed observation flags, and a recognized tool-turn relation. It never copies arbitrary runtime metadata. Activity's plain-text copy also shows the configured source, opaque turn and recognized relation when available.

The proof endpoint reads **one exact event key** from the Python Store. It joins the published Activity row to its source cursor and projection cursor inside one read transaction, requiring capture enabled and matching policy epoch and source sequence. It returns the receiver identity and source revision tuple `(lease_epoch, policy_epoch, dirty_seq)` with the admitted projected row. The HTTP response also checks the database file identity, so replacing the SQLite file with one bearing the same receiver ID invalidates the proof. The TUI compares the exact receiver, event, source, session, agent, service and exported fields against the selected row. A failed read, changed field, pending projection, receiver swap or capture revocation denies app-mediated copy and export. The export dialog checks the same proof at opening, confirmation, in its writer thread and before its Ctrl+C or Super+C action copies selected preview text. The Python Store remains the sole database writer.

The export remains a **frozen loaded snapshot**. It is not an independent attestation, a complete history export, or a guarantee about the source after the final proof check. There is a possible source change between the final check and local file creation; eliminating that race requires a receiver-owned export transaction. The file preview says the current capture policy is not rechecked inside the file itself. Once text has been displayed, Agenturm cannot retract what was seen or prevent terminal-level mouse selection outside its copy action. The user must still review the unredacted JSON before sharing. Existing native Activity export policy is outside this slice.

## Trust boundary and remaining stages

| Stage | Trust and capture rule | Gate |
| --- | --- | --- |
| This slice | An untrusted harness supplies only protocol v1 typed facts. Python admission validates them, projects content-free Activity, and proves the current source revision before TUI copy/export. | Exact source, event, session and receiver; no arbitrary metadata; revocation and correction invalidate stale rows. |
| Captured content | Add a separately versioned transport and explicit, source-specific opt-in grant. Bound bytes and records; redact on the receiver side; store exact source references and policy epochs; erase on revocation. | Review content must be traceable to one retained source and policy-current at display and export. Never enable through a metadata field or by default. |
| Native Pi parity | Compare native and process paths over a broader Pi corpus, including branch edits, late results, missing/ambiguous calls, malformed rows, source gaps and capture changes. | Review/export must identify every supported content kind, its exact source and any missing coverage. Native span and trace membership still differ. |

The existing Pi process fixture proves late result turn correlation only when one retained earlier call with the same source, session and operation exists. `matched_call` is a receiver-derived relation, not a native trace proof. Other relations, including `missing_call` and `ambiguous_call`, keep the turn uncertain. Source rewrites and retention gaps cannot reconstruct lost history.

## Verification

The focused tests create two process sources, a late result and an independent session. They check exact session filtering, opaque operation and turn IDs, the `matched_call` relation, omission of injected prompt/output/path metadata, source and receiver spoof rejection, a corrected row, capture revocation while an export is queued, non-object projected payloads, and a same-receiver database-file replacement. Real Ctrl+C key journeys prove that selected process Activity and export-preview text cannot bypass the proof after revocation, while ordinary selected text in Agents still copies. The source provenance allowlist accepts the full 256-character identity bound admitted by the protocol. No performance claim is made.
