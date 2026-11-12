# Agent task labels

The Agents view identifies a session by its first substantive recorded request. Follow-ups such as “switch model,” “use gpt-6.1-sol,” “continue,” and “go ahead” remain in the prompt history without renaming the task. The source line shows runtime, project, and relevant worker counts. Recent activity stays in the inspector; operational requests remain in Prompts.

For example, a session that starts with “Fix refresh token expiry,” then receives “Add cancellation coverage” and “Switch model to gpt-6.1-sol,” keeps **Fix refresh token expiry** as its task label. The later requests and their original text remain inspectable in Prompts; the per-turn trace still retains that turn’s substantive request.

## How identity is selected

1. Prefer the initial request supplied by the receiver. Native adapters retain it separately from the current turn and persist it as `agent.session.task` in captured telemetry. The session API exposes it as additive `initial_task` metadata.
2. For older receivers or producers without that field, use the earliest substantive user request available in the exactly scoped activity window. Worker identity and producer/session scope are preserved.
3. If no suitable prompt is available, retain a meaningful runtime-supplied task. If that too is missing or only contains steering, show the existing untitled-session fallback.

Complete known runtime envelopes are separated, including Claude Code’s local-command caveat and local stdout/stderr. Native model/control envelopes are recognised regardless of their block order. Canonical caveat prefixes truncated into old stored labels are suppressed only in the task projection; original captured records remain intact. Arbitrary XML, incomplete user-authored markup, inline mentions, and code examples are preserved. Standalone model/effort controls and acknowledgements are excluded from identity. Requests to implement model switching, change model loading, or use a model to perform substantive work remain ordinary requests. Operational text on the first line does not hide a substantive request on a later line. Input-reply transport envelopes remain replies rather than task names.

## Bounded collection and restart

Codex, Claude Code, and Pi retain the first substantive observed task until the source file rotates or is replaced. When startup skips into the tail of a large JSONL file, an identity-only prefix read examines at most **512 KiB and 256 complete records**, also bounded by the normal per-read byte limit. It does not ingest old activity or overwrite the latest run state. Codex internal-review classification is honored during that prefix read; internal review identity is excluded and existing captures are redacted through the established path.

OpenCode examines at most the first **32 user messages**, with a **256 KiB aggregate identity payload budget** and at most **20 linked text parts per legacy user message**. The first substantive request takes precedence over a generated session title. Successful identities are cached for the current bounded session roster; database replacement and a change of message-table source invalidate their cache keys. An unchanged identity is not reread on every database update.

If the initial request falls outside these windows, has been deleted, or was never captured, Agenturm uses the earliest useful evidence available. It does not claim to recover missing history. Metadata-only capture does not read or retain identity text. The changes are additive; no database migration or automatic provider call is needed.

After updating, restart Agenturm and any separately running receiver so that live native sessions are rediscovered with the new identity rules. Updating only the terminal while retaining an older receiver can improve the activity-window fallback, but cannot recover an original prompt the receiver does not provide.

## Summaries

The existing **AI summary** action remains available in the inspector for a broader account of the recorded work. It uses the configured provider when explicitly requested. Ordinary agent labels remain local, immediate, and stable; no model request is made to populate or refresh the roster. See the [summary and privacy documentation](usage.md#optional-ai-summaries).

## Validation

Regression coverage includes controls before and after the original task, later substantive turns, original prompt preservation, reader restart, large native files, file rotation, metadata-only capture, late internal-review classification and redaction, both OpenCode schemas, generated-title precedence, bounded identity reads, cache reuse and database replacement, worker isolation, and rendered Agents/inspector labels at 120×32 and 80×24.
