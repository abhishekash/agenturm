"""Small, dependency-free client for instrumenting Python agent code."""
from __future__ import annotations
from .http_transport import urlopen as receiver_urlopen

import asyncio
import contextvars
import json
import secrets
import time
import urllib.request
from typing import Any

from .otlp_transport import OtlpTransport, TraceScope

_ACTIVE_RUN: contextvars.ContextVar["Run | None"] = contextvars.ContextVar("observatory_run", default=None)
_ACTIVE_SPAN: contextvars.ContextVar["Span | None"] = contextvars.ContextVar("observatory_span", default=None)
_AUTO_PARENT = object()
MAX_REQUEST_BYTES = 8 * 1024 * 1024


def _random_hex(byte_count):
    value = secrets.token_hex(byte_count)
    while not any(char != "0" for char in value):
        value = secrets.token_hex(byte_count)
    return value


class Client:
    """Emit Python spans to the selected receiver transport."""

    def __init__(self, endpoint=None, service="python-agent",
                 token=None, timeout=3.0, strict=False, capture_exception_message=False,
                 resource_attributes=None, *, transport="agenturm", session_id=None,
                 harness_id=None, turn_id=None, worker_id=None):
        if transport not in {"agenturm", "otlp"}:
            raise ValueError("Unknown trace transport")
        if transport == "agenturm" and any(
            value is not None for value in (session_id, harness_id, turn_id, worker_id)
        ):
            raise ValueError("Client-level native linkage requires OTLP transport")
        self.transport = transport
        self.service = str(service)
        self._configuration_error = None
        self._otlp = None
        self._trace_scope = TraceScope()
        self.token = None
        self.strict = bool(strict)
        self.capture_exception_message = bool(capture_exception_message) and transport == "agenturm"
        self.resource_attributes = dict(resource_attributes or {})
        if transport == "agenturm":
            endpoint = "http://127.0.0.1:4318" if endpoint is None else endpoint
            self.endpoint = endpoint.rstrip("/")
            self.timeout = float(timeout)
        else:
            self.endpoint = ""
            self.timeout = None
        try:
            if transport == "otlp":
                if token is not None:
                    raise ValueError("The Rust OTLP transport accepts no receiver token")
                self._trace_scope = TraceScope(session_id, harness_id, turn_id, worker_id)
                self._otlp = OtlpTransport(endpoint, timeout)
                self.endpoint = self._otlp.endpoint
                self.timeout = self._otlp.timeout
            else:
                from .receiver_state import endpoint_token
                self.token = endpoint_token(endpoint, token)
        except (ValueError, OSError) as exc:
            if self.strict:
                raise
            self._configuration_error = exc
        self.last_error: Exception | None = self._configuration_error

    def trace(self, name="agent.run", *, task=None, session_id=None, attributes=None,
              harness_id=None, turn_id=None, worker_id=None):
        scope = None
        if self.transport == "otlp":
            scope = self._trace_scope
            if any(value is not None for value in (session_id, harness_id, turn_id, worker_id)):
                scope = TraceScope(session_id, harness_id, turn_id, worker_id)
            session_id = scope.session_id
        elif any(value is not None for value in (harness_id, turn_id, worker_id)):
            raise ValueError("Native harness, turn, and worker linkage requires OTLP transport")
        return Run(self, name, task, session_id, attributes or {}, scope)

    def _post_spans(self, spans):
        if self.transport == "otlp":
            return self._post_otlp_spans(spans, self._trace_scope)
        base = {"service": self.service, "spans": []}
        base_size = len(json.dumps(base, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))
        batch = []
        batch_size = base_size
        successful = True
        last_error = None

        def send():
            nonlocal successful, last_error, batch, batch_size
            if not batch:
                return
            if not self._post("/api/spans", {"service": self.service, "spans": batch}):
                successful = False
                last_error = self.last_error
            batch = []
            batch_size = base_size

        for span in spans:
            try:
                item_size = len(json.dumps(span, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))
            except Exception as exc:
                self.last_error = exc
                if self.strict:
                    raise
                successful = False
                last_error = exc
                continue
            if base_size + item_size > MAX_REQUEST_BYTES:
                exc = ValueError("a span exceeds the receiver's 8 MiB request limit")
                self.last_error = exc
                if self.strict:
                    raise exc
                successful = False
                last_error = exc
                continue
            separator = 1 if batch else 0
            if batch and batch_size + separator + item_size > MAX_REQUEST_BYTES:
                send()
                separator = 0
            batch.append(span)
            batch_size += separator + item_size
        send()
        if not successful:
            self.last_error = last_error
        return successful

    def _post_otlp_spans(self, spans, scope):
        try:
            if self._configuration_error is not None:
                raise self._configuration_error
            self._otlp.export(spans, scope, strict=self.strict)
            self.last_error = None
            return True
        except (OSError, ValueError, KeyError, TypeError) as exc:
            return self._failure(exc)

    def _failure(self, exc):
        self.last_error = exc
        if self.strict:
            raise exc
        return False

    def _post(self, path, payload):
        if self.transport == "otlp":
            return self._failure(ValueError("OTLP transport accepts only trace spans"))
        try:
            if self._configuration_error is not None:raise self._configuration_error
            data = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
            headers = {"Content-Type": "application/json"}
            if self.token:
                headers["Authorization"] = f"Bearer {self.token}"
            request = urllib.request.Request(self.endpoint + path, data=data,
                                             headers=headers, method="POST")
            with receiver_urlopen(request, timeout=self.timeout) as response:
                if not 200 <= response.status < 300:
                    raise OSError(f"Goechala returned HTTP {response.status}")
            return True
        except Exception as exc:
            self.last_error = exc
            if self.strict:
                raise
            return False


