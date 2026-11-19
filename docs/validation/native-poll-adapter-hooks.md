# Native session adapter hook checkpoint

19 November 2026. The generic JSONL session collector now calls optional hooks on its registered format adapter for initialization, file rotation, bounded-tail notice, continuation root, post-poll spans and status metadata. Claude Code sidecar identity and local-command checks moved into its adapter; Kiro approval reset, bounded history note, parent outcome and title fallback moved into its adapter. Codex and Claude Code supply their own continuation-root predicates. The stable agent namespace is declared by the adapter, with a runtime-derived fallback for an injected format.

The collector still owns bounded reads, offsets, admission, persistence, diagnostics and status publication. No format adapter receives the Store. This removes runtime-name branches from the hot poll path, but **does not finish** harness extraction: Kiro worker linking and folder permissions still have collector-level wiring, and the trusted adapters still run in-process. The opt-in process plugin contract remains separate.

The fixture provenance manifest records the new source hashes against commit `d2ac01c` and retains the preceding hashes as historical evidence. The change does not assert a new observed producer version or alter the synthetic fixture bytes.

Validation: 79 focused native session, lifecycle, source, Kiro permissions, port and provenance tests passed after adding an injected-adapter hook case; 39 additional native activity, review, inventory and current-schema tests passed. `compileall` and `git diff --check` passed. The full merged-run CI is a separate gate.
