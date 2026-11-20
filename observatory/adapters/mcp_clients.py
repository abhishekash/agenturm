"""Trusted MCP config declarations for clients without bundled session readers."""
from __future__ import annotations

import os
from pathlib import Path

from ..mcp_config_registry import McpConfigContribution


def mcp_config_contributions():
    return (
        McpConfigContribution(
            "cursor", "Cursor", (("mcpServers",),),
            lambda: (Path(os.environ.get("CURSOR_CONFIG_DIR", Path.home() / ".cursor")) / "mcp.json",),
            lambda root: (root / ".cursor" / "mcp.json",), priority=2),
        McpConfigContribution(
            "gemini", "Gemini CLI", (("mcpServers",),),
            lambda: ((Path(os.environ.get("GEMINI_CLI_HOME", Path.home() / ".gemini")) / "settings.json", "User"),
                     (Path(os.environ.get("GEMINI_CLI_SYSTEM_SETTINGS_PATH", "/etc/gemini-cli/settings.json")), "System")),
            lambda root: (root / ".gemini" / "settings.json",), priority=3),
        McpConfigContribution(
            "copilot", "GitHub Copilot CLI", (("mcpServers",),),
            lambda: (Path(os.environ.get("COPILOT_HOME", Path.home() / ".copilot")) / "mcp-config.json",),
            lambda _root: (), priority=4),
        McpConfigContribution(
            "vscode", "VS Code", (("servers",),),
            lambda: (), lambda root: (root / ".vscode" / "mcp.json",), priority=7),
    )
