"""OpenTelemetry SDK adapter for the Rust producer transport."""
from __future__ import annotations

import logging
import re

from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

from .otel_payload import join_text, structured_message_text
from ..otlp_transport import _reported_cost_attributes, _reported_token_values

_LOGGER = logging.getLogger(__name__)


_MESSAGE_TEXT = re.compile(
    r"llm\.(input|output)_messages\.([0-9]{1,3})\.message\.content"
    r"(?:s\.([0-9]{1,3})\.message_content\.text)?"
)
_MESSAGE_SCOPES = frozenset(
    "openinference.instrumentation." + name for name in ("openai", "anthropic", "langchain")
)


def _captured_text(span):
    scope = getattr(span, "instrumentation_scope", None)
    scope_name = getattr(scope, "name", "")
    attributes = span.attributes or {}
    if scope_name not in _MESSAGE_SCOPES:
        return structured_message_text(attributes, scope_name)
    messages = {"input": [], "output": []}
    for key, text in attributes.items():
        match = _MESSAGE_TEXT.fullmatch(key)
        if match is None or not isinstance(text, str) or not text:
            continue
        direction, index, block = match.groups()
        prefix = f"llm.{direction}_messages.{index}.message"
        if attributes.get(prefix + ".role") not in {"user", "assistant"}:
            continue
        if block is not None:
            kind = attributes.get(f"{prefix}.contents.{block}.message_content.type")
            if kind != "text":
                continue
        messages[direction].append((int(index), int(block) if block is not None else -1, text))
    result = {}
    for direction, parts in messages.items():
        text = join_text(text for _, _, text in sorted(parts))
        if text is not None:
            result[f"agenturm.payload.{direction}"] = text
    return result


def _span_metadata(span, capture_content=False):
    context = span.get_span_context()
    if context is None:
        return None
    return {
        "trace_id": f"{context.trace_id:032x}",
        "span_id": f"{context.span_id:016x}",
        "parent_span_id": f"{span.parent.span_id:016x}" if span.parent else None,
        "start_unix_nano": span.start_time,
        "end_unix_nano": span.end_time,
        "status": span.status.status_code.name,
        "attributes": {
            **{field["key"]: field["value"]["stringValue"]
               for field in _reported_cost_attributes(span.attributes)},
            **_reported_token_values(span.attributes),
            **(_captured_text(span) if capture_content else {}),
        },
    }


class MetadataSpanExporter(SpanExporter):
    """Use the existing SDK processor with a bounded structural exporter."""

    def __init__(self, client):
        self.client = client

    def export(self, spans):
        if self.client._post_spans(
            _span_metadata(span, self.client.capture_content) for span in spans
        ):
            return SpanExportResult.SUCCESS
        _LOGGER.warning("Agenturm OTLP span export failed: %s", self.client.last_error)
        return SpanExportResult.FAILURE
