# Fake inbound connector checkpoint

Recorded 20 November 2026. This checkpoint adds a standalone local HTTP connector executable and installer under `examples/fake_inbound_connector/`. It uses the existing explicit process source adapter protocol, manifest verification, host-owned Store cursor, projection, supervision and receiver health. No core dispatch or UI source file was edited.

The fake service pages bounded lifecycle and usage metadata. The connector handles idle pages, a one-second `429` retry, process restart at an acknowledged cursor, corrected event revisions and `410` cursor expiry. Expiry produces a durable `cursor_expired` diagnostic before re-reading from the new revision. Raw content and malformed fields cause worker failure before the cursor advances. The installer and child permit only exact loopback `/events` URLs and the child rejects redirects.

Validation on this branch:

```sh
python3 -m unittest tests.test_fake_inbound_connector
```

Five tests passed, including an installed plugin observed through a real receiver `/api/health` response. The test suite uses only a local fake HTTP server; no account or external message is touched. The existing plugin test suite and repository checks should run in CI before release. This is a **contract proof**, not a general connector release. Grants are declarative rather than a network sandbox. Output delivery, credentials, distributed quotas and full message/tool/check capture remain open architecture gates.
