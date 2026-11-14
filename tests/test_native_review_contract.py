"""Schema-derived, synthetic native records must survive the complete evidence path.

Codex Text blocks/command-list wrappers were confirmed from schema-only local
samples; no personal text, commands, paths or identifiers are copied here.
"""

import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from observatory.core import Store
from observatory.local_agents import Session, identity
from observatory.review_facts import task_run_usage
from observatory.workflow_evidence import ReviewHistory, scope_for, check_kind
from observatory.work_receipts import freeze_receipt


class NativeReviewContractTests(unittest.TestCase):
    def test_codex_wrapped_checks_native_edits_text_and_usage_reach_export(self):
        with tempfile.TemporaryDirectory() as directory:
            store = Store(Path(directory) / "events.db")
            self.addCleanup(store.close)
            path = Path(directory) / "rollout.jsonl"
            path.write_bytes(
                (
                    Path(__file__).parent / "fixtures/native_review/codex.jsonl"
                ).read_bytes()
            )
            session = Session(store, path, "Codex")
            session.poll()
            item = dict(
                service="codex",
                agent_id=session.agent_id,
                session_id=session.session_id,
                trace_id=identity(session.source_id, session.turn),
                task=session.task_description,
                state="Completed",
            )
            item["_review_usage"] = task_run_usage(
                store.sessions(), item, store.traces()
            )
            scope = scope_for(item, SimpleNamespace(receiver_id=store.receiver_id))
            history = ReviewHistory(scope)
            history.accept(store.activity_page(trace_id=scope.run_id, limit=200))
            projection = history.projection(item)
            self.assertEqual(
                {row["path"] for row in projection["edits"]},
                {"src/evidence.py", "tests/test_evidence.py"},
            )
            self.assertEqual(len(projection["checks"]), 2)
            self.assertTrue(all(row["exit_code"] == 0 for row in projection["checks"]))
            self.assertTrue(
                all(
                    row["result"] == "passed" and row["later_edit_count"] == 2
                    for row in projection["checks"]
                )
            )
            self.assertEqual(
                projection["agent_update"]["text"],
                "Implemented validation fidelity; rerun checks after the later edit.",
            )
            self.assertEqual(
                projection["request"]["text"], "Repair validation fidelity"
            )
            self.assertEqual(projection["usage"]["values"]["input_tokens"], 123)
            for content in freeze_receipt(projection).values():
                self.assertIn("src/evidence.py", content)
                self.assertNotIn("assistant message", content)

    def test_masked_failures_and_unknown_shell_results_remain_unverified(self):
        for command in (
            "pytest || true",
            "/bin/sh -c 'pytest; echo done'",
            "pytest | cat",
            "pytest &",
            "sh -nc 'pytest'",
            "sh -n -c 'pytest'",
            "sh -c -c 'pytest'",
            "pytest # ignored && npm test",
            "pytest >(cat)",
            "pytest $'--help'",
            "npm run test --if-present",
            "npm run test --if-present=true",
            "PYTEST_ADDOPTS=--collect-only pytest",
            "npm_config_if_present=true npm run test",
        ):
            self.assertIsNone(check_kind(command), command)
        self.assertEqual(
            check_kind('/usr/bin/zsh -lc "npm run test:walkthrough"'),
            "test:walkthrough",
        )
        self.assertEqual(check_kind("git diff --check"), "git diff --check")

    def test_failed_and_chain_does_not_assign_failure_to_the_test(self):
        from observatory.workflow_evidence import check_observation

        event = dict(
            domain="shell",
            subject="/bin/sh -lc 'cd missing && pytest -q'",
            result="error",
            detail={"exit_code": 1},
            attributes={'process.command_line':"/bin/sh -lc 'cd missing && pytest -q'",'process.exit.code':1},
        )
        check = check_observation(event)
        self.assertEqual(check["result"], "unknown")
        self.assertTrue(check["compound"])
        self.assertIn("individual failure unavailable", check["result_basis"])
        self.assertEqual(
            check_observation(
                {**event, "detail": {"exit_code": 0}, "result": "success",
                 'attributes':{**event['attributes'],'process.exit.code':0}}
            )["result"],
            "passed",
        )

    def test_a_detailed_success_never_hides_a_generic_failure(self):
        scope = scope_for(
            dict(service="codex", agent_id="worker", session_id="s", trace_id="a" * 32),
            SimpleNamespace(receiver_id="r"),
        )
        common = dict(
            service="codex",
            agent_id="worker",
            session_id="s",
            trace_id="a" * 32,
            span_id="shared",
            domain="shell",
            action="execute",
            time_unix_nano=10,
        )
        history = ReviewHistory(scope)
        history.accept(
            {
                "receiver_id": "r",
                "activity": [
                    {
                        **common,
                        "id": "one",
                        "subject": "pytest -q",
                        "result": "success",
                    },
                    {
                        **common,
                        "id": "two",
                        "subject": "Shell command",
                        "result": "error",
                    },
                ],
            }
        )
        self.assertEqual(len(history.projection({})["errors"]), 1)

    def test_failed_file_change_is_an_attempt_not_a_successful_edit(self):
        scope = scope_for(
            dict(service="codex", agent_id="worker", session_id="s", trace_id="a" * 32),
            SimpleNamespace(receiver_id="r"),
        )
        history = ReviewHistory(scope)
        history.accept(
            {
                "receiver_id": "r",
                "activity": [
                    dict(
                        id="failed-edit",
                        service="codex",
                        agent_id="worker",
                        session_id="s",
                        trace_id="a" * 32,
                        domain="file",
                        action="update",
                        subject="denied.py",
                        result="denied",
                        time_unix_nano=10,
                    )
                ],
            }
        )
        projected = history.projection({})
        self.assertEqual(projected["edits"][0]["result"], "denied")
        self.assertEqual(projected["edits"][0]["effect"], "attempt; effect unknown")

    def test_authenticated_http_cli_review_and_saved_task_link_compose(self):
        import os, subprocess, sys, threading
        from observatory.server import BoundedHTTPServer, handler_for
        from observatory.saved_tasks import TaskStore

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            store = Store(root / "events.db")
            self.addCleanup(store.close)
            path = root / "rollout.jsonl"
            path.write_bytes(
                (
                    Path(__file__).parent / "fixtures/native_review/codex.jsonl"
                ).read_bytes()
            )
            session = Session(store, path, "Codex")
            session.poll()
            server = BoundedHTTPServer(
                ("127.0.0.1", 0), handler_for(store, "synthetic", quiet=True)
            )
            threading.Thread(target=server.serve_forever, daemon=True).start()
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
            environment = {
                **os.environ,
                "XDG_STATE_HOME": directory,
                "OBSERVATORY_TOKEN": "synthetic",
            }
            result = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "observatory",
                    "review",
                    "--endpoint",
                    f"http://127.0.0.1:{server.server_port}",
                    "--service",
                    "codex",
                    "--agent-id",
                    session.agent_id,
                    "--session-id",
                    session.session_id,
                    "--trace-id",
                    identity(session.source_id, session.turn),
                    "--format",
                    "json",
                ],
                capture_output=True,
                text=True,
                env=environment,
                timeout=15,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            projection = json.loads(result.stdout)
            self.assertEqual(len(projection["checks"]), 2)
            self.assertEqual(len(projection["edits"]), 2)
            self.assertEqual(projection["usage"]["values"]["input_tokens"], 123)
            evidence = root / "review.json"
            evidence.write_text(result.stdout)
            tasks = TaskStore(root / "tasks.db")
            task = tasks.create("Native evidence recovery")
            result = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "observatory",
                    "task",
                    "link",
                    task["id"],
                    "--tasks-db",
                    str(tasks.path),
                    "--evidence-file",
                    str(evidence),
                    "--json",
                ],
                capture_output=True,
                text=True,
                env=environment,
                timeout=15,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            linked = tasks.get(task["id"])["links"][0]
            self.assertEqual(linked["review_snapshot"], projection)
            self.assertEqual(linked["usage"]["input_tokens"], 123)

    def test_known_output_clipping_is_visible_without_disabling_more_pages(self):
        scope = scope_for(
            dict(service="codex", agent_id="worker", session_id="s", trace_id="a" * 32),
            SimpleNamespace(receiver_id="r"),
        )
        history = ReviewHistory(scope)
        history.accept(
            {
                "receiver_id": "r",
                "next_cursor": "next-page",
                "activity": [
                    dict(
                        id="clipped",
                        service="codex",
                        agent_id="worker",
                        session_id="s",
                        trace_id="a" * 32,
                        domain="shell",
                        action="execute",
                        subject="pytest -q",
                        result="success",
                        detail="output excerpt",
                        detail_clipped=True,
                        time_unix_nano=10,
                    )
                ],
            }
        )
        self.assertEqual(history.cursor, "next-page")
        self.assertTrue(
            any(
                "clipped" in reason
                for reason in history.projection({})["window"]["reasons"]
            )
        )
