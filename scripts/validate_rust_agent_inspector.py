"""Exercise the default collector and agent evidence tabs in isolated PTYs."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import sys
import tempfile
import time
from types import SimpleNamespace

from validate_rust_real_session_roster import Cockpit, environment, run


def write_json(path, value):
    path.write_text(json.dumps(value) + "\n", encoding="utf-8")
    path.chmod(0o600)


def tap(cockpit, keys):
    cockpit.key(keys)
    # Let crossterm resolve a standalone Escape before sending another CSI.
    until = time.monotonic() + 0.15
    while time.monotonic() < until:
        cockpit.receive()


def validate(app, bundle):
    with tempfile.TemporaryDirectory(prefix="agenturm-inspector-qualification-") as directory:
        work = Path(directory)
        staged = work / "bundle"
        shutil.copytree(bundle, staged)
        binary = staged / "bin/agenturm"
        shutil.copy2(app, binary)
        codex = work / "codex"
        sessions = codex / "sessions/2026/11/27"
        sessions.mkdir(parents=True)
        session = "01a21c90-1111-7111-8111-111111111111"
        source = sessions / f"rollout-2026-11-27T10-00-00-{session}.jsonl"
        rows = [
            {"type": "session_meta", "payload": {"id": session, "cwd": str(work), "source": "cli", "base_instructions": "Header fixture " * 1500}},
            {"type": "event_msg", "timestamp": "2026-11-27T10:00:00Z", "payload": {"type": "task_started", "turn_id": "turn-1"}},
            {"type": "event_msg", "timestamp": "2026-11-27T10:00:01Z", "payload": {
                "type": "item_completed", "thread_id": session, "turn_id": "turn-1",
                "item": {"type": "UserMessage", "content": [{"type": "text", "text": "INSPECTOR_PROMPT_MARKER"}]}}},
            {"type": "event_msg", "timestamp": "2026-11-27T10:00:02Z", "payload": {
                "type": "item_completed", "thread_id": session, "turn_id": "turn-1",
                "item": {"type": "McpToolCall", "id": "mcp-call", "server": "fixture-provider", "tool": "inspector-tool", "status": "completed", "arguments": {"query": "INSPECTOR_MCP_PAYLOAD"}}}},
        ]
        # Later events must not hide the earlier call behind a general page cap.
        for index in range(600):
            rows.append({"type": "event_msg", "timestamp": "2026-11-27T10:01:00Z", "payload": {
                "type": "item_completed", "thread_id": session, "turn_id": "turn-1",
                "item": {"type": "CommandExecution", "id": f"command-{index}", "command": "echo fixture", "status": "completed", "exit_code": 0, "aggregated_output": "fixture output " * 1000}}})
        source.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
        source.chmod(0o600)
        assert source.stat().st_size > 8 * 1024 * 1024, "Exercise a real-sized transcript above the previous 8 MiB cap"
        env = environment(SimpleNamespace(codex_home=codex, claude_home=work / "missing-claude"), work)
        config = json.loads(run(binary, env, work, "discover"))
        source_id = config["sources"][0]["source_id"]
        config_file = work / "grant-config.json"
        write_json(config_file, config)
        run(binary, env, work, "cockpit", "--once", "--json")
        db = work / "state/agenturm-rust/history.sqlite"
        assert not db.with_name("sources.json").exists()
        for _ in range(16):
            retained = json.loads(run(binary, env, work, "query", "--db", db, "--limit", "16384"))["observations"]
            if len(retained) > 600:
                break
            run(binary, env, work, "cockpit", "--once", "--json")
        assert len(retained) > 600, f"Fixture must exceed the general history page limit: {len(retained)}"
        # This intentionally reproduces the stale sibling file from the report.
        write_json(db.with_name("sources.json"), {"version": 2, "plugin_host": "/missing/old-build/host", "plugin_catalog": "/missing/old-build/catalog.json", "sources": config["sources"]})
        for width, height in ((120, 32), (80, 24), (60, 18)):
            run(binary, env, work, "content", "grant", "--db", db, "--config", config_file, "--source-id", source_id, "--kind", "codex.mcp_payload")
            with Cockpit(binary, env, work, width, height) as cockpit:
                try:
                    cockpit.markers("roster", "Agenturm command center", "All 1")
                    tap(cockpit, b"\r")
                    cockpit.markers("inspector", "Overview", "Prompts", "Details", "Activity", "MCP", "Tools")
                    tap(cockpit, b"P")
                    cockpit.markers("first-use prompt permission", "Enable prompt history", "a Enable this source")
                    assert "INSPECTOR_PROMPT_MARKER" not in cockpit.screen.contents()
                    tap(cockpit, b"a")
                    cockpit.markers("active prompt authority", "1 loaded")
                    assert "Metadata only" not in cockpit.screen.contents()
                    tap(cockpit, b"\x1b[C")
                    cockpit.markers("verified prompt delivery", "INSPECTOR_PROMPT_MARKER")
                    tap(cockpit, b"\x1b")
                    cockpit.until(lambda frame: "INSPECTOR_PROMPT_MARKER" not in frame, "redaction")
                    tap(cockpit, b"\x1b")
                    cockpit.markers("prompt return", "Overview", "Details")
                    tap(cockpit, b"\x1b[C")
                    cockpit.markers("details", "Session details")
                    tap(cockpit, b"\x1b[C")
                    cockpit.markers("activity", "Event detail")
                    tap(cockpit, b"\r\r")
                    cockpit.markers("activity row inspector", "Event detail")
                    tap(cockpit, b"\x1b")
                    tap(cockpit, b"\x1b[1;5C")
                    cockpit.markers("MCP before page limit", "inspector-tool")
                    tap(cockpit, b"\r")
                    cockpit.markers("MCP detail", "fixture-provider")
                    tap(cockpit, b"m")
                    cockpit.markers("MCP payload picker", "MCP operations")
                    tap(cockpit, b"\r")
                    cockpit.markers("active MCP authority", "INSPECTOR_MCP_PAYLOAD")
                    tap(cockpit, b"\x1b")
                    cockpit.until(lambda frame: "INSPECTOR_MCP_PAYLOAD" not in frame, "MCP redaction")
                    tap(cockpit, b"\x1b")
                    tap(cockpit, b"\x1b[1;5C")
                    cockpit.markers("tools", "Operations")
                    tap(cockpit, b"\r")
                    cockpit.until(lambda frame: "shell" in frame.lower(), "tool evidence")
                    tap(cockpit, b"\x1b")
                    tap(cockpit, b"\x1b")
                    cockpit.markers("return to roster", "Agenturm command center")
                    cockpit.quit()
                except Exception:
                    print(cockpit.screen.contents(), file=sys.stderr)
                    raise
            run(binary, env, work, "content", "revoke", "--db", db, "--source-id", source_id)
            print(f"Verified fresh prompt permission, stale config, prompt/MCP delivery, activity/tools and terminal restoration at {width}x{height}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    args = parser.parse_args()
    validate(args.app.resolve(), args.bundle.resolve())
