"""Bounded metadata-only OTLP/HTTP export to a literal loopback endpoint."""
from __future__ import annotations

import http.client
import ipaddress
import json
import math
import os
import re
from dataclasses import dataclass
from urllib.parse import urlsplit

MAX_EXPORT_SPANS = 1024
MAX_EXPORT_BYTES = 4 * 1024 * 1024
MAX_REPLY_BYTES = 4096
_PREFIX = b'{"resourceSpans":[{"scopeSpans":[{"scope":{"name":"agenturm.python"},"spans":['
_SUFFIX = b']}]}]}'
_STATUS = {"UNSET": 0, "OK": 1, "ERROR": 2}
_SESSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,511}")
_EXECUTION = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,511}")
_HARNESS = re.compile(r"[a-z][a-z0-9._/-]{0,95}")


@dataclass(frozen=True)
class TraceScope:
    """Explicit native linkage applied to every span in one export."""

    session_id: str | None = None
    harness_id: str | None = None
    turn_id: str | None = None
    worker_id: str | None = None

    def __post_init__(self):
        for label, value, pattern in (
            ("session", self.session_id, _SESSION),
            ("harness", self.harness_id, _HARNESS),
            ("turn", self.turn_id, _EXECUTION),
            ("worker", self.worker_id, _EXECUTION),
        ):
            if value is not None and (not isinstance(value, str) or not pattern.fullmatch(value)):
                raise ValueError(f"OTLP {label} identity must be an exact native identifier")
        if self.harness_id is not None and (
            self.session_id is None or self.harness_id.startswith("otlp.")
        ):
            raise ValueError("OTLP harness linkage requires an explicit session and native harness")
        if self.turn_id is not None and self.harness_id is None:
            raise ValueError("OTLP turn linkage requires an explicit harness and session")
        if self.worker_id is not None and self.turn_id is None:
            raise ValueError("OTLP worker linkage requires an explicit turn")

    def attributes(self):
        return [
            {"key": key, "value": {"stringValue": value}}
            for key, value in (
                ("session.id", self.session_id),
                ("agenturm.source.id", self.harness_id),
                ("agenturm.turn.id", self.turn_id),
                ("agenturm.worker.id", self.worker_id),
            )
            if value is not None
        ]


def _trace_endpoint(endpoint):
    raw = endpoint
    if raw is None:
        raw = (os.environ.get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
               or os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
               or "http://127.0.0.1:14318")
    try:
        if not isinstance(raw, str) or not raw or any(ch.isspace() or ord(ch) < 32 for ch in raw):
            raise ValueError
        url = urlsplit(raw)
        host = ipaddress.ip_address(url.hostname or "")
        port = url.port if url.port is not None else 80
        authority = (r"\[[0-9a-fA-F:]+\](?::[0-9]+)?" if host.version == 6
                     else r"[0-9.]+(?::[0-9]+)?")
        if (
            url.scheme != "http" or not host.is_loopback or not re.fullmatch(authority, url.netloc)
            or url.username is not None or url.password is not None
            or "?" in raw or "#" in raw or not 1 <= port <= 65535
            or url.path not in {"", "/", "/v1/traces"}
        ):
            raise ValueError
    except ValueError:
        raise ValueError("OTLP endpoint must be a literal loopback HTTP trace receiver") from None
    address = f"[{host}]" if host.version == 6 else str(host)
    return str(host), port, f"http://{address}:{port}/v1/traces"


def _span_id(value, width):
    if (
        not isinstance(value, str) or len(value) != width
        or any(ch not in "0123456789abcdefABCDEF" for ch in value)
        or not any(ch != "0" for ch in value)
    ):
        raise ValueError("Invalid OTLP trace or span identity")
    return value.lower()


def _project(span, attributes):
    start = span["start_unix_nano"]
    end = span["end_unix_nano"]
    if type(start) is not int or type(end) is not int or not 0 < start <= end <= (1 << 63) - 1:
        raise ValueError("Invalid OTLP span timing")
    status = _STATUS.get(span["status"])
    if status is None:
        raise ValueError("Invalid OTLP span status")
    result = {
        "traceId": _span_id(span["trace_id"], 32),
        "spanId": _span_id(span["span_id"], 16),
        "name": "agenturm.span",
        "startTimeUnixNano": str(start),
        "endTimeUnixNano": str(end),
        "status": {"code": status},
        "attributes": attributes,
    }
    if span.get("parent_span_id") is not None:
        parent = _span_id(span["parent_span_id"], 16)
        if parent == result["spanId"]:
            raise ValueError("Invalid OTLP parent span identity")
        result["parentSpanId"] = parent
    return json.dumps(result, separators=(",", ":")).encode("ascii")


def _check_reply(response):
    if not 200 <= response.status < 300:
        raise OSError(f"OTLP receiver returned HTTP {response.status}")
    lengths = response.headers.get_all("Content-Length", [])
    if (
        len(lengths) != 1 or not re.fullmatch(r"[0-9]{1,4}", lengths[0])
        or int(lengths[0]) > MAX_REPLY_BYTES or response.chunked
    ):
        raise OSError("OTLP receiver returned an unbounded reply")
    length = int(lengths[0])
    try:
        body = response.read(length)
        if len(body) != length:
            raise ValueError
        reply = json.loads(body)
        if not isinstance(reply, dict):
            raise ValueError
        partial = reply.get("partialSuccess", {})
        if not isinstance(partial, dict):
            raise ValueError
        rejected = partial.get("rejectedSpans", 0)
        if isinstance(rejected, str) and re.fullmatch(r"[0-9]{1,20}", rejected):
            rejected = int(rejected)
        if type(rejected) is not int or rejected < 0:
            raise ValueError
    except (ValueError, RecursionError, http.client.HTTPException):
        raise OSError("OTLP receiver returned an invalid reply") from None
    if rejected:
        raise OSError("OTLP receiver rejected one or more spans")


class OtlpTransport:
    """Send structural span evidence without inherited proxies or redirects."""

    def __init__(self, endpoint, timeout):
        self.host, self.port, self.endpoint = _trace_endpoint(endpoint)
        try:
            self.timeout = float(timeout)
        except (ValueError, TypeError, OverflowError):
            raise ValueError("OTLP timeout must be between 0 and 30 seconds") from None
        if not math.isfinite(self.timeout) or not 0 < self.timeout <= 30:
            raise ValueError("OTLP timeout must be between 0 and 30 seconds")

    def export(self, spans, scope):
        attributes = scope.attributes()
        batch = []
        overhead = len(_PREFIX) + len(_SUFFIX)
        size = overhead
        for span in spans:
            item = _project(span, attributes)
            if overhead + len(item) > MAX_EXPORT_BYTES:
                raise ValueError("OTLP span exceeds the request byte limit")
            if batch and (len(batch) == MAX_EXPORT_SPANS or size + len(item) + 1 > MAX_EXPORT_BYTES):
                self._send(batch)
                batch = []
                size = overhead
            size += len(item) + bool(batch)
            batch.append(item)
        if batch:
            self._send(batch)

    def _send(self, batch):
        body = _PREFIX + b",".join(batch) + _SUFFIX
        connection = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
        try:
            connection.request(
                "POST", "/v1/traces", body=body,
                headers={"Content-Type": "application/json", "Connection": "close"},
            )
            with connection.getresponse() as response:
                _check_reply(response)
        except http.client.HTTPException:
            raise OSError("OTLP receiver returned an invalid HTTP response") from None
        finally:
            connection.close()
