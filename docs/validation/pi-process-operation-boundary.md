# Pi process operation boundary

Recorded 20 November 2026. This checkpoint adds an optional, strict `identity.operation_id` to process `tool` facts. The host stores the native call ID as admitted evidence, keeps it immutable across revisions, and publishes only a receiver, source and session scoped SHA-256 key in Activity. A call and its result can now be correlated in Activity, Review and text export without exposing the source ID or claiming that either event is an actual trace span. Process turn IDs also use a source and session scoped 32-hex key accepted by exact Activity turn drilldown; the earlier `plugin-turn:<hash>` value could not pass the API's turn filter. The installed Rust sample adapter accepts `turn_id` and `operation_id` on its typed JSONL rows and proves the wire path through a SHA-pinned installed executable. The Python Store remains the sole writer and capture policy owner.

The Pi source fixture contains a user message, assistant tool call, tool result, usage and explicit `stopReason: "error"`. The native Pi reader and process projection agree on the observed tool failure, usage total and interrupted lifecycle. The fixture also found a native Pi state bug: `parse` produced an interrupted span while `observe_state` left the inspector running. The adapter now reports interrupted for this explicit error stop. Process Activity remains partial: it contains only user/assistant roles and coarse tool facts, while native Pi Activity also contains a model usage row. The process session reports usage counters separately. Neither path infers completion from a tool result.

The process contract does not yet parse production Pi session files in an independent Rust worker. A replacement needs a bounded session file cursor that carries Pi's session and turn continuity across polls, atomic admission of multiple facts from one source line, source rotation and loss detection, and source-to-derived tests over a broader Pi corpus. Native Pi remains the default. Process facts currently have no approved prompt, arguments, output, model name, tool name, trace membership or span transport. Review therefore shows only the supported policy-current Activity subset, and export labels its partial coverage. This checkpoint does not claim full Pi parity or a performance gain.

| Boundary | Verified behavior |
| --- | --- |
| Admission | `operation_id` is optional only on `tool`, restricted to bounded identifier syntax; free text payloads remain refused before writes. |
| Correction | A revision cannot change the tool's native session, turn, worker or operation identity. Old Activity is hidden until republished. |
| Projection | Both phases share an opaque operation key within one source/session. The same native ID in another source or session produces another key. Exact turn drilldown works through the Store and HTTP API. No synthetic trace or span is published. |
| Policy and retention | Revocation erases the rows and fences the worker. Existing retention invalidation withholds Activity for affected sessions. |
| Upgrade | The `plugin_activity_projection_v2` marker invalidates earlier projection cursors, so the source is republished before Activity becomes visible. |

## Verification

- `uv run python -m unittest tests.test_plugin_activity tests.test_plugin_projection tests.test_plugin_retention tests.test_pi_process_boundary tests.test_native_sessions -q` — targeted Python suite.
- `cargo test --locked -p agenturm-sample-harness` and `cargo clippy --locked -p agenturm-sample-harness --all-targets -- -D warnings` — strict Rust sample parsing and lint.
- `AGENTURM_RUST_SAMPLE_BIN=... uv run python -m unittest tests.test_rust_sample_harness_plugin -v` — installed Rust binary, correction, policy revocation and Activity/Review correlation.

The tests use a bounded synthetic Pi fixture and a separately installed Rust sample. They do not establish coverage of arbitrary Pi versions or actual production process replacement.
