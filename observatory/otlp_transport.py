"""Bounded OTLP/HTTP export to a literal loopback endpoint."""
from __future__ import annotations

import http.client
import ipaddress
import json
import math
import os
import re
import socket
import time
from collections.abc import Mapping
from dataclasses import dataclass
from urllib.parse import unquote, urlsplit

MAX_EXPORT_SPANS = 1024
MAX_EXPORT_BYTES = 4 * 1024 * 1024
MAX_REPLY_BYTES = 4096
MAX_PAYLOAD_TEXT_BYTES = 2048
MAX_PAYLOAD_BATCH_BYTES = 64 * 1024
_PROGRESS_PATH = "/native/v1/progress"
_PROGRESS_STAGE = re.compile(r"[A-Za-z][A-Za-z0-9._-]{0,63}")
_PROGRESS_CREDENTIAL = re.compile(
    r"(?:^|[._-])(?:sk-|[sr]k_(?:live|test)_|gh[pousr]_|github_pat_|akia|aiza"
    r"|xox[baprs]-|(?:bearer|authorization|password|api_key|secret)[_-])",
    re.IGNORECASE,
)
PAYLOAD_KEYS = ("agenturm.payload.input", "agenturm.payload.output")
_CREDENTIAL_MARKERS = re.compile(
    r"private key|sk-|[sr]k_(?:live|test)_|gh[pousr]_|github_pat_|akia|aiza|xox[baprs]-|\bbearer\b",
    re.IGNORECASE,
)
_CREDENTIAL_FIELD = re.compile(
    r"authorization|password|passwd|token|secret|credential|api[_-]?key",
    re.IGNORECASE,
)

_PREFIX = b'{"resourceSpans":[{"scopeSpans":[{"scope":{"name":"agenturm.python"},"spans":['
_SUFFIX = b']}]}]}'
_STATUS = {"UNSET": 0, "OK": 1, "ERROR": 2}
_SESSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,511}")
_EXECUTION = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,511}")
_HARNESS = re.compile(r"[a-z][a-z0-9._/-]{0,95}")
_BEARER = re.compile(r"Bearer [0-9a-f]{64}")
_COST_AMOUNT = "agenturm.reported_cost.amount"
_COST_CURRENCY = "agenturm.reported_cost.currency"
_COST_FIELDS = (
    (_COST_AMOUNT, _COST_CURRENCY),
    ("gen_ai.usage.cost", "gen_ai.usage.cost.currency"),
    ("llm.cost.total", "llm.cost.currency"),
)
_USD_COST = "llm.cost_usd"
_COST_DECIMAL = re.compile(r"([0-9]+)(?:\.([0-9]*))?(?:[eE]([+-]?[0-9]+))?")
_CURRENCY = re.compile(r"[A-Z]{3}")
_NANOUNITS = 10**9
_TOKEN_FIELDS = (
    ("gen_ai.usage.input_tokens", "llm.usage.input_tokens", "llm.token_count.prompt"),
    ("gen_ai.usage.output_tokens", "llm.usage.output_tokens", "llm.token_count.completion"),
    ("gen_ai.usage.cache_read.input_tokens", "gen_ai.usage.cache_read_input_tokens",
     "gen_ai.usage.cached_input_tokens", "llm.usage.cache_read_input_tokens",
     "llm.token_count.prompt_details.cache_read", "cache_read_input_tokens", "cached_input_tokens"),
    ("gen_ai.usage.cache_write.input_tokens", "gen_ai.usage.cache_write_input_tokens",
     "gen_ai.usage.cache_creation.input_tokens", "gen_ai.usage.cache_creation_input_tokens",
     "llm.usage.cache_creation_input_tokens", "llm.token_count.prompt_details.cache_write",
     "cache_creation_input_tokens"),
    ("gen_ai.usage.reasoning.output_tokens", "gen_ai.usage.reasoning_tokens",
     "llm.usage.reasoning_tokens", "llm.token_count.completion_details.reasoning", "reasoning_tokens"),
    ("gen_ai.usage.total_tokens", "llm.usage.total_tokens", "llm.token_count.total"),
)
_TOKEN_DECIMAL = re.compile(r"[0-9]{1,19}")


