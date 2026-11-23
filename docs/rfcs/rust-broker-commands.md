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
agenturm broker cockpit --broker-config broker.json
agenturm broker task list --broker-config broker.json
```

These clients resolve history through the service. The service owns the
database path, checkpoints and durable receipts. Collection validates the
adapter facts, commits them through the broker, then sends the adapter ACK.
An exact retry recovers the committed result after a lost reply.

Queries return the existing JSON projection and stable cursor fields.
Continue a traversal with the same filters and watermark. The broker limits
source-free reads to its configured allowlist. The inspector uses the
background workspace request loop for session and turn navigation.

Pass the returned `snapshot_id` back unchanged. Backup and restore give the published history a fresh read incarnation while preserving its logical workspace identity. A cursor from an earlier branch is rejected even when new writes reach the same numeric watermark. Reopen a pinned cockpit or history gateway after restoring its broker endpoint. Unrestored histories keep their existing numeric snapshot IDs. Review acknowledgements apply to the branch where they were recorded. Earlier decision notes remain available as prior history evidence.

Use `broker review --linked-traces` with an exact turn to page its linked
spans. Use `--turn-of-trace <trace-id> --trace-source-id <otlp-source>`
in place of `--turn-id` to select the native turn from an explicitly
scoped trace. The broker checks both source IDs against its allowlist.
Each trace-selected Review page resolves the turn and reads its evidence
in a single authenticated request and SQLite snapshot.

### Send live OTLP traces through the broker

Add each `otlp.*` import source to the broker's allowlist. Run the
loopback receiver or launcher using the configured collector identity.
Both commands check the selected source before opening the receiver.
For the foreground receiver, provide a private file owned by the collector
containing a random token of 64 lowercase hex digits. Configure each
authorized producer to send that token in an Authorization bearer header.

```sh
agenturm broker serve-traces --broker-config broker.json \
  --listen 127.0.0.1:4318 --source-id otlp.local --token-file producer-token
agenturm broker run --broker-config broker.json --source-id otlp.launch \
  -- your-instrumented-program --your-flag
```

The receiver accepts uncompressed OTLP/HTTP JSON and protobuf. The launcher
uses an ephemeral loopback port and supplies a per-launch bearer token
through `OTEL_EXPORTER_OTLP_TRACES_HEADERS`. It preserves the program's
arguments and exit status. Exports receive a successful HTTP response after
the broker admits the batch or recognizes an exact retry.

Native source, session, turn and worker attributes are assertions made
by the producer. For linked Review, the broker validates the allowed
OTLP source and native source against its allowlist and checks that the
exact native scope exists in the selected history snapshot.

Run the Linux guest verification with
`python3 scripts/validate_rust_broker_otlp.py --bin-dir /opt/agenturm/bin`
to check the installed binaries with separate storage, collector and
reader identities.

### Preview opted-in telemetry text

The trace receiver defaults to metadata. To accept private captured text,
enable the exact source at the broker with
`--native-telemetry-payload-sources otlp.example=harness.codex`. For a
standalone SDK session, configure `otlp.example` without a native source and
use `--payload-scope standalone` on the launcher. Names in this policy must
also be present in `--native-sources`.

```sh
agenturm broker run --broker-config broker.json --source-id otlp.example \
  --payload-scope harness.codex -- python -m observatory run --transport otlp \
  --capture-content --harness-id harness.codex --session-id example-session agent.py
agenturm trace-payload binding --source-id otlp.example \
  --payload-scope harness.codex > payload-binding.json
agenturm broker grant approve --broker-config broker.json --binding payload-binding.json
agenturm broker trace-payload snapshot --broker-config broker.json \
  --source-id otlp.example --session-id example-session
