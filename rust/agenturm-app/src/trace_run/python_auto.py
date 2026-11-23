"""Instrument a target in its own selected Python process."""
import inspect
import os
import sys

_MISSING = (
    "Automatic Python instrumentation needs the Rust-compatible Agenturm SDK and "
    "a supported OpenAI, Anthropic, or LangChain environment. "
    "From this checkout, run the selected Python interpreter with -m pip install '.[auto]'."
)

def _setup_error():
    print(_MISSING, file=sys.stderr)
    sys.exit(2)


try:
    from observatory.auto import SetupError, run_agent
except ImportError:
    _setup_error()

if "managed_launcher" not in inspect.signature(run_agent).parameters:
    _setup_error()

_mode, _capture, _service, _session, _target, *_arguments = sys.argv[1:]
_endpoint = os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"]
if _mode == "module":
    # -P protects the launcher import. The target has ordinary -m lookup.
    sys.path.insert(0, os.getcwd())

try:
    run_agent(
        _target,
        _arguments,
        endpoint=_endpoint,
        service=_service,
        module=_mode == "module",
        capture_content=_capture == "capture",
        transport="otlp",
        session_id=_session,
        managed_launcher=True,
    )
except SetupError:
    _setup_error()
except KeyboardInterrupt:
    sys.exit(130)
