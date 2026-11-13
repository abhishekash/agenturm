# Privacy and capture boundaries

Agenturm is designed for local inspection. CLI receivers bind to loopback and authenticate by default. Telemetry uses an owner-only database in the common Agenturm state directory; explicit database paths remain supported. Local tokens are owner-only regular files and are published atomically. See [receiver setup and explicit migration](receiver.md).

## Native session discovery

On loopback receivers, the local monitor reads recent Codex, Claude Code, and Pi session files and OpenCode's local SQLite database. It tails session files and opens the OpenCode database read-only. Initial file reads are bounded to recent files and a limited portion of each file; OpenCode discovery is bounded to 50 recent sessions, 30 assistant messages per session, 200 tool parts per message, and 100 todo items per session. Captured activity can include messages, task checklist text, shell commands, file paths and diffs, browser activity, tool inputs/results, and MCP arguments/results. OpenCode reasoning parts are excluded. Metadata-only mode keeps checklist counts and status but drops their text.

The local MCP inventory parses known Codex, Claude Code, Cursor, Gemini CLI, OpenCode, and VS Code configuration files. It supports OpenCode's `opencode.json` and `opencode.jsonc` MCP layouts, including the current nested `mcp.servers` format. For running same-user Gemini CLI, Cursor, and OpenCode processes, it checks the process working directory for the corresponding project config; that directory is used only for the lookup and is not returned. It retains and returns only server names, client names, and configuration scope; command, argument, URL, header, and environment values are discarded and never sent to the receiver API. It does not launch or contact configured servers. Discovery is bounded to known config locations and files up to 1 MiB.

On Linux, session health checks same-user known agent processes for an exact open
session-file match. Only matching PIDs, process start identity, and CPU/I/O counters
(including bounded child-process observations) are used for activity hints. This
does not retain process arguments or inspect environments/session-file contents for matching. A bounded Codex command-mode probe retains only whether the process is a shared app-server/daemon. Shared processes never establish per-session progress or liveness, even if only one session is observed; matches are not inferred from a shared project directory. Process observations are
in-memory health metadata, not additional stored telemetry.

Hierarchy capture stores only opaque canonical identities, native session/worker IDs, relationship kind/state, and bounded provenance (adapter and record reference). It does not store spawn arguments, environments, sidecar descriptions, or extra transcript content. Metadata-only mode retains structural relationships when a session header or sidecar supplies them; it still omits task text, prompts, arguments, and outputs. Hidden approval-review relationships are filtered out of visible cockpit rows, counts, and exports.

Recognizable credentials and selected sensitive structured fields are redacted before storage on a best-effort basis. Private reasoning text, opaque encrypted reasoning, binary payloads, process environments, and credential files are excluded. Upstream schemas and truncation can leave gaps. Treat the local database as sensitive because normal agent activity may itself contain private project information.

Native activity content can be limited for new ingestion with:

```bash
agenturm serve --metadata-only
```

This does not erase content already stored. To stop native session discovery, use `--no-local-agents`.

## SDK capture

The SDK launcher omits prompts and model responses by default. Content capture is opt-in. The metadata-only setting for native session discovery is separate from SDK capture options.

## Local UI preferences and exports

The cockpit saves only theme, active view, and non-sensitive filter flags in its
local UI preferences. It does not save searches, session scope, credentials, or
selected evidence there. Set `AGENTURM_UI_PREFERENCES=0` to disable persistence.

Pressing `x` opens an export privacy preview; no file is created until explicit
confirmation. Exports can contain private recorded content and are not an
additional redaction pass. They include currently loaded selected evidence, not
complete session history. New files are owner-only and existing files are not
overwritten. You control whether and where they are shared.

## Network and retention

The receiver binds to `127.0.0.1` by default. Do not expose it to a network without authentication and a protected TLS/reverse-proxy path. New managed stores default to 14-day retention; existing explicit stores require an opt-in policy. Host/Origin validation applies even with explicit `--no-auth`. The local token is not a sandbox against other processes running as the same OS user. See [receiver setup](receiver.md).

Retention performs logical deletion, not guaranteed secure erasure. Deleted
content may remain in SQLite free pages, journals, filesystem snapshots, or
backups. Protect the database and its backups as sensitive files; a retention
setting is not a substitute for disk encryption or an appropriate disposal
policy.

Token totals reflect emitted telemetry and may include cached context. Costs remain unknown when the producer does not report them. Neither token nor cost totals should be treated as a billing statement.

## Attention and task continuity

Receiver databases have an opaque persistent receiver identifier. Older receivers
use an explicitly labeled endpoint namespace; credentials never enter that key.
The local `workflow.sqlite3` records incident identities, acknowledgements and
notification delivery claims independently from telemetry. Desktop notifications
omit prompt/command content and require opt-in. A seen request is not a native
approval. Initial inventory and stale capture do not generate fresh alerts.

Structured task briefs and decision versions remain user-owned local content in
additive tables in `tasks.sqlite3`. Notes are not automatically interpreted as
decisions. Explicitly copying an excerpt creates an editable unsaved draft;
metadata-only records do not provide content for excerpts. User notes, decisions,
source references and Git snapshots can contain sensitive information.
Review exports use the explicit frozen preview and existing owner-only,
no-overwrite export behavior. No new cloud service or model call is introduced.

Opt-in workflow-pattern diagnostics compute safe fingerprints and persist observations and source
references, without adding copies of raw tool arguments. They remain observations
about a bounded captured window, and do not trigger desktop alerts.

Workflow acknowledgements are retained for 90 days. Resolved unacknowledged
incident payloads and delivery claims are cleaned after 30 days; minimal generic
execution references supporting a valid acknowledgement remain for its 90-day
lifetime. Unresolved incidents do not expire automatically. Cleanup follows
these policies rather than deleting a pending native request.