agenturm broker trace-payload preview --broker-config broker.json --selection payload.json
```

The grant administrator approves the binding and the dedicated broker reader
runs preview. The selection JSON names `binding` with `source_id` and
`native_source_id` (null for a standalone session), the exact `trace_id`,
`span_id`, `session_id`, `turn_id`, and `worker_id` (null when absent),
and the returned `snapshot_id`. The response labels the text as
`producer_captured_telemetry`. A claimed native linkage is a producer
assertion. In the local or broker cockpit, open Traces with `8`, press
`d` to select a span, and use Enter to open the private text modal. Escape
clears the display and returns to the span list.

Local `run` supports the same `--payload-scope` with an explicit
`--source-id` and a generated bearer. Local `serve-traces` requires an
owner-only bearer file when payload capture is enabled. Use
`trace-payload grant|revoke|snapshot|preview --db history.sqlite` in the
local workflow. A manual Python OTLP client uses `capture_content=True`
and explicit `agenturm.payload.input` or `agenturm.payload.output` text.
The automatic producer projects text from supported OpenInference user and
assistant message fields. A text slot is limited to 2 KiB and a batch to
64 KiB of captured text.

Metadata queries, review receipts, and SSH history reads omit payload text.
An exact retry can recover its prior admission. Changing a captured text
slot or adding a payload to a span already admitted as metadata conflicts
with that occurrence. A broker backup retains the data in its private
history file. Restore rotates the read snapshot and requires a fresh
approval. Pruning removes the retained text and keeps replay evidence.

The Python and Node clients accept explicit bounded text fields. Set
`captureContent: true` on the Node client and provide the same
`agenturm.payload.input` and `agenturm.payload.output` attributes.
OpenInference selects user and assistant text for OpenAI, Anthropic and
LangChain. The LangGraph callback and OpenAI Agents exporter also require
their own `capture_content=True` setting. Pydantic AI and Google ADK can
supply structured GenAI message text through the Python SDK exporter.
The Google ADK legacy trace fields are also supported when its content
capture is enabled. These projections omit headers, tools, images,
reasoning fields, and arbitrary provider objects.

Provider setup is documented in the [OpenAI Agents tracing guide](https://openai.github.io/openai-agents-python/tracing/),
[Pydantic AI instrumentation settings](https://pydantic.dev/docs/ai/api/models/instrumented/),
and [Google ADK trace guide](https://google.github.io/adk-docs/observability/traces/).

The broker cockpit supports saved tasks, handoffs, request seen state and
review decisions alongside remote history. These user-owned records live
under the client's state directory. Their namespace binds the authenticated
endpoint and the broker's persistent history identity. A fresh database at
the same socket opens a fresh workspace. A running cockpit pins its remote
reads and exact link validation to the selected history identity.

## Read history through an SSH tunnel

Configure `--native-reader <uid>:<gid>` on the broker and run the read gateway
as that identity. Share a randomly generated 64-digit lowercase hex token
with the intended client. Place a copy in an owner-only file on each machine
and grant the gateway access to the native reader socket. Each token holder
can read metadata for the sources granted to that reader.

```sh
agenturm broker serve-history --broker-config broker.json \
  --listen 127.0.0.1:14319 --token-file reader-token
```

Connect through an authenticated SSH session, then direct the client to the
active local forward. The HTTP client dials the literal loopback address and
uses the bearer file for this receiver. Keep the forward active while using
the client, since TCP loopback does not identify the process listening on a
port. Use a different secret for each receiver.

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:14329:127.0.0.1:14319 receiver-host
agenturm cockpit --endpoint http://127.0.0.1:14329 --token-file receiver-token
agenturm attention status --endpoint http://127.0.0.1:14329 --token-file receiver-token
agenturm attention watch --endpoint http://127.0.0.1:14329 --token-file receiver-token --once
```

Session navigation, exact worker and turn history, Review, linked traces and
attention read the receiver's retained metadata. Tasks, review records and
request seen state are local to the client and scoped to the loopback endpoint
and persistent history ID. An active client checks that ID on each request.
Restart the gateway and reconnect after offline history restoration.

The version 1 read API accepts one bearer-authenticated JSON `POST` to
`/v1/history/read` per connection. First send `{"version":1,"request":{"op":"identity"}}`.
Use the returned `history_identity` in subsequent requests and the returned
snapshot ID when traversing a page. Session, observation, exact trace Review,
trace-selected evaluation metadata and workspace anchor checks follow the
broker's source and history rules. An evaluation request uses the selected
trace's snapshot to verify membership, then returns current retained outcome
metadata for the requested OTLP source.
Headers are limited to 8 KiB, request bodies to 16 KiB and replies to 4 MiB.
The gateway applies five-second HTTP input and output budgets and disables
response caching.

From Traces in either cockpit, press `E` to choose a loaded trace, then Enter
for its evaluation revisions. `s` shows current outcomes for the same source
with counts for the displayed page. Escape returns to the previous view with
the selected revision or trace intact. The disposable Linux fixture is
`python3 scripts/validate_rust_eval_cockpit.py --bin-dir /opt/agenturm/bin`.

Protected content, broker administration and observation admission use
their own native broker commands. Receiver host inspection and native
terminal routing are unavailable from the attached cockpit. Direct TLS and
compatibility with the Python `/api/*` connection, metrics and Review
endpoints remain open cutover work.

