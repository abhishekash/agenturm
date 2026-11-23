"""Verify exact Python Review drafting, approval and reveal against a tagged receiver."""

import argparse
import copy
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import sys
import tempfile
from types import SimpleNamespace

from validate_rust_python_receiver_client import serve, stop


TRACE = "a" * 32
PROMPT = "Keep explicit receiver scope for this draft fixture"
FORGED = "UNTRUSTED_FROZEN_RECEIPT_TEXT"
OTHER = "Earlier retained prompt in the same execution"


class PolicyMonitor:
    def __init__(self):
        self.value = {"enabled": False}
        self.reads = 0
        self.revoke_at = None

    def status(self):
        self.reads += 1
        if self.revoke_at is not None and self.reads >= self.revoke_at:
            return {"enabled": False, "capture_content": False}
        return self.value


def seed(store):
    rows = [dict(trace_id=TRACE, span_id=None, time_unix_nano=stamp, service="fixture", scope_name="fixture",
                 event_name="agent.activity", body=text, severity_number=9, severity_text="INFO", resource_attributes={},
                 attributes={"session.id": "session", "agent.activity.domain": "message",
                             "agent.activity.action": "user", "agent.activity.subject": text})
            for stamp, text in ((10, OTHER), (20, PROMPT))]
    store.ingest_logs(rows)


