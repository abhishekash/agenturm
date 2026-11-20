"""Kiro standing permission rules, trusted folders and policy blocks."""
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.kiro_permissions import parse_yaml, read_rules, standing_sources, workspace_permissions
from observatory.local_agents import LocalAgents, Session
from observatory.ui_kiro_inspector import KIRO_PERMISSIONS, permission_summary
from observatory.ui_session_inspector import SessionInspectorRegistry

USER_RULES = """rules:
  - capability: shell
    match:
      - hugo *
      - "git status"
    exclude: ['rm *']
    effect: allow
  - capability: web_fetch
    effect: allow
  - capability: mcp
    match:
      - confluence-mcp/confluence_get_page
    effect: deny
"""


class KiroPermissionRuleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "kiro"

    def write(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def test_parses_the_yaml_kiro_writes(self):
        self.assertEqual(parse_yaml(USER_RULES)["rules"][0],
                         {"capability": "shell", "match": ["hugo *", "git status"], "exclude": ["rm *"],
                          "effect": "allow"})
        self.assertEqual(parse_yaml("rules: []\nworkspaceTrusted: true\n"), {"rules": [], "workspaceTrusted": True})
        self.assertIsNone(parse_yaml(""))

    def test_unsupported_yaml_is_unreadable_not_guessed(self):
        for text in ("rules: &anchor []\n", "rules:\n\t- a\n", "rules:\n  - capability: shell\n   effect: allow\n",
                     "rules: {capability: shell}\n"):
            with self.subTest(text=text):
                result = read_rules(self.write(self.root / "bad" / "permissions.yaml", text))
                self.assertEqual(result["status"], "unreadable")
                self.assertEqual(result["rules"], [])

    def test_reads_json_rules_and_agent_permissions(self):
        self.assertEqual(read_rules(self.write(self.root / "j" / "permissions.json", json.dumps(
            {"rules": [{"capability": "fs_read", "match": "./**", "effect": "allow"}]})))["rules"],
            [{"capability": "fs_read", "effect": "allow", "match": ["./**"]}])
        agent = self.write(self.home / "agents" / "reviewer.json", json.dumps(
            {"name": "reviewer", "permissions": {"rules": [{"capability": "shell", "effect": "allow"}]}}))
        self.assertEqual(read_rules(agent)["rules"], [{"capability": "shell", "effect": "allow"}])
        self.assertEqual(read_rules(self.root / "missing.yaml")["status"], "absent")

    def test_sources_follow_kiro_lookup_order(self):
        self.write(self.home / "settings" / "permissions.json", '{"rules": []}')
        workspace = self.root / "project"
        local_agent = self.write(workspace / ".kiro" / "agents" / "reviewer.json", "{}")
        sources = standing_sources(self.home, "abc", "reviewer", workspace)
        self.assertEqual([label for label, _ in sources], ["User", "Workspace", "Agent reviewer"])
        self.assertEqual(sources[0][1].name, "permissions.json")
        self.assertEqual(sources[1][1], self.home / "workspace-roots" / "abc" / "permissions.yaml")
        self.assertEqual(sources[2][1], local_agent)
        self.assertEqual(len(standing_sources(self.home, "abc", "vibe")), 2)
        self.assertEqual(len(standing_sources(self.home, "abc", "../escape")), 2)

    def test_trusted_folders_are_named_from_their_sessions(self):
        self.write(self.home / "workspace-roots" / "abc" / "permissions.yaml", "rules: []\nworkspaceTrusted: true\n")
        self.write(self.home / "workspace-roots" / "def" / "permissions.yaml", "rules: []\n")
        self.write(self.home / "workspace-roots" / "ghi" / "permissions.yaml", "rules: []\nworkspaceTrusted: true\n")
        self.write(self.home / "sessions" / "abc" / "sess_1" / "session.json",
                   json.dumps({"workspacePaths": ["/work/project"]}))
        folders = workspace_permissions(self.home, {})
        self.assertEqual([(item["key"], item["folder"], item["trusted"]) for item in folders],
                         [("abc", "/work/project", True), ("ghi", None, True)])


class KiroSessionPermissionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "kiro"
        self.store = Store(self.root / "events.db")
        self.session_dir = self.home / "sessions" / "abc" / "sess_1"
        self.session_dir.mkdir(parents=True)
        (self.session_dir / "session.json").write_text(json.dumps(
            {"id": "sess_1", "workspacePaths": ["/work/project"], "agentMode": "vibe", "autopilot": False}))
        (self.home / "settings").mkdir()
        (self.home / "settings" / "permissions.yaml").write_text(USER_RULES)
        (self.home / "workspace-roots" / "abc").mkdir(parents=True)
        (self.home / "workspace-roots" / "abc" / "permissions.yaml").write_text("rules: []\nworkspaceTrusted: true\n")
        self.env = patch.dict(os.environ, {"KIRO_HOME": str(self.home)})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_inspector_lists_rules_blocks_and_trusted_folders(self):
        path = self.session_dir / "messages.jsonl"
        path.write_text(json.dumps({"id": "c-call", "timestamp": "2026-11-18T12:00:00Z", "payload": {
            "type": "tool_call", "toolCallId": "c", "toolName": "fs_write", "status": "denied", "kind": "edit",
            "args": {"path": "/x/.kiro/settings/mcp.json"},
            "_meta": {"kiro": {"policyDenial": {
                "capability": "fs_write", "resource": "/x/.kiro/settings/mcp.json", "effect": "deny",
                "matchedRule": {"capability": "fs_write", "match": ["~/.kiro/settings/"], "effect": "deny"},
                "scope": "kiro", "source": "kiro-scope"}}}}}) + "\n")
        agents = LocalAgents(self.store)
        agents.sessions = {path: Session(self.store, path, "Kiro")}
        agents.poll()
        status = agents.status()
        item = status["sessions"][0]
        self.assertEqual(item["permissions"]["counts"], {"policy_denied": 1})
        short, details = permission_summary(item, status["kiro_folders"])
        self.assertEqual(short, "No approval requests recorded · 1 blocked by rule · autopilot off · trusted folder")
        self.assertIn("Blocked by rule fs_write · fs_write  /x/.kiro/settings/mcp.json", details)
        self.assertIn("allow shell: hugo *, git status · except rm *", details)
        self.assertIn("deny mcp: confluence-mcp/confluence_get_page", details)
        self.assertIn("allow web_fetch: anything", details)
        self.assertIn("Workspace /work/project · trusted", details)
        self.assertIn("Trusted folders (1)\n  /work/project · this session", details)

    def test_metadata_only_hides_full_folder_paths(self):
        path = self.session_dir / "messages.jsonl"
        path.write_text("")
        agents = LocalAgents(self.store, capture_details=False)
        agents.sessions = {path: Session(self.store, path, "Kiro", capture_details=False)}
        agents.poll()
        status = agents.status()
        self.assertEqual(status["kiro_folders"][0]["folder"], "project")
        inspector = SessionInspectorRegistry((KIRO_PERMISSIONS,)).for_session(
            status["sessions"][0], status)
        self.assertNotIn("/work/project", repr(inspector))
        self.assertIn("Trusted folders (1)", inspector.detail_sections[0][1])
        workspace = next(source for source in status["sessions"][0]["permissions"]["rules"]
                         if source["source"] == "Workspace")
        self.assertEqual(workspace["folder"], "project")


if __name__ == "__main__":
    unittest.main()
