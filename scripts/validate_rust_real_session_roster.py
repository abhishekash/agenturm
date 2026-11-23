"""Read native session metadata and qualify Agenturm's real default cockpit."""

from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import sqlite3
import struct
import subprocess
import tempfile
import termios
import time
from uuid import UUID, uuid4

from validate_rust_private_eval_details import TerminalCells

PROVIDERS = ("codex", "claude")
IDS = re.compile(r"[A-Za-z0-9._:/-]{1,256}")
MAX_HEADER = 256 * 1024
MAX_HEADER_ROWS = 16
MAX_HISTORICAL_PASSES = 64
HISTORICAL_DEADLINE = 90


class ValidationError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ValidationError(message)


def safe_id(value):
    return isinstance(value, str) and IDS.fullmatch(value) is not None


def uuid(value):
    try:
        return str(UUID(value)) == value
    except (ValueError, TypeError, AttributeError):
        return False


@dataclass(frozen=True)
class Native:
    session: str
    kind: str
    project: str | None


def native_header(provider, path):
    try:
        with path.open("rb") as stream:
            for _ in range(MAX_HEADER_ROWS if provider == "claude" else 1):
                line = stream.readline(MAX_HEADER + 1)
                if not line or len(line) > MAX_HEADER:
                    break
                record = json.loads(line)
                if not isinstance(record, dict):
                    continue
                if provider == "codex":
                    if record.get("type") != "session_meta":
                        break
                    value = record.get("payload")
                    if not isinstance(value, dict) or not uuid(value.get("id")):
                        break
                    source = value.get("source")
                    subagent = source.get("subagent") if isinstance(source, dict) else None
                    reviewer = (subagent == "review" or value.get("model") == "codex-auto-review"
                                or value.get("thread_source") == "guardian_review"
                                or isinstance(subagent, dict) and subagent.get("other") == "guardian")
                    kind = "review" if reviewer else "subagent" if subagent is not None else "session"
                    return Native(value["id"], kind, value.get("cwd"))
                identity = record.get("sessionId")
                if uuid(identity):
                    kind = "subagent" if "subagents" in path.parts or record.get("isSidechain") is True else "session"
                    return Native(identity, kind, record.get("cwd"))
    except (OSError, UnicodeError, ValueError):
        pass
    return None


def roots(args):
    return {"codex": args.codex_home / "sessions", "claude": args.claude_home / "projects"}


def file_candidates(root):
    if not root.is_dir():
        return {}
    paths = {}
    for path in root.rglob("*.jsonl"):
        if path.is_symlink() or not path.is_file():
            continue
        key = path.stem[-36:]
        if uuid(key):
            paths.setdefault(key, []).append(path)
    return paths


def inspect_existing(args):
    path = args.db.resolve(strict=True)
    wal, shm = [path.with_name(path.name + suffix) for suffix in ("-wal", "-shm")]
    require(not wal.exists() or shm.is_file(), "A live WAL without an existing shared index requires a separate snapshot")
    inventory = {provider: file_candidates(root) for provider, root in roots(args).items()}
    with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=3) as db:
        db.execute("PRAGMA query_only=1")
        schema = db.execute("PRAGMA user_version").fetchone()[0]
        items = db.execute(
            "SELECT source_id, session_id, count(*), max(observed_at_unix_ns) "
            "FROM agenturm_observations WHERE scope='session' GROUP BY source_id, session_id "
            "ORDER BY max(observed_at_unix_ns) DESC LIMIT 256"
        ).fetchall()
        cursors = db.execute("SELECT count(*) FROM agenturm_source_state").fetchone()[0]
    print(f"History schema {schema}, {len(items)} recent source/session pairs, {cursors} source cursors")
    cutoff = time.time_ns() - 900 * 10**9
    for source, session, count, stamp in items:
        require(safe_id(source) and safe_id(session), "History contains an unexpected identifier")
        provider = source.split(".")[1] if source.startswith("harness.") else ""
        files = inventory.get(provider, {}).get(session, ())
        hints = {header.kind for f in files if (header := native_header(provider, f)) is not None}
        kind = next(iter(hints)) if len(hints) == 1 else "ambiguous" if hints else "unverified"
        print(f"{source} {session} native={kind} rows={count} updated-within-15m={stamp >= cutoff}")