def _token_integer(value):
    if type(value) is str:
        if len(value) > 19 or not _TOKEN_DECIMAL.fullmatch(value):
            return None
        value = int(value)
    return value if type(value) is int and 0 <= value < 1 << 63 else None


def _reported_token_values(attributes):
    if not isinstance(attributes, Mapping):
        return {}
    reported = {}
    for aliases in _TOKEN_FIELDS:
        values = {_token_integer(attributes[key]) for key in aliases if key in attributes}
        if len(values) == 1 and None not in values:
            reported[aliases[0]] = str(values.pop())
    return reported


def _cost_nanounits(value):
    if type(value) not in (int, float, str) or (type(value) is int and value.bit_length() > 64):
        return None
    text = str(value)
    match = _COST_DECIMAL.fullmatch(text) if len(text) <= 48 else None
    if match is None:
        return None
    whole, fraction, exponent = match.groups()
    exponent = int(exponent or 0)
    if not -64 <= exponent <= 64:
        return None
    fraction = fraction or ""
    digits = whole + fraction
    significant = digits.strip("0")
    if not significant:
        return 0
    shift = 9 + exponent + len(digits) - len(digits.rstrip("0")) - len(fraction)
    if not 0 <= shift <= 19 - len(significant):
        return None
    result = int(significant) * 10**shift
    return result if result < 1 << 63 else None


def _reported_cost_attributes(attributes):
    if not isinstance(attributes, Mapping):
        return []
    if "llm.cost_known" in attributes and attributes["llm.cost_known"] is not True:
        return []
    amounts, currencies = set(), set()
    if _USD_COST in attributes:
        amounts.add(_cost_nanounits(attributes[_USD_COST]))
        currencies.add("USD")
    for amount_field, currency_field in _COST_FIELDS:
        if amount_field in attributes:
            if currency_field not in attributes:
                return []
            amounts.add(_cost_nanounits(attributes[amount_field]))
        if currency_field in attributes:
            value = attributes[currency_field]
            if not isinstance(value, str) or not _CURRENCY.fullmatch(value):
                return []
            currencies.add(value)
    if len(amounts) != 1 or None in amounts or len(currencies) != 1:
        return []
    amount = amounts.pop()
    text = f"{amount // _NANOUNITS}.{amount % _NANOUNITS:09d}".rstrip("0").rstrip(".")
    return [
        {"key": _COST_AMOUNT, "value": {"stringValue": text}},
        {"key": _COST_CURRENCY, "value": {"stringValue": currencies.pop()}},
    ]


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


def _trace_endpoint(raw):
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


def _trace_authorization(endpoint, configured_endpoint):
    if not configured_endpoint:
        return None
    try:
        if _trace_endpoint(configured_endpoint)[2] != endpoint:
            return None
    except ValueError:
        return None
    headers = os.environ.get("OTEL_EXPORTER_OTLP_TRACES_HEADERS", "")
    if len(headers) > 4096:
        return None
    authorization = []
    for item in headers.split(","):
        key, _, value = item.partition("=")
        if key.strip().lower() == "authorization":
            authorization.append(unquote(value))
    if len(authorization) == 1 and _BEARER.fullmatch(authorization[0]):
        return authorization[0]
    return None


def _span_id(value, width):
    if (
        not isinstance(value, str) or len(value) != width
        or any(ch not in "0123456789abcdefABCDEF" for ch in value)
        or not any(ch != "0" for ch in value)
    ):
        raise ValueError("Invalid OTLP trace or span identity")
    return value.lower()


