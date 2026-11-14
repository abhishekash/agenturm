# Persistent attention and optional native lifecycle input

14 November 2026. Stage F implementation, prepared for integration and peer review. This is a read-only observer of agent state: requests are answered in their native terminals. No agent is started and no permission is approved by these commands.

## Without an open cockpit

Run against an existing authenticated receiver:

```sh
agenturm attention watch --endpoint http://127.0.0.1:14318 --desktop
agenturm attention watch --endpoint http://127.0.0.1:14318 --once
agenturm attention status
```

`watch` uses bounded native/session/activity inventory and the same pure source projection and attention classifier as the roster. `--desktop` explicitly enables generic desktop notification attempts. Without that flag it emits JSON changes only. `--once` exits 2 for incomplete reads instead of claiming a complete empty result. `status` reads durable local metadata without a live receiver read; missing state is explicitly distinguished from live success.

The default workflow store is shared with cockpits. `--state` selects an isolated store when desired. A separate store deliberately has separate receipts and deduplication. The additive workflow schema v2 upgrade preserves incidents, claims, acknowledgements, baselines and error receipts. Stop and upgrade/restart all cockpits and watchers sharing this store before enabling the bridge: old clients opened afterward refuse workflow writes, while an already-running old client cannot be retroactively upgraded by changing a database version. Monitoring remains available with a visible unsupported-workflow status. The watcher never starts a receiver and needs no model-provider key. Existing local-token discovery, bearer authentication and redirect refusal apply.

The first ready inventory for a receiver is quiet. It commits a receiver-clock baseline and quiet incident identities atomically. Old requests first discovered later stay quiet when their native source time predates that baseline. A fresh, never-delivered request with a comparable newer source time can notify after a watcher restart, including a request that arose while the watcher was offline. Without a comparable source clock, a previously observed fresh scope must exhibit a new request identity; no historical wait is reconstructed from receipt time.

Migration, authentication, incomplete reads and incomplete initial native discovery do not finalize a baseline. Lifetime collector error counters do not block recovered healthy inventory. Current backlog/pending-storage and source/read failures remain explicit. Receipts and atomic notification claims are the shared authority across two cockpits and a watcher. Seen requests remain pending until positive resolution. Delivered, acknowledged or uncertain in-flight delivery claims do not automatically repeat. Failed delivery remains inspectable and can be retried explicitly from Workflow.

## A bounded optional native contract

The native bridge is opt-in. It does not change native configuration, install hooks or replace ordinary zero-config collection. Create an owner-only binding with `attention native bind --help`. The binding fixes receiver, producer, canonical actor, session, source and optional confirmed parent, plus a recorded trace/turn. These are checked against authenticated recorded evidence; sender JSON cannot choose a sibling, parent or another session.

`generic-v1` accepts at most 8 KiB of JSON and a strict field allowlist. It requires a stable event ID, authoritative positive integer sequence, exact configured identities, and a stable request ID for request/resolution events. Duplicate JSON keys, non-finite numbers, contradictory identities, reordered events and reused request IDs are rejected as uncertain. Request content, commands and approval decisions are not accepted. The bridge records only lifecycle metadata.

A minimal event uses the identity fields from its binding:

```json
{
  "schema": "agenturm-native-event-v1",
  "sender_id": "<from binding>",
  "receiver_id": "<from binding>",
  "service": "<from binding>",
  "agent_id": "<from binding>",
  "session_id": "<from binding>",
  "trace_id": "<from binding>",
  "turn_id": "",
  "source_id": "<from binding>",
  "parent_agent_id": "",
  "event_id": "request-event-1",
  "sequence": 1,
  "kind": "approval_requested",
  "request_id": "native-request-1",
  "time_unix_nano": 0
}
```

Use `attention native send --binding /private/binding.json` with that JSON on stdin. A later `request_resolved` event must name the same request and have a greater authoritative sequence. Unrelated work or a new turn cannot blindly clear a bridge request. Multiple pending requests in the same exact scope remain uncertain rather than arbitrarily selecting one. Unknown timestamps stay unknown; a separate transport receipt timestamp is explicitly labelled and never used to manufacture request age or native ordering.

