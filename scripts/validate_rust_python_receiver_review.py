#!/usr/bin/env python3
"""Compare Rust Review with the tagged Python receiver using disposable evidence."""

import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import threading

from observatory.review_cli import collect_review
from observatory.server import BoundedHTTPServer, handler_for
from observatory.terminal import Api
from tests.test_current_native_schema import CurrentOpenCodeBoundaryTests


class Policy:
    def __init__(self, owner):
        self.owner = owner
        self.capture = True
        self.generation = 1

    def status(self):
        return {
            "enabled": True,
            "capture_content": True,
            "capture_policy_schema": 1,
            "capture_global_revision": 1,
            "sessions": [{
                "service": self.owner["producer"],
                "agent_id": self.owner["agent_id"],
                "session_id": self.owner["session_id"],
                "capture_content": self.capture,
                "capture_revision": self.generation,
            }],
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    fixture = CurrentOpenCodeBoundaryTests()
    fixture.setUp()
    try:
        fixture.poll()
        trace = fixture.trace()
        turn = fixture.store.review_turn(trace)
        owner = turn["scope_owner"]
        token = "synthetic-review-token-for-tagged-test-only"
        token_file = fixture.root / "receiver.token"
        token_file.write_text(token + "\n", encoding="utf-8")
        token_file.chmod(0o600)
        handler = handler_for(fixture.store, token, quiet=True)
        with BoundedHTTPServer(("127.0.0.1", 0), handler) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                endpoint = f"http://127.0.0.1:{server.server_port}"
                api = Api(endpoint, token)
                binary = args.binary.resolve()
                common = [
                    str(binary), "review", "--receiver-protocol", "python-v0.2.8",
                    "--endpoint", endpoint, "--token-file", str(token_file),
                    "--service", "opencode", "--format", "json",
                    "--agent-id", owner["agent_id"], "--session-id", owner["session_id"],
                    "--pages", "5",
                ]
                env = dict(os.environ, HOME=str(fixture.root), XDG_STATE_HOME=str(fixture.root / "state"))

                def run(selector, *extra):
                    return subprocess.run(
                        [*common, selector, trace, *extra], env=env,
                        capture_output=True, check=False, timeout=20,
                    )

                def read(result):
                    assert result.returncode == 0, result.stderr.decode()
                    receipt = json.loads(result.stdout)
                    assert receipt["format"] == "agenturm-review-v1"
                    assert receipt["scope"]["receiver"] == fixture.store.receiver_id
                    return receipt

                def oracle(whole_turn):
                    return collect_review(
                        api, service="opencode", agent_id=owner["agent_id"],
                        session_id=owner["session_id"], trace_id=trace,
                        whole_turn=whole_turn, pages=5,
                    )

                result = run("--turn-of-trace", "-o", str(fixture.root / "frozen.json"))
                receipt = read(result)
                python = oracle(True)
                assert receipt["scope"] == python["scope"]
                assert receipt["window"]["count"] == python["window"]["count"]
                assert receipt["request"] == python["request"]
                assert receipt["agent_update"] == python["agent_update"]
                assert receipt["usage"]["values"]["input_tokens"] == python["usage"]["values"]["input_tokens"] == 17
                assert len(receipt["usage"]["step_trace_ids"]) == 2
                assert receipt["checks"][0]["command"] == python["checks"][0]["command"] == "pytest tests -q"
                assert receipt["checks"][0]["exit_code"] == 0
                assert receipt["checks"][0]["result"] == "passed"
                assert "An unrelated following request" not in result.stdout.decode()
                for suffix in ["json", "md", "html"]:
                    path = fixture.root / f"frozen.{suffix}"
                    assert stat.S_IMODE(path.stat().st_mode) == 0o600
                    assert path.is_file()
                assert json.loads((fixture.root / "frozen.json").read_text()) == receipt
                assert "default-src 'none'" in (fixture.root / "frozen.html").read_text()
                rerun = run("--turn-of-trace", "-o", str(fixture.root / "frozen.json"))
                assert rerun.returncode != 0 and rerun.stdout == b""
                assert json.loads((fixture.root / "frozen.json").read_text()) == receipt

                step_result = run("--trace-id")
                step = read(step_result)
                python_step = oracle(False)
                assert step["scope"] == python_step["scope"]
                assert step["scope"]["turn_id"] == ""
                assert step["usage"]["step_trace_ids"] == []
                assert step["window"]["count"] == python_step["window"]["count"]
                assert "Current turn implementation complete" not in step_result.stdout.decode()

                policy = Policy(owner)
                fixture.store.local_agent_monitor = policy
                activity = fixture.store.activity_page
                activity_reads = 0

                def advance_cache_version():
                    # The earlier successful review populated the receiver's
                    # actual response cache. A disposable SQLite commit makes
                    # the next activity read reach the revocation fixture.
                    with fixture.store._connect() as database:
                        database.execute("CREATE TABLE IF NOT EXISTS review_fixture_ticks (tick INTEGER)")
                        database.execute("INSERT INTO review_fixture_ticks VALUES (1)")

                def revoke(*positional, **keyword):
                    nonlocal activity_reads
                    activity_reads += 1
                    page = activity(*positional, **keyword)
                    policy.capture = False
                    policy.generation += 1
                    return page

                fixture.store.activity_page = revoke
                advance_cache_version()
                revoked = run("--turn-of-trace", "-o", str(fixture.root / "revoked.json"))
                assert revoked.returncode != 0 and revoked.stdout == b""
                assert b"capture policy changed" in revoked.stderr
                assert b"Also preserve the pending request boundary" not in revoked.stderr
                assert not list(fixture.root.glob("revoked.*"))
                assert not list(fixture.root.glob(".agenturm-review-*.pending"))
                assert activity_reads == 1
                denied = read(run("--turn-of-trace"))
                assert activity_reads == 1
                assert denied["request"] is None and denied["agent_update"] is None
                assert denied["window"]["count"] == 0
                assert "Metadata-only capture" in denied["window"]["reasons"]

                policy.capture = True
                policy.generation += 1

                def repeat(*positional, **keyword):
                    page = activity(*positional, **{**keyword, "cursor": None})
                    if keyword.get("cursor"):
                        page["review_context"] = []
                    page["next_cursor"] = "same-opaque-cursor"
                    return page

                fixture.store.activity_page = repeat
                advance_cache_version()
                repeated = run("--turn-of-trace")
                assert repeated.returncode != 0 and repeated.stdout == b""
                assert b"repeated a Review activity cursor" in repeated.stderr

                fixture.store.activity_page = activity
                token_file.write_text("incorrect-review-token-for-tagged-test-only\n", encoding="utf-8")
                unauthorized = run("--trace-id")
                assert unauthorized.returncode != 0 and unauthorized.stdout == b""
                assert b"denied the bearer token" in unauthorized.stderr

                print("PASS tagged Python 0.2.8 Review step, OpenCode turn, frozen files, no overwrite, policy revocation, metadata-only, cursor and bearer checks")
            finally:
                server.shutdown()
                worker.join(timeout=5)
    finally:
        fixture.doCleanups()


if __name__ == "__main__":
    main()
