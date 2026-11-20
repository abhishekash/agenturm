# Held-open legacy writer: Linux two-UID negative gate

20 November 2026. This is a disposable, feature-off cutover experiment. Python `Store` remains the production writer. No Rust service was given authority over a canonical database.

## What the fixture proves

`scripts/validate_legacy_held_open_writer.py` runs as root only inside an isolated Linux VM or container. It creates temporary directories, chooses unused numeric UIDs/GIDs, and changes ownership only inside that temporary tree. A legacy-identity Python `Store` creates a WAL-mode database, commits a row, and keeps its SQLite connection open. The parent makes a detached SQLite backup under a service-only `0700` directory, confirms the committed row and receiver identity, and then lets the old connection begin a write transaction.

The parent moves the old main file and its WAL/SHM sidecars into that private directory and changes their ownership and permissions to the service identity. The already-open legacy transaction **still commits** a new row, DDL, and a changed `receiver_id` to the archived SQLite state. The fixture reads those exact changes after the old process exits. This is the negative result: moving and changing ownership of an old database cannot substitute for proving that every old writer has stopped.

Fresh legacy-identity `Store` and direct SQLite opens of both the protected backup candidate and archived pathname fail. The candidate's bytes, committed row and original receiver identity stay unchanged throughout the old transaction and the fresh-open attempts. Its backup includes a committed WAL row. The fixture asserts that at least one archived inode or sidecar changes, so it cannot pass by silently skipping the adversarial write.

Run the existing image without creating a new host account or touching host-owned databases:

```sh
docker run --rm --network none --read-only --tmpfs /tmp:rw,exec,size=128m \
  -v "$PWD":/src:ro -w /src python:3.12.15-slim-bookworm \
  python scripts/validate_legacy_held_open_writer.py
```

The local Docker run passed all three printed checks. The Rust CI job runs the same fixture on its isolated Ubuntu VM with `sudo`.

## Boundary and remaining gate

The private candidate and archive are **test-only paths**. The current Rust service accepts only an empty probe data root or an explicit scratch database; it cannot open a migrated canonical Store. This fixture uses the current Python `Store` as a legacy-identity stand-in; it does not pin or execute every supported older wheel or CLI. It proves a held-open writer hazard on this Linux filesystem and fresh-open denial for the chosen non-root identities. It does not prove old-process drain, authenticated multi-store backup production, production path publication, platform ACLs, or forward recovery.

Before any Rust writer cutover, the launcher must identify and stop old writers **before** backup/archive, then test supported older artifacts against the actual protected canonical and archive paths. Retain Python authority if that exclusion cannot be proved. See the [writer cutover gate](rust-writer-cutover-gate.md).
