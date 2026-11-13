"""Snapshots distinguish a ready empty store from pending readiness and failure."""

from contextlib import redirect_stdout, redirect_stderr
import io, os
from pathlib import Path
import socket, subprocess, sys, tempfile
import unittest
from unittest.mock import patch
from observatory.terminal import run_cockpit, print_snapshot, Snapshot, CockpitError
from types import SimpleNamespace


class OnceTests(unittest.TestCase):
    def test_ready_empty_cold_start_uses_private_managed_storage_and_unknown_cost(self):
        with tempfile.TemporaryDirectory() as directory:
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            environment = {
                **os.environ,
                "XDG_STATE_HOME": directory,
                "OBSERVATORY_TOKEN": "",
            }
            result = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "observatory",
                    "cockpit",
                    "--port",
                    str(port),
                    "--no-local-agents",
                    "--once",
                ],
                capture_output=True,
                text=True,
                env=environment,
                cwd=str(Path(__file__).resolve().parents[1]),
                timeout=20,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Runs 0  Spans 0", result.stdout)
            self.assertIn("Known cost Not reported", result.stdout)
            path = Path(directory) / "agenturm" / "observatory.db"
            self.assertTrue(path.exists())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            from observatory.core import Store

            store = Store(path)
            with store._connect() as db:
                self.assertEqual(
                    float(
                        db.execute(
                            "SELECT value FROM schema_meta WHERE key='retention_days'"
                        ).fetchone()[0]
                    ),
                    14,
                )
            store.close()

    def test_readiness_deadline_returns_partial_status_without_false_zero_snapshot(
        self,
    ):
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.dict(
                os.environ, {"XDG_STATE_HOME": directory, "OBSERVATORY_TOKEN": ""}
            ),
        ):
            health = dict(
                ok=True,
                indexing={"ready": False},
                security={"authentication_required": True},
            )
            elapsed = [0]

            def clock():
                elapsed[0] += 20
                return elapsed[0]

            output = io.StringIO()
            errors = io.StringIO()
            with (
                patch("observatory.terminal.Api.get", return_value=health),
                patch("observatory.terminal.time.monotonic", side_effect=clock),
                redirect_stdout(output),
                redirect_stderr(errors),
            ):
                status = run_cockpit(once=True)
            self.assertEqual(status, 3)
            self.assertEqual(output.getvalue(), "")
            self.assertIn("incomplete", errors.getvalue())

    def test_read_failure_emits_no_empty_looking_snapshot(self):
        output = io.StringIO()
        feed = SimpleNamespace(
            refresh=lambda: None,
            snapshot=Snapshot(error="Loading sessions timed out"),
            api=SimpleNamespace(endpoint="http://localhost:14318"),
        )
        with redirect_stdout(output), self.assertRaises(CockpitError):
            print_snapshot(feed)
        self.assertEqual(output.getvalue(), "")

    def test_default_import_persists_managed_retention_but_explicit_path_does_not(self):
        import json
        from observatory.core import Store

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "synthetic.jsonl"
            source.write_text(
                json.dumps(
                    dict(
                        trace_id="a" * 32,
                        span_id="1" * 16,
                        name="agent.run",
                        start_unix_nano=1,
                        end_unix_nano=2,
                        attributes={},
                    )
                )
                + "\n"
            )
            environment = {
                **os.environ,
                "XDG_STATE_HOME": directory,
                "OBSERVATORY_TOKEN": "",
            }
            for explicit in (False, True):
                command = [sys.executable, "-m", "observatory"]
                path = (
                    root / "explicit.db"
                    if explicit
                    else root / "agenturm" / "observatory.db"
                )
                if explicit:
                    command += ["--db", str(path)]
                result = subprocess.run(
                    [*command, "import-jsonl", str(source)],
                    capture_output=True,
                    text=True,
                    env=environment,
                    timeout=10,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                store = Store(path)
                with store._connect() as db:
                    policy = db.execute(
                        "SELECT value FROM schema_meta WHERE key='retention_days'"
                    ).fetchone()
                self.assertIsNone(policy) if explicit else self.assertEqual(
                    float(policy[0]), 14
                )
                store.close()
