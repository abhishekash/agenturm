# Opt-in Rust Codex session reader

`agenturm-codex-harness` is a separately installed Rust process adapter for one explicitly selected **Codex rollout JSONL session file**. It uses the shared bounded JSONL runtime, the versioned process protocol, a SHA-pinned executable and host-owned durable admission. The built-in Codex reader remains the default. Installing the adapter does not replace it.

Build with the repository's Rust 1.100.0 toolchain, then install a copy for a session file you authorize:

```sh
cargo build --locked --release -p agenturm-codex-harness
python examples/rust_codex_harness_plugin/install.py \
  --binary /absolute/path/to/target/release/agenturm-codex-harness \
  --destination /absolute/path/to/codex-plugin \
  --source /absolute/path/to/rollout.jsonl
agenturm serve --plugin-config /absolute/path/to/codex-plugin/plugins.toml
```

The installer only copies and pins a locally built executable and refuses existing files or symlinked path components. It does not download, launch or activate code. The receiver verifies the artifact, grants and manifest before admitting facts. The process reads the configured file and sends only lifecycle state with a coarse reason code, reported token counters, user/assistant role, tool category/result and recognized check category/result. It never sends prompt text, assistant prose, command strings, paths, tool arguments, output, raw abort reasons or model names. Opaque operation IDs link calls and results, and explicit turn IDs survive restart in the durable cursor.

Completion requires `task_complete` or an `item_completed` `AgentMessage` with `phase: "final_answer"`. Interruption requires `task_aborted` or `turn_aborted`. The coarse reason codes identify which source event was observed. A PR mentioned in prose, a normal assistant message, a tool result, EOF and a silent worker exit never mean completion. Explicit terminal facts at the same timestamp with conflicting states produce an `unknown` session with `ambiguous_lifecycle`.

Late facts carrying an older explicit turn ID retain that fact's turn scope without rewinding the durable current-turn checkpoint. A historical terminal or waiting event becomes a coarse diagnostic instead of changing the current session outcome.

This first slice recognizes `session_meta`, `turn_context`, `event_msg`, modern `token_usage_record` and selected `response_item` rows. It classifies simple allowlisted test/lint/build shell commands into observational check facts using their explicit exit code or status. An output without a boolean `isError` becomes a visible coarse diagnostic; it is never called successful. The legacy cumulative `token_count` row is diagnostic only. A changed repeat of the same modern `response_id` fails before acknowledgment instead of adding the usage twice. Internal `codex-auto-review` files fail visibly rather than publishing ordinary user-session facts.

One complete line is admitted as a unit. The cursor persists byte position, a checked tail digest, full header digest, turn ID and a source generation. A detected replacement, truncation or checked-region rewrite emits `possible_source_gap` and advances the generation, including for a same-inode rewrite. A previously observed missing source/header and malformed identity, time, usage or result metadata fail before acknowledgment. Edits outside the checked region remain an append-only assumption. The process is POSIX-only and tested on Linux x86_64.

This is a **metadata-only, partial** vertical slice. It does not discover the Codex session directory, retain content, reproduce native spans/traces, map every Codex version or event, or prove native Review/export parity. Unknown rows emit coarse diagnostics. It does not automatically replace the built-in reader. See the [validation and parity gaps](../../docs/validation/rust-codex-reader.md). No performance gain is asserted.
