# Codex typed facts into a detached Rust Store

21 November 2026. This is a test-only integration proof for the opt-in Codex process adapter and Rust `AdmitFacts` storage path. Python `Store` remains production authority; the fixture does not route receiver writes to Rust.

## What the fixture proves

The root-only, two-UID fixture starts the actual `agenturm-codex-harness` against a private, client-owned JSONL file. The child emits typed `message`, `tool`, `check`, and `usage` records. A host-side test coordinator validates and prepares those facts, then submits them to the Rust storage service on a detached clone of a real Python `Store` schema.

The fixture checks that:

- Codex output contains only bounded typed metadata; raw prompt, tool, command, and output strings are neither sent in the batch nor persisted in evidence.
- The source files are created with mode `0600`, owned by the client UID, and unreadable by the distinct storage-service UID.
- Rust cursor, evidence, dirty sequence, and empty-content receipts match an independent Python `Store` admission for the same facts.
- After the Rust service commits but drops its reply, the durable cursor and request receipt are visible before the child receives an ACK. Restarting the child at the old cursor reproduces the same batch; Rust returns an exact replay, and the retry does not duplicate evidence or receipts.
- A Codex diagnostic is refused as unsupported by this facts-only mapper. Malformed usage fails visibly before any source is activated or evidence is admitted.
- A stalled child frame read times out and kills/reaps the child. The fixture also cleans up any child left running by an earlier assertion failure.

## Scope limits

The host coordination and ACK are manually driven by the fixture. This proves the harness protocol and detached storage contract together, but it does not exercise production receiver orchestration, Rust writer cutover, or a bundled production adapter. The test does not establish Codex source-to-Review, Activity, or export parity. Unsupported diagnostics fail the facts-only batch visibly rather than being dropped. No performance result is claimed.

The storage service uses only the marked detached clone and a fixed path. Canonical data remains unchanged. See the [generic real-Store facts admission design](rust-real-clone-facts-design.md) for the storage-side limits.

## Validation

The fixture is run by the Rust CI job after building both `agenturm-storage-service` and `agenturm-codex-harness`. Local checks on the implementation branch passed Rust formatting, workspace tests, Clippy, Python syntax compilation, and the private-checkout two-UID Docker fixture.
