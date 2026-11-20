# Rust Kiro process reader vertical slice

Recorded 20 November 2026. `agenturm-kiro-harness` is an opt-in, separately installed Rust reader for one configured Kiro `messages.jsonl` or `sub-executions/<worker>.jsonl` file. It uses the shared bounded JSONL process runtime, Python-owned policy and durable admission, and the existing content-free plugin fact contract. The built-in Kiro reader remains the default. There is no speed claim.

Kiro JSONL has no session header. The adapter derives a bounded native session or worker ID from the configured path and validates it before emitting facts. Its durable checkpoint binds a hash of that path, last turn, open-turn flag, one queued prompt and one pending interaction. A changed path to the same inode fails closed; a replaced inode or first-line rewrite emits a possible-gap diagnostic and advances the generation. The checkpoint never contains a workspace path or prompt. A first line is both inspected for context and translated as evidence.

## Evidence represented

| Kiro observation | Process fact |
| --- | --- |
| New user prompt, `turn_start`, explicit `turn_end` with `end_turn`, `cancelled` or `error` | Content-free user message and running/completed/interrupted/failed lifecycle with explicit reason. A queued user prompt is held across restart until the next `turn_start`, then assigned to that turn. |
| `assistant` with `operationType: Say` | Content-free assistant message. Other assistant operations produce a coarse diagnostic. |
| Tool call and result with an explicit `success` boolean; explicit policy denial metadata | Correlated opaque operation IDs and coarse started/succeeded/failed/denied tool facts. A result without `success` fails before cursor acknowledgment. |
| One approval or user-input interaction with exact call ID | `paused` while pending, then `running` only after a matching resolution. Coarse diagnostics record that decision details remain unrepresented. A second overlapping interaction fails before acknowledgment rather than inventing readiness. |
| Credit usage summary, subagent announcement or completion, and display error | Coarse diagnostics indicating observed but unrepresented evidence. No prompt, command, path, output, credit amount or permission detail is stored in a process fact. |

The current process usage schema accepts integer token counters, while Kiro reports credit units. The adapter emits `kiro_credit_usage_unrepresented` and leaves token usage absent; it does not turn credits into tokens or zero. The current lifecycle contract lacks native `incomplete`. An unknown `turn_end.stopReason` emits `kiro_unknown_stop_reason` and retains the prior reported state. The UI must treat this as partial coverage, not a successful end. Worker files are individually scoped; their exact parent and outcome require a second file and are not inferred from directory layout alone.

## Validation and remaining gates

The installed receiver test compares explicit Kiro stop status with the native reader, checks content-free message and tool Activity, verifies a real native credit span while the process leaves usage absent, exercises a queued prompt across restart, one permission wait/resolution, worker path identity, result refusal, capture revocation, same-path replacement, symlink retargeting and installer symlink refusal. Rust unit tests cover path binding to the same inode under different session directories, queued-turn and permission restoration, and terminal provenance.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p agenturm-kiro-harness
AGENTURM_RUST_KIRO_BIN=target/debug/agenturm-kiro-harness \
  python -m unittest tests.test_rust_kiro_harness_plugin -v
```

Directory discovery, rules and permissions projection, credit-aware usage, cross-file parent/outcome links, native spans and Review/export parity, wider Kiro version coverage, and edits outside the checked file region remain open. The configured source is a trusted local file path, not a filesystem sandbox. No production Kiro replacement is enabled.
