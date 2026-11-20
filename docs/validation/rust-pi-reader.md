# Rust Pi process reader vertical slice

Recorded 20 November 2026. This checkpoint adds `rust/agenturm-pi-harness`, a separately installed Rust reader for one explicitly configured actual Pi session JSONL file. It uses the existing framed process protocol, host admission, one Python Store writer, policy-current projection and opt-in manifest. It does not replace the built-in Pi reader.

The bounded JSONL tail, cursor and poll/ack machinery now live in the shared [`agenturm-jsonl-source` runtime](rust-jsonl-source-runtime.md); Pi retains only its format translator and executable binding.

The installed-artifact conformance fixture uses Pi `session` and `message` rows with a user turn, an assistant message containing reported usage and two tool calls, two tool results, and an explicit error stop. A native Pi session and the Rust process path both report an interrupted session and four tool Activity observations. The process path reports the supported usage counters, correlates call/result phases through opaque operation IDs, and exposes exact turn Activity and a partial Review subset. It sends no prompt, argument, tool name, path or output into the host Store. A restart keeps the last user turn through the durable cursor; a subsequent explicit stop belongs to that turn. Replacement emits a coarse gap diagnostic and starts a new session and turn.

The parser emits all facts from a complete Pi line as one cursor unit. At a record, byte or structural budget boundary, it leaves the whole line unread and acknowledges only prior complete lines. A single line that cannot fit fails visibly without advancing the durable cursor. The cursor checks file identity, offset, preceding 256-byte digest and full header digest. The header digest prevents a distant session-header rewrite from inheriting a prior turn. A durable source generation changes at each detected gap, separating event IDs even when same-inode truncation reuses a native Pi row ID. An installed test exercises that case through host admission. Unsupported row kinds and roles emit coarse diagnostics. A malformed explicit result flag prevents an invented success. Capture revocation erases the projected data and fences the old worker.

## Late tool result correlation · 20 November 2026

A Pi result can appear after another user message. The Pi process reader now omits `turn_id` on the result fact instead of assigning the current, later turn. The host reconstructs the result's turn from exactly one policy-current, retained, admitted `started` tool fact with the same source, native session and operation ID. The call must have an earlier source timestamp and no later admission timestamp; equal source timestamps do not prove order. A missing, reused, future or conflicting call leaves a Pi result's turn unavailable and exposes `tool_turn_relation` as `missing_call`, `ambiguous_call`, `future_call`, `unproven_order`, `conflicting_turn` or `call_turn_unavailable`. A successful link says `matched_call`. Other adapters may supply an explicit turn on a terminal tool fact without a separate started fact; when there is no call candidate, the host retains that producer assertion with `unverified_native_turn` rather than claiming correlation. The bundled Pi process ID is a narrow legacy exception: older retained Pi results borrowed the later current turn, so their raw turn is withheld when no call survives. The original raw turn remains distinguishable via `native_turn_observed`; the host does not store a second mutable call map or put call IDs into the process cursor. Derived Activity and Review use the same policy-current projection, and text export still labels process coverage partial. Category can be recovered from the call when the result reports `other`; contradictory coarse categories publish `other` with a conflict marker. A correction rebuilds the relation from latest retained revisions; revocation and source gaps remove it with the Activity row. The `plugin_activity_projection_v3` upgrade marker hides old projected Activity until it is rebuilt from retained evidence, without requiring a new source write.

The installed fixture compares one native Pi file with the restarted Rust process reader. Both report the same running lifecycle after a late failed result, and the native `execute_tool` span stays with its original call trace. The process result shares its opaque turn and operation with the original call, not with the later user message. The fixture also pins a pre-existing native inconsistency: the native *log* result Activity entry follows the newer user's trace even though its tool span follows the original call trace; native Activity exposes no `turn_id` for these entries. This slice does not claim native Activity trace parity or repair that native path.

| Source evidence | Process Activity / Review status | Remaining limit |
| --- | --- | --- |
| Session header and user turn IDs | Opaque session/turn identity and exact turn filtering | One configured file; no directory discovery. |
| Assistant message and reported usage | Content-free assistant role and typed session usage counters | No model name, text or native model Activity row. |
| Tool call/result IDs and explicit outcome | Opaque operation link; original turn only with one verified retained call; failed result is reviewable | No native span or trace membership; missing/ambiguous calls retain an unknown turn. |
| Explicit stop/error | Completed/interrupted lifecycle | No terminal state inferred from a tool result or PR. |
| Source rewrite, retention, revocation | Gap/unknown state or withheld Activity, according to source policy | Lost source history cannot be reconstructed. |
| Prompt, arguments, path, output | Never admitted or exported by this process contract | Full source-backed Review/export needs a separately approved capture policy and transport. |

## Remaining parity gates

| Area | Current slice | Required before a production Pi replacement |
| --- | --- | --- |
| Discovery | One explicitly configured file | Bounded directory discovery, ownership conflicts and finite source retention coverage. |
| Identity | Session header, optional worker and last user turn | Broader Pi version corpus, branches and parent/worker relationship conformance. |
| Activity | Content-free role and coarse tool phases | Model Activity, names where policy permits, unsupported Pi event types and exact native action mapping. |
| Review/export | Policy-current partial Activity and opaque operation keys | Native spans, trace membership, captured evidence policy and source-backed review proof. |
| Source changes | Replacement/truncation/header/near-tail rewrite detection with a distinct generation | Explicit treatment of distant in-place edits and rotated unread data. |
| Platform | POSIX executable, Linux x86_64 installed test | Cross-platform packaging, release qualification and retained-history behavior. |

No performance gain or full Pi parity is asserted. The Python Store remains the sole database writer.

## Verification

- On the final rebase onto main `43c4b17`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, and `cargo build --locked -p agenturm-pi-harness` passed. The Pi crate passed 13 Rust tests.
- `AGENTURM_RUST_PI_BIN=... uv run python -m unittest tests.test_rust_pi_harness_plugin -q` passed eight tests against the separately installed current binary. The focused plugin Activity, projection, retention, admission and Pi process boundary set passed 58 Python tests. Those two runs cover 66 tests without a skipped installed fixture.
- Independent review found a cross-adapter turn regression in the shared Activity projection. After the compatibility fix, 19 plugin Activity tests, eight installed Pi tests, and 36 installed Codex/Claude/Kiro/OpenCode tests passed. The four-harness run had one Codex failure before the fix. The additional tests pin Kiro's terminal denied-call turn and older Pi results with no retained call.

An independent Rust reviewer inspected the cursor and whole-line acceptance loop and found that rejected lines retain their offset and context and that gap diagnostics page coherently. That review did not cover the entire Pi translation or installed test surface; those behaviors have the targeted checks above and remain subject to broader Pi corpus validation.
