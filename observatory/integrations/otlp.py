"""OpenTelemetry SDK adapter for the metadata-only Rust producer transport."""
from __future__ import annotations

import logging

from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

_LOGGER = logging.getLogger(__name__)


def _span_metadata(span):
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
    }


class MetadataSpanExporter(SpanExporter):
    """Use the existing SDK processor with a bounded structural exporter."""

    def __init__(self, client):
        self.client = client

    def export(self, spans):
        if self.client._post_spans(_span_metadata(span) for span in spans):
            return SpanExportResult.SUCCESS
        _LOGGER.warning("Agenturm OTLP span export failed: %s", self.client.last_error)
        return SpanExportResult.FAILURE
