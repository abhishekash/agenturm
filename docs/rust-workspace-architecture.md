# Rust workspace and plugin architecture

Agenturm uses one root Cargo workspace. Crates live under `crates/`, grouped by
architectural responsibility rather than collected in one flat directory. A
crate should represent a cohesive API or implementation boundary; the folder
hierarchy explains which side of the host/provider boundary it belongs to.

## Layout

```text
crates/
├── applications/   # CLI, terminal UI, and OS service integration
├── core/           # Stable host-facing kernel and wire contracts
├── domain/         # Pure reducers and bounded domain queries
├── plugins/
│   ├── api/         # Extension-specific public contracts
│   ├── capabilities/# Capability providers
│   ├── connectors/  # Connector implementations
│   ├── fixtures/    # Executable protocol fixtures
│   ├── harnesses/   # Provider-specific agent readers
│   └── host/        # Discovery, supervision, shared source runtime, and trust gates
├── storage/         # Store and storage-service boundaries
└── test-support/
    ├── fixtures/   # Shared, synthetic golden inputs
    └── plugins/    # Test-only extension packages
```

The root `Cargo.toml` discovers packages with family-specific workspace globs.
Add a crate inside the appropriate family; do not add a top-level package or
create another workspace for production code. Test-only standalone fixtures
may define their own `[workspace]` when isolation is part of the test.

## Dependency direction

```text
applications ──compose──▶ plugin host ──uses──▶ providers
      │                         │                  │
      ├──▶ core contracts ◀─────┴──────────────────┘
      ├──▶ domain
      └──▶ storage ◀──────────── plugin host

test-support ──supports tests only; never a production dependency──▶ crates
```

- **Core contracts** (`agenturm-kernel`, `agenturm-protocol`) define host-owned
  facts, traits, and wire shapes. Keep provider-specific parsing and policy out
  of them.
- **Plugin host** (`crates/plugins/host`) owns process supervision, bounded
  source runtimes, and host-side trust/admission checks. It depends on host
  contracts and host-owned storage packages, not on every provider
  implementation.
- **Providers** are grouped by kind. Harnesses parse their own native formats;
  capabilities and connectors implement their respective extension surfaces.
  They use shared contracts instead of adding provider-name branches to the
  host or application.
- **Applications, domain, and storage** remain separate. UI/application code
  composes providers; pure domain crates do not depend on harnesses; storage
  owns durable state and admission boundaries.
- **Test support** contains synthetic fixtures and extension examples only.
  Never add real user sessions, credentials, or production runtime state there.

The workspace contains both trusted in-process contributions and explicitly
configured child-process adapters. Grouping a crate under `plugins/` signals
its architectural role; it does **not** mean every provider is dynamically
loaded, sandboxed, or covered by a stable third-party ABI. Follow the existing
registry, manifest, and protocol for the extension type being changed.

## Adding a provider

1. Put a harness, capability, or connector in its corresponding
   `crates/plugins/<family>/` directory. Put genuinely shared host machinery in
   `crates/plugins/host/`; do not move provider parsing into the host.
2. Depend on the narrowest existing kernel, protocol, or extension API. Keep
   storage authority, capture policy, and validation host-owned.
3. Register an in-process contribution through the existing registry, or use
   the explicit process manifest and protocol for a child adapter. Do not add a
   provider-specific dispatch branch to generic host code.
4. Add synthetic contract and lifecycle tests. Put shared golden data under
   `crates/test-support/fixtures/` and test-only extension crates under
   `crates/test-support/plugins/`.
5. Validate from the workspace root:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```

The workspace uses Rust 2024, resolver 3, and shared edition/toolchain/license
metadata. Crate versions remain local because executable and internal contract
release cycles are not necessarily the same.
