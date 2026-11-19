"""Trusted OpenCode reader registration for the bundled collector."""
from __future__ import annotations


def reader_spec():
    from ..native_readers import NativeReaderSpec
    from ..opencode import OpenCodeReader

    return NativeReaderSpec(
        "opencode",
        lambda store, capture_details: OpenCodeReader(store, capture_details=capture_details),
        mcp_client="OpenCode")
