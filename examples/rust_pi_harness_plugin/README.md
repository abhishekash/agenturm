# Opt-in Rust Pi session reader

`agenturm-pi-harness` is a separately installed Rust process adapter for one explicitly selected **actual Pi coding-agent session JSONL file**. It does not read synthetic sample records. It reads the Pi `session` header and `message` rows, then emits strict content-free lifecycle, usage, message and tool facts through `source-adapter/1`. The host verifies the executable hash, owns the durable cursor and capture policy, and remains the only Agenturm database writer. The built-in Pi reader remains the default; installing this adapter does not replace it.

Build with the repository's pinned Rust 1.100.0 toolchain, then install a copy for a file you choose:

```sh
cargo build --locked --release -p agenturm-pi-harness
python examples/rust_pi_harness_plugin/install.py \
  --binary /absolute/path/to/target/release/agenturm-pi-harness \
  --destination /absolute/path/to/pi-plugin \
  --source /absolute/path/to/pi-session.jsonl
agenturm serve --plugin-config /absolute/path/to/pi-plugin/plugins.toml
```

The installer writes a SHA-pinned manifest and explicit receiver configuration. It does not download, start or activate code. This POSIX file reader is tested on Linux x86_64; Windows is not qualified. Configure only a Pi session file you authorize the plugin to read. This first slice watches one file, not an entire Pi sessions directory.

The reader maps user and assistant roles, explicit Pi `stopReason: "stop"` and `"error"`, reported input/output/cache read/cache write counters, and tool call/result metadata. User messages, nonterminal assistant messages and tool results report a running lifecycle observation, matching Pi's native state transitions; only explicit stop/error marks a terminal outcome. A tool name is reduced to `file`, `shell`, `mcp` or `other`; names, arguments, prompts and outputs never enter the wire fact. A result is marked succeeded or failed only when Pi reports a boolean `isError`. A missing or malformed result flag fails the worker before cursor acknowledgment. `toolCallId` correlates call and result through the host's opaque operation key. Assistant `toolUse` does not imply completion.

Each newline-terminated Pi line maps to zero or more facts. A poll admits **all** facts from a line and advances its byte cursor, or leaves that line unread for the next acknowledged poll. If one line cannot fit the receiver's record, byte or structural budget, the worker fails visibly without acknowledging it. The cursor stores the last user turn ID, file device/inode, byte offset, a digest of the preceding 256 bytes, a digest of the session header and a source generation. A restart resumes the recorded turn. Replacement, truncation, a changed header or a nearby rewrite reports `possible_source_gap`, advances the generation and scans from the start with a cleared turn. The new generation prevents a reused row ID from colliding with an old revision after same-inode truncation. A missing previously observed file or header fails visibly. Unknown Pi row kinds and roles produce coarse `unsupported_pi_row` or `unsupported_pi_role` diagnostics, so they cannot silently imply complete coverage. A partial line waits for completion after initial discovery; an observed file losing its complete header fails. The source line cap and scan cap are each 1 MiB; assistant content is limited to 128 blocks. In-place edits outside the cursor's checked region can still escape detection; this reader assumes append-only Pi files and does not claim complete history.

This is a metadata-only vertical slice. It does not carry model identity, reasoning usage, native spans, trace membership, prompt text, tool names or output. Review and export expose only the policy-current content-free Activity subset, labeled as partial. It does not automatically disable or replace the built-in Pi reader. [Validation and parity gaps](../../docs/validation/rust-pi-reader.md) are recorded separately. No performance gain is asserted.
