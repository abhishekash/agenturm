# Failed full-history HTTP checkpoint at e795

The registered B→C run used the immutable common generated corpus: 5,751 traces, 31,000 spans and 41,000 logs. B was `5e0c39786066fb88c0e270418573e361aeb6cf9a`; C and harness were `e795f064c861cd59b54d1180697e4b19c9959b34`. Separate receiver processes built their own production schema from identical registered raw clones. Authentication, namespaces, index manifests, source invariance, orderly drains and clone cleanup passed.

**Both loaded arms failed. No loaded p95, resource win or readiness target is accepted.** Each first long POST caused the next fixed 100 ms slot to be missed. The writer refused burst catch-up and stopped; most subsequent read attempts therefore ran after actual writer progress ended. Those fast responses are not loaded-latency evidence.

| Arm | Preparation + receiver readiness (s) | Accepted write batches / 420 | Reads begun before final accepted write finished | Successful among those reads | Later reads after writer stopped |
| --- | ---: | ---: | ---: | ---: | ---: |
| B | 430.143 | 44 / 420 | 10 | 0 | 70 |
| C | 353.820 | 35 / 420 | 8 | 4 | 72 |

The readiness field above includes child clone/schema preparation and probing. It is **not** the primary logical-cold or first-useful-TUI measurement, whose preparation exclusion and native-monitor/UI coverage are different. The latter gates remain open.

Accepted writer inputs differed, the writer did not span the read windows, and genuine 503s remained. The harness correctly set both arm validity and overall publication to false and left target results null. No request or failure was filtered out, retried or silently replaced. The measured rate and deadline policy are unchanged.

[The complete generated-only result](full-http-e795-invalid.json.gz) is retained compressed; authentication configurations were excluded, and credential-key/Bearer checks ran before packaging. Uncompressed SHA-256: `29eff78d42425e9a014250e1b766feb90204ccb08fba2f350e12249fd457411b`.

## Follow-up, not a pass

Short diagnostics found ingestion waiting behind long derivations, duplicate local/compatibility repair, repeated SQL setup and repeated semantic-fact projection. Coalescing, atomic raw/identity publication, connection-local preparation and bounded one-trace fact reuse are being tested with exact serialized-output parity. Each component diagnostic has a distinct source/fixture scope. Fresh combined controlled HTTP, cold/idle, storage, terminal and one-hour validation is still required. This failed run remains part of the evidence after any fix.
