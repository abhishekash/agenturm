# Pi requested-command admission primitive

Recorded 20 November 2026. This is an **inactive receiver-owned primitive**. Installed configuration, process negotiation, the Rust Pi emitter, HTTP, TUI, copy, export, and Review do not call it. The Python Store remains the only production writer. No execution, result, success, full Pi parity, or performance claim follows from this change.

## Exact boundary

An internal caller can pass a typed `PiCommandRef` and a host-minted `PiCommandGrant` to `commit_batch`. The reference nominates one directory member, one complete JSONL line, and one indexed assistant `toolCall` block. It contains source location and hashes, exact event/revision, native session/turn, assistant row ID, block index, call ID, and `kind = "pi.shell_command_text"`. It carries **no command or argument bytes**. A separate opt-in mode, `pi-review-text-command-v1`, and `pi-command-ref/1` are required to mint the grant, but installed config does not accept them yet.

The host requires one same-batch v1 `tool` fact with `category = "shell"`, `result = "started"`, exact event ID, session, turn, and operation ID. It opens the configured root and member without following symlinks, checks the durable member checkpoint and canonical initial generation, complete prefix and row hashes, session header, row timestamp, assistant role, block index, `toolCall` ID, `name = "bash"`, and string `arguments.command`. It independently derives the last user turn by reading preceding bounded JSONL rows. The command must be nonempty and at most 4 KiB of source UTF-8. The stored redacted command is also capped at 4 KiB of UTF-8. Redaction is best effort and cannot guarantee removal of all secrets.

The command table is separate from the existing Pi user-text table. A command row, its exact reference receipt, source cursor, and metadata fact commit in the same SQLite writer transaction. An exact acknowledged replay must match the command receipt; altered or missing refs fail closed. A malformed, forged, changed-file, or stale-lease reference rejects the batch without acknowledging its cursor. A source gap or unanchored generation withholds command content while preserving metadata progress. A duplicate occurrence cannot replace previously admitted command text. Ambiguous member ownership, evidence retention, and source-gap invalidation remove or withhold command rows. Revocation and content configuration changes delete command rows and receipts atomically with the policy transition. This is logical deletion, not SQLite page or backup sanitization.

## Verification and remaining gate

`tests/test_plugin_pi_command.py` covers exact redacted admission, crash replay, forged reference fields, non-bash blocks, started-shell fact binding, wrong timestamp and turn, changed file and symlink, ambiguous member ownership, gap and retention invalidation, revocation, and configuration downgrade. The existing Pi content, admission, and batch-handler suites guard the default path.

Before any public preview, add a separately versioned wire envelope and installed capture mode, a bounded Rust emitter, and an independently reviewed read/delivery gate tied to the exact selected Activity item. The preview must say **Requested shell command**. A started fact alone does not prove execution or success. Review, copy, and export require separate policy-current delivery decisions.
