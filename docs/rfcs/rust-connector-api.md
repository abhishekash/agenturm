# Rust connector API: first contract slice

Status: API crate and contract tests implemented; runtime integration is not implemented.

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

This slice does not register connectors in the app/kernel, launch connector processes, evaluate grants, persist connector checkpoints or delivery receipts, schedule retries/dead letters, fence revocation during in-flight work, manage credentials, or sandbox code. These are explicit follow-on requirements, not implicit behavior of the traits. Full runtime connector work remains behind the Rust service/recovery/packaging critical path in the execution plan.

## Acceptance gates for runtime integration

- Missing, wrong-scope, stale-artifact and revoked grants deny work before launch/send.
- Inbound rows and cursor commit atomically; crash after commit but before ACK replays without duplicate durable facts.
- Outbound stable key/request digest are persisted before send; timeout reconciliation checks the same key; retry occurs only after confirmed absence and retains the same key/data.
- Retry budget, backoff, dead-letter and uncertain states survive process restart.
- A blocked/malformed/oversized connector does not stall its siblings; cleanup covers descendants.
- Diagnostics expose bounded safe codes only; no payload, prompt, credential or secret endpoint is logged.
- Fake source/sink conformance tests run without real external accounts or network access.
