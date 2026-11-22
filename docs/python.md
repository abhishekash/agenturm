# Python agents

## Install the CLI

Python 3.11+ and [uv](https://docs.astral.sh/uv/getting-started/installation/) are required. Install the published CLI:

```bash
uv tool install --force agenturm
agenturm
```

For an existing checkout, run `./scripts/install.sh` (or `scripts/install.ps1` on Windows).

## Observe local Codex and Claude Code sessions

Start `agenturm`. Recent local sessions are discovered automatically on the same machine. The monitor runs in the terminal and does not start agents.

## Automatically instrument a Python SDK agent

Install the automatic capture extra into the same environment as your agent:

```bash
uv pip install --python .venv/bin/python agenturm[auto]
```

Run your script with the separate capture command:

```bash
.venv/bin/python -m observatory run --endpoint http://127.0.0.1:14318 --service research-agent agent.py
```

Replace the service name, Python executable, and script with yours. Use `--module` before the script for a Python module. Script arguments follow the script name. This separate launcher captures supported OpenAI, Anthropic, and LangChain SDK calls. Prompts and responses are excluded unless `--capture-content` is given.

## Add manual spans

Install the client into the agent environment, then use `Client`:

```python
from observatory import Client

telemetry = Client(service="research-agent")
with telemetry.trace(task="Research a question", session_id="thread-42") as run:
    with run.span("search", attributes={"tool.name": "web_search"}):
        result = search_web()
```

See the complete [API and integration examples](usage.md). The optional integrations are installed with `agenturm[langchain]` and `agenturm[openai-agents]`.

For setup examples for Pydantic AI, Google ADK, and any OTLP-compatible agent,
see [Connect agent frameworks](frameworks.md).

## Send Python spans to the Rust receiver

The Rust application can own the receiver and history while the instrumented
agent runs in its existing Python environment. The Python client keeps its
legacy transport by default. Select `transport="otlp"` to send structural
trace evidence to Rust.

```python
from observatory import Client

telemetry = Client(transport="otlp", strict=True)
with telemetry.trace(session_id="thread-42", harness_id="harness.pi") as run:
    with run.span("lookup") as span:
        span.set_status("OK")
```

Run the script through the Rust launcher.

```bash
agenturm run --db ./trace-history.sqlite --source-id otlp.python -- .venv/bin/python agent.py
agenturm query --db ./trace-history.sqlite --harness-id otlp.python
```

To connect an existing producer, start the standalone Rust receiver and supply
its trace endpoint in the producer's environment.

```bash
agenturm serve-traces --db ./trace-history.sqlite --listen 127.0.0.1:14318 --source-id otlp.python
```

```bash
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:14318/v1/traces .venv/bin/python agent.py
```

The client accepts a literal loopback HTTP endpoint. It checks the explicit
`endpoint` first, followed by `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`,
`OTEL_EXPORTER_OTLP_ENDPOINT`, and the loopback default on port 14318. Supply
the exact `session_id` for a standalone receiver. The Rust launcher can assign
a session to a run without one. To link to native history, give the exact
`harness_id` and `session_id`. Add `turn_id` to identify a specific native
turn and `worker_id` when the record has a worker identity.

The transport sends IDs, parent relationships, timing, status, and explicit
native linkage. It bounds each request to 1,024 spans and 4 MiB. Delivery
failures, including partial rejections, are available in `Client.last_error`.
With `strict=True`, a delivery error is raised when the application completes
successfully. An application exception still propagates. Progress messages
belong to the legacy transport.

The OpenAI Agents and LangChain wrappers can use the same client. Set the
explicit scope on `Client` for the OpenAI Agents exporter or on the active
`trace` for LangChain. The OpenAI wrapper retains its deterministic mapping
of prefixed SDK IDs, and LangChain retains the parent relationships from its
callback IDs.

To instrument an existing script automatically, use the Python launcher
inside the Rust launcher with the `auto` extra in the producer environment.

```bash
agenturm run --db ./trace-history.sqlite --source-id otlp.python -- \
  .venv/bin/python -m observatory run --transport otlp \
  --session-id thread-42 --harness-id harness.pi agent.py
```

The automatic OTLP path keeps the installed instrumentors and exports their
structural span evidence. It uses the Rust trace endpoint directly. The
`--capture-content` option applies to the legacy transport.