def run(app, env, cwd, *args, allowed=(0,), timeout=45):
    command = subprocess.run([app, *args], env=env, cwd=cwd, capture_output=True, timeout=timeout)
    require(command.returncode in allowed, f"Agenturm {args[0]} exited with status {command.returncode}")
    return command.stdout


def environment(args, work):
    # Use only isolated writable state/config. The source roots are read inputs
    # and no inherited OTLP port, adapter grant, or trace configuration is used.
    return {
        "HOME": str(work), "XDG_STATE_HOME": str(work / "state"),
        "XDG_CONFIG_HOME": str(work / "config"), "XDG_DATA_HOME": str(work / "data"),
        "CODEX_HOME": str(args.codex_home), "CLAUDE_CONFIG_DIR": str(args.claude_home),
        "AGENTURM_OTLP_PORT": "0", "PATH": os.defpath,
        "LANG": "C.UTF-8", "TERM": "xterm-256color", "NO_COLOR": "1",
    }


def discovered(app, env, work, selection=()):
    config = json.loads(run(app, env, work, "discover", *selection))
    entries = []
    for entry in config.get("sources", []):
        provider = next((name for name in PROVIDERS if entry.get("plugin_id") == f"example.rust-{name}-harness"), None)
        if provider is None:
            continue
        path = Path(entry["source_root"])
        header = native_header(provider, path)
        if header is None:
            entries.append((provider, None, entry["source_id"], path))
        else:
            entries.append((provider, header, entry["source_id"], path))
    return config, entries


def summary(app, env, work, db=None):
    args = ("cockpit", "--once", "--json", "--limit", "256")
    if db is not None:
        args += ("--db", db)
    result = json.loads(run(app, env, work, *args, timeout=8 if db is not None else 45))
    require(result.get("format") == "agenturm-summary/1", "Unexpected session summary format")
    require(not result.get("has_more"), "Session summary is partial")
    return result["sessions"]


def audit(args, app, env, work):
    config, entries = discovered(app, env, work, ("--sessions-per-harness", str(args.cap)))
    classes = Counter((provider, header.kind if header is not None else "unverified") for provider, header, _, _ in entries)
    for (provider, kind), count in sorted(classes.items()):
        print(f"Discovered {provider} {kind}: {count}")
    config_path, db = work / "sources.json", work / "static.sqlite"
    config_path.write_text(json.dumps(config), encoding="utf-8")
    run(app, env, work, "collect", "--config", config_path, "--db", db, allowed=(0, 1))
    for phase, rows in [("explicit", summary(app, env, work, db)), ("default", summary(app, env, work))]:
        labels = Counter(row["harness_id"].split(".")[1] for row in rows if row["harness_id"].startswith("harness."))
        duplicates = len(rows) - len({(row["harness_id"].split(".")[1], row["session_id"]) for row in rows})
        print(f"{phase} retained {dict(labels)}, duplicate native IDs across source keys {duplicates}")


