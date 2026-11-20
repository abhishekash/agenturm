# Offline migration candidate producer

20 November 2026. This is an explicitly invoked, feature-off preparation tool. Python `Store` remains Agenturm's production writer. The producer makes a historical candidate for the [Rust verifier](rust-migration-candidate-2026-11-20.md); it does not authorize a Rust writer, move a canonical path, archive old data, or prove a common stopped-write epoch.

## Usage and output

After an operator has stopped **all** telemetry, workflow, task, outbound, direct SQLite, and old-binary writers, call `python -m observatory.migration_candidate` with five absolute arguments: `--telemetry`, `--workflow`, `--tasks`, `--output`, and `--verifier`. The verifier must be a trusted build of `agenturm-migration-verify`. The three source paths and the output parent must be private, real directories; the source files must be private regular files owned by the caller. The output directory must not exist. This tool is deliberately absent from normal receiver startup.

The producer creates the output directory with mode `0700` and immediately writes `state.json` as `incomplete`. It reads the source main files and any existing WAL/SHM through no-follow file descriptors, rejects rollback journals and incomplete WAL/SHM pairs, and checks file identity and SHA-256 before and after copying. SQLite's online backup runs against **private clones** inside the output directory, so opening SQLite cannot update the canonical `-shm` file. The temporary clones and their sidecars are removed after each backup. Detached telemetry, workflow, and task backups are mode `0600`, fsynced, and described by absolute path, device, inode, size, SHA-256, and schema version.

The producer reads the detached files to construct the existing `migration_manifest_v1`: receiver identity, every source cursor and optional Pi replay receipt, and bounded workflow/task counts. It fsyncs the manifest and directory, invokes the Rust verifier, checks its exact receiver reply, then atomically changes `state.json` to `verified` with the manifest SHA-256 and fsyncs again. Any exception or interrupted preparation leaves the directory absent or **incomplete**; a manifest alone is not a completed producer result. The marker records validation at preparation time; later use must rehash and reverify the candidate. The output is review evidence, never a cutover certificate.

## Evidence and limits

Installed tests use actual `Store`, `WorkflowStore`, and `TaskStore` databases, including a source row retained in telemetry WAL. They compare canonical main/WAL/SHM bytes before and after, inspect backup descriptors and `verified` state, and cover verifier failure, interrupted backup, rollback journal, and source symlink rejection. The Rust verifier remains the final consistency check. CI builds its binary and runs the installed fixture; queued CI is not reported as passing.

Source identity/digest checks detect ordinary changes during a copy; they cannot prove that all three stores were stopped at the same instant or exclude a writer that resumes afterward. Cloning a live main/WAL pair can race a writer, so the stopped-writer requirement is an external prerequisite. The output parent and trusted verifier are operator choices. This tool has no authority over a held-open older writer, all active Store instances, outbound delivery, permissions of a future service principal, publish/rollback, or forward recovery after Rust's first durable write. Those remain in the [writer cutover gate](rust-writer-cutover-gate.md).

The [HTML checkpoint](rust-migration-candidate-producer-2026-11-20.html) presents this record in a responsive light/dark layout.
