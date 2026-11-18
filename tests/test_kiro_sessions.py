"""Kiro native sessions: discovery, lifecycle, approvals, tools and workers."""
import json
import os
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from observatory.core import Store
from observatory.local_agents import LocalAgents, Session, discover_mcp_configurations, identity, kiro_home
from observatory.session_status import session_health
from observatory.ui_monitoring import task_usage_label
from observatory.ui_workflow import resume_command

SESSION_ID = "sess_11111111-2222-3333-4444-555555555555"


def record(record_id, second, payload):
    return {"id": record_id, "timestamp": f"2026-11-18T12:00:{second:02d}.000Z", "payload": payload}


def prompt(record_id, second, text, **extra):
    return record(record_id, second, {"type": "user", "content": text, "documents": [], "images": [], **extra})


def turn_start(second, turn):
    return record(turn + "-turn-start", second, {"type": "turn_start", "executionId": turn, "turnId": turn,
                                                 "boundarySource": "observed"})


def turn_end(second, turn, reason="end_turn"):
    return record(turn + "-turn-end", second, {"type": "turn_end", "stopReason": reason, "executionId": turn,
                                               "turnId": turn, "boundarySource": "observed"})


def tool(second, call_id, name, args, content, *, kind="execute", status="completed", success=True, meta=None,
         duration=None):
    call = {"type": "tool_call", "toolCallId": call_id, "toolName": name, "args": args, "status": status,
            "kind": kind, "_meta": {"kiro": {"toolOrigin": "default", **(meta or {})}}}
    if "path" in args:
        call["filePath"] = args["path"]
    result = {"type": "tool_result", "toolCallId": call_id, "content": content, "success": success}
    if duration is not None:
        result["durationMs"] = duration
    return [record(call_id + "-call", second, call), record(call_id + "-result", second, result)]


def approval(second, call_id, command):
    return record(call_id + "-pending", second, {
        "type": "pending_interaction", "interactionType": "tool_approval", "toolCallId": call_id,
        "question": command, "consentRound": 1,
        "options": [{"optionId": "accept", "name": "Allow", "kind": "allow_once"}],
        "_meta": {"kiro": {"toolId": "run_command", "command": command}}})


def resolved(second, call_id, option="accept"):
    return record(call_id + "-resolved", second, {"type": "interaction_resolved", "toolCallId": call_id,
                                                  "outcome": "selected", "selectedOption": option,
                                                  "consentRound": 1})


class KiroSessionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = Store(self.root / "events.db")
        self.home = self.root / "kiro-home"
        self.session_dir = self.home / "sessions" / "0123456789abcdef" / SESSION_ID
        self.session_dir.mkdir(parents=True)
        (self.session_dir / "session.json").write_text(json.dumps({
            "schemaVersion": "1.0.0", "id": SESSION_ID, "title": "Generated Session Title",
            "workspacePaths": ["/work/kiro-demo"], "modelId": "claude-test", "status": "in_progress"}))

    def session(self, details=True, path=None):
        path = path or self.session_dir / "messages.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
        return Session(self.store, path, "Kiro", capture_details=details)

    def append(self, session, *rows):
        with session.path.open("a") as stream:
            for row in rows:
                stream.write(json.dumps(row) + "\n")
        session.poll()
        self.assertEqual(session.skipped, 0)

    def logs(self):
        with self.store._connect() as db:
            rows = db.execute("SELECT body,attributes FROM logs WHERE service='kiro' ORDER BY rowid").fetchall()
        return [(row["body"], json.loads(row["attributes"])) for row in rows]

    def spans(self):
        with self.store._connect() as db:
            rows = db.execute("SELECT name,status,attributes FROM spans WHERE service='kiro'").fetchall()
        return [(row["name"], row["status"], json.loads(row["attributes"])) for row in rows]

    def test_discovery_reads_current_layout_only(self):
        sessions = self.home / "sessions"
        (self.session_dir / "messages.jsonl").write_text("")
        (self.session_dir / "sub-executions").mkdir()
        (self.session_dir / "sub-executions" / "worker-1.jsonl").write_text("")
        (self.session_dir / "snapshots" / "abc").mkdir(parents=True)
        (self.session_dir / "snapshots" / "abc" / "state.jsonl").write_text("")
        (sessions / "cli").mkdir()
        (sessions / "cli" / "legacy.jsonl").write_text("")
        (sessions / "0123456789abcdef" / "stray.jsonl").write_text("")
        agents = LocalAgents(self.store)
        with patch.dict(os.environ, {"CODEX_HOME": str(self.root / "codex"),
                                     "CLAUDE_CONFIG_DIR": str(self.root / "claude"),
                                     "PI_CODING_AGENT_DIR": str(self.root / "pi"),
                                     "KIRO_HOME": str(self.home)}):
            agents.discover()
        self.assertEqual(agents.discovered_sources, 2)
        self.assertEqual({path.name for path in agents.sessions}, {"messages.jsonl", "worker-1.jsonl"})
        self.assertEqual({session.runtime for session in agents.sessions.values()}, {"Kiro"})

    def test_kiro_home_follows_kiro_rules(self):
        with patch.dict(os.environ, {"KIRO_HOME": str(self.home)}):
            self.assertEqual(kiro_home(), self.home)
        with patch.dict(os.environ, {"KIRO_HOME": "relative/dir"}), patch.object(Path, "home", return_value=self.root):
            self.assertEqual(kiro_home(), self.root / ".kiro")
        with patch.dict(os.environ, {"KIRO_HOME": "/"}), patch.object(Path, "home", return_value=self.root):
            self.assertEqual(kiro_home(), self.root / ".kiro")

    def test_turn_tools_and_completion_are_recorded(self):
        session = self.session()
        self.assertEqual(session.session_id, SESSION_ID)
        self.assertEqual(session.project, "kiro-demo")
        self.assertEqual(session.working_directory, "/work/kiro-demo")
        self.assertEqual(session.model, "claude-test")
        self.append(session,
            prompt("user-1", 0, "Fix the failing parser test"),
            turn_start(0, "turn-1"),
            record("say-1", 1, {"type": "assistant", "content": "Running the tests first.", "operationType": "Say"}),
            record("think-1", 1, {"type": "assistant", "content": "private reasoning", "operationType": "Reasoning"}),
            *tool(2, "call-shell", "execute_bash", {"command": "pytest -q", "cwd": "/work/kiro-demo"},
                  "Output:\n1 failed\n\n\nExit Code: 1", status="failed", success=False, duration=1500),
            *tool(3, "call-edit", "str_replace", {"path": "/work/kiro-demo/parser.py", "oldStr": "a", "newStr": "b"},
                  "Replaced text", kind="edit"),
            *tool(4, "call-mcp", "mcp_codebase_memory_mcp_search_graph", {"query": "parser"}, "{}", kind="other",
                  meta={"toolOrigin": "client", "serverName": "codebase-memory-mcp"}),
            *tool(5, "call-bg", "execute_bash", {"command": "npm run dev", "run_in_background": True},
                  "Output:\nstarted\n\nExit Code: 0"),
            record("usage-1", 6, {"type": "usage_summary", "elapsedTime": 6000, "status": "success",
                                  "promptTurnSummaries": [{"unit": "credit", "usage": 0.25}]}),
            turn_end(6, "turn-1"))
        self.assertEqual(session.run_state, "completed")
        self.assertEqual(session.task_description, "Fix the failing parser test")
        self.assertTrue(session.outcome_id)

        spans = self.spans()
        root = [attrs for name, _, attrs in spans if name == "agent.run"]
        self.assertTrue(any(attrs["agent.run.state"] == "completed" for attrs in root))
        self.assertTrue(all(attrs["agent.runtime"] == "Kiro" for _, _, attrs in spans))
        shell = next((status, attrs) for name, status, attrs in spans if name == "execute_bash" and
                     attrs.get("observatory.duration_unknown") is None)
        self.assertEqual(shell[0], "ERROR")
        mcp = next(attrs for name, _, attrs in spans if name == "codebase-memory-mcp / search_graph")
        self.assertEqual(mcp["mcp.tool.name"], "search_graph")

        events = {(attrs["agent.activity.domain"], attrs["agent.activity.action"], attrs["agent.activity.subject"]):
                  (body, attrs) for body, attrs in self.logs()}
        body, attrs = events[("shell", "execute", "pytest -q")]
        self.assertEqual(attrs["agent.activity.result"], "error")
        self.assertEqual(attrs["process.exit.code"], 1)
        self.assertIn("kiro.result_exit_code", body)
        _, background = events[("shell", "execute", "npm run dev")]
        self.assertEqual(background["agent.activity.result"], "unknown")
        self.assertNotIn("process.exit.code", background)
        _, edit = events[("file", "edit", "/work/kiro-demo/parser.py")]
        self.assertEqual(edit["agent.activity.result"], "success")
        _, mcp_event = events[("mcp", "search_graph", "codebase-memory-mcp")]
        self.assertEqual(mcp_event["mcp.server.name"], "codebase-memory-mcp")
        self.assertIn(("message", "user", "Fix the failing parser test"), events)
        self.assertIn(("message", "assistant", "Running the tests first."), events)
        self.assertIn(("agent", "completed", "kiro-demo"), events)
        usage_body, _ = events[("model", "usage", "claude-test")]
        self.assertIn("0.25", usage_body)
        self.assertNotIn("private reasoning", json.dumps(self.logs()))

    def test_pending_approval_waits_until_resolved(self):
        session = self.session()
        self.append(session, prompt("user-1", 0, "Deploy"), turn_start(0, "turn-1"),
                    approval(1, "call-1", "make deploy"))
        self.assertEqual(session.run_state, "waiting")
        self.assertEqual(session.waiting_for, "Approval")
        expected = identity(session.source_id, session.turn, "request", "call-1:1")
        self.assertEqual(session.request_id, expected)
        self.assertGreater(session.request_time_ns, 0)
        stamp = session.request_time_ns
        health = session_health({"run_state": session.run_state, "waiting_for": session.waiting_for}, stamp)
        self.assertEqual(health["state"], "Waiting")
        session.poll()
        self.assertEqual((session.request_id, session.request_time_ns), (expected, stamp))

        self.append(session, resolved(2, "call-1"),
                    *tool(2, "call-1", "execute_bash", {"command": "make deploy"}, "Output:\nok\n\nExit Code: 0"))
        self.assertEqual(session.run_state, "running")
        self.assertEqual((session.waiting_for, session.request_id), ("", ""))
        actions = [attrs["agent.activity.action"] for _, attrs in self.logs()]
        self.assertIn("approval_request", actions)
        self.assertIn("interaction_resolved", actions)

    def test_parallel_approvals_keep_waiting_until_all_resolve(self):
        session = self.session()
        self.append(session, prompt("user-1", 0, "Two commands"), turn_start(0, "turn-1"),
                    approval(1, "call-a", "a"), approval(1, "call-b", "b"), resolved(2, "call-a"))
        self.assertEqual(session.run_state, "waiting")
        self.assertEqual(session.request_id, identity(session.source_id, session.turn, "request", "call-b:1"))
        self.append(session, resolved(3, "call-b"))
        self.assertEqual(session.run_state, "running")

    def test_stop_reasons_map_to_outcomes(self):
        for reason, state in (("cancelled", "interrupted"), ("error", "failed"), ("tool_use", "incomplete")):
            with self.subTest(reason=reason):
                directory = self.home / "sessions" / "ws" / ("sess_" + reason)
                directory.mkdir(parents=True)
                session = self.session(path=directory / "messages.jsonl")
                self.append(session, prompt("u-" + reason, 0, "Task"), turn_start(0, "t-" + reason),
                            turn_end(1, "t-" + reason, reason))
                self.assertEqual(session.run_state, state)
                self.assertEqual(session.waiting_for, "")

    def test_steering_message_stays_in_current_turn(self):
        session = self.session()
        self.append(session, prompt("user-1", 0, "Write the parser"), turn_start(0, "turn-1"))
        turn = session.turn
        self.append(session, prompt("user-2", 1, "Also add tests", source="steer"))
        self.assertEqual(session.turn, turn)
        self.assertEqual(session.run_state, "running")
        self.assertEqual(session.task_description, "Write the parser")

    def test_turn_start_without_prompt_opens_its_own_turn(self):
        session = self.session()
        self.append(session, turn_start(0, "turn-no-prompt"))
        self.assertEqual(session.turn, "turn-no-prompt")
        self.assertEqual(session.run_state, "running")

    def test_metadata_only_capture_omits_content(self):
        session = self.session(details=False)
        self.append(session, prompt("user-1", 0, "secret request text"), turn_start(0, "turn-1"),
                    approval(1, "call-1", "cat secret-file"), resolved(2, "call-1"),
                    *tool(2, "call-1", "execute_bash", {"command": "cat secret-file"}, "Output:\nhunter2\n\nExit Code: 0"),
                    turn_end(3, "turn-1"))
        self.assertEqual(session.working_directory, "")
        self.assertEqual(session.task_description, "")
        stored = json.dumps(self.logs()) + json.dumps(self.spans())
        for secret in ("secret request text", "secret-file", "hunter2", "Generated Session Title"):
            self.assertNotIn(secret, stored)
        self.assertEqual(session.run_state, "completed")

    def test_title_is_only_a_fallback_label(self):
        session = self.session()
        self.append(session, record("say-1", 0, {"type": "assistant", "content": "Continuing", "operationType": "Say"}))
        self.assertEqual(session.task_description, "Generated Session Title")

    def test_workers_link_to_their_spawner_and_take_its_outcome(self):
        workers = self.session_dir / "sub-executions"
        parent_path = self.session_dir / "messages.jsonl"
        parent_path.touch()
        agents = LocalAgents(self.store)
        child_path, nested_path = workers / "worker-1.jsonl", workers / "worker-2.jsonl"
        workers.mkdir()
        for path in (child_path, nested_path):
            path.touch()
        agents.sessions = {path: Session(self.store, path, "Kiro") for path in (parent_path, child_path, nested_path)}
        parent, child, nested = (agents.sessions[path] for path in (parent_path, child_path, nested_path))
        self.assertEqual(child.relationship_state, "unresolved")
        self.assertIsNone(child.parent_agent_id)

        def write(path, *rows):
            with path.open("a") as stream:
                for row in rows:
                    stream.write(json.dumps(row) + "\n")

        start = lambda second, child_id: record("start-" + child_id, second, {
            "type": "sub_agent_start", "parentExecutionId": "turn-1", "subSessionId": child_id,
            "subAgentName": "context-gatherer", "prompt": "Look around"})
        done = lambda second, child_id, status: record("done-" + child_id, second, {
            "type": "sub_agent_complete", "parentExecutionId": "turn-1", "subSessionId": child_id,
            "status": status, "response": "done"})
        write(parent_path, prompt("user-1", 0, "Plan"), turn_start(0, "turn-1"), start(1, "worker-1"))
        write(child_path, record("w1-say", 2, {"type": "assistant", "content": "Working", "operationType": "Say"}),
              start(3, "worker-2"))
        write(nested_path, *tool(4, "w2-read", "read_file", {"path": "/work/kiro-demo/a.py"}, "text", kind="read"))
        agents.poll()
        agents.poll()
        self.assertEqual(child.parent_agent_id, parent.agent_id)
        self.assertEqual(nested.parent_agent_id, child.agent_id)
        self.assertEqual(nested.relationship_state, "confirmed")
        self.assertEqual(nested.native_session_id, SESSION_ID)
        self.assertEqual(nested.run_state, "running")
        self.assertEqual(nested.task_description, "")

        write(child_path, done(5, "worker-2", "error"))
        write(parent_path, done(6, "worker-1", "success"))
        agents.poll()
        agents.poll()
        self.assertEqual(nested.run_state, "failed")
        self.assertEqual(child.run_state, "completed")
        self.assertEqual(parent.run_state, "running")
        nodes, announced = agents.hierarchy_nodes()
        self.assertEqual(announced, {})
        self.assertEqual({node["agent_id"]: node["parent_agent_id"] for node in nodes},
                         {parent.agent_id: None, child.agent_id: parent.agent_id, nested.agent_id: child.agent_id})

    def usage(self, record_id, second, credits, turn):
        return record(record_id, second, {"type": "usage_summary", "elapsedTime": 2000, "status": "success",
                                          "executionId": turn, "turnId": turn,
                                          "promptTurnSummaries": [{"unit": "credit", "unitPlural": "credits",
                                                                   "usage": credits}]})

    def wait_for_sessions(self):
        for _ in range(100):
            try:
                return self.store.sessions()
            except Exception:
                time.sleep(0.05)
        return self.store.sessions()

    def test_credits_are_recorded_per_run_and_session(self):
        session = self.session()
        self.append(session, prompt("user-1", 0, "First"), turn_start(0, "turn-1"),
                    self.usage("usage-1", 2, 0.25, "turn-1"), turn_end(2, "turn-1"),
                    prompt("user-2", 3, "Second"), turn_start(3, "turn-2"),
                    self.usage("usage-2", 5, 0.125, "turn-2"), turn_end(5, "turn-2"))
        model = [attrs for name, _, attrs in self.spans() if name == "Kiro · claude-test"]
        self.assertEqual(sorted(attrs["kiro.usage.credits"] for attrs in model), [0.125, 0.25])
        self.assertTrue(all("gen_ai.usage.input_tokens" not in attrs for attrs in model))
        row = next(item for item in self.wait_for_sessions() if item["session_id"] == SESSION_ID)
        self.assertEqual(row["credits"], 0.375)
        self.assertIs(row["credits_reported"], True)
        self.assertEqual(row["latest_run"]["credits"], 0.125)
        self.assertEqual(task_usage_label(row["latest_run"], compact=True), "0.125 credits")
        self.assertEqual(self.store.metrics()["credits"], 0.375)

    def test_inspector_credit_summary_names_run_and_session(self):
        from observatory.ui_monitoring import credit_summary
        session = {"credits": 74.34033906, "credits_reported": True, "trace_count": 3}
        self.assertEqual(credit_summary({"credits": 0.5, "credits_reported": True}, session).splitlines()[0],
                         "This run 0.5 · Session 74.3403 across 3 runs")
        self.assertEqual(credit_summary({}, session, running=True).splitlines()[0],
                         "This run reported when the turn ends · Session 74.3403 across 3 runs")
        self.assertEqual(credit_summary({}, {}), "")

    def test_runs_without_credits_are_not_zero_credit_runs(self):
        session = self.session()
        self.append(session, prompt("user-1", 0, "First"), turn_start(0, "turn-1"), turn_end(1, "turn-1"))
        row = next(item for item in self.wait_for_sessions() if item["session_id"] == SESSION_ID)
        # Unreported credits are absent, not zero, so other runtimes' records are unchanged.
        self.assertNotIn("credits", row)
        self.assertNotIn("credits", row["latest_run"])
        self.assertNotIn("credits", self.store.metrics())
        self.assertEqual(task_usage_label(row["latest_run"]), "Tokens — · Cost —")

    def test_prompt_queued_mid_turn_does_not_steal_the_running_turn(self):
        session = self.session()
        self.append(session, prompt("user-1", 0, "First task"), turn_start(0, "turn-1"),
                    approval(1, "call-1", "make"), prompt("user-2", 2, "Queued task"))
        first = session.turn
        self.assertEqual(session.run_state, "waiting")
        self.append(session, resolved(3, "call-1"),
                    *tool(3, "call-1", "execute_bash", {"command": "make"}, "Output:\nok\n\nExit Code: 0"),
                    self.usage("usage-1", 4, 0.5, "turn-1"), turn_end(4, "turn-1"))
        self.assertEqual(session.turn, first)
        self.assertEqual(session.run_state, "completed")
        self.append(session, turn_start(5, "turn-2"))
        self.assertEqual(session.turn, "user-2")
        self.assertEqual(session.run_state, "running")
        self.assertEqual(session.tasks.get("user-2"), "Queued task")
        from observatory.local_agents import identity as trace_identity
        spans = {name: attrs for name, _, attrs in self.spans()}
        with self.store._connect() as db:
            traces = {row[0]: row[1] for row in db.execute(
                "SELECT name,trace_id FROM spans WHERE service='kiro' AND name IN ('execute_bash','Kiro · claude-test')")}
            message = db.execute("""SELECT trace_id FROM logs WHERE service='kiro'
                AND json_extract(attributes,'$."agent.activity.subject"')='Queued task'""").fetchone()
        self.assertEqual(traces["Kiro · claude-test"], trace_identity(session.source_id, first))
        self.assertEqual(traces["execute_bash"], trace_identity(session.source_id, first))
        self.assertEqual(message[0], trace_identity(session.source_id, "user-2"))
        self.assertIn("agent.run", spans)

    def test_permissions_record_each_answer_and_autopilot(self):
        (self.session_dir / "session.json").write_text(json.dumps({
            "id": SESSION_ID, "title": "T", "workspacePaths": ["/work/kiro-demo"], "autopilot": True}))
        session = self.session()
        old_style = record("call-old-pending", 4, {"type": "pending_interaction", "interactionType": "tool_approval",
                                                   "toolCallId": "call-old", "question": "Write File"})
        self.append(session, prompt("user-1", 0, "Do things"), turn_start(0, "turn-1"),
                    approval(1, "call-1", "make test"), resolved(2, "call-1", "always-accept"),
                    *tool(2, "call-1", "execute_bash", {"command": "make test"}, "Output:\n\nExit Code: 0"),
                    approval(3, "call-2", "rm -rf build"), resolved(3, "call-2", "reject_once"),
                    old_style, record("call-old-resolved", 9, {"type": "interaction_resolved", "toolCallId": "call-old",
                                                               "outcome": "cancelled"}),
                    approval(10, "call-3", "deploy"))
        summary = session.kiro_permission_summary()
        self.assertIs(summary["autopilot"], True)
        self.assertEqual(summary["counts"], {"requested": 4, "allow_always": 1, "reject_once": 1, "cancelled": 1})
        latest = summary["recent"]
        self.assertEqual([entry["decision"] for entry in latest], ["pending", "cancelled", "reject_once", "allow_always"])
        self.assertEqual(latest[-1]["capability"], "")
        self.assertEqual(latest[-1]["tool"], "run_command")
        self.assertEqual(latest[-1]["subject"], "make test")
        self.assertEqual(latest[-1]["resolved_ns"] - latest[-1]["requested_ns"], 1_000_000_000)

        from observatory.ui_monitoring import permission_summary
        short, details = permission_summary({"permissions": summary})
        self.assertEqual(short, "4 requests: 1 always allowed, 1 denied, 1 cancelled, 1 waiting · autopilot on")
        self.assertIn("Always allowed run_command  make test  (answered after 1s)", details)
        self.assertIn("Denied", details)
        self.assertEqual(permission_summary({}), ("", ""))
        status = LocalAgents(self.store)
        status.sessions = {session.path: session}
        entry = next(item for item in status.status()["sessions"] if item["session_id"] == SESSION_ID)
        self.assertEqual(entry["permissions"]["counts"]["requested"], 4)

    def test_permission_subjects_follow_metadata_only_capture(self):
        session = self.session(details=False)
        self.append(session, prompt("user-1", 0, "x"), turn_start(0, "turn-1"),
                    approval(1, "call-1", "cat secret-file"), resolved(2, "call-1"))
        summary = session.kiro_permission_summary()
        self.assertEqual(summary["recent"][0]["subject"], "")
        self.assertEqual(summary["recent"][0]["decision"], "allow_once")
        self.assertNotIn("secret-file", json.dumps(summary))

    def test_mcp_configuration_names_from_user_and_project_settings(self):
        user = self.home / "settings" / "mcp.json"
        project = self.root / "project" / ".kiro" / "settings" / "mcp.json"
        for path, name in ((user, "user-server"), (project, "project-server")):
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps({"mcpServers": {name: {"command": "secret-binary", "env": {"TOKEN": "x"}}}}))
        with patch.dict(os.environ, {"KIRO_HOME": str(self.home)}):
            found = discover_mcp_configurations(project_directories=[self.root / "project"])
        kiro = [item for item in found if item["client"] == "Kiro"]
        self.assertEqual({(item["scope"], item["server"]) for item in kiro},
                         {("User", "user-server"), ("Project", "project-server")})
        self.assertNotIn("secret-binary", json.dumps(found))

    def test_resume_command_targets_top_level_sessions_only(self):
        self.assertEqual(resume_command({"runtime": "Kiro", "session_id": SESSION_ID, "working_directory": "/work/x"}),
                         "cd -- /work/x && kiro-cli chat --resume-id " + SESSION_ID)
        self.assertEqual(resume_command({"runtime": "Kiro", "session_id": "worker-1"}), "")


if __name__ == "__main__":
    unittest.main()
