# Process plugin ownership of registered readers

Recorded 20 November 2026. Explicit `replaces` configuration can now claim either a registered JSONL source or a registered reader such as OpenCode, provided the plugin manifest declares the matching `harness.<id>/1` feature. The receiver verifies config, grants, protocol and artifact before opening the Store, then removes the claimed reader from the trusted collector registry. This prevents two active readers for the same named harness. A missing or unknown claim fails before Store creation.

The currently admitted process facts are limited and the supplied sample adapters do not have OpenCode evidence parity. Replacing OpenCode in a real receiver would remove richer built-in evidence; this option is for explicit development and conformance work until a production adapter passes the full parity matrix. The Python Store remains the single writer, and the Rust host is opt-in.

Validation: `/home/abhishekash/abhi-codes/agentic/agenturm/.venv/bin/python -m unittest tests.test_plugin_config tests.test_collector_runtime tests.test_native_reader_registry` — 19 passed. The new case verifies that an OpenCode claim removes only the OpenCode reader and leaves Pi source discovery in place. `git diff --check` passed.
