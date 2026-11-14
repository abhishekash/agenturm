# Recognize a finite set of unchanged schema words

Eleven exact built-in ASCII strings used by the operation schema can pass through the sanitizer unchanged after all node, depth, character and UTF-8 byte accounting. Their credential-key check remains false only after the existing maximum-length guard. The fixed set stores no user strings or results. Nonliteral, escaped and subclass inputs retain the general path, and sensitive parent keys still redact their children.

## Equivalence checks

The focused gate passed **46 tests in 9.625 seconds**. It includes 176 complete-output hashes generated from the immutable e795 implementation across literal values, sensitive parents/children, quoting, byte exhaustion, depth and node limits. Additional tests exercise patched small limits and a string subclass whose UTF-8 encoding fails. The tests apply the current credential regexes and redactor directly to each literal, so future policy changes cannot silently invalidate the shortcut. Existing exhaustive Unicode/ASCII/control-obfuscated-key tests still pass.

The source plan received independent read-only review. A separate-process short diagnostic used the same 96 generated traces and exact full tuples/error references. Original ASCII-only f477 source and this candidate alternated in two B→C→C→B groups; every output hash matched. Median development observations were 2.596 and 2.487 seconds. Their spread and ordinary concurrent host work do **not** establish a general throughput improvement. The [complete observations](fact-literal-diagnostic.json) retain all eight timings and exact module hashes. No registered baseline comparison, API p95, cold readiness, monitor CPU/RSS or one-hour acceptance is claimed.
