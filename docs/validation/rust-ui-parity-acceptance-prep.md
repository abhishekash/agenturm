# UI-Q acceptance preparation — not final acceptance

24 November 2026 · branch `work/ui-parity-acceptance-resume` · [HTML companion](rust-ui-parity-acceptance-prep.html)

**Outcome:** the Python oracle is runnable, 47 exact-tag tests pass, and a reusable exact-binary PTY smoke harness is prepared. **No Rust executable was run or qualified.** Recovery source `2b162cbda0cd5c308b6ca5eb620853d55f3ce064` is explicitly **not** the final candidate. No Cargo build/test was started while the parent compiled serially.

## Identity and isolation

| Item | Recorded evidence |
| --- | --- |
| Owned worktree | `/tmp/opencode/agenturm-ui-parity-acceptance` only |
| Oracle | Clean, read-only `/tmp/agenturm-python-028-reference`, tag `v0.2.8`, SHA `2802d79930728bb331db1d4d718d6dea1c9c374d`; clean and unchanged after both completed runs |
| Oracle installation | Tagged source imports via `validation_support.bind_application`; not an installed wheel or a shared global tool |
| Private interpreter | `/tmp/opencode/uiq-oracle-venv/bin/python`, CPython 3.14.10 |
| Runtime dependencies | Textual 8.2.8, Rich 15.0.0, markdown-it-py 4.2.0, mdit-py-plugins 0.6.1, platformdirs 4.13.4; versions match the tag's lock |
| Tagged lock SHA-256 | `cb0271e65d61206ce50ef007bfc96ad8689ba46e1dec5311032afdbfb3c086ad` |
| Platform | Linux x86_64, kernel `7.0.0-38-generic` |
| Terminal | Textual headless Pilot, `TERM=xterm-256color`, UTC; 120×32, 80×24, 60×18 |
| Fixture | `uiq-tagged-manual-feed-v1`, tagged `ExampleApi`/`ManualFeed` plus three synthetic requests; timestamp anchor `1795521600000000000` (2026-11-24 12:00 UTC) |
| Fixture SHA-256 | `35d4074b72a27302947b5344cc6f1cfc8fda9ee02b6c844848646913ef17195a`; identical across test/capture reruns |

`validation_support.create_owned_directory` and `isolated_env` are reused. Children receive a private HOME, XDG state/config/data, TMPDIR and no inherited provider credentials or source-root overrides. `-B` prevents writes of bytecode into the oracle. No real user logs, other worktrees, external tmux sessions or other agents' processes were modified. No generated binary or large SVG capture is committed.

The plan, inventory, shared contracts (Markdown and HTML), and UI-03/04/05/06 lane notes were read. Separate UI-07/shell-integration note files are absent from this recovery tree; shared contracts and lane hook sections are available, but the parent must supply any newer handoff notes with the final candidate.

## Completed checks

### Exact tagged Python tests

Command:

```sh
/tmp/opencode/uiq-oracle-venv/bin/python -B scripts/capture_python_ui_oracle.py \
  --reference /tmp/agenturm-python-028-reference \
  --output /tmp/opencode/uiq-python-oracle-prep-02 --tests
```

**47 passed, 0 failures, 0 errors, 0 skipped**, in 178.233 seconds. The exact 47 IDs and raw result are in `/tmp/opencode/uiq-python-oracle-prep-02/tagged-tests.txt` and `results.json`; the [committed test ID record](rust-ui-parity-acceptance-prep-tests.txt) preserves the small result independently of temporary captures.

| Exact tagged selection | Tests | Main acceptance links |
| --- | ---: | --- |
| `tests.test_prompt_keyboard` | 4 | UI-04: newest selection, wide arrows, unchanged compact confirmation, focus remapping, help/resize |
| `tests.test_keyboard_navigation` | 3 | UI-02/06: scrollable floor help, literal typing, nested selector Escape, unsaved brief protection, empty Workflow focus |
| `tests.test_inspector_layout` | 13 | UI-03/04: sections, exact older Activity handoff, readable/original and copy/export, retained pages, races, retry, reader geometry |
| `tests.test_session_scope_picker` | 8 | UI-03: exact worker/session identity, no-match/cancel/clear, disappearing choice, bounded chooser geometry |
| `tests.test_terminal_themes` | 3 | UI-07: five theme selection and evidence syntax backgrounds |
| `tests.test_status_confidence_ui` | 4 | UI-02/05: history delay, empty-history error, initialization, disconnect/auth/recovery |
| `tests.test_today_outcomes` | 10 | UI-03: Today/History, reviewed outcomes, current work of any age, retained lifecycle evidence |
| `tests.test_terminal_review.FeedFilterRaceTest` | 2 | UI-04: matching prompt scope captured before reads; superseded poll discarded |

These are the tag's own sizes and fixtures, not a claim that all 47 tests run at every target size. Several tests intentionally call application methods to isolate semantics; only explicitly keyboard-driven tests establish those key journeys.

### Deterministic oracle captures and clock checks

Final capture command (without rerunning the tagged suite):

```sh
/tmp/opencode/uiq-oracle-venv/bin/python -B scripts/capture_python_ui_oracle.py \
  --reference /tmp/agenturm-python-028-reference \
  --output /tmp/opencode/uiq-python-oracle-prep-04
```

**42 SVG/state captures**, approximately 1.92 MB total SVG, remain outside Git. At each starting size: roster; latest Prompts; older readable request; same request Original; help; framework palette; navigator resize to 60×18; seven root destinations. `captures.json` records the actual viewport (including resized frames), focus, exact source/request, reader text/scroll, key sequences where supplied, artifact name and SHA-256. `results.json` records the harness hash and unchanged reference. Root Host/Findings are empty in this particular synthetic fixture; this is not an all-populated matrix. Only the seeded metadata and requests have fixed timestamps; app timers, ages and SVG document IDs are not claimed byte-identical.

