# Native adapter hook isolation checkpoint

19 November 2026. A failure in one registered adapter's optional peer-link hook is now recorded and the collector continues invoking the other adapter groups. A failing or malformed status-metadata hook contributes no extra fields for that session and cannot make the whole health response fail. Adapter status metadata cannot overwrite host-owned identity, capture, lifecycle, process, diagnostics or hierarchy fields.

The collector still treats trusted adapter parsing and evidence admission as separate paths; this change specifically isolates optional peer and status hooks. It does not sandbox code running in-process. The provenance manifest binds the new collector hash to commit `26d0ac8` and retains the previous hash as history. No synthetic fixture bytes or producer-version claim changed.

Validation: 79 focused native provenance, port, Kiro, session, lifecycle and source projection tests passed. A fault-injection test demonstrates that a healthy adapter's peer hook still runs after another raises, that health metadata fails closed for that adapter, and that a status hook cannot spoof `source_id`. `compileall` and `git diff --check` passed. Full merged-run CI remains a separate gate.
