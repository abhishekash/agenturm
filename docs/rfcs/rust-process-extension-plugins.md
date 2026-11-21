# Rust supervised capability and pane plugins

Status: implementation complete; awaiting merge.

## Context

Agenturm has three extension families: harness/source adapters, capabilities,
and data-only UI contributions. Harnesses can be selected as supervised
processes through the user-managed source adapter catalog. Capabilities and
panes have a public Rust trait contract and can be grouped in a build-time
`ExtensionPlugin`; those packages are trusted code linked into the application.
The source adapter catalog does not load capability or pane code. The trusted
atomic kernel port, explicit inspector catalog/selection, supervised one-shot
runner, and independent Rust example are implemented. Full workspace tests, strict Clippy,
release example build, and independent Rust/security reviews passed locally.

The application must not load third-party Rust dynamic libraries. Kernel traits
carry Rust-owned types and have no stable cross-version binary ABI. Runtime
installable capability/pane plugins should use the supervised child-process
boundary and bounded JSON protocol instead.

## Goals

- Let a separately built executable derive bounded metrics and an optional
  data-only pane from already collected observations.
- Keep the existing `Capability`, `UiContribution`, generic renderer, and
  build-time package path compatible.
- Keep plugin identity, output validation, timeouts, process cleanup and
  failure isolation in Rust host code.
- Give the plugin only a documented typed projection, with no message bodies,
  commands, tool output, source roots, or arbitrary adapter payloads.
- Keep plugin discovery explicit through a user-managed catalog; do not scan
  directories or download executable code.

## Non-goals

- Loading `.so`/`.dylib`/`.dll` Rust libraries or defining a stable Rust ABI.
- Sandboxing, reducing OS privileges, denying filesystem/network access, or
  claiming that SHA-256 pins isolate plugin code.
- Providing host APIs for plugins to write observations, advance source cursors,
  or query the store, or to control terminal layout, navigation, focus, or
  rendering. Grants restrict the request DTO only. A child runs with the
  Agenturm user's OS privileges and may independently read files that user can
  access; the protocol does not enforce filesystem or database access control.
- Adding connector plugins in this slice; connectors remain a later family.

## Proposed boundary

Add a distinct extension catalog rather than changing source-adapter catalog
v1. Each entry has a stable plugin ID, absolute or catalog-relative executable
path, lower-case SHA-256, protocol version, exact feature declaration, and
explicit typed-data grants. The only v1 grants are `usage`, `message-role`,
`tool`, `check`, `lifecycle`, and `diagnostic`. Installing a catalog entry is
explicit consent to share the selected typed fields with that local executable.
The protocol envelope always discloses source ID, timestamp, observation kind,
snapshot receipt ID, selection scope, and a partial/truncated flag. The
observation count is the number of envelope rows before cropping; grants filter
typed payload fields only, so envelope-only rows remain counted and transmitted.
Those envelope fields are disclosed even when no grant permits a payload field,
and are part of the user's consent when enabling the plugin.
Every plugin declares `capability/1`; it may also declare
`ui-contribution/1`. The result's metrics and optional pane are registered under
the same plugin ID, so there are no separately supplied contribution IDs.
Reject a pane response when the feature was not declared, and require a pane
response when `ui-contribution/1` was declared. Validate the selected catalog
entry and artifact before opening a store; recheck the artifact before each
child launch; require the child hello to match the declared ID, protocol and
exact feature set. Existing source-adapter entries and collection config v1/v2
retain their current meanings. The inspector enables extensions only through
an explicit `--extension-catalog <path>` option and one or more explicit
`--extension <plugin-id>` selections. A catalog may declare up to 64 entries,
but a single inspector session may select at most four. Parse and validate the
catalog before opening the store; validate selected IDs, duplicate selection,
features, and grants; hash only selected executable artifacts. Unknown IDs and
more than four selections fail before store open or process launch. A catalog
without at least one `--extension` selection is an error.

The extension catalog is a bounded user-managed JSON file (maximum 64 KiB and
64 declarations); it is not scanned or fetched automatically:

