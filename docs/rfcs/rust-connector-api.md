# Rust connector API and trusted runtime

Status: bounded contracts and an opt-in trusted process runtime are available.

## Goal

Connectors are a separate extension family from harness adapters. A connector talks to an external system; harness adapters discover and decode an agent's local activity. Inbound connectors import remote records. Outbound connectors deliver host-authorized actions. The two directions have different acknowledgement and uncertainty rules, so the API exposes separate traits.

The current slice is `rust/agenturm-connector-api`. It defines versioned, serializable DTOs and Rust traits that can later be carried by a process protocol. It deliberately gives the connector no store, cursor table, credential store, or policy authority.

## Inbound contract

An inbound connector receives the last durable checkpoint and a host-provided count/byte budget. It returns a bounded batch with a next checkpoint. The host validates and projects allowlisted fields, validates domain facts, and commits the records with the checkpoint in one transaction. Only after that commit does it call `ack`. On failed commit, the host must not ACK, so replay starts from the last committed checkpoint.

The API rejects oversized checkpoints, batches, record counts, repeated record IDs and invalid IDs. These checks bound the DTO; they do not prove the payload is safe or authorized. The host still owns projection and domain validation.

## Outbound contract

The host constructs a delivery containing a stable delivery ID, idempotency key, destination ID, action, payload and SHA-256 digest over the destination/action/payload tuple. Lookup requests carry the full delivery identity, and responses echo its destination, key and digest so a host can reject a response for another request. A connector can send and look up a key. Lookups distinguish accepted, confirmed missing, and unknown. A timeout is ambiguous; the host must reconcile the same key before any retry. Reusing a key with different request data is a conflict.

In this first API version, the digest is computed and validated by the Rust host using this crate's JSON representation; an external connector returns the digest it was given and does not recompute it. This is not a cross-language canonical JSON specification. A future process protocol that needs third-party recomputation must define canonical bytes and test vectors before claiming interoperability.

This is an at-least-once attempt model. Exactly-once external effects are not promised: they require a sink that deduplicates or can authoritatively look up the idempotency key. If the sink cannot resolve an ambiguous result, the host must preserve an `unknown` outcome for reconciliation rather than blindly resend.

## Trust and policy boundary

Descriptor feature declarations describe protocol support; they are not permissions. The API does not implement grants or endpoint policy. Before runtime activation, the host must bind grants to connector ID, pinned artifact digest, exact source/destination identity, permitted fields/actions and a revocable policy epoch, then recheck authorization at admission or dispatch.

Process supervision is a crash/protocol boundary, not an OS sandbox. A child under the Agenturm user's identity can still access that user's files, credentials and network. Do not activate untrusted third-party connector executables until a least-privilege OS boundary and credential isolation exist. Prompt/content and credential fields require separate grants; connector diagnostics use a closed enum code set that cannot carry payloads or secrets.

## Current evidence and limits

Fourteen tests cover version/identity validation, bounded inbound batches, duplicate record rejection, checkpoint progress and control-character validation, modeled commit-before-ACK ordering, idempotent/stale ACK behavior, outbound request digest validation and response correlation, idempotency conflict, fake accept-then-timeout reconciliation, distinct unknown/missing/accepted states, bounded serialization, and a closed error-code vocabulary that rejects token-like strings. The fake-source test models the intended host/store sequence; it is not a Store integration test.

## Trusted process runtime

Build the standalone driver with `cargo build --locked -p agenturm-runtime
--bin agenturm-connector`. The driver takes a connector ledger and a JSON
configuration. The configuration declares `descriptor` (the public connector
descriptor), absolute `executable`, lower-case `sha256`, and exact `target`.
For inbound collection, the target is the host-assigned observation source ID.
For outbound delivery, it is the destination ID in every queued request.

The driver requires an explicit grant for the whole configuration before it
opens a connector process. Revocation takes effect before the next fenced
exchange. An unchanged reapproval preserves the epoch. Changed configuration
or reapproval after revocation does not restore old work. The child receives
one length-prefixed JSON request with the version, descriptor, target and
request ID. A matching single-frame response is required. Commands run with
a cleared environment, bounded frame size and invocation deadline. This
transport is for pinned executables that the operator already trusts.

```sh
agenturm-connector connector.sqlite source.json grant
agenturm-connector connector.sqlite source.json watch history.sqlite
agenturm-connector connector.sqlite sink.json grant
agenturm-connector connector.sqlite sink.json enqueue-status delivery-id stable-key completed
agenturm-connector connector.sqlite sink.json watch
agenturm-connector connector.sqlite sink.json revoke
```

Inbound projection currently admits only exact `session_id` and supported
lifecycle `state` fields. The host atomically commits the resulting observations
with the checkpoint in the Rust observation store. The next collect repeats
the ACK for the durable checkpoint before polling, including after a crash.
Outbound currently permits the exact `write_status` action with a lifecycle
state string. The ledger binds delivery IDs and idempotency keys to the request
digest and grant epoch. Each `drive` advances one delivery when due. It persists
uncertain state before sending, reconciles ambiguous effects using the same
key, and retries only after confirmed absence. Attempts and reconciliation
have independent persisted bounds and backoff. A sink that cannot confirm
acceptance or absence leaves the delivery unresolved.

`watch` stays in the foreground for one pinned configuration. It polls an
inbound connector using an exclusive history writer lease, or advances up to
eight due outbound deliveries per cycle. Progress appears as safe JSON
events. These report inbound counts, delivery state changes, retry errors,
startup and shutdown. Delivery keys, receipts and payloads stay out of the
watch log. SIGINT or SIGTERM stops the worker after the current bounded
operation. A revoked grant stops further processing. For one step at a time,
use `collect <history.sqlite>` or `drive <delivery-id>`.

Run `cargo test --locked -p agenturm-runtime --test connectors` for
the local fixture executables. The suite exercises authorization denial,
commit-before-ACK recovery, enqueue conflict checks, accept-then-timeout
reconciliation, confirmed-missing retry, watcher shutdown, and the standalone
command entry point. The fixtures use temporary local files and no remote accounts.

Additional field permissions, independent parallel execution within one
ledger, credential provisioning, and an OS boundary for untrusted executables
remain open.

## Acceptance gates for runtime integration

- Missing, wrong-scope, stale-artifact and revoked grants deny work before launch/send.
- Inbound rows and cursor commit atomically; crash after commit but before ACK replays without duplicate durable facts.
- Outbound stable key/request digest are persisted before send; timeout reconciliation checks the same key; retry occurs only after confirmed absence and retains the same key/data.
- Retry budget, backoff, dead-letter and uncertain states survive process restart.
- A blocked/malformed/oversized connector does not stall its siblings; cleanup covers descendants.
- Diagnostics expose bounded safe codes only; no payload, prompt, credential or secret endpoint is logged.
- Fake source/sink conformance tests run without real external accounts or network access.
