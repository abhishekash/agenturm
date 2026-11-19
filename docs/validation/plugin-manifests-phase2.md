# Inert plugin manifest checkpoint

`observatory/plugin_manifest.py` now reads **only explicitly supplied TOML paths**. It caps each file at 64 KiB, validates IDs, semantic versions, a bounded protocol-major range, entrypoint argv, an artifact SHA-256, provided and required versioned features, explicit dependencies and grants. Discovery never imports a Python module, resolves a command, or launches a process. The future process host must verify the artifact hash immediately before execution; this validator does not claim to authenticate or sandbox code.

`plan_activation` receives an explicit enabled set plus host features and allowed grants. It rejects missing/disabled dependencies, unknown plugins, incompatible protocols, denied grants, missing or ambiguous feature providers and cycles before returning a deterministic startup order. Multiple providers require an explicit dependency choice. No plugin is started by this module.

The manifest is an **internal preliminary schema**. Configuration schemas, installation/upgrade operations, artifact verification, process supervision, activation leases, durable cursor/ack transactions and extension-point conformance remain for later phases. No arbitrary project directory is scanned for executable extensions. The Python Store remains the only write authority.

Validation: **5 focused tests passed** for inert discovery, ordering, provider conflicts, cycles, protocol/grant checks and malformed or oversized files. `git diff --check` passed. This branch builds on the [Pi adapter checkpoint](pi-adapter-phase3.html) and the [reviewed plan](../rfcs/rust-plugin-kernel-plan.html).
