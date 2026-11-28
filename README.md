# Agenturm

<p align="center">
  <strong>Inspect your agents’ work. Run by run.</strong><br />
  A local-first terminal cockpit for the work between a prompt and a result.
</p>

<p align="center">
  <a href="https://github.com/abhishekash/agenturm/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/abhishekash/agenturm/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/abhishekash/agenturm/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/abhishekash/agenturm"></a>
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
</p>

Agenturm brings supported coding-agent sessions and OpenTelemetry data into one keyboard-driven view. See which sessions need attention, follow the work recorded for a run, and inspect the evidence behind a reported result—without losing sight of what the capture does *not* establish.

**Agenturm is an observability cockpit, not an agent orchestrator.** It does not steer conversations, approve actions, or rerun tests. Keep using each agent in its own terminal; open Agenturm alongside it. For telemetry collection, `agenturm run` can wrap a program to configure its OTLP endpoint, but it does not inject instrumentation or change the program’s logic.

<p align="center">
  <img src="https://raw.githubusercontent.com/abhishekash/abhishekash/main/assets/agenturm-demo.gif" alt="Agenturm terminal walkthrough using synthetic agent-session records" width="960" />
</p>

<p align="center"><sub>Real terminal UI · synthetic records · no visitor or local machine data in the demo</sub></p>

<p align="center"><a href="https://www.agenturm.dev/#demo">Explore the interactive walkthrough</a></p>

## Why Agenturm?

An agent’s “done” is a useful signal—not a substitute for understanding what happened. Agenturm keeps the recorded work close to the decision you need to make next.

- **Find the session that needs you.** Scan multiple agents and, when the source records the relationship, follow delegated workers without blending their evidence together.
- **Recover the request and the story.** Move from a session to its captured prompts, activity, tools, errors, and progress.
- **Review facts, not just summaries.** Compare reported outcomes with recorded command results and later edits. A passing check followed by another edit is still something to review.
- **Pick up work deliberately.** Keep a local task brief, decisions, rationale, and links to the runs that informed them.
- **Bring telemetry into the same picture.** Receive OpenTelemetry traces, logs, and metrics from instrumented processes through OTLP/HTTP.
- **Stay in your terminal.** Navigate and inspect with the keyboard; press **F1** in the cockpit for the current keymap.

Evidence is source-backed and bounded. Runtime support, capture permissions, and available history affect what Agenturm can show; missing evidence is not treated as proof that an event did not happen.

## Get started

### Install

**macOS (Apple Silicon) or Linux:**

```sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
```

Or install with Homebrew:

```sh
brew install abhishekash/tap/agenturm
```

### Open the cockpit

```sh
agenturm
```

Keep your coding agents running. Agenturm discovers supported local session records and opens the cockpit in your terminal. It does not create demo sessions or require an account or API key for local inspection.

**Supported release platforms:** Apple Silicon macOS 13 or newer; Linux x86-64 or ARM64 with glibc 2.39 or newer. Intel Macs, Rosetta, and native Windows are not currently supported.

See [installation and upgrade details](docs/install.md) if the command is not on your `PATH` or you need a custom install location.

## What it can inspect

| Source | How it connects |
| --- | --- |
| Codex, Claude Code, OpenCode, and Pi | Discovers supported local session records. OpenCode’s local database is read-only. |
| OpenTelemetry-instrumented processes | Sends traces, logs, and metrics to Agenturm’s local OTLP/HTTP receiver. |

The available detail depends on each runtime, its configuration, and the retained records. For process telemetry, the process must already use an OpenTelemetry SDK and exporter; Agenturm does not inject instrumentation. See [usage](docs/usage.md) and [receiver setup](docs/receiver.md).

## Bring telemetry from a process

Run an already-instrumented program through Agenturm’s local receiver:

```sh
agenturm run \
  --db ./history.sqlite \
  --source-id otlp.example \
  -- ./your-agent
```

Agenturm configures the standard OTLP/HTTP environment variables for the selected process and stores received data in SQLite. The program still needs its own OpenTelemetry SDK and OTLP exporter. To connect an existing process to a receiver you manage, see the [receiver guide](docs/receiver.md).

## Inspect and hand off

The terminal cockpit is the starting point. For automation, exports, and explicit scopes, the CLI can produce a review from stored evidence without running the agent or its checks:

```sh
agenturm review \
  --db ./history.sqlite \
  --harness-id <source-id> \
  --session-id <session-id> \
  --turn-id <turn-id> \
  --format markdown
```

Reviews can distinguish a producer’s completion message from recorded command results, show later edits, and disclose incomplete coverage. They describe captured evidence; they do not certify the current worktree or independently attest that a command ran. See the [changelog](CHANGELOG.md) for recent capabilities and [release notes](docs/releases/) for versioned changes.

## Local by default

Session records and telemetry can contain prompts, commands, file paths, tool inputs, and results. Treat the local database and exports as sensitive project data.

- The normal workflow stores data in local SQLite; the receiver binds to loopback by default.
- Local inspection does not require an Agenturm account or API key.
- Linux live prompt history is enabled by default for unconfigured sources; explicit revocations are respected and tool payload permissions stay separate. [Access and opt-out](docs/default-prompt-history.html).
- Content access and exports have explicit boundaries. Retention is logical deletion, not secure erasure.

Read [privacy and capture boundaries](docs/privacy.md) before sharing a database or review export. For network access, follow the receiver guide’s authentication and TLS requirements rather than exposing the local receiver directly.

## Development

Agenturm is a Rust workspace. Install the toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml), then run from the repository root:

```sh
cargo run -p agenturm-app
```

Before submitting a change, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Crates are grouped by responsibility under `crates/`—applications, core contracts, domain, plugins, storage, and test support. Read the [workspace and plugin architecture guide](docs/rust-workspace-architecture.md) before adding a crate, and see [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance.

## Documentation

- [Install and upgrade](docs/install.md)
- [Usage](docs/usage.md)
- [Privacy and capture boundaries](docs/privacy.md)
- [Receiver, OTLP, migration, and retention](docs/receiver.md)
- [Workspace and plugin architecture](docs/rust-workspace-architecture.md)
- [Changelog](CHANGELOG.md) · [Releases](https://github.com/abhishekash/agenturm/releases)

## License

Agenturm is released under the [MIT License](LICENSE).
