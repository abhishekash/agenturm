# Plugin process supervision checkpoint

19 November 2026 · branch `architecture/plugin-supervision`

An opt-in `PluginSupervisor` runs at most four explicitly configured source adapters, with one bounded polling thread per source. Each process uses the existing manifest, executable hash, source lease, capture policy and durable cursor path. A failure closes the process and retries from the Store cursor with exponential backoff. Repeated failures quarantine the source until the owner constructs a new supervisor; revocation and a competing lease stop retries immediately.

The supervisor exposes source, plugin, state, last cursor, last successful poll and commit times, restart count, consecutive failures, projection error type and a host-authored failure code/reason. `/api/health` includes those diagnostics when a supervisor is attached. The reason distinguishes capture revocation from another worker fencing the source; it never includes the child process's stderr or raw record payload. Empty idle polls do not write evidence or advance the cursor.

Validation uses a separate executable that crashes once and then resumes, a repeatedly crashing executable that reaches quarantine, capture revocation, a hung executable interrupted during shutdown, duplicate-source rejection, and the health response. The focused tests pass locally. This is a process lifecycle proof, not default activation. No CLI configuration or installed third-party package is enabled by this change; those require a reviewed activation UX and complete capture/retention policy. The Python Store remains the sole writer.

Process separation does not sandbox plugin filesystem or network access. The supervisor bounds the number of workers and poll deadlines, not child RSS. A timed-out process is terminated and may replay its last unacknowledged batch. It does not claim to cancel a Store write that has already committed.