def _payload_text(value):
    if not isinstance(value, str) or not value or len(value) > MAX_PAYLOAD_TEXT_BYTES:
        raise ValueError("OTLP captured text is invalid or exceeds its byte limit")
    try:
        size = len(value.encode("utf-8"))
    except UnicodeError:
        raise ValueError("OTLP captured text is not valid Unicode") from None
    if size > MAX_PAYLOAD_TEXT_BYTES:
        raise ValueError("OTLP captured text exceeds its byte limit")
    if _CREDENTIAL_MARKERS.search(value) or any(
        (":" in line or "=" in line) and _CREDENTIAL_FIELD.search(line)
        for line in value.splitlines()
    ) or re.search(r"://[^/\s]+:[^/\s]+@", value):
        raise ValueError("OTLP captured text was withheld")
    if any(
        (ord(ch) < 32 and ch not in "\n\t") or 127 <= ord(ch) <= 159
        or ch in "\u061c\u200e\u200f"
        or "\u2028" <= ch <= "\u202e" or "\u2066" <= ch <= "\u2069"
        for ch in value
    ):
        raise ValueError("OTLP captured text was withheld")
    return size


def _payload_attributes(span, scope):
    attributes = span.get("attributes") or {}
    payload = []
    size = 0
    for key in PAYLOAD_KEYS:
        if key in attributes:
            if scope.session_id is None:
                raise ValueError("OTLP captured text requires an explicit session identity")
            text = attributes[key]
            size += _payload_text(text)
            payload.append({"key": key, "value": {"stringValue": text}})
    return payload, size


def _project(span, scope, attributes, capture_content):
    if span is None:
        raise ValueError("OTLP span context is unavailable")
    start = span["start_unix_nano"]
    end = span["end_unix_nano"]
    if type(start) is not int or type(end) is not int or not 0 < start <= end <= (1 << 63) - 1:
        raise ValueError("Invalid OTLP span timing")
    status = _STATUS.get(span["status"])
    if status is None:
        raise ValueError("Invalid OTLP span status")
    payload, payload_bytes = _payload_attributes(span, scope) if capture_content else ([], 0)
    native_attributes = span.get("attributes")
    result = {
        "traceId": _span_id(span["trace_id"], 32),
        "spanId": _span_id(span["span_id"], 16),
        "name": "agent.run" if span.get("agent_run") is True else "agenturm.span",
        "startTimeUnixNano": str(start),
        "endTimeUnixNano": str(end),
        "status": {"code": status},
        "attributes": [
            *attributes, *_reported_cost_attributes(native_attributes),
            *({"key": key, "value": {"intValue": value}}
              for key, value in _reported_token_values(native_attributes).items()),
            *payload,
        ],
    }
    if span.get("parent_span_id") is not None:
        parent = _span_id(span["parent_span_id"], 16)
        if parent == result["spanId"]:
            raise ValueError("Invalid OTLP parent span identity")
        result["parentSpanId"] = parent
    return json.dumps(result, separators=(",", ":")).encode("ascii"), payload_bytes


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


