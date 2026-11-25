"""Capture the read-only v0.2.8 oracle; never qualify a Rust candidate here."""
from __future__ import annotations

import argparse
import asyncio
from datetime import datetime
import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from zoneinfo import ZoneInfo

from validation_support import (application_revision, bind_application,
                                create_owned_directory, file_hash, isolated_env)

TAG_SHA = "2802d79930728bb331db1d4d718d6dea1c9c374d"
SIZES = ((120, 32), (80, 24), (60, 18))
ANCHOR = 1795521600000000000  # 2026-11-24 12:00 UTC; synthetic, not live evidence.
TAGGED_MODULES = (
    "tests.test_prompt_keyboard", "tests.test_keyboard_navigation",
    "tests.test_inspector_layout", "tests.test_session_scope_picker",
    "tests.test_terminal_themes", "tests.test_status_confidence_ui",
    "tests.test_today_outcomes",
    "tests.test_terminal_review.FeedFilterRaceTest",
)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def verify_reference(root):
    revision = application_revision(root)
    if revision["head"] != TAG_SHA or revision["dirty"]:
        raise ValueError("Oracle must be a clean checkout of the exact v0.2.8 tag")
    return revision


def fixture():
    # Reuse the tagged fake API/Feed rather than inventing a Snapshot contract.
    from tests.test_inspector_layout import prompt
    from tests.test_terminal_search import ManualFeed
    clock = SimpleNamespace(time_ns=lambda: ANCHOR)
    with patch("tests.test_terminal_search.time", clock), patch("tests.test_inspector_layout.time", clock):
        feed = ManualFeed()
        feed.api.activity.extend(dict(prompt(i), detail=(
            "<environment_context>synthetic workspace</environment_context>\n\n"
            f"Request body {i}\n    indented instructions\n" + "Recorded instruction line.\n" * 50
        )) for i in range(1, 4))
    feed.refresh()
    return feed


def clock_matrix():
    from observatory.session_status import local_day_start_ns, today_bucket
    original_zone = os.environ.get("TZ")
    rows = []
    try:
        for zone in ("UTC", "America/New_York", "Europe/Berlin"):
            os.environ["TZ"] = zone
            time.tzset()
            for date in ("2026-03-08", "2026-03-29", "2026-10-25", "2026-11-01", "2026-11-24"):
                now = int(datetime.fromisoformat(date + "T12:00:00+00:00").timestamp() * 10**9)
                expected = int(datetime.fromtimestamp(now / 10**9, ZoneInfo(zone)).replace(
                    hour=0, minute=0, second=0, microsecond=0).timestamp() * 10**9)
                actual = local_day_start_ns(now)
                assert actual == expected, (zone, date, actual, expected)
                cases = [
                    (dict(state="Working", stamp=expected - 1, active=True), "running"),
                    (dict(state="Waiting", stamp=expected - 1, waiting_for="Approval"), "attention"),
                    (dict(state="Waiting", stamp=expected - 1, waiting_for="Approval", _workflow_reviewed=True), ""),
                    (dict(state="Completed", stamp=expected), "review"),
                    (dict(state="Completed", stamp=expected - 1), ""),
                    (dict(state="Error", stamp=expected - 1), ""),
                    (dict(state="Completed", stamp=now, _workflow_reviewed=True), ""),
                ]
                for item, bucket in cases:
                    assert today_bucket(item, now) == bucket, (zone, date, item)
                rows.append(dict(zone=zone, now_ns=now, day_start_ns=actual,
                                 expected_day_start_ns=expected, bucket_assertions=len(cases)))
    finally:
        if original_zone is None:
            os.environ.pop("TZ", None)
        else:
            os.environ["TZ"] = original_zone
        time.tzset()
    return rows


