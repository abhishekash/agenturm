# Source-neutral collection coverage

Recorded 20 November 2026. Agenturm's Host view and plain output now consume bounded `source_coverage` descriptors from the receiver instead of naming Codex, Claude, or OpenCode in rendering code. Trusted native readers can declare a label and a coverage callback. The generic collector contributes the bounded JSONL file scan, reader groups contribute visible session counts and any known omission limit, and the HTTP host adds a process-adapter group after projection. New reader labels therefore appear without a central UI branch.

## What the counts mean

The summary keeps files and sessions as separate units. `active` counts visible files or sessions in a group. `omitted` is exact only when `omitted_exact` is true; the process projection currently reports a Boolean limit signal, so the Host says **at least one omitted** when it fires. Missing active or omission counts remain unavailable. A reader with no coverage callback is explicitly partial because its discovery completeness is unknown. Pending sources, retention gaps, malformed reader coverage, failed projection, and clipped groups produce explicit partial or unavailable text. Reader coverage callback failures are isolated and counted as native collection errors. The source group list is capped at 24 display rows, with a visible overflow note.

Older receivers have a generic compatibility translation for their JSONL and legacy reader omission counters. That translation cannot recover per-harness counts absent from the old health response. The Host shows the known omissions without claiming complete coverage.

Labels remove terminal controls and are truncated at 32 terminal cells, including wide characters. At 80×24, Host shows the summary and source groups in its inspector. At 60×18, the same inspector is available through the existing expand action; the layout and navigation keys are unchanged.

## Verification

Focused tests cover a newly named reader, exact and lower-bound omissions, unknown counts, malformed and wide labels, more than eight simultaneous limits, reader callback failure, process projection failure, and rendered Host content at 80×24 and 60×18. The work leaves Python Store as the sole writer and makes no Rust performance claim.
