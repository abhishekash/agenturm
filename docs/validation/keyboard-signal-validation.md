# Real-terminal keyboard and signal validation

This is a generated-fixture functional check of runtime `55c6244bf0c1caad2a22cd5c826020f50a677e54` with committed harness `5ee3185528cce588d3bbee5c66b0ae240f38a046`. It is not a CPU/RSS or live-agent result. The later read-snapshot and capture-safety fixes need their final combined candidate checks separately.

| Exit path | Observed wrapper status | Terminal restored | Checks passed |
| --- | --- | --- | --- |
| Normal quit from reader | 0 | Yes | 26 |
| SIGINT from reader | 130 | Yes | 26 |
| SIGTERM from reader | 143 | Yes | 26 |
| SIGINT during startup | 130 | Yes | 1 restoration check plus explicit signal journal |
| SIGTERM during startup | 143 | Yes | 1 restoration check plus explicit signal journal |

Reader journeys use actual tmux input at 120×36, 80×24 and 60×18, with a 50×14 too-small frame and return. They cover prompt Up/Down/Home/End, section changes, reading, help, resize and restored terminal output. Each process uses the production terminal wrapper, isolated fixture home/config/state and synthetic Feed data. No provider, native agent, personal transcript or user tmux session is used.

The initial development harness incorrectly assigned `on_mount` on an instance; Textual dispatches declared class handlers, so the intended startup signal was never sent. The corrected fixture declares the lifecycle handler on a subclass and journals the actual signal before sending it. Earlier failed startup attempts remain failed artifacts, rather than being reported as product signal failures.

The resize test also relied on a captured title before focus had settled. A passive public-state journal now confirms the actual terminal size, populated table and restored table focus before Enter. It does not mutate framework state, prepare sources or substitute for visible prompt assertions. The History selection is explicit because these fixture sessions have unknown lifecycle and are correctly absent from Today; titles come from the first captured request.

Raw result directories are `/tmp/agenturm-final-keyboard-c-matrix-{q,sigint,sigterm}-reader` and `/tmp/agenturm-final-keyboard-c-matrix-{sigint,sigterm}-startup`. Each records exact source/harness/dependency manifests and the actual application/fixture module paths. The wrapper status is recorded separately from the fixture process, which writes the result and restored-output marker. The checks ran sequentially after the harness correction; no failed attempt was silently converted into a passing result.

This matrix proves these functional terminal paths on the named runtime and reference host. It does not establish a modal memory plateau, native provider compatibility, durable receipts under every failure, or the final resource budgets.