The final run also passed **15 fixed local-day checks plus 105 Today bucket assertions**: UTC, America/New_York and Europe/Berlin at noon UTC on 2026-03-08, 03-29, 10-25, 11-01 and 11-24. Local midnight is independently checked with `zoneinfo`, including different midnight/noon DST offsets. Old running work and pending approval remain eligible; reviewed pending input, old outcomes and reviewed completions do not. This qualifies the **Python policy only**, not Rust host timezone plumbing.

Earlier attempts: `prep-01` hit an accidental 1-second harness timeout and establishes nothing; `prep-02` produced 21 initial captures alongside the successful tagged suite; `prep-03` exercised the expanded 42-capture journey; `prep-04` supersedes capture metadata and adds the fixed-clock matrix. Do not combine these as repeated independent test-suite passes.

### Acceptance-harness self-tests

```sh
/tmp/opencode/uiq-oracle-venv/bin/python -B -m unittest -v \
  tests.test_ui_parity_acceptance_harness
```

**4 passed**: hash mismatch refuses a copy, exact private copy never overwrites, HOME/provider-root/credential isolation, and a clearly labelled **Python PTY stub** verifies resize plus q/Ctrl+C termios restoration. The stub is not Agenturm and is not Rust acceptance evidence.

## Prepared exact-binary gate

`scripts/validate_rust_ui_parity.py` reuses `Cockpit` from `validate_rust_default_cockpit.py` and `TerminalCells` from the existing private-detail PTY harness. It requires a clean source tree at the supplied source SHA and a supplied executable SHA-256, then snapshots the executable privately and checks its hash before/after. Optional archive hashing is recorded. Source-to-binary provenance still requires the builder's attestation; this is not reproducible-build proof.

```sh
/tmp/opencode/uiq-oracle-venv/bin/python -B scripts/validate_rust_ui_parity.py \
  --agenturm /absolute/frozen/bundle/agenturm --sha256 EXECUTABLE_SHA256 \
  --source-root /absolute/clean/candidate --source-sha FULL_SOURCE_SHA \
  --archive /absolute/frozen/archive.tar.gz \
  --output /tmp/opencode/uiq-final-empty-smoke
```

The smoke gate initializes an **empty private history through the real CLI** (`retention disable --db`), independently confirms zero observations, and launches `cockpit --db` with empty PATH and isolated HOME/XDG. No live default collection, adapter, receiver or source discovery runs. It prepares six PTY journeys (three sizes × q/interrupt): startup, Ctrl+P, literal `?[]123/` search/no-match, clear/cancel, F1 help and scroll/return, palette Activity execution, `1` return, cross-size resize, below-floor recovery, alternate-screen departure and unchanged termios. Ctrl+C is sent **inside help**, because public evidence Ctrl+C is copy by contract. Frames/transcripts/results survive a failed journey; no success is inferred from silence.

**Not yet run against Rust.** This is deliberately an empty retained-history smoke gate, not a replacement for populated prompt, grants, forms, selected identity, theme, clipboard, export, failure shutdown, attached/broker/provider or actual live-startup qualification. The reused cell parser does not prove terminal-emulator color/contrast/OSC behavior.

## Findings to carry to the final candidate

| ID / severity | Gate / lane | Reference or required contract | Observed recovery source | Reproduce / fix / rerun |
| --- | --- | --- | --- | --- |
| PREP-ARCH-01 / High, recovery-source only | Pluggable architecture; UI-06/shared shell | Shared contract §1 and §9 prohibit provider-specific behavior in UI; provider capabilities own native commands | `rust/agenturm-ui/src/workbench/workflow_resume.rs:40–49` matches Codex/Claude source families; `:52–61` assembles `codex resume` / `claude --resume` in the UI | Static source read; no executable reproduction claimed. Move provider command planning/validation behind the existing typed adapter/host capability; UI receives command/reason plus exact request proof. Keep worker denial and safe identifiers. Recheck final diff and Workflow preview/copy/exact-row return with supported and unsupported adapters. |

**Finding count:** one high recovery-source architecture finding; no final-candidate findings can yet be confirmed. No grant bypass was demonstrated. The naming prohibition is prepared as a PTY assertion, but an empty roster cannot qualify unnamed/duplicate/nickname scenarios across roster, chooser, inspector, incidents and task runs.

## Final gate status and parent handoff

| Plan gate | Status now | Exact missing evidence |
| --- | --- | --- |
| Reference inventory | Pending final reconciliation | Map inventory rows to integrated final source and latest lane notes; old inventory statuses are not final results |
| Screen/state comparison | Python-side preparation complete; paired gate blocked | Exact Rust frames at three sizes, full state/theme/NO_COLOR and provider-mode matrix, visual review |
| Keyboard/navigation | Python-side subset passed; Rust gate blocked | Exact installed inspector → Prompts → older → Original → Activity/Review → return, forms/history/copy/export and selection-preserving resize |
| Exact candidate | Blocked | Frozen Rust source SHA, clean source path, executable/bundle path and SHA-256, matching plugin host/adapters, archive if available, builder/test manifest |
| User acceptance | Not requested or established | Explicit approved differences and required mismatch reruns; no source-only acceptance |
| Full Rust suite | Deferred by parent RAM constraint | Parent completion/resource permission, then exact-candidate full suite/counts and relevant integration/broker/attached tests |

Please supply the frozen final candidate and any new integration notes. Keep `docs/validation/rust-ui-parity-acceptance.md` and `.html` reserved for that stage. This prep does **not** establish full parity, release readiness, the Rust default cutover, real agent liveness, or production collection correctness.
