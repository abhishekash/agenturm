# Standalone Rust sample harness plugin

This is a separately built, opt-in Rust process adapter for Agenturm's `source-adapter/1` protocol. It reads an explicitly configured append-only JSONL file and emits typed, content-free lifecycle, usage, relationship, message, tool, check and source-gap metadata. It does not import Agenturm's Python package or write the receiver database. The host verifies the pinned executable, supervises the process, admits records under its capture policy and acknowledges a batch after the Store commit.

Build it with Rust 1.100.0 or the repository's pinned toolchain, then install the built executable:

```sh
cargo build --locked --release -p agenturm-sample-harness
python examples/rust_sample_harness_plugin/install.py \
  --binary /absolute/path/to/target/release/agenturm-sample-harness \
  --destination /absolute/path/to/rust-sample-plugin \
  --source /absolute/path/to/sample-events.jsonl
agenturm serve --plugin-config /absolute/path/to/rust-sample-plugin/plugins.toml
```

The installer copies the executable, pins its SHA-256 in a manifest and writes an explicit receiver config. It does not start the adapter or change receiver data. The current artifact is tested on Linux x86_64; the source uses POSIX file identity and is not built for Windows. The installer does not download executable code.

Example source records:

```jsonl
{"id":"parent-start","session":"parent","at_ns":"100","kind":"lifecycle","state":"running"}
{"id":"child-start","session":"child","at_ns":"110","kind":"lifecycle","state":"started"}
{"id":"link","session":"child","at_ns":"120","kind":"relationship","parent_session_id":"parent","relationship_kind":"delegated"}
{"id":"tokens","session":"child","at_ns":"130","kind":"usage","usage":{"input_tokens":12,"output_tokens":3}}
{"id":"child-end","session":"child","at_ns":"200","kind":"lifecycle","state":"completed","reason_code":"normal"}
{"id":"request","session":"child","at_ns":"210","kind":"message","role":"user","turn_id":"turn-1"}
{"id":"call","session":"child","at_ns":"220","kind":"tool","category":"shell","result":"started","turn_id":"turn-1","operation_id":"call-1"}
{"id":"result","session":"child","at_ns":"230","kind":"tool","category":"shell","result":"succeeded","turn_id":"turn-1","operation_id":"call-1"}
{"id":"check","session":"child","at_ns":"240","kind":"check","category":"test","result":"passed"}
```

`id` and `session` are stable, bounded source identities. `at_ns` is a nonnegative signed-64-bit nanosecond timestamp encoded as decimal text. A correction appends the same `id` with a larger integer `revision`; the host replaces the earlier occurrence. Each distinct usage `id` is additive, so cumulative counters must not be repeated under new IDs. A relationship is confirmed only if the parent session is also observed in the same source; otherwise it remains unresolved. Cycles and conflicting same-time parent claims are reported explicitly by the host. Message facts allow only `user` or `assistant` roles. Tool facts allow `shell`, `file`, `mcp` or `other` categories with `started`, `succeeded`, `failed` or `denied` results. Check facts allow `test`, `lint`, `build`, `review` or `other` categories with `passed`, `failed`, `skipped` or `unknown` results. Optional `turn_id` is a stable native turn identifier. Optional `operation_id` is allowed only on tool facts; use the same native call ID for its start and result. The host keeps revision identity fixed, publishes source and session scoped opaque keys, and permits exact Activity turn drilldown. These facts appear in Activity with empty detail text and no synthetic trace or span; a failed check does not change the session outcome.

The adapter accepts complete newline-terminated records only, with a 16 KiB line cap, 256-record poll cap and about 1 MiB scan cap. Its cursor includes the file device/inode, byte offset and SHA-256 of the preceding 256 bytes. Replacement, truncation or a nearby rewrite emits `possible_source_gap` before scanning the changed file. Earlier in-place edits outside that window violate this example's append-only source contract. A missing previously observed file or malformed complete record makes the worker fail visibly without acknowledging the cursor; fix the source and restart the receiver. The adapter writes no source content or paths to stderr.

This adapter does not replace any built-in harness at evidence parity. Its message, tool and check facts are content-free: prompt text, command names, paths, tool output, check logs, trace membership and exact review-turn proof remain separate work. The source uses a finite retention horizon and reports a gap if earlier evidence is evicted. An installed platform release remains separate work. It carries no performance claim.