async def captures(output):
    from observatory.terminal_ui import ObservatoryApp
    from textual.widgets import Static
    records = []
    for width, height in SIZES:
        feed = fixture()
        app = ObservatoryApp(feed, persist_preferences=False)
        prefix = f"{width}x{height}"
        async with app.run_test(size=(width, height)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            app.set_agent_filter("all")
            app.select_agent("source-1")
            app.query_one("#agent-table").focus()

            async def save(label, keys=()):
                await pilot.pause()
                name = f"{prefix}-{label}.svg"
                (output / name).write_text(app.export_screenshot(title=f"Python v0.2.8 · {prefix} · {label}"))
                record = dict(size=[app.size.width, app.size.height], label=label, keys=list(keys),
                    source=app.selected_source, prompt=app.session_prompts.selected,
                    mode=app.session_mode, focus=getattr(app.focused, "id", None),
                    screen=type(app.screen).__name__, artifact=name,
                    sha256=file_hash(output / name))
                if app.screen is app.screen_stack[0]:
                    detail = app.query_one("#agent-detail")
                    record.update(reader=detail.plain_text, scroll_y=detail.scroll_y,
                                  reader_region=list(detail.region),
                                  hints=app.query_one("#shortcuts", Static).render().plain)
                records.append(record)
                write_json(output / "captures.json", records)

            await save("roster")
            await pilot.press("enter", "right", "enter")
            await app.workers.wait_for_complete()
            await pilot.pause()
            navigator = "agent-prompts" if width >= 120 else "agent-prompt-select"
            assert app.focused.id == navigator, (prefix, app.focused)
            newest = app.session_prompts.selected
            await save("prompts-latest", ("enter", "right", "enter"))
            if width >= 120:
                await pilot.press("up", "enter")
            else:
                await pilot.press("enter", "up", "enter")
            await pilot.pause()
            selected = app.session_prompts.selected
            assert selected != newest and app.focused.id == "agent-detail"
            assert "environment_context" not in app.query_one("#agent-detail").plain_text
            await save("older-readable")
            await pilot.press("z")
            await pilot.pause()
            assert "<environment_context>" in app.query_one("#agent-detail").plain_text
            await save("older-original", ("z",))
            await pilot.press("f1")
            await save("help", ("f1",))
            await pilot.press("escape")
            assert app.session_prompts.selected == selected and app.focused.id == "agent-detail"
            await pilot.press("escape")
            assert app.selected_source == "source-1" and app.focused.id == "agent-table"
            await pilot.press("ctrl+p")
            await save("palette", ("ctrl+p",))
            await pilot.press("escape")
            assert app.focused.id == "agent-table"
            # Resizing is an actual Pilot event, not a direct layout method call.
            await pilot.press("enter", "enter")
            await app.workers.wait_for_complete()
            await pilot.resize_terminal(60, 18)
            await pilot.pause()
            assert app.selected_source == "source-1" and app.session_mode == "prompts"
            assert app.focused.id == "agent-prompt-select"
            await save("resized-prompts", ("resize 60x18",))
            await pilot.press("escape")
            await pilot.resize_terminal(width, height)
            for key, page in (("1", "agents"), ("2", "activity"), ("3", "mcp"),
                              ("4", "traces"), ("5", "host"), ("6", "metrics"), ("7", "findings")):
                await pilot.press(key)
                await pilot.pause()
                assert app.query_one("#pages").active == page
                await save(f"root-{page}", (key,))
    return records


def worker(args):
    before = verify_reference(args.reference)
    bind_application(args.reference)
    result = dict(kind="python-oracle-only", reference=before, fixture_version="uiq-tagged-manual-feed-v1",
                  harness_sha256=file_hash(__file__),
                  fixture_clock_ns=ANCHOR, python=sys.version, terminal=dict(term=os.environ["TERM"], timezone=os.environ["TZ"]),
                  packages={name: importlib.metadata.version(name) for name in
                            ("textual", "rich", "markdown-it-py", "mdit-py-plugins", "platformdirs")},
                  lock_sha256=file_hash(args.reference / "uv.lock"), captures=[], tests=None)
    write_json(args.output / "fixture.json", {name: getattr(fixture().api, name) for name in
               ("sessions", "activity", "mcp", "traces", "spans", "metric_catalog", "metric_points", "usage_series")})
    result["fixture_sha256"] = file_hash(args.output / "fixture.json")
    try:
        result["fixed_clock_matrix"] = clock_matrix()
        if args.tests:
            suite = unittest.defaultTestLoader.loadTestsFromNames(TAGGED_MODULES)
            def ids(node):
                if isinstance(node, unittest.TestSuite):
                    return [name for child in node for name in ids(child)]
                return [node.id()]
            names = ids(suite)
            with (args.output / "tagged-tests.txt").open("w") as log:
                tested = unittest.TextTestRunner(stream=log, verbosity=2).run(suite)
            result["tests"] = dict(ids=names, run=tested.testsRun,
                failures=[(case.id(), error) for case, error in tested.failures],
                errors=[(case.id(), error) for case, error in tested.errors],
                skipped=[(case.id(), reason) for case, reason in tested.skipped],
                successful=tested.wasSuccessful())
        result["captures"] = asyncio.run(captures(args.output))
    except Exception as error:
        result["capture_error"] = repr(error)
        if (args.output / "captures.json").exists():
            result["captures"] = json.loads((args.output / "captures.json").read_text())
        raise
    finally:
        result["reference_unchanged"] = verify_reference(args.reference) == before
        write_json(args.output / "results.json", result)
    return 0 if not args.tests or result["tests"]["successful"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New owned directory; never an existing output")
    parser.add_argument("--tests", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.reference = args.reference.resolve(strict=True)
    args.output = args.output.absolute()
    if args.worker:
        return worker(args)
    verify_reference(args.reference)
    create_owned_directory(args.output)
    for name in ("home", "tmp"):
        (args.output / name).mkdir(mode=0o700)
    env = isolated_env(args.output, args.reference)
    env.update(HOME=str(args.output / "home"), TMPDIR=str(args.output / "tmp"),
               PYTHONDONTWRITEBYTECODE="1", TZ="UTC", LANG="C.UTF-8")
    command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker",
               "--reference", str(args.reference), "--output", str(args.output)]
    if args.tests:
        command.append("--tests")
    return subprocess.run(command, env=env, cwd=args.output, timeout=600).returncode


if __name__ == "__main__":
    sys.exit(main())
