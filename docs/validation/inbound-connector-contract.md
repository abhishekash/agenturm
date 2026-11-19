# Inbound connector contract checkpoint

Recorded 20 November 2026. The change makes the installed loopback connector's versioned capability mandatory at activation. A generic `requires_features` field in explicit plugin config is checked against the pinned manifest during inert server preflight, then against the live child handshake after Store activation and before polling. The fake inbound artifact now advertises `connector-adapter/1`; the receiver starts it through the existing source supervisor and Store cursor without a connector-specific kernel branch. The installer uses no-follow directory handles and exclusive file creation, refusing linked or dangling destination entries.

The fixture keeps bounded pagination, `429` backoff, `410` gap diagnostics and metadata-only lifecycle/usage mapping. New tests prove a commit-before-ack restart resumes without duplicate evidence, capture revocation fences the child and removes its projection, required features reach inert server preflight, missing child feature fails the later handshake, and linked installer paths are refused. Existing installed receiver health, rate-limit, expiry, correction and invalid-content tests remain.

Validation after rebase:

```sh
python3 -m unittest tests.test_fake_inbound_connector tests.test_plugin_config tests.test_plugin_runtime tests.test_plugin_supervisor tests.test_sample_harness_plugin tests.test_plugin_process tests.test_plugin_admission tests.test_plugin_projection
python3 -m unittest tests.test_plugin_config tests.test_fake_inbound_connector tests.test_plugin_supervisor
python3 -m py_compile observatory/plugin_config.py observatory/plugin_supervisor.py examples/fake_inbound_connector/agenturm_fake_inbound.py examples/fake_inbound_connector/install.py
python3 -m py_compile observatory/server.py
git diff --check origin/main...HEAD
```

The initial 83 selected plugin tests passed. After the server preflight fix and public `PluginSource.runtime_features` cleanup, 28 connector/config/supervisor tests passed; syntax and diff checks passed. The [v1 connector contract](../inbound-connector-protocol.md) describes the frame and durability semantics. This is a loopback fixture, not general network access or a credentialed production connector. The endpoint config remains same-user editable within the loopback class; endpoint identity pinning is a future gate. No performance gain is asserted.
