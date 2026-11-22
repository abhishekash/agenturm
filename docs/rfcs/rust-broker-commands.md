# Native broker commands

The Linux bundle includes `agenturm-storage-service` and explicit broker clients. The broker owns the history database and authenticates the client UID and primary GID. The client verifies the configured broker UID and GID before sending a request. Configure each native source ID in the service allowlist.

Create the data directory as mode `0700`, owned by the broker identity. Create the transport directory as mode `2710`, owned by the broker identity with the client's primary group. Run the service as its dedicated non-root identity.

```sh
agenturm-storage-service --data-root /srv/agenturm/history \
  --transport-dir /run/agenturm --receiver-id receiver:local \
  --client-uid 1000 --client-gid 1000 --native-sources harness.pi,harness.codex
```

Use the actual broker and client identities for the installation. A separate
client identity must be able to traverse the transport directory and connect
to its socket. Its permissions must deny direct access to the data directory.
Source adapters also need the appropriate OS isolation before they can be
treated as untrusted executables.

Create an explicit client configuration.

```json
{
  "version": 1,
  "socket": "/run/agenturm/agenturm-native-storage-v1.sock",
  "receiver_id": "receiver:local",
  "service_uid": 45000,
  "service_gid": 45000
}
```

The source configuration uses the same adapter catalog and native format as
local collection. Its source IDs must match the broker's allowlist.

```sh
agenturm broker collect --broker-config broker.json --config sources.json
agenturm broker collect --broker-config broker.json --config sources.json --watch
agenturm broker query --broker-config broker.json --harness-id harness.pi
agenturm broker review --broker-config broker.json \
  --harness-id harness.codex --session-id session-1 --turn-id turn-1
agenturm broker inspect --broker-config broker.json
```

These clients resolve history through the service. The service owns the
database path, checkpoints and durable receipts. Collection validates the
adapter facts, commits them through the broker, then sends the adapter ACK.
An exact retry recovers the committed result after a lost reply.

Queries return the existing JSON projection and stable cursor fields.
Continue a traversal with the same filters and watermark. The broker limits
source-free reads to its configured allowlist. The inspector uses the
background workspace request loop for session and turn navigation.

Private-content disclosure, grant mutation, retention and maintenance over
the broker require further implementation. The broker inspector shows
recorded history. The local cockpit provides the currently implemented saved
task, decision and private-content actions. [Broker protocol](rust-storage-broker.md)
