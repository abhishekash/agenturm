# Rust Kiro process reader vertical slice

Recorded 20 November 2026. `agenturm-kiro-harness` is an opt-in, separately installed Rust reader for one configured Kiro `messages.jsonl` or `sub-executions/<worker>.jsonl` file. It uses the shared bounded JSONL process runtime, Python-owned policy and durable admission, and the existing content-free plugin fact contract. The built-in Kiro reader remains the default. There is no speed claim.

Kiro JSONL has no session header. The adapter derives a bounded native session or worker ID from the configured path and validates it before emitting facts. Its durable checkpoint binds a hash of that path, last turn, open-turn flag, one queued prompt, up to two pending interactions keyed by call ID and consent round, and one unresolved tool call with its status and category. A changed path to the same inode fails closed; a replaced inode or first-line rewrite emits a possible-gap diagnostic and advances the generation. The checkpoint never contains a workspace path, prompt, or tool argument. A first line is both inspected for context and translated as evidence.

## Evidence represented

| Kiro observation | Process fact |
| --- | --- |
| New user prompt, `turn_start`, explicit `turn_end` with `end_turn`, `cancelled` or `error` | Content-free user message and running/completed/interrupted/failed lifecycle with explicit reason. A queued user prompt is held across restart until the next `turn_start`, then assigned to that turn. |
| `assistant` with `operationType: Say` | Content-free assistant message. Other assistant operations produce a coarse diagnostic. |
| Tool call and result with an explicit `success` boolean or a call status of `denied`/`failed`; explicit policy denial metadata | Correlated opaque operation IDs and coarse started/succeeded/failed/denied tool facts. A call's denial or failure takes precedence over a result's `success: true`. A result without `success` or a terminal call status fails before cursor acknowledgment. |
| Up to two approval or user-input interactions keyed by exact call ID and `consentRound` | `paused` while any request remains pending, then `running` only after all matching resolutions. Coarse diagnostics record that decision details remain unrepresented. A third overlapping interaction fails before acknowledgment rather than inventing readiness. |
| Credit usage summary, subagent announcement or completion, and display error | Coarse diagnostics indicating observed but unrepresented evidence. No prompt, command, path, output, credit amount or permission detail is stored in a process fact. |

The current process usage schema accepts integer token counters, while Kiro reports credit units. The adapter emits `kiro_credit_usage_unrepresented` and leaves token usage absent; it does not turn credits into tokens or zero. The current lifecycle contract lacks native `incomplete`. An unknown `turn_end.stopReason` emits `kiro_unknown_stop_reason` and retains the prior reported state. The UI must treat this as partial coverage, not a successful end. Worker files are individually scoped; their exact parent and outcome require a second file and are not inferred from directory layout alone. The bounded cursor retains one unresolved call. A second overlapping call or a new turn while a call remains unresolved fails before acknowledgment; this reader does not claim parity on those streams. The same bounded cursor retains at most two simultaneous interactions and rejects a third before acknowledgment.

## Validation and remaining gates

The installed receiver test compares explicit Kiro stop status with the native reader, checks content-free message and tool Activity, verifies a real native credit span while the process leaves usage absent, exercises a queued prompt across restart, consent rounds across restart without false readiness, denied/failed call status against native results, late-result turn attribution with a fail-closed process boundary, worker path identity, result refusal, capture revocation, same-path replacement, symlink retargeting and installer symlink refusal. Rust unit tests cover path binding to the same inode under different session directories, queued-turn and permission restoration, call status restoration, turn boundary refusal, and terminal provenance.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p agenturm-kiro-harness
AGENTURM_RUST_KIRO_BIN=target/debug/agenturm-kiro-harness \
  python -m unittest tests.test_rust_kiro_harness_plugin -v
```

Directory discovery, rules and permissions projection, credit-aware usage, cross-file parent/outcome links, native spans and Review/export parity, wider Kiro version coverage, and edits outside the checked file region remain open. The configured source is a trusted local file path, not a filesystem sandbox. No production Kiro replacement is enabled.