class Cockpit:
    def __init__(self, app, env, work, width, height, db=None):
        self.width = width
        self.screen = TerminalCells(width, height)
        self.master, self.slave = pty.openpty()
        try:
            fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
            self.modes = termios.tcgetattr(self.slave)
            command = [app] if db is None else [app, "cockpit", "--db", db]
            self.process = subprocess.Popen(command, cwd=work, env=env, stdin=self.slave,
                                            stdout=self.slave, stderr=self.slave)
        except BaseException:
            os.close(self.master)
            os.close(self.slave)
            raise

    def __enter__(self):
        return self

    def __exit__(self, *exception):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)

    def receive(self):
        if select.select([self.master], [], [], .05)[0]:
            self.screen.feed(os.read(self.master, 8192))

    def until(self, predicate, phase, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.receive()
            if predicate(self.screen.contents()):
                return
            require(self.process.poll() is None, f"Cockpit exited during {phase}")
        raise ValidationError(f"Cockpit did not reach {phase} at width {self.width}")

    def key(self, data):
        os.write(self.master, data)

    def markers(self, phase, *labels):
        self.until(lambda frame: all(label in frame for label in labels), phase)

    def quit(self):
        self.key(b"q")
        require(self.process.wait(timeout=10) == 0, "Cockpit quit failed")
        require(termios.tcgetattr(self.slave) == self.modes, "Cockpit did not restore terminal modes")


def process_holds(row):
    process = Path("/proc") / str(row["pid"])
    try:
        expected_name = row["harness"]
        comm, exe = (process / "comm").read_text().strip(), os.readlink(process / "exe")
        filename = Path(exe.removesuffix(" (deleted)")).name
        claimed = ("claude", "claude-code") if expected_name == "claude" else ("codex",)
        if comm not in claimed or not (filename in claimed or filename.startswith(f"{expected_name}-")):
            return False
        path = row["resolved"].stat()
        expected_file = (path.st_dev, path.st_ino)
        if expected_file != (row.get("device", path.st_dev), row.get("inode", path.st_ino)):
            return False
        for link in (process / "fd").iterdir():
            try:
                opened = link.stat()
                if (opened.st_dev, opened.st_ino) == expected_file:
                    return True
            except FileNotFoundError:
                continue
    except OSError:
        return False
    return False


def validate_expected_source(row, root, kind):
    require(uuid(row["session"]), "Expected metadata needs a valid native session ID")
    path = (root / row["path"]).resolve(strict=True)
    require(path.is_relative_to(root) and path.is_file(),
            "Expected source must be a regular file under its provider root")
    header = native_header(row["harness"], path)
    require(header is not None and header.session == row["session"] and header.kind == kind,
            f"Selected native header does not match {row['harness']} {row['session']}")
    row["resolved"] = path
    # Discovery's current contract is a path-bound source key. Retain both
    # source and native session ID when independently validating admission.
    digest = hashlib.sha256(os.fsencode(path)).hexdigest()[:32]
    row["source"] = f"harness.{row['harness']}.{digest}"


def validate_expected_display(row):
    require(isinstance(row["title"], str) and bool(row["title"].strip()),
            "Supply an independently sourced title")
    require(Path(row["project"]).is_absolute(), "Supply an independently sourced project path")
    require(row.get("status") in {None, "Working", "Ready", "Inactive", "Needs you", "Paused", "Unknown"},
            "Unexpected expected roster status")


def load_expected(args):
    expected = json.loads(args.expected.read_text(encoding="utf-8"))
    keep, exclude = expected["sessions"], expected["excluded"]
    by_provider = roots(args)
    require(5 <= len(keep) <= 20, "Provide more than four and at most 20 independently selected sessions")
    require(exclude and {row["kind"] for row in exclude} == {"review"}, "Include a guardian review negative control")
    require(any(row.get("kind") == "subagent" for row in keep), "Include a regular native subagent positive control")
    for row in keep + exclude:
        provider = row["harness"]
        root = by_provider[provider].resolve(strict=True)
        validate_expected_source(row, root, row.get("kind", "session"))
    keys = {(row["source"], row["session"]) for row in keep}
    require(len(keys) == len(keep), "Expected sessions must have unique exact source/native ID pairs")
    live = [row for row in keep if isinstance(row.get("pid"), int) and not isinstance(row.get("pid"), bool) and row["pid"] > 0]
    if args.require_live:
        require(len(live) >= 5, "Live witness requires five distinct exact transcript/session pairs")
        require(all(process_holds(row) for row in live),
                "Native process ownership could not be verified in the current process namespace")
        print(f"Verified {len(live)} open native transcripts across {len({row['pid'] for row in live})} processes")
    for row in keep:
        validate_expected_display(row)
    return keep, exclude


def bind_expected(rows, excluded, entries):
    forbidden = {row["resolved"] for row in excluded}
    require(not any(path.resolve() in forbidden for _, _, _, path in entries), "Discovery selected a guardian review")
    by_path = {path.resolve(): (provider, header, source) for provider, header, source, path in entries}
    for row in rows:
        selected = by_path.get(row["resolved"])
        if selected is not None:
            provider, header, source = selected
            require(provider == row["harness"] and header is not None and header.session == row["session"]
                    and source == row["source"], f"Wrong discovery binding for {row['session']}")


def wait_for_admission(app, env, work, db, cockpit, rows, excluded, timeout):
    expected = {(row["source"], row["session"]) for row in rows}
    forbidden = {(row["source"], row["session"]) for row in excluded}
    deadline = time.monotonic() + timeout
    missing = expected
    while time.monotonic() < deadline:
        cockpit.receive()
        if not db.exists():
            continue
        try:
            retained = summary(app, env, work, db)
        except (ValidationError, subprocess.TimeoutExpired):
            time.sleep(.1)
            continue
        admitted = {(row["harness_id"], row["session_id"]) for row in retained}
        require(not (admitted & forbidden), "A guardian review created a durable session")
        missing = expected - admitted
        if not missing:
            return
        time.sleep(.2)
    ids = ", ".join(sorted(session for _, session in missing))
    raise ValidationError(f"Admitted {len(expected) - len(missing)} of {len(expected)} expected sessions; missing {ids}")


def export_roster(cockpit, export_directory):
    editor = "Export loaded view"
    pending = "Wait for the current read before exporting"
    output = export_directory / "view.json"
    cockpit.key(b"x")
    cockpit.markers("metadata export editor", editor, "New directory")
    cockpit.key(os.fsencode(export_directory))

    def filled(frame):
        compact = "".join(line.strip(" │") for line in frame.splitlines())
        return editor in frame and str(export_directory) in compact

    cockpit.until(filled, "complete metadata export path")
    deadline = time.monotonic() + 10
    last_submit = float("-inf")
    while time.monotonic() < deadline:
        if output.is_file():
            cockpit.markers("metadata export completion", "Agenturm command center")
            return json.loads(output.read_text(encoding="utf-8"))
        now = time.monotonic()
        frame = cockpit.screen.contents()
        # A background roster read can reject Enter while leaving the
        # populated form open. Retrying after it closes would open a row.
        delay = .3 if pending in frame else .8
        if filled(frame) and now - last_submit >= delay:
            cockpit.key(b"\r")
            last_submit = now
        cockpit.receive()
    raise ValidationError(f"Cockpit did not save the metadata export at width {cockpit.width}")


def verify_roster(cockpit, rows, excluded, export_directory):
    def ready(frame):
        count = re.search(r"\bAll\s+(\d+)\b", frame)
        return "Agenturm command center" in frame and count is not None and int(count[1]) >= len(rows)

    cockpit.until(ready, "command center readiness after admission")
    # This disposable copy preserves exact metadata at narrow terminal widths.
    exported = export_roster(cockpit, export_directory)
    require(exported["coverage"] == "loaded_page", "Loaded roster page is partial")
    indexed = {item["label"]: item["value"] for item in exported["rows"]}
    for excluded_row in excluded:
        require(f"{excluded_row['source']} / {excluded_row['session']}" not in indexed,
                f"Guardian review is visible: {excluded_row['session']}")
    for row in rows:
        source, session = row["source"], row["session"]
        label = f"{source} / {session}"
        value = indexed.get(label, "")
        require(value.startswith(f"task={row['title']} · project={row['project']} · project_origin="),
                f"Exact roster title or project differs for {source} / {session}")
        select_human_row(cockpit, row, len(indexed))
    cockpit.quit()


def inspector_owner(frame):
    fields = {}
    for line in frame.splitlines():
        for prefix in ("Source", "Session"):
            start = f"│{prefix}  "
            if line.startswith(start):
                fields[prefix] = line[len(start):].strip(" │")
    return fields.get("Source"), fields.get("Session")


def roster_selection(frame, query):
    lines = frame.splitlines()
    if ("Agenturm command center" not in frame or "Enter / Esc done" in frame
            or not any(line.strip() == f"/ {query}" for line in lines)):
        return None
    for index, line in enumerate(lines):
        if line.lstrip().startswith("› ") and index + 1 < len(lines):
            return line, lines[index + 1]
    return None


def select_human_row(cockpit, row, max_matches):
    source, session = row["source"], row["session"]
    hint = row["title"]
    if hint == "Name unavailable":
        hint = row.get("agent") or Path(row["project"]).name
    query = hint[:28].strip()
    require(query and all(char.isprintable() for char in query), "Expected title is not a single printable line")
    cockpit.key(b"a/" + query.encode() + b"\r")
    cockpit.until(lambda frame: roster_selection(frame, query) is not None,
                  f"named roster search for {session}")
    # Search can preserve a previous selection in the middle of the results.
    # Up clamps at the first match. Rendered rows can be identical, so the
    # exact inspector owner is the only useful witness for each subsequent Down.
    cockpit.key(b"\x1b[A" * max_matches)
    visited = set()
    for _ in range(max_matches):
        cockpit.key(b"\r")
        cockpit.markers(f"inspector for named roster row {session}", "Session overview", "Recorded turns on page")
        owner = inspector_owner(cockpit.screen.contents())
        require(None not in owner, "Inspector omitted an exact source or session field")
        cockpit.key(b"\x1b")
        cockpit.until(lambda frame: roster_selection(frame, query) is not None,
                      f"return to named roster row {session}")
        if owner == (source, session):
            status = row.get("status")
            agent = row.get("agent")

            def expected_selection(frame):
                selection = roster_selection(frame, query)
                if selection is None:
                    return False
                selected, context = selection
                # The fixed-width status and Updated columns occupy the final
                # 21 cells, plus one outer margin on wide layouts.
                end = len(selected) - (10 if cockpit.width >= 80 else 9) - 1
                actual_status = selected[end - 11:end].strip()
                return (status is None or status == actual_status) and (agent is None or agent in context)

            cockpit.until(expected_selection, f"current native status and context for {session}")
            cockpit.key(b"\r")
            cockpit.markers(f"reopen after Esc for {session}", "Session overview", "Recorded turns on page")
            require(inspector_owner(cockpit.screen.contents()) == owner,
                    f"Esc changed the selected session {source} / {session}")
            cockpit.key(b"\x1b")
            cockpit.until(lambda frame: roster_selection(frame, query) is not None,
                          f"leave exact inspector for {session}")
            return
        if owner in visited:
            break
        visited.add(owner)
        cockpit.key(b"\x1b[B")
        cockpit.receive()
    raise ValidationError(f"Named roster search did not select {source} / {session}")


def verify(args, app, env, work):
    rows, excluded = load_expected(args)
    _, entries = discovered(app, env, work, ("--sessions-per-harness", "8"))
    bind_expected(rows, excluded, entries)
    db = work / "state" / "agenturm-rust" / "history.sqlite"
    for width, height in [(120, 32), (80, 24), (60, 18)]:
        # Reopen through default startup at all widths. Never substitute a DB
        # prefilled by another collector for the first-run case.
        with Cockpit(app, env, work, width, height) as cockpit:
            wait_for_admission(app, env, work, db, cockpit, rows, excluded, args.deadline)
            verify_roster(cockpit, rows, excluded, work / f"export-{width}")
        print(f"Verified {len(rows)} exact sessions, titles, projects, and selection at {width}×{height}")
    print(f"Verified {'live process witness' if args.require_live else 'recorded native roots'} with catch-up and restart")


def finish_historical_collection(app, env, work, config, db, row):
    source = row["resolved"]
    before = source.stat()
    require(before.st_size > 0, "Historical transcript is empty")
    with source.open("rb") as stream:
        stream.seek(-1, os.SEEK_END)
        require(stream.read(1) == b"\n", "The historical display fixture needs a complete final row")
    deadline = time.monotonic() + HISTORICAL_DEADLINE
    previous = None
    for cycle in range(1, MAX_HISTORICAL_PASSES + 1):
        left = deadline - time.monotonic()
        if left <= 0:
            break
        run(app, env, work, "collect", "--config", config, "--db", db, timeout=min(45, left))
        with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True, timeout=3) as store:
            record = store.execute("SELECT cursor FROM agenturm_source_state WHERE source_id=?",
                                   [row["source"]]).fetchone()
        require(record is not None, "Historical Claude source has no committed checkpoint")
        proof = record[0].split(":", 4)
        require(len(proof) == 5 and proof[0] == "v2" and all(part.isdecimal() for part in proof[1:4]),
                "Unexpected historical Claude checkpoint format")
        device, inode, offset = map(int, proof[1:4])
        after = source.stat()
        require((device, inode) == (before.st_dev, before.st_ino)
                and (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
                == (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns),
                "Historical Claude source changed during collection")
        require(offset <= before.st_size and (previous is None or offset > previous),
                "Historical Claude checkpoint did not advance")
        if offset == before.st_size:
            print(f"Verified historical Claude checkpoint at {offset} bytes after {cycle} collection passes")
            return
        previous = offset
    raise ValidationError("Historical Claude collection did not reach the complete source within its bounds")


def historical(args, app, env, work):
    row = json.loads(args.expected.read_text(encoding="utf-8"))
    require(row["harness"] == "claude", "Select one actual historical Claude transcript")
    validate_expected_source(row, roots(args)["claude"].resolve(strict=True), "session")
    row["source"] = "harness.claude.e3-historical"
    config_path, db = work / "sources.json", work / "history.sqlite"
    config_path.write_text(json.dumps({
        "version": 2, "plugin_host": str(app.with_name("agenturm-plugin-host")),
        "plugin_catalog": str(app.parent.parent / "agenturm-plugins.json"),
        "sources": [{"plugin_id": "example.rust-claude-harness", "source_id": row["source"],
                     "source_root": str(row["resolved"])}],
    }), encoding="utf-8")
    config_path.chmod(0o600)
    env = {**env, "AGENTURM_CONTENT_CONFIG": str(config_path)}
    display = any(field in row for field in ("title", "project", "status"))
    if display:
        validate_expected_display(row)
        finish_historical_collection(app, env, work, config_path, db, row)
    else:
        run(app, env, work, "collect", "--config", config_path, "--db", db)
    observed = summary(app, env, work, db)
    require([(item["harness_id"], item["session_id"]) for item in observed] == [(row["source"], row["session"])],
            "Explicit collection did not admit the exact historical Claude session")
    print(f"Verified historical Claude admission for {row['session']} in an isolated explicit store")
    if display:
        for width, height in [(120, 32), (80, 24), (60, 18)]:
            with Cockpit(app, env, work, width, height, db) as cockpit:
                verify_roster(cockpit, [row], [], work / f"historical-{width}")
            print(f"Verified historical Claude title, project and exact selection at {width}×{height}")


def mutation(args, app, env, work):
    env = {**env, "CODEX_HOME": str(work / "fixture-codex"), "CLAUDE_CONFIG_DIR": str(work / "fixture-claude")}
    codex_home = Path(env["CODEX_HOME"])
    root = codex_home / "sessions" / datetime.now().strftime("%Y/%m/%d")
    root.mkdir(parents=True)
    session = str(uuid4())
    stamp = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    leaf = f"rollout-{stamp[:19].replace(':', '-')}-{session}.jsonl"
    db = work / "state" / "agenturm-rust" / "history.sqlite"

    def transcript(label):
        project = work / f"project-{label}"
        project.mkdir()
        path = root / label / leaf
        path.parent.mkdir()
        turn = str(uuid4())
        header = {"type": "session_meta", "payload": {
            "id": session, "cwd": str(project), "source": "cli", "agent_nickname": label,
        }}
        started = {"type": "event_msg", "timestamp": stamp,
                   "payload": {"type": "task_started", "turn_id": turn}}
        path.write_text("".join(json.dumps(row) + "\n" for row in (header, started)), encoding="utf-8")
        digest = hashlib.sha256(os.fsencode(path)).hexdigest()[:32]
        return path, turn, {"source": f"harness.codex.{digest}", "session": session,
                            "project": str(project), "agent": label}

    def write_name(title):
        row = {"id": session, "thread_name": title,
               "updated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")}
        with (codex_home / "session_index.jsonl").open("a", encoding="utf-8") as stream:
            print(json.dumps(row), file=stream)

    def retained(cockpit, expected):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            cockpit.receive()
            if not db.is_file():
                continue
            records = {row["harness_id"]: row["lifecycle"]["state"]
                       for row in summary(app, env, work, db)
                       if row["session_id"] == session and row["lifecycle"]}
            if records == expected:
                return
            time.sleep(.2)
        raise ValidationError("Independent source states did not catch up")

    original, original_turn, first = transcript("Alpha")
    write_name("Fixture original name")
    with Cockpit(app, env, work, 60, 18) as cockpit:
        retained(cockpit, {first["source"]: "running"})
        cockpit.markers("first human title", "Fixture original name")
        write_name("Fixture renamed human task")
        cockpit.until(lambda frame: "Fixture renamed human task" in frame and "Fixture original name" not in frame,
                      "renamed human title")
        select_human_row(cockpit, {**first, "title": "Fixture renamed human task", "status": "Working"}, 1)
        retained(cockpit, {first["source"]: "running"})

        _, _, second = transcript("Beta")
        retained(cockpit, {first["source"]: "running", second["source"]: "running"})
        with original.open("a", encoding="utf-8") as stream:
            finished = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
            print(json.dumps({"type": "event_msg", "timestamp": finished,
                              "payload": {"type": "task_complete", "turn_id": original_turn}}), file=stream)
        independent = {first["source"]: "completed", second["source"]: "running"}
        retained(cockpit, independent)
        rows = [{**first, "title": "Name unavailable", "status": "Ready"},
                {**second, "title": "Name unavailable", "status": "Working"}]
        cockpit.key(b"a")
        cockpit.markers("both source states in the command center", "Name unavailable", "Alpha", "Beta", "Working 1", "Ready 1")
        verify_roster(cockpit, rows, [], work / "mutation-narrow")
    with Cockpit(app, env, work, 80, 24) as cockpit:
        retained(cockpit, independent)
        verify_roster(cockpit, rows, [], work / "mutation-reopened")
    print("Verified human rename, two independent sources with one native ID, catch-up and restart in disposable state")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, help="path to the installed Agenturm bundle binary")
    parser.add_argument("--codex-home", type=Path, default=Path.home() / ".codex")
    parser.add_argument("--claude-home", type=Path, default=Path.home() / ".claude")
    modes = parser.add_subparsers(dest="mode", required=True)
    read = modes.add_parser("inspect-existing", help="read identified live history metadata")
    read.add_argument("--db", type=Path, required=True)
    audit_mode = modes.add_parser("audit", help="compare discovery and explicit/default admissible sessions")
    audit_mode.add_argument("--cap", type=int, choices=range(1, 9), default=4)
    strict = modes.add_parser("verify", help="verify independently identified real sessions in a fresh cockpit")
    strict.add_argument("--expected", type=Path, required=True)
    strict.add_argument("--require-live", action="store_true")
    strict.add_argument("--deadline", type=int, choices=range(5, 181), default=120)
    retained = modes.add_parser("historical", help="admit one actual historical Claude transcript explicitly")
    retained.add_argument("--expected", type=Path, required=True)
    modes.add_parser("mutation", help="test an exact human-title rename and independent same-ID sources in disposable state")
    args = parser.parse_args()
    if args.mode != "inspect-existing" and args.app is None:
        parser.error("supply --app with the bundle binary under test")
    args.codex_home = args.codex_home.resolve()
    args.claude_home = args.claude_home.resolve()
    try:
        if args.mode == "inspect-existing":
            inspect_existing(args)
        else:
            app = args.app.resolve(strict=True)
            with tempfile.TemporaryDirectory(prefix="agenturm-real-roster-") as folder:
                work = Path(folder)
                command = {"audit": audit, "verify": verify, "historical": historical, "mutation": mutation}[args.mode]
                command(args, app, environment(args, work), work)
    except (ValidationError, sqlite3.Error, OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError) as error:
        # Errors from native/terminal output are never reflected. Validation
        # errors name an expected metadata ID or the failed acceptance phase.
        parser.exit(1, f"{error if isinstance(error, ValidationError) else type(error).__name__}\n")


if __name__ == "__main__":
    main()