class _DeadlineSocket(socket.socket):
    """Apply one deadline to every send and buffered HTTP receive."""

    def __init__(self, family, deadline):
        super().__init__(family, socket.SOCK_STREAM)
        self.deadline = deadline

    def _arm_timeout(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("OTLP request deadline exceeded")
        self.settimeout(remaining)

    def connect(self, address):
        self._arm_timeout()
        return super().connect(address)

    def sendall(self, data, flags=0):
        self._arm_timeout()
        return super().sendall(data, flags)

    def recv_into(self, buffer, nbytes=0, flags=0):
        self._arm_timeout()
        return super().recv_into(buffer, nbytes, flags)


class _DeadlineConnection(http.client.HTTPConnection):
    def __init__(self, host, port, timeout):
        super().__init__(host, port, timeout=timeout)
        self.deadline = time.monotonic() + timeout

    def connect(self):
        # The endpoint validator already pins a numeric loopback address.
        family = socket.AF_INET6 if ":" in self.host else socket.AF_INET
        self.sock = _DeadlineSocket(family, self.deadline)
        self.sock.connect((self.host, self.port))


class OtlpTransport:
    """Send structural span evidence without inherited proxies or redirects."""

    def __init__(self, endpoint, timeout, *, capture_content=False):
        configured_endpoint = os.environ.get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        if endpoint is None:
            endpoint = (configured_endpoint or os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
                        or "http://127.0.0.1:14318")
        self.host, self.port, self.endpoint = _trace_endpoint(endpoint)
        self._authorization = _trace_authorization(self.endpoint, configured_endpoint)
        self._authorization_target = (self.host, self.port)
        self.capture_content = bool(capture_content)
        if self.capture_content and self._authorization is None:
            raise ValueError("OTLP captured text requires an authenticated trace receiver")

        try:
            self.timeout = float(timeout)
        except (ValueError, TypeError, OverflowError):
            raise ValueError("OTLP timeout must be between 0 and 30 seconds") from None
        if not math.isfinite(self.timeout) or not 0 < self.timeout <= 30:
            raise ValueError("OTLP timeout must be between 0 and 30 seconds")

    def export(self, spans, scope, *, strict):
        attributes = scope.attributes()
        batch = []
        overhead = len(_PREFIX) + len(_SUFFIX)
        size = overhead
        captured_bytes = 0
        failure = None

        def send():
            nonlocal failure
            try:
                self._send(batch)
            except OSError as exc:
                if strict:
                    raise
                failure = exc

        for span in spans:
            try:
                item, item_payload_bytes = _project(span, scope, attributes, self.capture_content)
                if overhead + len(item) > MAX_EXPORT_BYTES:
                    raise ValueError("OTLP span exceeds the request byte limit")
            except (KeyError, TypeError, ValueError) as exc:
                if strict:
                    raise
                failure = exc
                continue
            if batch and (
                len(batch) == MAX_EXPORT_SPANS or size + len(item) + 1 > MAX_EXPORT_BYTES
                or captured_bytes + item_payload_bytes > MAX_PAYLOAD_BATCH_BYTES
            ):
                send()
                batch = []
                size = overhead
                captured_bytes = 0
            size += len(item) + bool(batch)
            captured_bytes += item_payload_bytes
            batch.append(item)
        if batch:
            send()
        if failure is not None:
            raise failure

    def _send(self, batch):
        body = _PREFIX + b",".join(batch) + _SUFFIX
        self._send_body("/v1/traces", body)

    def progress(self, trace_id, event_id, scope, stage, percent):
        if (
            type(stage) is not str or not _PROGRESS_STAGE.fullmatch(stage)
            or _PROGRESS_CREDENTIAL.search(stage)
            or re.fullmatch(r"[0-9a-fA-F]{32,64}", stage)
        ):
            raise ValueError("Progress stage must be a safe identifier")
        body = {
            "trace_id": trace_id, "event_id": event_id,
            "observed_at_unix_ns": str(time.time_ns()), "stage": stage,
        }
        if percent is not None:
            if type(percent) not in (int, float) or not 0 <= percent <= 100:
                raise ValueError("Progress percent must be between 0 and 100")
            basis_points = round(percent * 100)
            if basis_points / 100 != percent:
                raise ValueError("Progress percent supports at most two decimal places")
            body["percent_basis_points"] = basis_points
        for key, value in (
            ("session_id", scope.session_id), ("target_source_id", scope.harness_id),
            ("turn_id", scope.turn_id), ("worker_id", scope.worker_id),
        ):
            if value is not None:
                body[key] = value
        self._send_body(_PROGRESS_PATH, json.dumps(body, separators=(",", ":")).encode("ascii"))

    def _send_body(self, path, body):
        target = (self.host, self.port)
        headers = {"Content-Type": "application/json", "Connection": "close"}
        if self._authorization is not None and target == self._authorization_target:
            headers["Authorization"] = self._authorization
        connection = _DeadlineConnection(*target, self.timeout)
        try:
            connection.request("POST", path, body=body, headers=headers)
            with connection.getresponse() as response:
                _check_reply(response)
        except http.client.HTTPException:
            raise OSError("OTLP receiver returned an invalid HTTP response") from None
        finally:
            connection.close()