```json
{
  "version": 1,
  "extensions": [
    {
      "plugin_id": "example.prompt-signals",
      "executable": "bin/prompt-signals-plugin",
      "sha256": "<64 lowercase hexadecimal characters>",
      "protocol": 1,
      "features": ["capability/1", "ui-contribution/1"],
      "grants": ["usage", "message-role"]
    }
  ]
}
```

Enable it for the read-only inspector with:

```sh
agenturm inspect --db ./agenturm.sqlite --extension-catalog ./agenturm-extensions.json --extension example.prompt-signals
```

Use the existing length-prefixed JSON framing in a one-shot child invocation
per snapshot. The parent sends a `hello` frame with protocol, package ID, and a
request ID unique per child invocation; the child returns exactly one matching
`hello_ack` frame with its exact feature set. The parent then sends one
`evaluate` frame using the same request ID; the child returns exactly one
matching `evaluate_result` frame with that same request ID and exits with code
zero. EOF, any extra frame, mismatched ID/request ID/protocol, or nonzero exit
is a failure. The invocation deadline covers artifact hash verification, spawn, handshake,
request writes, evaluation, response validation, EOF and exit. Child stderr is
discarded.

The request contains metadata (`snapshot_id`, pre-crop count of all observation
envelope rows, whether older observations were truncated, and a `selection_scope`
of `latest`, `filtered`, or `continuation`) and at most 500 observations. If the
selected page contains more than 500 rows, send its newest 500 in the store's
stable order and set `truncated` true; preserve the pre-crop count. Grants
filter payload fields only, not mandatory envelope rows. Also
set `truncated` true when the query page itself reports older rows, even if
their grants are unknown. `selection_scope` is derived from the query:
`continuation` when a before cursor is present, otherwise `filtered` when a
source or session filter is present, otherwise `latest`. It does not include
filter values or cursor contents. Its exact projection includes only
`source_id`, timestamp, observation kind, and granted typed fields. Grant
mapping is fixed: `usage` permits token counter fields; `message-role` permits
user/assistant role; `tool` permits tool category/result; `check` permits check
category/result; `lifecycle` permits state/reason code; and `diagnostic` permits
safe code/severity. Encode nanosecond timestamps as canonical decimal strings
to preserve the full `u64` range in JavaScript-based plugins. It excludes event
IDs, revisions, session/worker/turn/operation IDs, message bodies, commands,
tool output, source roots, and arbitrary adapter payloads. This v1 choice
prevents session-level joins; add opaque group IDs only with a later explicit
grant and privacy review. Do not serialize Rust `Observation` or `Snapshot`
wholesale. The projection serializer rejects a request over 4 MiB before
launching a child.
The response frame is capped at 4 MiB, with at most 256 metric rows and 256
pane rows; title, label and value limits reuse the kernel's existing 128-byte
title, 128-byte label and 1,024-byte value bounds.

Add one trusted in-process kernel `ObservationExtension` port for a single
atomic metrics-and-pane evaluation. Its kernel input includes admitted
`Observation` values and query metadata; this trusted trait input is not the
process protocol DTO. The process wrapper must independently construct the
strict, grant-filtered DTO described above from those values and must never
serialize `Observation` or `Snapshot` wholesale. The input includes the query's
derived `selection_scope`; the port returns one `ObservationExtensionOutput`
with metric rows and an optional `PaneModel`. The kernel evaluates it once per
refresh, validates the complete output, and publishes metrics and pane
atomically under the plugin ID. If durable history cannot be read, the kernel
skips this extension so an empty result cannot be mistaken for zero activity;
the store read failure remains visible. Registration rejects collisions against
existing capability IDs, pane IDs, and observation-extension IDs before the
plugin enters the registry. This avoids changing the existing
`Capability`/`UiContribution` contracts and avoids a cache between their
separate evaluation phases. A single generic extension failure is recorded if
the process fails or either output is invalid. If the catalog declares
`ui-contribution/1`, a pane is required; otherwise the pane field must be null.

