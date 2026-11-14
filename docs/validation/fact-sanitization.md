# Equivalent ASCII control filtering in operation facts

The common ASCII case no longer calls the Unicode category function for every character twice: credential-key normalization and captured-text projection use compiled C-level control-character patterns. Text still keeps tab/newline; credential keys remove them. Non-ASCII strings retain the original Unicode category path. Credential detection/redaction, ANSI removal, UTF-8 validation, node/depth/byte bounds and completeness flags are unchanged.

## Verification

The focused operation/Findings/pattern gate passed **43 tests in 8.665 seconds**. New properties cover all 128 ASCII characters and every Unicode code point in bounded windows, including invalid-surrogate omission. Control-obfuscated credential keys remain redacted in structured and quoted forms. This is a semantic-equivalence optimization, not a relaxation of redaction.

A short unprofiled diagnostic alternated **e795 original → candidate → candidate → original**, twice, in separate Python processes with asserted source-import paths. Each arm derived the same 96 generated traces / 576 spans / 672 logs in 16-trace transactions after copying the same seed and resetting only the disposable span-stat cache. Complete summary/activity/Findings/time tuples and serialized error references had one identical output hash in all eight arms.

Median diagnostic time was 3.320 seconds for the e795 original and 2.634 seconds for this source change. The [raw observations](fact-sanitization-diagnostic.json) preserve every arm and both source hashes. These are short development diagnostics under ordinary host load, **not** the registered 5e baseline comparison, full-history cold readiness, API latency, default-monitor CPU/RSS or one-hour acceptance. The failed e795 full HTTP workload remains failed and all resource targets remain open until combined validation.
