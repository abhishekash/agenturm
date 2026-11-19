# Standalone Rust sample harness plugin

This is a separately built, opt-in Rust process adapter for Agenturm's `source-adapter/1` protocol. It reads an explicitly configured append-only JSONL file and emits typed, content-free lifecycle, usage, relationship and source-gap metadata. It does not import Agenturm's Python package or write the receiver database. The host verifies the pinned executable, supervises the process, admits records under its capture policy and acknowledges a batch after the Store commit.

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
```

`id` and `session` are stable, bounded source identities. `at_ns` is a nonnegative signed-64-bit nanosecond timestamp encoded as decimal text. A correction appends the same `id` with a larger integer `revision`; the host replaces the earlier occurrence. Each distinct usage `id` is additive, so cumulative counters must not be repeated under new IDs. A relationship is confirmed only if the parent session is also observed in the same source; otherwise it remains unresolved. Cycles and conflicting same-time parent claims are reported explicitly by the host.

The adapter accepts complete newline-terminated records only, with a 16 KiB line cap, 256-record poll cap and about 1 MiB scan cap. Its cursor includes the file device/inode, byte offset and SHA-256 of the preceding 256 bytes. Replacement, truncation or a nearby rewrite emits `possible_source_gap` before scanning the changed file. Earlier in-place edits outside that window violate this example's append-only source contract. A missing previously observed file or malformed complete record makes the worker fail visibly without acknowledging the cursor; fix the source and restart the receiver. The adapter writes no source content or paths to stderr.

This adapter does not replace any built-in harness at evidence parity. Message, tool, check and review evidence, long-term retention and a packaged platform release remain separate work. It carries no performance claim.
