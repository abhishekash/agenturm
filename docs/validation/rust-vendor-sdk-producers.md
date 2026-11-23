# Rust OTLP vendor producer qualification

The fixture runs real SDK and framework code with loopback model responses or each SDK's offline model interface. The producer exits immediately after its workflow and flush. The test reopens Rust history and compares every retained trace ID, span ID, parent ID, timestamp and status with the SDK or callback observation. It also checks the exact native source, session, turn and worker, exit status, and exclusion of fixture payloads and credentials.

| Producer | Exercised path | Qualification |
| --- | --- | --- |
| OpenAI Python | OpenInference automatic capture of synchronous, streamed and rejected SDK requests | Passed locally with OpenAI 2.53.0 and OpenInference OpenAI 0.1.65 |
| LangChain | OpenInference automatic capture of a runnable sequence and the OpenAI SDK call it makes | Passed locally with LangChain Core 1.5.3, LangChain OpenAI 1.4.2 and OpenInference LangChain 0.1.79 |
| LangGraph | Agenturm callback handler on an actual state graph that invokes the model | Passed locally with LangGraph 1.2.10 |
| Anthropic Python | Automatic capture around an actual Messages SDK call | Passed in Automatic SDK capture through CLI |
| OpenAI Agents SDK | Agent and tool loop with the SDK's scripted model, processor and normalized span IDs | Passed in OpenAI Agents SDK local tracing |
| Pydantic AI | Native agent, model and tool instrumentation with the SDK's offline model | Passed in Pydantic AI native instrumentation |
| Google ADK | Native agent, model and tool instrumentation with the SDK's offline model | Passed in Google ADK OTLP interoperability |

Local runs used Python 3.12, OpenTelemetry SDK 1.45.1 and the Rust 0.3.4 Linux x86-64 bundle. Its archive SHA-256 is `74f85b9d0b1f0c9c2afc9dbcc9e97119087777b03deed1203a0072e28878a18c`. The three locally available SDKs also passed authenticated broker launch, with a separate Linux reader reopening their retained traces.

All seven cases passed their mandatory steps in [CI run 45087820065](https://github.com/abhishekash/agenturm/actions/runs/45087820065) at `603d71ec44be82500599d9711dd85a0419131f98`. Each SDK job received the Rust app built from that same revision and ran its selected cases with skips disabled. The broader CI run still had independent Python suite jobs in progress when this qualification was recorded.

The verified transport carries structural span evidence. Provider payloads, span links across traces, progress, usage and recorded cost have separate coverage requirements. Parent checks reflect recorded ancestry within each SDK trace.

Run with the optional SDKs already installed in the producer environment and a standalone Rust binary.

```sh
AGENTURM_RUST_APP_BIN=/path/to/agenturm \
  python -m unittest tests.test_rust_vendor_sdk_e2e -v
```

To use the broker, set `AGENTURM_RUST_BROKER_CONFIG` to a collector's configuration for a fresh test history. Allow the `otlp.<vendor>` sources under test and `harness.codex`. The same fixtures launch via `broker run` and verify the result via `broker query`. [Native broker commands](../rfcs/rust-broker-commands.md)
