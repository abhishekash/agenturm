# Interrupted session evidence

Validated locally on 2026-11-18, based on `9aba1d6209873d2d299478ee7c20eb866d6acc6e`.

## What changed

The inspector distinguishes the latest interrupted turn from earlier completed work in the same conversation. A raised or merged PR does not itself establish completion of a subsequent turn. The original session title remains stable; a separate **Turn request** identifies the interrupted work when that request is loaded.

Codex lifecycle Activity now retains the native event name and supplied reason through the existing redaction, content-capture and size limits. The inspector shows the event time and reason without treating an unrelated failed check as the cause. A generic `interrupted` reason explicitly leaves the actor and cause unknown. Loading, failed requests, metadata-only capture and bounded history are distinguished from a known reason.

Press **s** to open surrounding session Activity with the interruption selected and the Errors filter cleared. Press **t** for the recorded trace. Trace links prefer the selected execution, reject conflicting ownership and never infer identity from matching task titles. Legacy summaries with missing ownership fields still work when the selected session supplies an explicit link. A paused view includes **p Follow live**.

Late terminal events for a different Codex turn remain in history without overwriting the current turn or clearing its pending approval. Late tool results cannot reopen an already completed or interrupted turn. Regression cases cover abort, resume, completion and older final answers.

## Validation

- Initial complete suite: **1,395 tests**, **14 optional skips**, **4 failures**, in 1,822.216 seconds. This was not a green full-suite result.
- The failures were an overly strict explicit trace link, a navigation fixture that depended on task-title guessing, the pinned reader hash after the parser change, and the HTTP smoke test's disk-headroom guard.
- Corrected trace handling, supplied an explicit link in the navigation fixture, and preserved historical provenance while binding the new reader hash. The affected parser, navigation, evidence, UI and provenance follow-up suite passed: **80 tests**, 214.964 seconds.
- HTTP subprocess smoke passed separately: **1 test**, 13.718 seconds. It used `/dev/shm` for temporary files because the normal temporary filesystem had less than the required 4 GiB reserve. No reserve check or assertion was weakened. This is functional evidence, not a disk-performance measurement.
- Final delta validation passed: **12 tests**, 22.241 seconds, covering the additional older final-answer guard and loading/error/partial evidence messages. A subsequent store-backed check verifies long redacted reasons through the actual Activity projection, and malformed lifecycle records cannot stop the reader: **11 tests passed**, 1.853 seconds.
- Keyboard journeys exercise **150×40**, **80×24** and **60×18**. Synthetic rendered screens were inspected at all three sizes. The narrow layout uses the existing single-pane inspector, scrolling and persistent actions. No additional panel borders or per-row status markers were introduced; duplicated status evidence and latest-request text were removed from the interruption overview.
- `git diff --check` passed. No personal transcript contents were added to the repository.

## Limits and rollout

Evidence loading remains bounded to a recent session window. If the relevant event or request is outside it, the inspector says so and offers Activity/Prompts inspection; it does not infer a cause. A native reason such as `interrupted` cannot establish who interrupted the work. Completion remains agent-reported rather than independent verification of a PR or its checks.

Restart a local checkout's Agenturm receiver and TUI after updating to pick up the parser and inspector changes. Already stored events without native reason detail may require normal source replay; this change does not invent missing historical reasons. This fix does not publish a new package release.