The durable metadata outbox keeps pending writes after failures. `attention native retry --binding ...` retries them in order. A deterministic event identity makes a post-before-local-commit crash safe to replay. Writes require an atomic expected-receiver precondition in the receiver transaction and a matching final receiver read before local confirmation. A receiver replacement cannot silently redirect a bound event.

There are limits of 1,000 native senders, events and request states. Unsent or still-referenced state is not silently evicted to meet a limit. Capacity and uncertainty are visible through native status metadata and stderr. A `native hook` observer prints no decision JSON and returns a non-blocking exit even when observation fails.

## Runtime capability boundary

Versions available for read-only inspection here were Claude Code 2.1.304, Codex CLI 0.167.1, OpenCode 2.2.5 and Pi 1.3.0. CLI version inspection is not a live compatibility trial. No personal transcript or native configuration was modified.

- **Codex:** official external `notify` is a completion channel and supplies session/turn IDs. The `codex-notify-v1` adapter forwards completion metadata only. A fixed binding covers one turn. `--follow-recorded-turns` supports repeated turns only for the same configured native source: each native turn ID is transformed using the existing reader identity function, then independently checked against exact recorded API evidence and matching native source metadata. Its local transport counter is explicitly not native ordering or request proof. It cannot generate or resolve approval requests. Configure `notify` manually to invoke `agenturm attention native hook --binding /private/binding.json`; Codex appends its JSON argument. [Official Codex configuration](https://developers.openai.com/codex/config-advanced/).
- **Claude Code:** the documented permission hook lacks a stable tool-use ID, and permission notifications do not supply a stable request ID. Neither is silently upgraded into an exactly identified approval. No guessed approval hook recipe is installed. A host that supplies authoritative identities can use the explicit generic contract. [Official hooks reference](https://code.claude.com/docs/en/hooks).
- **OpenCode:** the current V2 plugin API exposes event subscriptions and explicit permission request lookups. V1 handler assumptions are not reused against V2. No untested V2 payload mapping is presented as a working approval adapter; an explicitly versioned integration must supply the generic contract's exact identities and ordering. [Official server plugin API](https://opencode.ai/docs/build/plugins/server/).
- **Pi:** final settled state differs from a low-level agent end, and extension prompt lifecycle notifications do not document unique request IDs. They are not guessed into approval incidents. Live Pi/Claude event delivery was not exercised here. [Official extension contract](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md).

## Verification and limits

The first 47-test attention/store/controller/native gate passed. A 51-test gate including authenticated real HTTP polling, native metadata ingestion, foreign receiver write rejection, no-Textual subprocess execution and restart deduplication passed in 4.079 seconds. The expanded 76-test gate, including Workflow keyboard journeys, passed in 40.128 seconds. The final 55-test schema/HTTP/native/controller delta passed in 4.210 seconds; a further clock/codec gate verifies that an explicitly unknown bridge clock cannot inherit a native collector clock. Independent review is recorded during integration. Both HTML widths were checked in Chrome at 1280 px/light and 390 px/dark with no overflow or external assets; the mobile report was visually inspected.

Tests use isolated receiver databases, workflow stores and mocked desktop delivery. They cover quiet baseline, old delayed inventory, offline fresh requests, absent clocks, namespace replacement, partial reads, durable claims among three consumers, acknowledgement without approval, source/worker/parent isolation, metadata bounds, replay, ordering and matching resolution. Ordinary source collection remains available when optional bridge input is uncertain.

This stage does not establish a whole-monitor CPU improvement, universal runtime compatibility or participant adoption. Existing resource targets and real Claude/Pi validation remain open. A desktop process crash after claiming but before confirmed delivery remains an explicitly uncertain at-most-once attempt; it is not automatically retried as if absence of confirmation proved no notification occurred.
