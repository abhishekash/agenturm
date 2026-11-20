# Rust cooperative writer lease experiment

20 November 2026. This is a feature-off Linux experiment for a future Rust storage service. `agenturm-storage-lease` is not called by Python `Store`, the receiver launcher, the plugin host, or the scratch-admission CLI. Python remains Agenturm's only production database writer.

## Contract

`WriterLease::acquire` takes an existing absolute path to a singly linked regular database fixture. It rejects a target with multiple hard links, since different filenames for one inode would derive different sibling locks. It binds the lexical path (including a final symlink), the resolved target path and device/inode, and a private sibling lock file derived from the resolved target. An existing nonregular lock entry is rejected before open; the lock is then opened with `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC | O_CREAT` and owner-only `0600` permissions, checked against the current Linux user and its pathname, then locked with a nonblocking `std::fs::File::try_lock`. A process-local registry shares that open lock between repeated acquisitions for the same target; clones retain it until the final handle drops. The tiny probe binary exists only for cross-process tests.

Call `verify_admission()` immediately before a prospective write. It rechecks process identity, lock descriptor/path, lexical symlink, resolved path, database target identity and single-link count. Unlink/recreate of the lock, adding a hard link, symlink retargeting, or atomic replacement of the database makes the next admission fail. This is a point-in-time operational check: an already in-flight write is not fenced, and a swap-and-swap-back race is not excluded. Manual fork with an inherited handle is unsupported; a child fails verification, and `O_CLOEXEC` prevents the descriptor from leaking across exec. There is no in-process quiescence integration with Store, no legacy-binary exclusion, and no Rust cutover.

## Focused acceptance

- A second process cannot acquire the same resolved target, including through a symlink alias; neither process changes database bytes or creates SQLite sidecars.
- Repeated acquisitions in one process retain one OS lock until the final handle drops. Killing the owner releases the OS lock.
- A symlinked or nonprivate lock file is rejected. Replacing the lock pathname while held makes the next admission fail.
- A second process cannot use a hardlink alias to derive a different lock for the same database inode. An existing lease refuses its next admission if a new hard link appears.
- Retargeting an explicit database symlink or replacing the database inode makes the next admission fail.

The broader [writer cutover gate](rust-writer-cutover-gate.md) remains binding. In particular, a cooperating advisory lock cannot prevent an older same-user binary from writing an accessible SQLite file, and a future service must prove worker/read-lease quiescence before releasing its production lock. No performance gain is asserted.
