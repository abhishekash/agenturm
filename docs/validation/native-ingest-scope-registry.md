# Trusted native ingest scope registry

Recorded 20 November 2026. This checkpoint moves the shared native capture-scope list out of the generic Store service. The in-process JSONL collector declares `observatory.local_agents`; the OpenCode adapter declares `observatory.opencode`. Store loads only declarations from Agenturm's installed `observatory.adapters` package, validates and deduplicates their bounded scope names, and uses the same immutable set for native capture eligibility, no-op repair, inline summary publication, generic agent-node suppression, and exact capability source filtering. This registry does not grant an independently installed process plugin native authority.

## Durable owner fence

Store builds the six span/log owner-clearing SQLite triggers from the validated scope set. An unchanged, complete set needs no DDL on startup. On a legacy upgrade, scope change, or missing trigger, Store first takes SQLite's writer lock, then reads the marker and trigger set, replaces the fences, resets capture-owner claims if needed, and records the marker in that same transaction. A second Store cannot make a stale reset decision while the first migrates and re-establishes a claim. A Store using an older scope set refuses trusted native writes and exact capability-source reads after another Store installs new declarations. Raw OTLP data cannot acquire a capture owner by claiming a scope name; only a trusted native writer creates that claim after its row is accepted. A raw replacement or deletion clears the claim before it can be used for a capability aggregate.

For an older database with the same six triggers and no registry marker, the known two-scope set is treated as the prior version, so existing valid owner claims survive the upgrade. Repeated startup with an unchanged set also preserves claims. If a scope is added or removed, owner claims are cleared conservatively; the active collector must re-ingest facts to establish them again. This can temporarily reduce Prompt signals coverage, which remains partial rather than reporting completeness.

The local collector's stable record-ID digest, native file-position ordering and legacy log cleanup remain explicit compatibility rules in `core.py` and `native_policy.py`. They are tied to the old `observatory.local_agents` storage format. Process adapters still use the separate Store admission and lease protocol. This slice does not make a third-party native Python adapter an external plugin, and it does not move database writer authority to Rust.

## Verification

- Five registry tests cover trusted declarations, duplicate and SQL-unsafe rejection before Store creation, preservation of existing owner claims across a legacy-marker upgrade and repeated startup, raw span/log revocation, scope-set changes, stale writer rejection, and synthetic contributed-scope triggers. One test starts two SQLite initializers concurrently and verifies the second cannot read the prior marker before the first commits or erase a re-established claim. A fault-injection test verifies a failed trigger replacement rolls back schema, marker and owner claims together.
- Thirty-three native no-op, atomic publication and storage retry tests passed.
- Seven Prompt signals and registry tests passed, including the source-scoped capability path.

No performance gain is asserted. Full native-to-process Review/export parity, the default reader cutover, and Rust storage authority remain open.
