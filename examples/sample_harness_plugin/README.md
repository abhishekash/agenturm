# Standalone sample harness plugin

This example is an independent, single-file Python executable using only the standard library. It reads one explicitly configured append-only JSONL file and sends content-free lifecycle, usage, relationship, message, tool and check facts over Agenturm's provisional `source-adapter/1` process protocol. It imports no Agenturm package modules. Linux and macOS are the tested execution shape; this example does not establish Windows plugin support.

To install it into an empty directory and generate a pinned manifest plus receiver configuration:

```sh
python examples/sample_harness_plugin/install.py \
  --destination /absolute/path/to/sample-plugin \
  --source /absolute/path/to/sample-events.jsonl
agenturm serve --plugin-config /absolute/path/to/sample-plugin/plugins.toml
```

The installer copies the script, hashes that copy, writes the manifest and TOML config, and prints the config path. It does not start the adapter or alter an Agenturm database. Agenturm starts it only when that named config is supplied. The config does not replace a built-in harness.

Example input:

```jsonl
{"id":"start-1","session":"demo-1","at_ns":"100","kind":"lifecycle","state":"started"}
{"id":"usage-1","session":"demo-1","at_ns":"150","kind":"usage","usage":{"input_tokens":12,"output_tokens":3}}
{"id":"end-1","session":"demo-1","at_ns":"200","kind":"lifecycle","state":"completed","reason_code":"normal"}
{"id":"child-1","session":"demo-child","at_ns":"210","kind":"relationship","parent_session":"demo-1","relation_kind":"delegated"}
{"id":"request-1","session":"demo-1","at_ns":"220","kind":"message","role":"user"}
{"id":"tool-1","session":"demo-1","at_ns":"230","kind":"tool","category":"shell","result":"succeeded"}
{"id":"check-1","session":"demo-1","at_ns":"240","kind":"check","category":"test","result":"passed"}
```

`id` and `session` are stable source identities. Message facts allow only a `user` or `assistant` role. Tool facts allow `shell`, `file`, `mcp` or `other` categories with `started`, `succeeded`, `failed` or `denied` results. Check facts allow `test`, `lint`, `build`, `review` or `other` categories with `passed`, `failed`, `skipped` or `unknown` results. These facts appear in Activity without message text, command names, paths or output; a failed check does not change the session outcome. Relationship records use a stable parent session ID and a `delegated` or `review` kind. The host confirms a link only when that parent is observed in the same policy-current source; missing parents remain unresolved and cycles are invalid. A correction appends the same `id` with a higher integer `revision`; the host replaces that occurrence in its durable inbox. Usage values for distinct IDs are additive; do not repeat cumulative totals under new IDs. The adapter waits for a newline before accepting a record, reads at most 256 records and approximately 1 MiB of source data per poll, and rejects any line over 16 KiB. Its cursor includes the file identity, byte offset and a digest of the preceding 256 bytes. Replacement, truncation or a nearby same-inode rewrite emits a durable `possible_source_gap` warning before reading the changed file. Older in-place edits outside that window violate this example's append-only source contract.

The file may be absent initially. If a previously observed file disappears, the worker fails visibly; on reappearance it reports a possible gap. A malformed complete line or oversized line stops that worker without advancing the durable cursor; health shows the failure and the source file must be fixed before restarting the receiver. The last source diagnostic remains visible in `local_agents.plugin_workers[].last_source_diagnostic` after a restart. The executable runs with the user's OS permissions and is not a filesystem sandbox. The adapter still cannot replace Pi, Codex or another built-in harness with equivalent content capture, trace membership or review proof. The 50,000-distinct-event source window uses an explicit retention checkpoint; affected sessions become unknown when earlier evidence is evicted.