def verify(app, root, store, server_module, collect_review, api_type):
    token = secrets.token_urlsafe(32)
    token_file = root / "receiver.token"
    token_file.write_text(token + "\n")
    token_file.chmod(0o600)
    state = root / "rust.sqlite"
    span = root / "seed.jsonl"
    span.write_text(json.dumps(dict(trace_id="b" * 32, span_id="c" * 16,
                                    start_unix_nano="1", end_unix_nano="2", status="OK")) + "\n")
    environment = {**os.environ, "HOME": str(root), "XDG_STATE_HOME": str(root / "state")}

    def run(*args, expected=0):
        process = subprocess.run([app, *args], env=environment, capture_output=True, text=True, timeout=20)
        assert process.returncode == expected, (args[:2], process.returncode, process.stderr)
        assert token not in process.stdout + process.stderr and FORGED not in process.stdout + process.stderr
        if expected:
            assert not process.stdout
            return process.stderr
        return process.stdout

    run("import-jsonl", "--db", state, "--file", span, "--source-id", "otlp.fixture", "--session-id", "fixture")
    monitor = PolicyMonitor()
    store.local_agent_monitor = monitor
    server = server_module.BoundedHTTPServer(("127.0.0.1", 0), server_module.handler_for(store, token, quiet=True))
    thread = serve(server)
    endpoint = f"http://127.0.0.1:{server.server_port}"
    auth = ("--receiver-protocol", "python-v0.2.8", "--endpoint", endpoint, "--token-file", token_file)

    def task(verb, *args, remote=False, expected=0):
        return run("task", verb, *args, "--db", state, *(auth if remote else ()), expected=expected)

    def receipt_from_receiver():
        return collect_review(api_type(endpoint, token), service="fixture", session_id="session", trace_id=TRACE)

    try:
        source_review = receipt_from_receiver()
        assert PROMPT in source_review["request"]["text"]
        assert source_review["request"]["source"]["time_unix_nano"] == 20
        receipt = root / "review.json"
        user_selection = source_review["request"]["source"]["id"]
        source_review["request"]["text"] = FORGED
        receipt.write_text(json.dumps(source_review))
        existing = json.loads(task("add", "--title", "Authored title", "--notes", "Authored note"))
        task_id = str(existing["id"])
        draft = root / "draft.json"
        output = task("draft", "--review-file", receipt, "--event-id", user_selection,
                      "--task-id", task_id, "--output", draft, remote=True)
        assert PROMPT not in output and stat.S_IMODE(draft.stat().st_mode) == 0o600
        data = json.loads(draft.read_text())
        assert data["excerpt"] == PROMPT and data["source"]["scope"]["run_id"] == TRACE
        assert "anchor" not in data["source"] and token not in draft.read_text()
        task("draft", "--review-file", receipt, "--output", draft, remote=True, expected=2)
        clone = root / "forged.json"
        forged = copy.deepcopy(data)
        forged["excerpt"] = "Modified retained source excerpt"
        clone.write_text(json.dumps(forged))
        task("approve-draft", clone, remote=True, expected=2)

        data["continuation"] = "Operator reviewed the next action"
        draft.write_text(json.dumps(data))
        approved = json.loads(task("approve-draft", draft, remote=True))
        approval_id = str(approved["approval"]["id"])
        assert approved["task"]["title"] == "Authored title" and approved["task"]["notes"] == "Authored note"
        assert approved["task"]["anchor"] is None and approved["task"]["revision"] > existing["revision"]
        task("approve-draft", draft, remote=True, expected=3)
        assert PROMPT not in task("continuity", task_id)
        task("continuity", task_id, "--approval-id", approval_id, expected=2)
        assert json.loads(task("continuity", task_id, "--approval-id", approval_id, remote=True))["excerpt"] == PROMPT
        assert PROMPT.encode() not in Path(str(state) + ".workspace.sqlite").read_bytes()

        raced = root / "raced.json"
        monitor.revoke_at = monitor.reads + 3
        denied = task("draft", "--review-file", receipt, "--output", raced, remote=True, expected=2)
        assert "capture policy" in denied and not raced.exists()
        task("continuity", task_id, "--approval-id", approval_id, remote=True, expected=2)
        monitor.revoke_at = None
        monitor.value = {"enabled": False, "capture_content": True,
                         "capture_policy_schema": 1, "capture_global_revision": 1}
        assert "capture policy" in task("draft", "--review-file", receipt, remote=True, expected=2)
        assert "capture policy" in task("continuity", task_id, "--approval-id", approval_id, remote=True, expected=2)
        # The tagged UI binds a new scope to the observed capture generation
        # before polling. The tagged headless helper lacks that binding step.
        from observatory.review_facts import bind_capture_policy, review_capture_policy
        from observatory.workflow_evidence import ReviewHistory, scope_for
        api = api_type(endpoint, token)
        health = api.get("/api/health")
        sample = SimpleNamespace(receiver_id=health["receiver_id"], local_status=health["local_agents"])
        item = dict(service="fixture", session_id="session", trace_id=TRACE)
        scope = scope_for(item, sample)
        item = bind_capture_policy(item, sample, scope)
        item.update(review_capture_policy(item, sample, scope))
        history = ReviewHistory(scope, capture_revision=item["capture_revision"])
        history.accept(api.get_review(scope.path()))
        fresh_review = history.projection(item)
        assert fresh_review["window"]["capture_revision"].startswith("feed-safety-v1:1:0:")
        receipt.write_text(json.dumps(fresh_review))
        fresh = json.loads(task("draft", "--review-file", receipt, remote=True))
        assert fresh["excerpt"] == PROMPT
        rust_review = json.loads(run("review", "--trace-id", TRACE, "--service", "fixture",
                                     "--session-id", "session", "--format", "json", *auth))
        assert rust_review["window"]["capture_revision"] == fresh_review["window"]["capture_revision"]
        receipt.write_text(json.dumps(rust_review))
        rust_draft = json.loads(task("draft", "--review-file", receipt, remote=True))
        assert rust_draft["excerpt"] == PROMPT
    finally:
        stop(server, thread)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--python-root", type=Path, required=True)
    args = parser.parse_args()
    app = args.bin_dir.resolve() / "agenturm"
    python_root = args.python_root.resolve(strict=True)
    sys.path.insert(0, str(python_root))
    from observatory.core import Store
    from observatory.review_cli import collect_review
    from observatory.terminal import Api
    from observatory import server
    assert Path(server.__file__).resolve().is_relative_to(python_root)
    with tempfile.TemporaryDirectory(prefix="agenturm-python-task-continuity-") as directory:
        root = Path(directory)
        store = Store(root / "python.sqlite")
        try:
            seed(store)
            verify(app, root, store, server, collect_review, Api)
        finally:
            store.close()
    print("PASS tagged Python Review task proof, private approval, replay, reveal and revocation")


if __name__ == "__main__":
    main()
