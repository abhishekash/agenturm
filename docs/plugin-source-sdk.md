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

The executable runs with the user's OS privileges and is not sandboxed. Host grants restrict Agenturm APIs, not filesystem or network syscalls. The manifest hash covers only the executable bytes. A Python entrypoint script that imports mutable installed packages is therefore not a closed, version-pinned artifact; package a self-contained executable or zipapp and explicitly approve its dependency chain before production activation. The current SDK and manifest are provisional; no default collector auto-discovers or launches third-party code.

Conformance checks exercise a compiled Rust fixture and a separate Python executable across the same host, durable inbox and lifecycle projection. The Python executable test is a protocol proof, not a production distribution or performance measurement. See the [execution plan](rfcs/rust-plugin-kernel-plan.md) and [projection checkpoint](validation/plugin-lifecycle-projection.md).