The inspector remains synchronous. At most four process extensions may be
enabled; each receives a cooperative 500 ms invocation deadline covering
artifact hash verification, process startup, handshake, request writes,
evaluation, response validation, EOF and exit. Deadline checks bound protocol
polling and run before and after synchronous operations. Standard filesystem
open/metadata/read calls, process creation, and JSON parsing cannot be
interrupted by this API; a stalled filesystem, scheduler or OS call may exceed
the deadline. Extensions should therefore use local executable files. Extension
artifacts are bounded regular files no larger than 64 MiB; only selected
catalog entries are hashed before the store opens. Rehash the selected artifact
before each spawn, check the deadline during every streaming hash chunk, and
abort before launch if hashing exceeds the remaining budget. The 500 ms limit
is not a hard wall-clock guarantee for a full invocation or inspector refresh.
Initial process-group supervision is Unix-only, matching the current runtime;
non-Unix hosts return an explicit unsupported-platform error rather than
running unsupervised.

The observation DTO contains only typed fields already admitted by the kernel:
source ID, timestamp, token counters, message role, tool/check category and
result, lifecycle state/reason code, and safe diagnostic code/severity, filtered
to the declared grants. No event, session, worker, turn or operation IDs are
sent; plugins therefore cannot join turns or sessions in this version. No
prompt content, command, path, raw error, environment variable, or source root
is included. Reject unknown fields, malformed response sequences, unsupported
features and duplicate plugin IDs. Keep all app-owned layout and text
sanitization in `agenturm-ui`.

Use nonblocking reads/writes and deadline-aware frame parsing so a child cannot
pin a blocking reader or writer thread. Launch the canonical executable
directly without a shell, with a fixed working directory of `/` and a cleared
environment containing only the protocol version. Version 1 cleanup is
deadline-driven; no mid-call cancellation token is exposed yet. On timeout or
malformed output, send SIGKILL to the process group, return a bounded generic
failure for that extension, and keep healthy extensions and Overview available.
A single shared reaper polls child exit without blocking the inspector; if its
worker cannot start, pending child handles are retained for a later retry. A
child that escapes the process group is outside the kill guarantee. Nonblocking
pipes ensure escaped descendants retaining a pipe cannot keep an Agenturm
worker thread blocked; their inherited file descriptors remain outside the
host's cleanup guarantee. If the OS cannot reap a killed child, its handle may
remain queued until a later reap succeeds or Agenturm exits. Do not relay raw
stderr or child error text into the UI.

The child still runs as the Agenturm user. Grants constrain only what the host
sends over the plugin protocol; they do not prevent code running as that user
from independently reading the SQLite database, source files, or other files
that the user can access. Hash verification detects changed artifacts but is
not a signature and has a check-to-launch race. Process groups provide cleanup,
not CPU, memory, filesystem, or network isolation.

## Implementation sequence

1. Add protocol DTOs, strict schemas, explicit bounds, and host-side validation
   in a new runtime module; unit test malformed, oversized and unknown-field
   cases before wiring the CLI.
2. Add one-shot nonblocking process supervision with a deadline and
   process-group cleanup; test timeout, child exit, malformed replies, and an
   escaped descendant retaining the protocol pipes.
3. Add a separately built fixture plugin executable that returns a metric and
   pane using only the typed request DTO.
4. Add extension catalog validation and an explicit CLI configuration path for
   the durable inspector; preserve source adapter catalog compatibility.
5. Prove real Pi collection, database reopen, process-plugin evaluation,
   capability metrics, pane selection/rendering beside Overview, and absence of
   prompt text from the extension request.

## Acceptance tests

- A catalog-selected independent executable contributes metrics and a pane
  through generic `UiModel`/Ratatui rendering beside Overview.
- Exact ID/protocol/features/request-ID handshake, grant-limited observation
  projection, bounded request/response sizes, row limits, strict schemas and
  pinned artifact checks are enforced.
- Unknown IDs, unsupported kinds/features, malformed output and duplicate
  plugin IDs fail closed without suppressing healthy extensions.
- Deadline expiry attempts process-group cleanup; raw stderr and private
  content do not appear in UI failures or requests. Escaped descendants cannot
  strand a blocking host worker. Documentation states same-user execution and
  the limits of process-group cleanup.
- Existing build-time `ExtensionPlugin` tests and real Pi/Codex source adapter
  collection continue to pass.