class Run:
    def __init__(self, client, name, task, session_id, attributes, otlp_scope=None):
        self.client = client
        self.trace_id = _random_hex(16)
        self.session_id = str(session_id).strip() if session_id is not None else None
        if self.session_id is not None and (not self.session_id or len(self.session_id) > 512):
            raise ValueError("session_id must contain 1 to 512 characters")
        self.spans: list[Span] = []
        root_attributes = dict(attributes)
        if task is not None:
            root_attributes.setdefault("agent.task", str(task))
        if self.session_id:
            root_attributes.setdefault("session.id", self.session_id)
        self.root = Span(self, name, root_attributes, parent_span_id=None)
        self.spans.append(self.root)
        self._run_token = None
        self._closed = False
        self._otlp_scope = otlp_scope

    def __enter__(self):
        if self._closed or self._run_token is not None:
            raise RuntimeError("run cannot be entered more than once")
        self._run_token = _ACTIVE_RUN.set(self)
        self.root.__enter__()
        return self

    def __exit__(self, exc_type, exc, tb):
        self._finish(exc_type, exc, tb)
        self._flush_after(exc)
        return False

    async def __aenter__(self):
        return self.__enter__()

    async def __aexit__(self, exc_type, exc, tb):
        self._finish(exc_type, exc, tb)
        await asyncio.to_thread(self._flush_after, exc)
        return False

    def _finish(self, exc_type, exc, tb):
        if self._closed:
            return
        self.root.__exit__(exc_type, exc, tb)
        if self._run_token is not None:
            _ACTIVE_RUN.reset(self._run_token)
            self._run_token = None
        self._closed = True

    def span(self, name, *, attributes=None, parent_span_id=_AUTO_PARENT):
        if self._closed:
            raise RuntimeError("run is already closed")
        if parent_span_id is _AUTO_PARENT:
            active = _ACTIVE_SPAN.get()
            parent_span_id = active.span_id if active is not None and active.run is self else self.root.span_id
        span = Span(self, name, attributes or {}, parent_span_id=parent_span_id)
        self.spans.append(span)
        return span

    def progress(self, stage, message="", percent=None):
        if self._closed:
            raise RuntimeError("run is already closed")
        if self._otlp_scope is not None:
            return self.client._failure(ValueError("OTLP transport does not export progress messages"))
        payload = {"trace_id": self.trace_id, "stage": str(stage), "message": str(message), "service": self.client.service}
        if percent is not None:
            payload["percent"] = percent
        if self.session_id:
            payload["session_id"] = self.session_id
        return self.client._post("/api/progress", payload)

    async def aprogress(self, stage, message="", percent=None):
        return await asyncio.to_thread(self.progress, stage, message, percent)

    def _flush_after(self, application_error):
        try:
            return self._flush()
        except Exception:
            if application_error is None or self._otlp_scope is None:
                raise

    def _flush(self):
        metadata = (
            {"trace_id": self.trace_id, "span_id": span.span_id, "parent_span_id": span.parent_span_id,
             "start_unix_nano": span.start_ns, "end_unix_nano": span.end_ns, "status": span.status}
            for span in self.spans
        )
        if self._otlp_scope is not None:
            return self.client._post_otlp_spans(metadata, self._otlp_scope)
        return self.client._post_spans([
            {**row, "name": span.name, "attributes": span.attributes, "events": span.events,
             "resource_attributes": self.client.resource_attributes}
            for row, span in zip(metadata, self.spans)
        ])


class Span:
    def __init__(self, run, name, attributes, parent_span_id):
        self.run = run
        self.name = str(name)
        self.span_id = _random_hex(8)
        self.parent_span_id = parent_span_id
        self.start_ns = time.time_ns()
        self.end_ns = self.start_ns
        self.status = "UNSET"
        self.attributes: dict[str, Any] = dict(attributes)
        self.events: list[dict[str, Any]] = []
        self._context_token = None
        self._finished = False

    def __enter__(self):
        if self._context_token is not None or self._finished:
            raise RuntimeError("span cannot be entered more than once")
        self._context_token = _ACTIVE_SPAN.set(self)
        return self

    def __exit__(self, exc_type, exc, tb):
        self.finish(exc)
        if self._context_token is not None:
            _ACTIVE_SPAN.reset(self._context_token)
            self._context_token = None
        return False

    def finish(self, error=None):
        if self._finished:
            return self
        if error is not None:
            self.status = "ERROR"
            self.attributes.setdefault("exception.type", type(error).__name__)
            exception = {"type": type(error).__name__}
            if self.run.client.capture_exception_message:
                message = str(error)
                self.attributes.setdefault("exception.message", message)
                exception["message"] = message
            self.add_event("exception", exception)
        self.end_ns = max(time.time_ns(), self.start_ns)
        self._finished = True
        return self

    async def __aenter__(self):
        return self.__enter__()

    async def __aexit__(self, exc_type, exc, tb):
        return self.__exit__(exc_type, exc, tb)

    def set_attribute(self, key, value):
        self.attributes[str(key)] = value
        return self

    def add_event(self, name, attributes=None):
        self.events.append({"name": str(name), "time_unix_nano": time.time_ns(),
                            "attributes": dict(attributes or {})})
        return self

    def set_status(self, status):
        if status not in {"UNSET", "OK", "ERROR"}:
            raise ValueError("status must be UNSET, OK, or ERROR")
        self.status = status
        return self
