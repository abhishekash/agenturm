# Fake outbound connector

This is a fixture-only proof of Agenturm's **separate side-effect contract**. It does not send messages or touch an external account. An explicit caller configures a loopback destination with the `write_fixture_metric` action, enqueues a bounded integer, and invokes dispatch. Enqueue never sends. The host owns the SQLite policy and delivery ledger; the separately installed executable owns only a framed action pipe and local HTTP request handling. The child imports no Agenturm package modules.

The installer pins a single executable artifact and writes a destination descriptor. It refuses linked destination directories and entries, including dangling links; files are created exclusively through a no-follow directory handle:

```sh
python3 examples/fake_outbound_connector/install.py \
  --destination /absolute/path/to/outbound-plugin \
  --endpoint http://127.0.0.1:8766/deliver
```

The sink fixture is implemented in `tests/test_fake_outbound_connector.py`; run that test module to exercise installation and delivery end to end. There is no receiver startup flag for outbound delivery, no automatic finding-to-send bridge, and no release-ready outbound CLI. The descriptor and manifest are inert until application code explicitly constructs `OutboundQueue`, calls `grant(...)`, `enqueue(...)`, and `dispatch_one(...)`. The manifest declares `action-adapter/1`, requires `delivery-attempt/1`, and requests `loopback-sink-write`. The host verifies the manifest, approved destination/action/artifact and exact loopback URL before dispatch. Grants are admission declarations, **not an OS network sandbox**; the child runs with the user's OS permissions.

## Durable state and uncertainty

The host generates one stable random idempotency key per delivery ID. Re-enqueueing the same ID and action returns the same row; changing its meaning is rejected. A dispatch claim records `in_flight` and increments attempts before contacting the sink. An accepted response with a receipt records `delivered`. A fixture `503` queues a bounded retry; after three attempts, the row is a visible `dead_letter`. A fixture `422` is terminal immediately. Revocation changes queued and retryable rows to `blocked`; re-granting does not silently resurrect them.

If a request times out or the child dies after dispatch, the row becomes `uncertain`. It cannot be selected for retry. `recover_in_flight()` conservatively moves stranded in-flight rows to `uncertain`, but refuses to run while a dispatch holds the local POSIX lock. That lock is held from claim through response handling and released by the OS if the process dies. `reconcile(...)` asks the sink for the idempotency key's status. A definitive accepted receipt marks delivered; a definitive missing status permits a retry with the **same key** while the grant remains active. Lookup failure leaves the row uncertain. Revocation cannot undo a request already accepted remotely; an in-flight or uncertain delivery remains visible until reconciliation. The fake sink guarantees idempotent POST and definitive status lookup for this proof. A real connector must establish those guarantees for its destination before treating `missing` as safe to retry.

This proof has one local action shape, one endpoint path, no credentials, no remote account and no automatic scheduler. The lock is a local POSIX file lock on the same filesystem as the Store; this example does not establish distributed lease semantics. Production outbound support still needs a versioned public action schema, destination-specific credential resolution, shared process budgets, operational UI, recipient approval and destination-specific status semantics. It must remain distinct from the inbound source protocol.