Run `python3 scripts/validate_rust_remote_history.py --bin-dir /opt/agenturm/bin`
in a disposable Linux guest to verify the read gateway and a separately
credentialed 60-column cockpit and attention client.

## Import trace files

Add `otlp.exported` to `--native-sources` and run these commands as the configured collector UID and primary GID. The CLI reads and validates the file and submits bounded structural span records. The broker opens history and commits each import as one batch. Client files can also be read on stdin with `--file -`.

```sh
agenturm broker import-jsonl --broker-config broker.json \
  --file spans.jsonl --source-id otlp.exported --session-id session-1
agenturm broker import-traces --broker-config broker.json \
  --file otlp.json --source-id otlp.exported
```

The same structural import and replay rules apply to local and broker history. [Span export format](rust-cutover-workflows.md#import-exported-spans)

## Record and read Host history

Run explicit Linux capture using the account that owns the processes you
want to observe. In broker mode give that collector a `host.<label>` source
in `--native-sources`. `--watch` records one sample per interval until
interrupted. The default is five seconds and the range is 1–60 seconds.

```sh
agenturm host record --db history.sqlite --source-id host.workstation --watch
agenturm host history --db history.sqlite --limit 10
agenturm broker host record --broker-config broker.json --source-id host.workstation --watch
agenturm broker host history --broker-config broker.json --source-id host.workstation
```

For ongoing collection, add a `host` block to a version 2 collection config.
A Host-only config can use the following JSON. A config with native adapters
keeps its plugin catalog and source settings alongside the Host block.

```json
{"version": 2, "host": {"source_id": "host.workstation", "interval_ms": 5000}}
```

```sh
agenturm collect --config sources.json --db history.sqlite --watch
agenturm broker collect --broker-config broker.json --config sources.json --watch
agenturm service install --config sources.json --db history.sqlite
```

The collector samples at the first collection tick after the configured
interval. It resumes the source checkpoint at restart and applies the history's
usual retention policy. Existing configs start collecting Host samples when
the owner adds the block.

The JSON history rows preserve optional load averages in thousandths, memory
in bytes and cumulative CPU counters in Linux clock ticks. The sample carries
the clock tick frequency when available. A missing value is `null` and an
observed zero is `0`. The response returns up to 64 samples and an `older`
position for the same history snapshot. Pass that JSON object to `--before`
with the same source filter to continue the traversal.

From the cockpit's session list, press `6` for host history. Enter opens the
selected sample's process list and Enter again opens the selected process's
history within the loaded page. Escape returns one level. Use `]` to read
older samples or `r` for the latest page. Press `H` from a local cockpit for
the live summary.

Each capture records up to 32 known runtime processes owned by the collector's
effective user. It includes source, Linux boot ID, PID and process start ticks
for exact process history. The view marks scan or row truncation as partial.
Processes that exit during sampling can be absent. Argument strings, environment variables, working paths
and inferred session links are absent. Native broker readers browse the
retained samples. The HTTP history gateway omits Host sources and samples.

Run `python3 scripts/validate_rust_host_history.py --bin-dir /opt/agenturm/bin`
in a disposable Linux guest to verify process ownership, each broker role and
the 60-column terminal drill-down.

## Grant administration

The service can authorize separate reader, grant administrator and maintenance
identities using `--native-reader UID:GID`,
`--native-grant-admin UID:GID` and `--native-maintenance UID:GID`.
The configured client remains the collector and reader. The roles apply
the native source allowlist. Each role's Linux identity is checked before
a request body is consumed. Configure the transport directory and socket
group access for those identities as described in the protocol guide.

Derive a binding from the exact pinned source configuration. This operation
reads the configuration and source identity without opening history or
approving content.

```sh
agenturm content binding --config sources.json --source-id harness.pi > binding.json
agenturm broker grant check --broker-config reader.json --binding binding.json
agenturm broker grant approve --broker-config grant-admin.json --binding binding.json
agenturm broker grant revoke --broker-config grant-admin.json --binding binding.json
```

For Codex operation content, add `--kind codex.mcp_payload`,
`--kind codex.edit_detail`, or `--kind codex.command_detail` when deriving the binding. Each
kind has its own approval. The JSON response reports the current epoch
when approved. It contains no disclosure token or source content.

## Maintenance

Configure an optional broker-owned backup directory with
`--native-backup-root`. The service chooses each backup name and returns
its receipt. Maintenance requests accept no filesystem destination.

```sh
agenturm broker integrity --broker-config maintenance.json
agenturm broker backup --broker-config maintenance.json
agenturm broker prune --broker-config maintenance.json --before-unix-ns 1790000000000000000
```

Pruning preserves durable source checkpoints and replay receipts. Older
query snapshots become explicitly expired. Backups disable private-content
approvals, so restored history requires fresh approval.

## Private evidence

Enable private delivery with a separate `--native-reader UID:GID` and
`--native-content-config /srv/agenturm/content.json` on the service. The
configuration must be broker-owned, mode `0600`, singly linked and under
secure absolute ancestors. It contains version `1` and a `sources` array of
resolved entries. Each entry contains `source_id`, `plugin_id`, `plugin_host`,
`adapter`, `artifact_sha256`, `features` and `source_root`, matching the exact
pinned collection configuration. The broker must have read access to the
source transcript and pinned adapter. It reopens this configuration and
verifies the adapter hash during proof checks. No client request chooses a
source filesystem path.

Give the collector ownership of its transcript and set its mode to
`0640` with a dedicated source-read group. Give the broker this group as
a supplementary group and allow it to traverse the source directory. Keep
the reader and grant administrator outside the source-read group. Preserve
these permissions when the collector creates or rotates transcripts. Socket
access uses a separate transport group, and the broker authenticates each
caller's configured UID and primary GID.

The separate reader can list and reveal approved occurrences. Collection
and grant administration identities cannot disclose private evidence.

```sh
agenturm broker content list --broker-config reader.json \
  --source-id harness.pi --session-id session-1
agenturm broker content preview --broker-config reader.json --selection selection.json
agenturm broker cockpit --broker-config reader.json
```

A user-text selection identifies the exact retained occurrence.

```json
{
  "source_id": "harness.pi",
  "session_id": "session-1",
  "event_id": "exact-event-id-from-the-prompt-list",
  "revision": 1,
  "kind": { "kind": "user_text" }
}
```

For Codex operation details, use `codex_mcp_payload`, `codex_edit_detail`,
or `codex_command_detail`
as the nested kind and include its exact `turn_id` and `operation_id` there.
Each operation kind requires its own approval. The cockpit builds these
selections from retained operation metadata.

The broker holds the proof and grant fence while the final sink flushes.
The cockpit acknowledges after its private display is redacted. CLI
previews acknowledge after stdout flushes. A random challenge binds this
acknowledgement to one bounded delivery; missing or incorrect
acknowledgements fail within the display deadline. Every private operation
pins the broker history identity, preventing stale selections from silently
crossing a replaced database. Ordinary queries retain metadata only.

## Investigate retained source evidence

```sh
agenturm broker investigate failures --broker-config reader.json --kind tool
agenturm broker investigate instruction 'keep the public API' \
  --broker-config reader.json --source-id harness.pi --session-id session-1
agenturm broker investigate path src/retry.py \
  --broker-config reader.json --source-id harness.codex --session-id session-2
agenturm broker investigate command 'pytest suite' \
  --broker-config reader.json --source-id harness.codex --session-id session-2
agenturm broker investigate command 'pytest suite' --failed \
  --broker-config reader.json --source-id harness.codex --session-id session-2
agenturm broker investigate source "$event_id" --reference "$source_reference" \
  --broker-config reader.json
```

`failures` lists reported failure candidates in the reader's authorized
history. It accepts optional source, session, and kind filters. A page
scans up to `--limit` rows at a fixed snapshot and can return a continuation
cursor even with zero matches. Eligible Codex shell candidates carry a
reference for grant-verified source inspection. A current command-detail
grant can confirm the selected failure while its occurrence remains retained.
See [failure investigation](rust-cutover-workflows.md#investigate-retained-failures).

For local history, run `agenturm investigate` with `--db`. Exact source and
phrase reads also require `--config`. Instruction search matches literal word
phrases in bounded approved user prompts. Path search matches the exact
recorded path or move destination of a Codex edit approved under
`codex.edit_detail`. Each search requires a source and session. Add
`--turn-id` to select the main worker in a particular turn, or give
`--worker-id` with the turn to select that worker.

Command search uses the independent `codex.command_detail` grant and the
exact retained native Codex `CommandExecution` occurrence. A match has a
`recorded_invocation` string or argument array in its original format.
It carries no execution-success claim. The proof withholds incomplete,
oversized, credential-like, or control-bearing invocations. Output, working
directory, and environment fields stay outside the projection.
With `--failed`, each match also requires a source-confirmed terminal failure.
Known informational exit-one commands and conflicting terminal evidence
do not qualify.

The command emits JSON Lines. Every match includes its event ID and
`source_reference`. A final coverage record reports rows examined, proved
and unavailable candidates, and any `next_cursor`. One invocation examines
up to `--limit` history rows (default 30, maximum 100) and proves at most 12
candidates. Pass `next_cursor` as `--cursor` with the same query and scope.
Exhausted coverage refers to retained, bounded evidence. Unavailable
candidates may include revoked, replaced, or unreadable source evidence.

Exact source reads take the event ID and reference from a match. They
recheck the retained snapshot, occurrence, and grant fingerprint under the
same proof fence.
Revocation, a renewed grant, or an expired snapshot requires a fresh
investigation. Match and source records flush while their individual
proof fences are held. Coverage, cursors, and references contain metadata.

## Source-backed task continuity

The reader can prepare an exact prompt draft using broker-owned sources.
The draft file contains a bounded approved excerpt and editable continuation.
Approval rechecks the source occurrence and current grant, then commits the
local task, source link and approval together while the broker fence is held.

```sh
agenturm broker task draft --broker-config reader.json \
  --source-id harness.pi --session-id session-1 --turn-id turn-1 \
  --event-id exact-event-id --revision 1 --output private-draft.json
agenturm broker task approve-draft private-draft.json --broker-config reader.json
agenturm broker task continuity 1 --broker-config reader.json
agenturm broker task continuity 1 --approval-id 1 --broker-config reader.json
```

Edit the draft's `title` and `continuation` before approval. The stored task
keeps the exact source proof and authored continuation. Revealing the source
excerpt checks its current grant and bytes again. Broker task proofs accept
no local `--config`; only the service chooses source paths. A source-backed
decision needs a draft bound to that task revision or an explicit
`--expected-revision`. Stale task edits return exit code `3`.

The client callback uses a verifier limited to the exact fenced occurrence,
so committing a task makes no nested broker request. The two databases do
not share a transaction. If a final acknowledgement is lost after the local
commit, inspect continuity metadata before retrying. The draft digest prevents
a second approval. Broker backups cover service-owned history; retain the
client state directory separately to preserve its workspace.

For a disposable root-capable Linux VM or container, the repeatable private
reader qualification is available alongside the protocol tests.

```sh
python3 scripts/validate_rust_native_private_content.py --bin-dir /opt/agenturm/bin
```

It uses unused numeric identities without creating system accounts. It
checks direct transcript and database access denial, exact source proof,
grant revocation, broker-backed task approval, and private modal redaction
before Escape, quit and Ctrl-C return control to the terminal.

[Broker protocol](rust-storage-broker.md)


## Read source-aware metadata with HTTP GET

The dedicated `broker serve-history` listener also provides these authenticated
metadata routes: `/api/health`, `/api/sessions`, `/api/activity`, `/api/traces`,
`/api/traces/<trace-id>/spans`, `/api/mcp`, `/api/metrics`, and `/api/hierarchy`.
The response format is `agenturm.http-history/1`. It exposes Rust evidence
and coverage rather than a Python receiver compatibility layer.

First read `/api/health` with the listener's bearer credential. Preserve its
`history_identity` as the `X-Agenturm-History-Binding` header on every other
GET. Missing binding returns 428; a replaced history or branch returns 409.
The reader's current source allowlist still applies. This is the same trusted
loopback/SSH-forward endpoint described above, without direct public TLS.

Queries accept `source_id`, `limit` (1–256), an opaque `cursor`, and optional
`snapshot_id`. Evidence routes also accept `session_id`. A trace's `/spans`
route requires an exact `otlp.*` source ID. Percent-encode query values;
duplicate, unknown, malformed and over-limit selectors reject the request.

`next_cursor` is an opaque JSON string: pass it unchanged as the next query's
`cursor`. Page filters report how many metadata rows were inspected, so a
filtered empty page can still have more evidence. Counts cover the current
page. Token counters, cost/credit units, nanosecond times and snapshot tokens
are strings to preserve exact integers in JavaScript clients.

These endpoints never return prompt or SDK message bodies, private operation
payloads, or Host samples. They cannot admit observations, change policy or
invoke a live sampler. The typed `/v1/history/read` protocol remains available
to the native cockpit. Private views and other receiver APIs retain their
separate implementation requirements.
