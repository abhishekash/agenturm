# Native MCP configuration contributors

Agenturm inventories configured MCP **server names** for its native harnesses. The collector returns a client name, scope, server name, and config filename. It never returns server settings, commands, environment variables, or tokens.

## Contract

Bundled modules in `observatory.adapters` can export `mcp_config_contributions()` and return a tuple of `McpConfigContribution` values. The declaration supplies:

| Field | Meaning |
| --- | --- |
| `id` | Stable, unique contribution identifier. |
| `client` | Display name and deduplication key for the harness. |
| `selectors` | Ordered paths to the mapping of MCP server names inside parsed TOML, JSON, or JSONC. |
| `global_paths()` | User or system config locations, evaluated when an inventory is collected. |
| `project_paths(root)` | Project config locations for a supplied project root. |
| `priority` | Deterministic registration order, then `id`. |

For example, a bundled harness with `settings.mcpServers` can declare:

```python
from pathlib import Path
from ..mcp_config_registry import McpConfigContribution

def mcp_config_contributions():
    return (McpConfigContribution(
        "example", "Example CLI", (("settings", "mcpServers"),),
        lambda: (Path.home() / ".example" / "config.json",),
        lambda root: (root / ".example" / "config.json",),
    ),)
```

Adding a bundled harness requires no change to `local_agents.py` or the generic collector. Config files are read only from declared paths. Project directories are passed in by the native session inventory; Agenturm does not search arbitrary project trees for config files.

This registry imports only modules in Agenturm's installed `observatory.adapters` package. It does not load Python from config files or project directories, discover entry points, or enable external process plugins. External connectors require a separate trust and lifecycle design.

## Current compatibility

| Client | User or system config | Project config | Server mapping |
| --- | --- | --- | --- |
| Codex | `CODEX_HOME/config.toml` | — | `mcp_servers` |
| Claude Code | `~/.claude.json` | `.mcp.json` | `mcpServers` |
| Cursor | `CURSOR_CONFIG_DIR/mcp.json` | `.cursor/mcp.json` | `mcpServers` |
| Gemini CLI | `GEMINI_CLI_HOME/settings.json`; system settings path | `.gemini/settings.json` | `mcpServers` |
| GitHub Copilot CLI | `COPILOT_HOME/mcp-config.json` | — | `mcpServers` |
| Kiro | `KIRO_HOME/settings/mcp.json` | `.kiro/settings/mcp.json` | `mcpServers` |
| OpenCode | `OPENCODE_CONFIG`, or config directory JSON/JSONC | root and `.opencode/` JSON/JSONC | `mcp.servers`, then `mcp` |
| VS Code | — | `.vscode/mcp.json` | `servers` |

Default home and config directory paths remain in effect when an environment override is absent. The public `discover_mcp_configurations(global_paths=...)` override still accepts explicit `(path, client, scope)` triples for callers and tests.

Each contribution and path factory is isolated. A faulty declaration, unreadable file, malformed config, or oversized file increments the existing error callback and does not hide other clients' names. The collector reads at most 1 MiB plus one byte per file, caps names and paths per declaration, and sorts output deterministically. Duplicate client or contribution IDs are rejected.

## Verification

The MCP registry tests cover a new bundled harness without collector edits, a broken declaration, a broken path factory, duplicate registration, a bounded read after file growth, and system scope. The native session tests cover the existing client formats, OpenCode v1/v2, environment overrides, malformed and oversized files, and project paths.

```sh
python -m unittest tests.test_mcp_config_registry tests.test_native_sessions \
  tests.test_kiro_sessions tests.test_native_reader_registry \
  tests.test_native_bootstrap tests.test_startup_receiver_health
```
