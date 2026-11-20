"""Trusted OpenCode reader registration for the bundled collector."""
from __future__ import annotations

import os
from pathlib import Path


def native_ingest_scopes():
    from ..native_ingest_scopes import NativeIngestScope

    return (NativeIngestScope("observatory.opencode"),)


def mcp_config_contributions():
    from ..mcp_config_registry import McpConfigContribution

    def global_paths():
        configured = os.environ.get("OPENCODE_CONFIG")
        if configured:
            return (Path(configured),)
        home = Path.home()
        xdg_home = Path(os.environ.get("XDG_CONFIG_HOME") or home / ".config")
        directory = Path(os.environ.get("OPENCODE_CONFIG_DIR") or xdg_home / "opencode")
        return (directory / "opencode.json", directory / "opencode.jsonc")

    return (McpConfigContribution(
        "opencode", "OpenCode", (("mcp", "servers"), ("mcp",)),
        global_paths,
        lambda root: (root / "opencode.json", root / "opencode.jsonc",
                      root / ".opencode" / "opencode.json",
                      root / ".opencode" / "opencode.jsonc"), priority=6),)


def reader_spec():
    from ..native_readers import NativeReaderSpec
    from ..opencode import MAX_SESSIONS, OpenCodeReader

    return NativeReaderSpec(
        "opencode",
        lambda store, capture_details: OpenCodeReader(store, capture_details=capture_details),
        mcp_client="OpenCode", label="OpenCode",
        coverage=lambda reader: {"discovered": reader.discovered_sessions,
                                 "omitted": reader.omitted_sessions,
                                 "limit": MAX_SESSIONS},
        status_extension=lambda reader, diagnostics: {
            "opencode_discovered_sessions": getattr(reader, "discovered_sessions", 0),
            "opencode_omitted_sessions": getattr(reader, "omitted_sessions", 0),
            "opencode_diagnostics": diagnostics,
            "opencode_read_errors": getattr(reader, "errors", 0),
        },
        status_keys=("opencode_discovered_sessions", "opencode_omitted_sessions",
                     "opencode_diagnostics", "opencode_read_errors"))


def turn_review_spec():
    from ..native_turn_review import NativeTurnReviewSpec
    from ..opencode_turn_review import review_turn

    return NativeTurnReviewSpec("opencode", review_turn)
