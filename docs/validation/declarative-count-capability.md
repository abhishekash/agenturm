# Declarative count capability

Recorded 20 November 2026. An explicitly installed third capability process now contributes a **Count observations** pane. The existing Prompt signals and Action observations configurations and wire payloads remain valid. This is a bounded count profile, not arbitrary calculations, a prompt efficacy score, or a general third-party SDK.

## Activate

```sh
python examples/count_observations_process/install.py --destination /absolute/private/counts
agenturm cockpit --capability-config /absolute/private/counts/capability.json
```

## Rust executable example

The Rust alternative installs an already-built executable; the installer does not run Cargo. The Python installer above remains unchanged.

```sh
cargo build --locked --bin agenturm-count-observations
python examples/count_observations_process/install_rust.py \
  --binary "$PWD/target/debug/agenturm-count-observations" \
  --destination /absolute/private/counts-rust
agenturm cockpit --capability-config /absolute/private/counts-rust/capability.json
```

The Rust installer rejects symlinked source or destination paths and existing capability files, copies the binary, and pins the copied bytes in the manifest. Activation remains explicit. This demonstrates an extension capability process; it does not migrate harnesses or declare a stable ABI.

The inert schema-v3 descriptor declares at most six exact `(domain, action)` selectors and at most six count or ratio rows. Selector keys are unique; domains and actions are bounded identifier tokens, not SQL or regular expressions. Labels use up to 14 single-cell ASCII characters so a ratio label fits the required two-column compact pane. The manifest pins one executable SHA-256 and asks only for `scoped-aggregate-read`. Startup remains explicit. The third process uses the existing `PaneRegistry` and Textual table; no central UI dispatch or layout branch was added.

For each selected service, session and canonical agent, the host requests at most 200 Activity events through the existing capture-scoped receiver endpoint. It counts only exact domain/action matches. A zero ratio denominator renders **unavailable**. The count describes the returned page; even a page without a next cursor can have incomplete source evidence. The pane says “page complete; source may be partial” or “recent page partial,” and labels all figures as observations.

## Publication fence and trust boundary

An exact capture-scoped Activity response now includes an opaque `capture_revision`, derived from the receiver ID, receiver process and collector instances, approved evidence sources and the durable capture-policy token. Ordinary Activity responses do not include it. The server still rechecks the source policy and SQLite data version before sending the response. A schema-v3 pane refuses a receiver without this revision.

The capability worker reads an exact page, computes typed counts and a host-only signature over selected event identities, timestamps, domain/action values and pagination cursor, then invokes the child. It makes a second capture-scoped read after the child responds and compares the policy revision and page signature before publishing. A policy revoke/re-enable or a change to the selected page during computation discards the output. The child receives only state, observed-event count, named counts and page-completeness flag. It receives no prompt text, tool output, paths, source IDs, event IDs, session IDs, agent IDs, revision token or signature. Every returned label, value, row order, summary and coverage phrase must exactly match the host’s computation.

This fence covers computation and publication checks, not indefinite display. A backend policy change after publication can leave a cached aggregate visible until the existing watcher result or approval expiry and next pane query. That approval window is 250 ms. Source evidence can itself be partial, and these count relationships do not establish prompt quality or causality.

## UI and validation

The full-screen cockpit keeps one selected pane at 60×18 and 80×24. The new pane adds no border or global status marker. Its two columns omit a repeated per-row coverage cell; the summary states coverage once. The same keyboard page navigation reaches it. No UI disk, HTTP or child I/O runs on the Textual event thread.

The installed fixture and receiver tests cover schema-v1/v2 coexistence, a third explicit pane, strict selectors/formulas, zero denominator, partial page, forged child text, missing revision denial, page change, revoke/re-enable generation change, source scope, and 60×18/80×24 rendering. The combined focused suite passes 43 tests. One pre-existing cockpit test fixture omitted reader `coverage` and `label` fields; this change completes that synthetic fixture so the impacted suite is green.
