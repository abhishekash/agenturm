# Process plugin admission port

21 November 2026. This architecture slice gives `PluginSession` and
`PluginSupervisor` an explicit trusted port for durable process-source
admission. `PythonStoreAdmissionPort` is the default and forwards to the
existing Python `Store` implementation.

The port covers source activation, provider schema setup, member cursors,
policy-current diagnostics, batch commit, and revocation classification. Batch
commit remains the acknowledgment boundary: the process host sends an ACK only
after the selected port returns the durable cursor. The supervisor asks its
factory for a fresh port for preflight and each worker/restart.

Port injection is trusted receiver configuration. Runtime protocol checking
only verifies that the required methods exist; it cannot establish their
durability, atomicity, authorization, or security semantics. Every alternate
implementation needs a dedicated admission-port conformance suite before it
is used by a receiver; this slice does not yet provide that reusable suite.

## Boundaries

- This does not add or activate a Rust admission backend. Production writes
  still go through Python `Store`.
- `PluginSession` still uses Python `Store` for projection reads. A future
  detached-clone Rust port therefore needs a deliberate projection bridge and
  cannot imply canonical Rust writer ownership.
- Private reference admission is passed through the port, but a backend must
  explicitly support its trusted provider registry. Unsupported private
  references must fail closed.
- Capture policy mutation, migrations, task/workflow/outbox writes, and general
  receiver storage are outside this port.

## Validation

The focused contract test injects a recording port and checks the exact
activation, diagnostic, and commit call order while retaining the same cursor,
receipt, and projection outcomes. Supervisor coverage checks factory use and
the existing restart, failure classification, and revocation behavior. The
production default remains `PythonStoreAdmissionPort`.
