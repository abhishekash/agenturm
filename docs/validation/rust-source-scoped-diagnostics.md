# Source-scoped diagnostic observations

This slice allows a harness to retain a privacy-safe diagnostic even when it
does not belong to a session. It uses an explicit `source` scope; it never
creates a synthetic session ID. Session-scoped observations continue to require
their source-native session ID.

Persisted diagnostics contain only a catalogued `code` and `severity`. The
closed catalog is shared by the kernel, runtime decoder, and store validation.
Unknown codes remain counted as unprojected facts and are not persisted. The
diagnostic record rejects extra fields, so messages, paths, and other payload
content cannot be smuggled through this projection.

SQLite schema v8 adds an explicit scope column and makes `session_id` nullable.
The migration rebuilds the v7 observations table transactionally and marks all
existing rows as session-scoped. The table check requires source-scoped rows to
have no session hierarchy IDs and requires session-scoped rows to have a
session ID. Session-filtered history queries exclude source-scoped rows.

Diagnostics skipped by older decoders cannot be reconstructed after their
source cursors have advanced. They become available as each source reports
them again after upgrading.

## Validation

- Kernel admission accepts source-scoped catalogued diagnostics and rejects
  private or unknown diagnostic codes.
- Runtime decoding stores source diagnostics, counts unknown codes without
  retaining them, and rejects payload extensions.
- Store tests cover v7-to-v8 migration, reopen/readback, explicit source scope,
  and exclusion from session-filtered queries.
- Workspace tests, Clippy, formatting, and build are recorded with the change.
