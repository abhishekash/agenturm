# Rust Codex process reader vertical slice

Recorded 20 November 2026. This checkpoint adds `rust/agenturm-codex-harness`, a separately installed Rust reader for one explicitly configured actual Codex rollout JSONL file. It uses the shared bounded JSONL runtime, framed source protocol, SHA-pinned manifest, Python Store admission and policy-current projection. Native Codex remains the default. This is not a full Codex migration or a speed claim.

## Evidence boundary and outcome provenance

The adapter reads `session_meta`, turn markers, modern reported usage, message roles, selected tool calls and completed items. It emits content-free `message`, `tool`, `check`, `usage` and `lifecycle` facts. Tool call IDs are hashed before crossing the child pipe; the host derives scoped, opaque operation and turn keys. Command, path, prompt, answer, output, model and raw reason text stay out of admitted facts. The installed test compares the 0.167.1-shaped synthetic Codex rollout with the built-in native reader for an explicit completed outcome and verifies usage, checks, Activity drilldown, partial coverage and private-content exclusion.

`task_complete` reports `completed/codex_task_complete`; final-answer phase reports `completed/codex_final_answer`. `task_aborted` and `turn_aborted` report `interrupted` with distinct coarse reason codes. An ordinary assistant message that says a PR was raised leaves the session running. Conflicting terminal facts at one timestamp project `unknown/ambiguous_lifecycle`. A tool result with no explicit boolean error flag emits `codex_tool_result_unknown` instead of an invented success. Modern usage is keyed by response ID: a changed duplicate fails before acknowledgment, preserving the prior counters.

An explicit turn ID on a late usage, tool or message fact scopes that fact but does not move the durable current-turn checkpoint. Only task-start and new user-message boundaries change it. A late terminal or waiting event for an older turn becomes a coarse historical diagnostic, so it cannot overwrite the current session outcome. Installed restart tests cover older-turn usage followed by unmarked current-turn tool activity.

The shared runtime admits all facts from a complete source line together, or leaves that line unread. Restart preserves an explicit turn. Detected truncation, replacement, header change or nearby rewrite emits `possible_source_gap` and creates a distinct event generation, even when the same inode and native row IDs recur. A malformed timestamp, identity, counter or unsupported internal `codex-auto-review` context fails visibly. The Python receiver still owns leases, policy epochs, cursor commit, retention, Activity projection and every database write.

## Remaining parity gates

| Area | Current slice | Required before production replacement |
| --- | --- | --- |
| Discovery | One explicit file | Bounded Codex sessions-directory discovery, multiple active threads and source ownership. |
| Identity | Session thread and explicit turn; opaque operation correlation | Subagent parent/worker and fork relationship corpus across Codex versions. |
| Activity | Content-free role, tool and recognized check outcomes | Full native action mapping, trace/span membership, prompt and tool evidence under capture policy. |
| Usage | Modern per-response counters; changed repeats fail closed | Legacy cumulative usage and correction revisions without double counting. |
| Outcomes | Explicit complete/final-answer/abort events with coarse reason codes | Versioned terminal semantics and source-backed Review/export conformance. |
| Source changes | Checked-header/tail gap generation | Distant in-place edit and rotated unread-data recovery. |
| Platform | POSIX executable, Linux x86_64 installed test | Cross-platform packaging, release qualification and retained-history behavior. |

Unknown rows generate coarse diagnostics. Source lines and scan are each capped at 1 MiB, complete-line fact batches at 256 records, the receiver JSON structure at 8,192 tokens and the cursor at 512 characters. The adapter makes no claim of complete history or performance improvement.

## Verification

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `cargo test --workspace --locked` with Rust 1.100.0.
- `cargo build --locked -p agenturm-codex-harness` followed by `AGENTURM_RUST_CODEX_BIN=... python -m unittest tests.test_rust_codex_harness_plugin -v` against the installed SHA-pinned artifact and disposable receiver.
- Existing native Codex and shared Pi process regression tests.
