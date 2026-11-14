# Shared operation evidence: first implementation increment

Status: C1 merged in [PR #75](https://github.com/abhishekash/agenturm/pull/75), commit `a601ad2236f22ef75600129e920ff763883bee7c`. All twelve CI jobs passed on code head `2ead5f2`, including Python 3.11–3.14 and native SDK interoperability. Findings ownership (C2), source-backed Overview (C3), and stages D–G remain open. This record makes no release or whole-monitor performance claim.

## User-visible behavior

Review checks, Activity failure groups, and recent unseen-error reminders use one typed operation projection. Command arguments, exit codes, and captured output retain their roles. A JSON object printed by a command cannot claim to be that command's input or exit status. Recorded `null`, `false`, zero, and empty MCP payloads retain presence; an ignored lower-priority output alias cannot replace them.

Grouping requires complete, unambiguous, sanitized inputs, captured cwd for shell operations, tool identity and MCP server identity, exact receiver, producer, actor, session and execution scope. Different arguments, directories or workers remain separate. Redacted/clipped inputs and insufficient error signatures stay occurrence-specific. A strictly later matching success can suppress a reminder and appears as an observation in failure groups. It does not delete archival failures or establish that their consequences were repaired. Equal timestamps do not prove order.

Qualified simple `rg`, `grep`, and `diff` exit 1 results are informational. Compound commands, `diff --check`, blocked/denied operations, conflicting terminal evidence and MCP errors retain uncertainty or failure. Display labels alone cannot prove passing checks or operation equivalence.

## Capture and ownership

The versioned role body bounds input and output independently, then bounds the complete serialized body to 8,000 bytes. Omitted container fields and clipped strings are explicit. Sanitization precedes correlation fingerprints; compact reminder references contain generic operation labels, identities and versioned hashes, without arguments or stderr. Native occurrence IDs and viewed-error receipts retain their existing identity.

Exact event identity overrides inherited aliases as a family. An event changing actor, runtime, namespace or session cannot borrow its container's turn, command or terminal result. Descriptive worker metadata does not silently change canonical identity. Receiver namespaces are read inside the same database snapshot as projected evidence, including after file replacement.

## Existing-store upgrade

`activity_operation_facts_v2` queues source span/log identities once, without scanning their payload JSON during initialization. The existing bounded durable repair queue reclassifies indexed results, including older unknown SDK rows with a recorded failing exit code. `trace_derived_v14` rebuilds derived summaries with the existing resumable cursor and atomic 16-trace chunks. Until repair is ready, readers retain the existing readiness contract rather than claiming partial results are complete.

Each trace retains at most 20 pending failure references and 20 latest distinct qualified successful-operation references. Owner-level reminders compare the newest two retained traces. This is a bounded recent reminder window, not exhaustive recovery analysis. Older records without complete capture provenance cannot prove equivalence.

## Verification

The focused ownership, projection, migration, receipt and Review gate passed 67 tests. A further 34-test pure/source/correlation gate passed in 3.166 seconds. Additional source tests exercise existing-index reclassification through `errors_only` and Review, cross-step same-turn success in session summaries, native emission of omitted container elements, and strict MCP chosen-output classification. The final 56-test source/native/Review gate passed in 28.434 seconds. Independent peer review found no remaining blockers: 34 tests passed in 3.230 seconds, interrupted rebuild passed, and receiver replacement, identity-family fencing, omitted native containers and falsy-output probes passed. The initial broad run exposed five obsolete positive fixtures/presentation assertions; they were corrected to use semantic command evidence and typed-output assertions. This also exposed a real typed-output display omission, now fixed: empty/falsy/numeric results remain visible without becoming distinguishing error signatures. The 87-test regression gate passed in 47.456 seconds; all twelve final CI jobs passed. Both HTML companions were checked in Chrome at 1280 px/light and 390 px/dark: no page overflow or external assets; the mobile report was visually inspected.

Fixtures use isolated stores and native files. Personal telemetry, workflow receipts and the user-owned review notes were not modified.
