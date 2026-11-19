# Source adapter SDK (provisional protocol 1)

An explicitly configured Agenturm source adapter can run as a separate executable and supply metadata evidence without importing the Store. `observatory.plugin_sdk.run_adapter` implements the child side of the framed `source-adapter/1` protocol. The host validates the manifest and executable hash, owns source identity and capture policy, commits the batch and cursor, then sends `ack`. A failed or lost acknowledgment may cause an exact replay.

An adapter implements `poll(PollRequest) -> PollBatch` and `ack(cursor) -> None`. `PollRequest` contains the host-owned source ID, the last committed cursor, and record/byte budgets. Return at most the budgets and advance the cursor whenever records are returned. An idle response may return no records at the unchanged cursor. Make source event IDs stable and increment revision for a correction to the same occurrence; native session/worker/turn IDs identify the source's own units. Persist any adapter-side checkpoint only after `ack`, and tolerate replay from the host cursor.

```python
from observatory.plugin_sdk import PollRequest, run_adapter
from observatory.plugin_wire import PollBatch

class MyAdapter:
    def poll(self, request: PollRequest) -> PollBatch:
        # Read a bounded slice from the explicitly configured source.
        return PollBatch(request.source_id, request.cursor, ())

    def ack(self, cursor: str) -> None:
        pass

run_adapter("example.source", MyAdapter())
```

`run_adapter` reserves stdout for framed protocol messages. It validates the handshake, source ID, cursor, record count, byte budget, wire shape, acknowledgment order and stop message. It does not validate the source's semantic truth; the host's metadata admission accepts only lifecycle, usage and diagnostic payloads today. Message, tool and relationship records are not yet admitted. Raw prompts, output, paths and free text are outside this metadata-only contract.

Usage records are **additive observations per distinct event ID**. A higher revision of the same event ID replaces that observation; an exact replay does not add it again. Use the same event ID when correcting a cumulative observation, or send separate IDs only for disjoint increments. The host sums reported `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens` and `total_tokens` independently per native session. An absent counter stays unknown; a reported zero stays zero. Health API totals above JavaScript’s largest exact integer are decimal strings. `total_tokens` is not inferred from input and output, and no model, cost or turn attribution is inferred. The current session details view labels these values “Reported source usage.” See the [usage projection checkpoint](validation/plugin-usage-projection.md).

The executable runs with the user's OS privileges and is not sandboxed. Host grants restrict Agenturm APIs, not filesystem or network syscalls. The manifest hash covers only the executable bytes. A Python entrypoint script that imports mutable installed packages is therefore not a closed, version-pinned artifact; package a self-contained executable or zipapp and explicitly approve its dependency chain before production activation. The current SDK and manifest are provisional; no default collector auto-discovers or launches third-party code.

## Explicit receiver activation

Create a TOML file outside untrusted project directories and name each approved manifest and source. The receiver reads only the file passed to `--plugin-config`; it does not scan for plugins. This initial path accepts up to four independent source adapters and metadata-only capture.

```toml
schema = 1

[[sources]]
manifest = "/absolute/path/to/approved-plugin.toml"
source_id = "my-source"
grants = ["configured-source-read"]
capture = "metadata-only"
poll_interval = 0.5
timeout = 5.0
```

To transfer one built-in source owner to a process adapter, add `replaces = ["pi"]` to that source row and declare `harness.pi/1` in its manifest's `provides` list. The receiver validates the claim, grants and executable hash before opening its Store, then excludes Pi from the trusted collector registry. A replacement requires local collection and is explicit per source; the same built-in source cannot be claimed twice. OpenCode is still a separate reader and is not eligible through this source-registry switch. If the child fails after activation, health reports the plugin failure; the trusted reader does not silently restart alongside it. This prevents two active readers from writing one source but means the replacement must establish its own semantic parity before production use. See the [source ownership checkpoint](validation/plugin-source-ownership.md).

Run `agenturm serve --plugin-config /absolute/path/to/plugins.toml` on a loopback address, then connect the cockpit to that receiver. The manifest separately pins the executable hash; an unexpected change prevents launch. `/api/health` reports worker state, restart count, last cursor, projection coverage and a host-authored failure reason. Revoking capture through the host policy API fences the worker and removes its admitted metadata. A source pauses at the current 50,000-distinct-event cap. Superseded revisions are compacted, but durable retention of distinct events remains required for continuously running production adapters.

Conformance checks exercise a compiled Rust fixture and a separate Python executable across the same host, durable inbox and lifecycle projection. The Python executable test is a protocol proof, not a production distribution or performance measurement. See the [execution plan](rfcs/rust-plugin-kernel-plan.md) and [projection checkpoint](validation/plugin-lifecycle-projection.md).
