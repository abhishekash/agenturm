"""Root-only two-UID proof for feature-off real Store clone admission."""
from __future__ import annotations

from contextlib import closing
import atexit
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from validate_rust_real_clone_activation import send
from validate_rust_storage_service_accounts import check, drop_to, identities, wait_socket


_CODEX_CHILDREN: list[subprocess.Popen] = []


def _terminate_codex_child(child: subprocess.Popen) -> None:
    if child.poll() is None:
        child.kill()
    try:
        child.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        child.kill()
        child.communicate()


def _stop_codex_children() -> None:
    for child in _CODEX_CHILDREN:
        _terminate_codex_child(child)


atexit.register(_stop_codex_children)


def child_request(args) -> None:
    request = json.loads(Path(args.request).read_text())
    reply = send(Path(args.socket), request, expect_reply=not args.reject,
                 discard=args.drop_reply)
    if reply is not None and args.reply:
        Path(args.reply).write_text(json.dumps(reply, separators=(",", ":")))


def run_client(request_path: Path, socket_path: Path, *, uid: int, gid: int,
               reply_path: Path | None = None, reject: bool = False,
               drop_reply: bool = False) -> dict | None:
    if reply_path is not None:
        reply_path.unlink(missing_ok=True)
    command = [sys.executable, str(request_path.parent / Path(__file__).name),
               "--client", "--socket", str(socket_path),
               "--request", str(request_path)]
    if reply_path is not None:
        command.extend(("--reply", str(reply_path)))
    if reject:
        command.append("--reject")
    if drop_reply:
        command.append("--drop-reply")
    result = subprocess.run(command, preexec_fn=drop_to(uid, gid),
                            capture_output=True, text=True, timeout=8)
    check(result.returncode == 0, f"client: {result.stdout}{result.stderr}")
    return json.loads(reply_path.read_text()) if reply_path and reply_path.exists() else None


def child_write_frame(process: subprocess.Popen, message: dict) -> None:
    raw = json.dumps(message, separators=(",", ":")).encode()
    check(0 < len(raw) <= 4 * 1024 * 1024, "Codex child frame exceeds its bound")
    process.stdin.write(struct.pack(">I", len(raw)) + raw)
    process.stdin.flush()


def child_read_exact(stream, length: int, deadline: float) -> bytes | None:
    result = bytearray()
    while len(result) < length:
        remaining = deadline - time.monotonic()
        ready, _, _ = select.select([stream], [], [], max(0, remaining))
        check(bool(ready), "Codex child frame read timed out")
        chunk = os.read(stream.fileno(), length - len(result))
        if not chunk:
            return None if not result else bytes(result)
        result.extend(chunk)
    return bytes(result)


def child_read_frame(process: subprocess.Popen, *, timeout: float = 5) -> dict | None:
    deadline = time.monotonic() + timeout
    try:
        prefix = child_read_exact(process.stdout, 4, deadline)
        if prefix is None:
            return None
        check(len(prefix) == 4, "Codex child sent a truncated frame prefix")
        length = struct.unpack(">I", prefix)[0]
        check(0 < length <= 4 * 1024 * 1024, "Codex child frame length is invalid")
        raw = child_read_exact(process.stdout, length, deadline)
        check(raw is not None and len(raw) == length, "Codex child sent a truncated frame")
        return json.loads(raw)
    except BaseException:
        _terminate_codex_child(process)
        raise


def verify_codex_frame_timeout() -> None:
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    _CODEX_CHILDREN.append(child)
    started = time.monotonic()
    try:
        child_read_frame(child, timeout=0.05)
    except AssertionError as error:
        check("frame read timed out" in str(error),
              f"unexpected stalled-child failure: {error}")
    else:
        raise AssertionError("stalled Codex child frame read did not time out")
    check(child.poll() is not None and time.monotonic() - started < 2,
          "stalled Codex child was not killed and reaped promptly")


def new_batch(source_id: str, cursor: str):
    from observatory.plugin_wire import PollBatch
    records = (
        {"event_id": "event-start", "revision": 0, "kind": "lifecycle",
         "time_ns": "100", "payload": {"state": "started"},
         "identity": {"session_id": "session-one"}},
        {"event_id": "event-finish", "revision": 0, "kind": "lifecycle",
         "time_ns": "200", "payload": {"state": "completed"},
         "identity": {"session_id": "session-one"}},
    )
    return PollBatch(source_id=source_id, cursor=cursor, records=records)


def new_fact_batch(source_id: str, cursor: str, suffix: str = "one"):
    from observatory.plugin_wire import PollBatch
    records = (
        {"event_id": f"{suffix}-message", "revision": 0, "kind": "message",
         "time_ns": "100", "payload": {"role": "assistant"},
         "identity": {"session_id": "native-session"}},
        {"event_id": f"{suffix}-tool", "revision": 0, "kind": "tool",
         "time_ns": "110", "payload": {"category": "shell", "result": "started"},
         "identity": {"session_id": "native-session", "turn_id": "turn-one",
                      "operation_id": "operation-one"}},
        {"event_id": f"{suffix}-check", "revision": 0, "kind": "check",
         "time_ns": "120", "payload": {"category": "test", "result": "passed"},
         "identity": {"session_id": "native-session"}},
        {"event_id": f"{suffix}-usage", "revision": 0, "kind": "usage",
         "time_ns": "130", "payload": {"input_tokens": 7, "total_tokens": 7}},
    )
    return PollBatch(source_id=source_id, cursor=cursor, records=records)


def request_for(lease: dict, batch, request_id: str, expected_cursor: str,
                *, fact_kinds: bool = False) -> dict:
    from observatory.plugin_admission import _validated
    digest, rows, _ = _validated(batch, lease["source_id"])
    records = [{
        "event_id_hash": row[1],
        "revision": row[2],
        "kind": row[3] if fact_kinds else "lifecycle",
        "time_ns": row[4],
        "payload_json": row[5],
        "record_digest": row[6],
        "native_session_id": row[7],
        "native_worker_id": row[8],
        "native_turn_id": row[9],
        "native_operation_id": row[10],
    } for row in rows]
    stable_lease = {key: lease[key] for key in
                    ("receiver_id", "plugin_id", "source_id", "generation", "epoch", "policy_epoch")}
    return {"version": 2, "request_id": request_id,
            "receiver_id": lease["receiver_id"],
            "operation": {"op": "admit_facts" if fact_kinds else "admit_lifecycle",
                          "lease": stable_lease,
                          "expected_cursor": expected_cursor, "cursor": batch.cursor,
                          "batch_digest": digest, "records": records}}


def exercise_codex_fact_path(*, root: Path, codex_binary: Path, clone: Path,
                             oracle: Path, receiver: str, socket_path: Path,
                             service_uid: int, service_gid: int,
                             client_uid: int, client_gid: int) -> None:
    from observatory.core import Store
    from observatory.plugin_admission import activate_source, commit_batch
    from observatory.plugin_wire import PollBatch, ProtocolError

    plugin_id = "example.rust-codex-harness"
    source_id = "codex-facts-source"
    generation = "c" * 64
    source_path = root / "codex-session.jsonl"
    corpus = [
        {"type": "session_meta", "payload": {"id": "codex-thread-1"}},
        {"type": "response_item", "timestamp": "2026-11-14T10:00:01Z",
         "payload": {"type": "message", "role": "assistant",
                     "content": "private Codex answer"}},
        {"type": "response_item", "timestamp": "2026-11-14T10:00:02Z",
         "payload": {"type": "function_call", "call_id": "call-1",
                     "name": "functions.exec_command",
                     "arguments": "private /private/synthetic/project/file.txt"}},
        {"type": "response_item", "timestamp": "2026-11-14T10:00:03Z",
         "payload": {"type": "function_call_output", "call_id": "call-1",
                     "isError": False, "output": "private tool output"}},
        {"type": "event_msg", "timestamp": "2026-11-14T10:00:04Z",
         "payload": {"type": "item_completed", "turn_id": "turn-1",
                     "item": {"type": "CommandExecution", "id": "check-1",
                              "command": ["/usr/bin/zsh", "-lc", "npm run check"],
                              "status": "completed", "exit_code": 0,
                              "aggregated_output": "private check output"}}},
        {"type": "token_usage_record", "timestamp": "2026-11-14T10:00:05Z",
         "payload": {"turn_id": "turn-1", "response_id": "response-1",
                     "usage": {"input_tokens": 7, "output_tokens": 3,
                               "cached_input_tokens": 2,
                               "reasoning_output_tokens": 1}}},
    ]
    def write_private_source(path: Path, rows: list[dict]) -> None:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write("".join(
                json.dumps(row, separators=(",", ":")) + "\n" for row in rows))
        os.chown(path, client_uid, client_gid)

    write_private_source(source_path, corpus)

    def assert_source_private(path: Path, label: str) -> None:
        probe = subprocess.run(
            [sys.executable, "-c", "open(__import__('sys').argv[1], 'rb').read(1)", str(path)],
            cwd=root, preexec_fn=drop_to(service_uid, service_gid),
            capture_output=True, text=True, timeout=5)
        check(probe.returncode != 0 and "PermissionError" in probe.stderr,
              f"storage service UID could read {label} source contents")

    assert_source_private(source_path, "Codex")

    activation = {
        "version": 2, "request_id": "c" * 32, "receiver_id": receiver,
        "operation": {"op": "activate_source", "plugin_id": plugin_id,
                      "source_id": source_id, "generation": generation},
    }
    activation_path = root / "codex-activation.json"
    activation_path.write_text(json.dumps(activation))
    activation_path.chmod(0o644)
    activated = run_client(
        activation_path, socket_path, uid=client_uid, gid=client_gid,
        reply_path=root / "client-results" / "codex-activation-reply.json")
    check(activated["result"]["disposition"] == "committed",
          "Rust Codex fact source activation failed")
    rust_lease = activated["result"]["lease"]

    oracle_store = Store(oracle)
    python_lease = activate_source(
        oracle_store, plugin_id=plugin_id, source_id=source_id, generation=generation)

    def launch(path: Path, child_source_id: str) -> subprocess.Popen:
        environment = os.environ.copy()
        environment["AGENTURM_PLUGIN_SOURCE_ROOT"] = str(path)
        # This proof feeds the same batch to the legacy Python admission oracle.
        environment["AGENTURM_PLUGIN_LEGACY_ADMISSION"] = "1"
        process = subprocess.Popen(
            [str(codex_binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, env=environment, cwd=root,
            preexec_fn=drop_to(client_uid, client_gid))
        _CODEX_CHILDREN.append(process)
        child_write_frame(process, {
            "type": "hello", "protocol": 1, "plugin_id": plugin_id,
            "source_id": child_source_id, "max_frame_bytes": 4 * 1024 * 1024,
        })
        hello = child_read_frame(process)
        check(hello == {
            "type": "hello_ack", "protocol": 1, "plugin_id": plugin_id,
            "features": ["source-adapter/1", "configured-root/1"],
        }, f"unexpected Codex hello response: {hello!r}")
        return process

    def poll(process: subprocess.Popen, child_source_id: str, cursor: str = "") -> dict:
        child_write_frame(process, {
            "type": "poll", "protocol": 1, "source_id": child_source_id,
            "cursor": cursor, "max_records": 48, "max_bytes": 1024 * 1024,
        })
        response = child_read_frame(process)
        check(isinstance(response, dict), "Codex child returned no batch")
        return response

    def make_fact_batch(response: dict) -> PollBatch:
        records = response.get("records")
        check(isinstance(records, list), "Codex batch has no records list")
        allowed = {"message", "tool", "check", "usage"}
        unsupported = {row.get("kind") for row in records} - allowed
        if unsupported:
            raise ProtocolError(
                f"unsupported Codex output kinds: {', '.join(sorted(unsupported))}")
        return PollBatch(source_id=source_id, cursor=response["cursor"], records=records)

    child = launch(source_path, source_id)
    first = poll(child, source_id)
    check(first.get("type") == "batch" and first.get("source_id") == source_id,
          "Codex child returned a malformed batch envelope")
    kinds = [row["kind"] for row in first["records"]]
    check(kinds == ["message", "tool", "tool", "tool", "check", "usage"],
          f"Codex typed fact coverage changed: {kinds!r}")
    check("private" not in json.dumps(first), "Codex child exposed private source content")
    batch = make_fact_batch(first)
    request_id = "d" * 32
    wire_request = request_for(rust_lease, batch, request_id, "", fact_kinds=True)
    request_path = root / "codex-facts.json"
    request_path.write_text(json.dumps(wire_request))
    request_path.chmod(0o644)

    # Python's API has no request-ID receipt table. Its durable cursor/digest
    # pair is replayable; Rust additionally persists the exact v2 request ID.
    python_cursor = commit_batch(oracle_store, python_lease, "", batch)
    check(python_cursor == batch.cursor, "Python Codex oracle did not commit cursor")
    with oracle_store._connect() as db:
        python_state = db.execute(
            "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch "
            "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
    check(commit_batch(oracle_store, python_lease, "", batch) == batch.cursor,
          "Python Codex exact retry failed")
    with oracle_store._connect() as db:
        check(db.execute(
            "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch "
            "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
              == python_state, "Python Codex retry changed durable state")
    oracle_store.close()

    # Drop the service reply after the request is sent. No child ACK is sent.
    run_client(request_path, socket_path, uid=client_uid, gid=client_gid,
               drop_reply=True)
    deadline = time.monotonic() + 5
    while True:
        with closing(sqlite3.connect(clone)) as db:
            rust_cursor = db.execute(
                "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id=?",
                (source_id,)).fetchone()[0]
            receipt = db.execute(
                "SELECT request_digest,source_id,cursor,batch_digest,disposition "
                "FROM storage_admission_request_v2 WHERE request_id=?", (request_id,)).fetchone()
            dirty = db.execute(
                "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
                (source_id,)).fetchone()
        if rust_cursor == batch.cursor and receipt is not None:
            break
        check(time.monotonic() < deadline,
              "Codex fact admission did not durably commit after lost reply")
        time.sleep(0.02)
    check(receipt[1:] == (source_id, batch.cursor,
                          wire_request["operation"]["batch_digest"], "committed"),
          "Codex receipt did not bind source, cursor, digest and disposition")
    check(dirty == (1,), "Codex dirty sequence was not durable")
    child.kill()
    _, child_stderr = child.communicate(timeout=5)
    check(child.returncode == -signal.SIGKILL,
          f"unacknowledged Codex child did not terminate: {child_stderr}")

    # Restart at the old, unacknowledged host cursor and require identical
    # output. Exact Rust replay must succeed before the child receives ACK.
    child = launch(source_path, source_id)
    replayed = poll(child, source_id)
    check(replayed == first, "Codex replay changed source facts or cursor")
    replay_reply = run_client(
        request_path, socket_path, uid=client_uid, gid=client_gid,
        reply_path=root / "client-results" / "codex-facts-replay.json")
    check(replay_reply["result"]["kind"] == "fact_admission"
          and replay_reply["result"]["disposition"] == "replay",
          "Codex storage replay did not return its durable receipt")
    with closing(sqlite3.connect(clone)) as db:
        before_ack = db.execute(
            "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
            (source_id,)).fetchone()
        replayed_receipt = db.execute(
            "SELECT request_digest,source_id,cursor,batch_digest,disposition "
            "FROM storage_admission_request_v2 WHERE request_id=?", (request_id,)).fetchone()
    check(before_ack == (batch.cursor, 1) and replayed_receipt == receipt,
          "Codex ACK would precede visible durable receipt or replay changed the receipt")
    child_write_frame(child, {
        "type": "ack", "protocol": 1, "source_id": source_id, "cursor": batch.cursor,
    })
    empty_batch = poll(child, source_id, batch.cursor)
    check(empty_batch["records"] == [] and empty_batch["cursor"] == batch.cursor,
          "Codex source repeated facts after ACK")
    child_write_frame(child, {"type": "stop", "protocol": 1})
    check(child.wait(timeout=5) == 0, "Codex child did not stop cleanly after ACK")

    with closing(sqlite3.connect(clone)) as rust_db, closing(sqlite3.connect(oracle)) as py_db:
        rust_state = rust_db.execute(
            "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch "
            "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
        py_state = py_db.execute(
            "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch "
            "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
        check(rust_state == py_state, "Rust Codex source state differs from Python Store")
        fields = ("event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                  "native_session_id,native_worker_id,native_turn_id,native_operation_id,"
                  "member_id,policy_epoch")
        rust_rows = rust_db.execute(
            f"SELECT {fields} FROM plugin_evidence_v1 WHERE source_id=? "
            "ORDER BY event_id_hash,revision", (source_id,)).fetchall()
        py_rows = py_db.execute(
            f"SELECT {fields} FROM plugin_evidence_v1 WHERE source_id=? "
            "ORDER BY event_id_hash,revision", (source_id,)).fetchall()
        check(rust_rows == py_rows, "Rust Codex facts differ from Python Store")
        for table, columns in (
            ("plugin_content_batch_v1", "cursor,refs_digest"),
            ("plugin_content_status_v1", "cursor,reason,refs_count,policy_epoch"),
        ):
            rust_receipt = rust_db.execute(
                f"SELECT {columns} FROM {table} WHERE source_id=?", (source_id,)).fetchone()
            python_receipt = py_db.execute(
                f"SELECT {columns} FROM {table} WHERE source_id=?", (source_id,)).fetchone()
            check(rust_receipt == python_receipt,
                  f"Rust Codex {table} differs from Python Store")
        admission_receipts = rust_db.execute(
            "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
            (request_id,)).fetchone()[0]
        check(admission_receipts == 1, "Codex replay duplicated its request receipt")
        persisted = "".join(row[0] for row in rust_db.execute(
            "SELECT payload_json FROM plugin_evidence_v1 WHERE source_id=?", (source_id,)))
        check(all(secret not in persisted for secret in (
            "private Codex answer", "private tool output", "private check output",
            "npm run check", "functions.exec_command", "/private/synthetic/project")),
            "Codex admission persisted source text")

    # The detached Rust-admitted facts are consumed by the current Python
    # projection/export path. Storage and wire admission are Rust-owned here;
    # Activity, session metadata, and Review/export projections remain Python.
    from observatory.plugin_projection import project_source
    from observatory.plugin_activity import export_proof
    # Exercise the production serializers without loading Textual in this root-only gate.
    from observatory.export_serialization import investigation_json, review_investigation_json

    projected_store = Store(clone)
    try:
        project_source(projected_store, source_id)
        sessions = projected_store.plugin_sessions()["sessions"]
        check(len(sessions) == 1, "Rust-admitted Codex session was not projected")
        session = sessions[0]
        check(session["metadata_usage"] == {
            "input_tokens": 7, "output_tokens": 3,
            "cache_read_tokens": 2,
        }, f"Codex usage was not projected to session metadata: {session['metadata_usage']!r}")

        activity_page = projected_store.activity_page()
        activity = activity_page["activity"]
        check({row["domain"] for row in activity} == {"message", "tool", "check"},
              f"Rust-admitted Codex Activity domains changed: "
              f"{[(row['domain'], row['action'], row['result']) for row in activity]!r}")
        check(len([row for row in activity if row["domain"] == "message"]) == 1
              and len([row for row in activity if row["domain"] == "tool"]) == 3
              and len([row for row in activity if row["domain"] == "check"]) == 1,
              "Rust-admitted Codex Activity fact counts changed")
        check(all(row["source"] == "plugin" and row["detail"] == ""
                  and row["metadata"].get("capture") == "content-free"
                  and row["metadata"].get("coverage") == "partial"
                  for row in activity),
              "Codex Activity lost content-free partial provenance")
        check(all("usage" not in row["domain"] and "token" not in row["domain"]
                  for row in activity), "Codex usage leaked into Activity")

        selected = next(row for row in activity if row["domain"] == "check")
        selected_export = json.loads(investigation_json("activity", selected))
        exported = selected_export["evidence"]
        check(exported["domain"] == "check" and exported["action"] == "lint"
              and exported["result"] == "passed",
              "Codex selected Activity JSON lost typed check facts")
        check(exported["source_provenance"]["capture"] == "content-free"
              and exported["source_provenance"]["coverage"] == "partial",
              "Codex Activity JSON lost partial provenance")

        # Generic Review receives only the content-free Activity context for
        # this metadata-only plugin session; no Codex or Pi text is attached.
        review_page = projected_store.activity_page(review=True)
        review_snapshot = {"activity": review_page["activity"],
                           "review_context": review_page["review_context"],
                           "coverage": "loaded content-free Activity only"}
        review_json = review_investigation_json(review_snapshot)
        check("pi_user_text" not in review_json and "redacted_text" not in review_json,
              "generic Codex Review unexpectedly included the Pi text payload")

        consumer_payload = (json.dumps(activity, ensure_ascii=False)
                            + json.dumps(selected_export, ensure_ascii=False) + review_json)
        check(all(secret not in consumer_payload for secret in (
            "private Codex answer", "private tool output", "private check output",
            "npm run check", "functions.exec_command", "/private/synthetic/project",
            "file.txt", "usage", "input_tokens", "output_tokens")),
            "Python consumer projection exposed private Codex content or usage in Activity")
        check(session["metadata_usage"]["input_tokens"] == 7,
              "Codex usage is missing from session metadata")

        proof = export_proof(projected_store, selected["id"])
        check(proof is not None and proof["record"]["id"] == selected["id"],
              "policy-current Activity export proof is unavailable")
        from observatory.plugin_admission import set_source_capture
        set_source_capture(projected_store, source_id=source_id, enabled=False)
        check(projected_store.activity_page()["activity"] == [],
              "capture revoke left Codex Activity visible")
        check(export_proof(projected_store, selected["id"]) is None,
              "capture revoke left Codex Activity export proof valid")
    finally:
        projected_store.close()

    # Unsupported Codex rows become diagnostics. The facts-only host mapper
    # refuses the whole batch instead of silently dropping diagnostic coverage.
    unsupported_id = "codex-unsupported-source"
    unsupported_activation = {
        "version": 2, "request_id": "0" * 32, "receiver_id": receiver,
        "operation": {"op": "activate_source", "plugin_id": plugin_id,
                      "source_id": unsupported_id, "generation": "b" * 64},
    }
    unsupported_path = root / "codex-unsupported-activation.json"
    unsupported_path.write_text(json.dumps(unsupported_activation))
    unsupported_path.chmod(0o644)
    unsupported_reply = run_client(
        unsupported_path, socket_path, uid=client_uid, gid=client_gid,
        reply_path=root / "client-results" / "codex-unsupported-activation-reply.json")
    unsupported_lease = unsupported_reply["result"]["lease"]
    unsupported_source = root / "codex-unsupported.jsonl"
    write_private_source(unsupported_source, [
            {"type": "session_meta", "payload": {"id": "codex-thread-unsupported"}},
            {"type": "event_msg", "timestamp": "2026-11-14T10:01:00Z",
             "payload": {"type": "item_completed", "item":{"type":"UnknownItem"}}},
        ])
    assert_source_private(unsupported_source, "unsupported Codex")
    child = launch(unsupported_source, unsupported_id)
    unsupported_batch_reply = poll(child, unsupported_id)
    check([row["kind"] for row in unsupported_batch_reply["records"]] == ["diagnostic"],
          "unsupported Codex item was not made visible as a diagnostic")
    unsupported_batch = PollBatch(
        source_id=unsupported_id, cursor=unsupported_batch_reply["cursor"],
        records=unsupported_batch_reply["records"])
    with closing(sqlite3.connect(clone)) as db:
        unsupported_before = db.execute(
            "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
            (unsupported_id,)).fetchone()
    try:
        records = unsupported_batch.records
        unsupported_kinds = {row.get("kind") for row in records} - {
            "message", "tool", "check", "usage"}
        if unsupported_kinds:
            raise ProtocolError(
                f"unsupported Codex output kinds: {', '.join(sorted(unsupported_kinds))}")
        request_for(unsupported_lease, unsupported_batch, "a" * 32, "", fact_kinds=True)
    except ProtocolError as error:
        check("unsupported Codex output" in str(error),
              f"unexpected unsupported Codex refusal: {error}")
    else:
        raise AssertionError("facts-only mapper accepted an unsupported Codex diagnostic")
    with closing(sqlite3.connect(clone)) as db:
        unsupported_after = db.execute(
            "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id=?",
            (unsupported_id,)).fetchone()
        unsupported_evidence = db.execute(
            "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
            (unsupported_id,)).fetchone()[0]
        unsupported_receipt_count = db.execute(
            "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
            ("a" * 32,)).fetchone()[0]
    check(unsupported_after == unsupported_before and unsupported_evidence == 0
          and unsupported_receipt_count == 0,
          "unsupported Codex batch changed detached Store state")
    child_write_frame(child, {"type": "stop", "protocol": 1})
    check(child.wait(timeout=5) == 0, "unsupported Codex process did not stop cleanly")

    # Malformed typed data exits before a batch can be acknowledged or
    # admitted. No source lease or evidence row is created for this input.
    malformed_id = "codex-malformed-source"
    malformed_source = root / "codex-malformed.jsonl"
    write_private_source(malformed_source, [
            {"type": "session_meta", "payload": {"id": "codex-thread-malformed"}},
            {"type": "token_usage_record", "timestamp": "2026-11-14T10:02:00Z",
             "payload": {"response_id": "response-bad",
                         "usage": {"input_tokens": "not-a-number"}}},
        ])
    assert_source_private(malformed_source, "malformed Codex")
    child = launch(malformed_source, malformed_id)
    child_write_frame(child, {
        "type": "poll", "protocol": 1, "source_id": malformed_id,
        "cursor": "", "max_records": 48, "max_bytes": 1024 * 1024,
    })
    check(child_read_frame(child) is None, "malformed Codex usage produced a batch")
    child.wait(timeout=5)
    check(child.returncode == 2, "malformed Codex child did not fail visibly")
    with closing(sqlite3.connect(clone)) as db:
        malformed_source_count = db.execute(
            "SELECT COUNT(*) FROM plugin_source_cursor_v1 WHERE source_id=?",
            (malformed_id,)).fetchone()[0]
        malformed_evidence = db.execute(
            "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
            (malformed_id,)).fetchone()[0]
        malformed_receipts = db.execute(
            "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE source_id=?",
            (malformed_id,)).fetchone()[0]
    check((malformed_source_count, malformed_evidence, malformed_receipts) == (0, 0, 0),
          "malformed Codex output changed detached Store state")


def main(binary: Path, codex_binary: Path) -> None:
    check(sys.platform == "linux" and os.geteuid() == 0,
          "run as root on isolated Linux")
    verify_codex_frame_timeout()
    binary_source = binary.resolve(strict=True)
    from observatory.core import Store
    from observatory.plugin_admission import activate_source, commit_batch, set_source_capture

    service_uid, client_uid, wrong_uid, service_gid, client_gid = identities()
    with tempfile.TemporaryDirectory(prefix="agenturm-real-clone-admit-") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        # Neither dropped UID can traverse a private CI checkout. Keep the
        # executable and the client-only script imports in this public root.
        script_dir = Path(__file__).resolve().parent
        for name in (Path(__file__).name, "validate_rust_real_clone_activation.py",
                     "validate_rust_storage_service_accounts.py"):
            client_script = root / name
            shutil.copy2(script_dir / name, client_script)
            client_script.chmod(0o644)
        package = root / "observatory"
        shutil.copytree(script_dir.parent / "observatory", package,
                        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        package.chmod(0o755)
        for path in package.rglob("*"):
            path.chmod(0o755 if path.is_dir() else 0o644)
        service_binary = root / "agenturm-storage-service"
        shutil.copy2(binary_source, service_binary)
        service_binary.chmod(0o755)
        codex_service_binary = root / "agenturm-codex-harness"
        shutil.copy2(codex_binary.resolve(strict=True), codex_service_binary)
        codex_service_binary.chmod(0o755)
        canonical = root / "canonical.db"
        original = Store(canonical)
        receiver = original.receiver_id
        activate_source(original, plugin_id="fixture.plugin", source_id="private-source",
                        generation="b" * 64, content_fingerprint="c" * 64)
        activate_source(original, plugin_id="fixture.plugin", source_id="owned-source",
                        generation="d" * 64)
        for source_id in ("bad-batch-source", "bad-status-source"):
            activate_source(original, plugin_id="fixture.plugin", source_id=source_id,
                            generation="e" * 64)
        activate_source(original, plugin_id="fixture.plugin", source_id="facts-source",
                        generation="f" * 64)
        with original._connect() as db:
            db.execute("INSERT INTO plugin_reference_owner_v1 VALUES('owned-source','pi-content')")
        original.close()

        data = root / "private-data"
        data.mkdir(mode=0o700)
        clone = data / "telemetry-clone.db"
        oracle = root / "oracle.db"
        with closing(sqlite3.connect(canonical)) as source_db:
            for target in (clone, oracle):
                with closing(sqlite3.connect(target)) as copy_db:
                    source_db.backup(copy_db)
        with closing(sqlite3.connect(clone)) as db:
            db.execute("INSERT INTO schema_meta VALUES('storage_activation_clone_v1','enabled')")
            db.commit()

        def canonical_state():
            paths = (canonical, Path(f"{canonical}-wal"), Path(f"{canonical}-shm"))
            return tuple((path.stat().st_ino, path.stat().st_mtime_ns,
                          hashlib.sha256(path.read_bytes()).hexdigest()) if path.exists() else None
                         for path in paths)

        canonical_before = canonical_state()
        oracle_store = Store(oracle)
        py_lease = activate_source(oracle_store, plugin_id="fixture.plugin",
                                   source_id="batch-source", generation="a" * 64)
        batch = new_batch("batch-source", "cursor-one")
        from observatory.plugin_admission import _validated
        _, rows, _ = _validated(batch, "batch-source")
        # The Rust slice deliberately has no private reference provider. Use
        # Python's normal metadata admission oracle and compare core evidence,
        # cursor, and dirty state; provider ownership remains a separate gate.
        python_result = commit_batch(oracle_store, py_lease, "", batch)
        check(python_result == "cursor-one", "Python oracle did not commit cursor")
        py_facts_lease = activate_source(oracle_store, plugin_id="fixture.plugin",
                                         source_id="facts-source", generation="f" * 64)
        facts_batch = new_fact_batch("facts-source", "facts-cursor-one")
        python_facts_result = commit_batch(oracle_store, py_facts_lease, "", facts_batch)
        check(python_facts_result == "facts-cursor-one",
              "Python mixed-fact oracle did not commit cursor")
        oracle_store.close()

        # The canonical input, clone, and transport tree are separately pinned.
        for path in data.iterdir():
            os.chown(path, service_uid, service_gid)
            path.chmod(0o600)
        os.chown(data, service_uid, service_gid)
        transport = root / "transport"
        transport.mkdir(mode=0o700)
        os.chown(transport, service_uid, client_gid)
        transport.chmod(0o2710)
        client_results = root / "client-results"
        client_results.mkdir(mode=0o700)
        os.chown(client_results, client_uid, client_gid)

        activation = {"version": 2, "request_id": "1" * 32, "receiver_id": receiver,
                      "operation": {"op": "activate_source", "plugin_id": "fixture.plugin",
                                    "source_id": "batch-source", "generation": "a" * 64}}
        activation_path = root / "activation.json"
        activation_path.write_text(json.dumps(activation))
        activation_path.chmod(0o644)
        socket_path = transport / "agenturm-storage-v1.sock"
        service_command = [str(service_binary), "--data-root", str(data),
                           "--transport-dir", str(transport), "--receiver-id", receiver,
                           "--client-uid", str(client_uid), "--client-gid", str(client_gid),
                           "--experimental-real-clone-activation", "enabled"]

        def start_service():
            process = subprocess.Popen(service_command,
                preexec_fn=lambda: (os.umask(0), drop_to(service_uid, service_gid)()),
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            wait_socket(process, socket_path)
            return process

        service = start_service()
        try:
            reply_path = client_results / "activation-reply.json"
            activated = run_client(activation_path, socket_path, uid=client_uid,
                                   gid=client_gid, reply_path=reply_path)
            check(activated is not None and
                  activated["result"]["disposition"] == "committed",
                  "Rust source activation failed")
            lease = activated["result"]["lease"]

            facts_activation = {
                "version": 2, "request_id": "6" * 32, "receiver_id": receiver,
                "operation": {"op": "activate_source", "plugin_id": "fixture.plugin",
                              "source_id": "facts-source", "generation": "f" * 64},
            }
            activation_path.write_text(json.dumps(facts_activation))
            facts_activated = run_client(activation_path, socket_path, uid=client_uid,
                                         gid=client_gid, reply_path=reply_path)
            check(facts_activated is not None and
                  facts_activated["result"]["disposition"] == "committed",
                  "Rust fact source activation failed")
            facts_lease = facts_activated["result"]["lease"]
            facts_request = request_for(
                facts_lease, facts_batch, "7" * 32, "", fact_kinds=True)
            facts_request_path = root / "admit-facts.json"
            facts_request_path.write_text(json.dumps(facts_request))
            facts_request_path.chmod(0o644)

            with closing(sqlite3.connect(clone)) as db:
                facts_before_invalid = db.execute(
                    "SELECT cursor,dirty_seq,policy_epoch,capture_enabled,content_fingerprint "
                    "FROM plugin_source_cursor_v1 WHERE source_id='facts-source'").fetchone()

            wrong_receiver = dict(facts_request)
            wrong_receiver["request_id"] = "8" * 32
            wrong_receiver["receiver_id"] = "wrong-receiver"
            wrong_receiver_path = root / "wrong-facts-receiver.json"
            wrong_receiver_path.write_text(json.dumps(wrong_receiver))
            wrong_receiver_path.chmod(0o644)
            run_client(wrong_receiver_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)

            wrong_source = json.loads(json.dumps(facts_request))
            wrong_source["request_id"] = "9" * 32
            wrong_source["operation"]["lease"]["source_id"] = "missing-source"
            wrong_source_path = root / "wrong-facts-source.json"
            wrong_source_path.write_text(json.dumps(wrong_source))
            wrong_source_path.chmod(0o644)
            run_client(wrong_source_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                facts_after_invalid = db.execute(
                    "SELECT cursor,dirty_seq,policy_epoch,capture_enabled,content_fingerprint "
                    "FROM plugin_source_cursor_v1 WHERE source_id='facts-source'").fetchone()
                missing_source = db.execute(
                    "SELECT COUNT(*) FROM plugin_source_cursor_v1 "
                    "WHERE source_id='missing-source'").fetchone()[0]
                check(facts_after_invalid == facts_before_invalid and missing_source == 0,
                      "identity mismatch changed real-clone source state")

            def existing_lease(source_id: str) -> dict:
                with closing(sqlite3.connect(clone)) as db:
                    row = db.execute(
                        "SELECT receiver_id,plugin_id,source_id,generation,lease_epoch,policy_epoch "
                        "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
                return dict(zip(("receiver_id", "plugin_id", "source_id", "generation",
                                 "epoch", "policy_epoch"), row))

            for number, source_id in (("4", "private-source"), ("5", "owned-source")):
                private_batch = new_batch(source_id, f"cursor-{source_id}")
                private_request = request_for(existing_lease(source_id), private_batch,
                                              number * 32, "")
                private_path = root / f"{source_id}.json"
                private_path.write_text(json.dumps(private_request))
                private_path.chmod(0o644)
                with closing(sqlite3.connect(clone)) as db:
                    before = db.execute(
                        "SELECT cursor,dirty_seq,policy_epoch,capture_enabled,content_fingerprint "
                        "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
                    count_before = db.execute(
                        "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?", (source_id,)).fetchone()[0]
                run_client(private_path, socket_path, uid=client_uid, gid=client_gid, reject=True)
                with closing(sqlite3.connect(clone)) as db:
                    after = db.execute(
                        "SELECT cursor,dirty_seq,policy_epoch,capture_enabled,content_fingerprint "
                        "FROM plugin_source_cursor_v1 WHERE source_id=?", (source_id,)).fetchone()
                    count_after = db.execute(
                        "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?", (source_id,)).fetchone()[0]
                    check((after, count_after) == (before, count_before),
                          f"private source {source_id} changed after rejected admission")

            for number, source_id, malformed_sql, malformed_args in (
                ("6", "bad-batch-source",
                 "INSERT INTO plugin_content_batch_v1 VALUES(?, '', ?)",
                 ("bad-batch-source", "f" * 64)),
                ("7", "bad-status-source",
                 "INSERT INTO plugin_content_status_v1 VALUES(?, '', 'not_requested', 1, 1)",
                 ("bad-status-source",)),
            ):
                malformed_batch = new_batch(source_id, "receipt-next")
                malformed_request = request_for(existing_lease(source_id), malformed_batch,
                                                number * 32, "")
                malformed_path = root / f"{source_id}.json"
                malformed_path.write_text(json.dumps(malformed_request))
                malformed_path.chmod(0o644)
                with closing(sqlite3.connect(clone)) as db:
                    db.execute(malformed_sql, malformed_args)
                    db.commit()
                    before = db.execute(
                        "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                        "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                        "WHERE source_id=?", (source_id,)).fetchone()
                run_client(malformed_path, socket_path, uid=client_uid, gid=client_gid,
                           reject=True)
                with closing(sqlite3.connect(clone)) as db:
                    after = db.execute(
                        "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                        "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                        "WHERE source_id=?", (source_id,)).fetchone()
                    evidence = db.execute(
                        "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
                        (source_id,)).fetchone()[0]
                    receipt = db.execute(
                        "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                        (malformed_request["request_id"],)).fetchone()[0] if db.execute(
                            "SELECT 1 FROM sqlite_master WHERE name='storage_admission_request_v2'"
                        ).fetchone() else 0
                    malformed_retained = db.execute(
                        "SELECT COUNT(*) FROM plugin_content_batch_v1 WHERE source_id=?",
                        (source_id,)).fetchone()[0] + db.execute(
                        "SELECT COUNT(*) FROM plugin_content_status_v1 WHERE source_id=?",
                        (source_id,)).fetchone()[0]
                    check((after, evidence, receipt, malformed_retained) == (before, 0, 0, 1),
                          f"malformed content receipt changed source {source_id}")

            request = request_for(lease, batch, "2" * 32, "")
            request_path = root / "admit.json"
            request_path.write_text(json.dumps(request))
            request_path.chmod(0o644)
            admit_reply_path = client_results / "admit-reply.json"

            # Deterministically abort an evidence insert and prove SQLite rolls
            # back both the cursor and the request receipt.
            with closing(sqlite3.connect(clone)) as db:
                db.execute("CREATE TABLE IF NOT EXISTS storage_admission_request_v2 ("
                           "request_id TEXT PRIMARY KEY, receiver_id TEXT NOT NULL, "
                           "request_digest TEXT NOT NULL, source_id TEXT NOT NULL, "
                           "cursor TEXT NOT NULL, batch_digest TEXT NOT NULL, "
                           "dirty_seq INTEGER NOT NULL, disposition TEXT NOT NULL)")
                db.execute("CREATE TRIGGER abort_batch BEFORE INSERT ON plugin_evidence_v1 "
                           "BEGIN SELECT RAISE(ABORT,'test precommit abort'); END")
                db.commit()
            run_client(request_path, socket_path, uid=client_uid, gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                cursor = db.execute("SELECT cursor FROM plugin_source_cursor_v1 "
                                    "WHERE source_id='batch-source'").fetchone()[0]
                evidence = db.execute("SELECT COUNT(*) FROM plugin_evidence_v1 "
                                      "WHERE source_id='batch-source'").fetchone()[0]
                receipts = db.execute("SELECT COUNT(*) FROM storage_admission_request_v2 "
                                      "WHERE request_id=?", (request["request_id"],)).fetchone()[0]
                check((cursor, evidence, receipts) == ("", 0, 0),
                      "precommit abort left partial admission state")
                db.execute("DROP TRIGGER abort_batch")
                db.commit()

            # Close the client after sending the complete request, simulating a
            # lost reply after commit. The durable receipt must make a restart
            # replay idempotent.
            run_client(request_path, socket_path, uid=client_uid, gid=client_gid,
                       drop_reply=True)
            deadline = time.monotonic() + 5
            while True:
                with closing(sqlite3.connect(clone)) as db:
                    cursor = db.execute("SELECT cursor FROM plugin_source_cursor_v1 "
                                        "WHERE source_id='batch-source'").fetchone()[0]
                    receipt_count = db.execute(
                        "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                        (request["request_id"],)).fetchone()[0]
                if cursor == "cursor-one" and receipt_count == 1:
                    break
                check(time.monotonic() < deadline and service.poll() is None,
                      "lost reply did not durably commit cursor and receipt")
                time.sleep(0.02)
            replay = run_client(request_path, socket_path, uid=client_uid, gid=client_gid,
                                reply_path=admit_reply_path)
            check(replay["result"]["disposition"] == "replay",
                  "same-process admission retry was not a replay")

            # A late conflict on the second fact must roll back the first
            # insert, cursor, dirty marker and shared request receipt.
            records = facts_request["operation"]["records"]
            first_fact_hash = records[0]["event_id_hash"]
            conflicting_fact = records[1]
            with closing(sqlite3.connect(clone)) as db:
                db.execute(
                    "INSERT INTO plugin_evidence_v1 "
                    "(source_id,event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                    "native_session_id,native_worker_id,native_turn_id,native_operation_id,"
                    "member_id,policy_epoch,admitted_ns) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    ("facts-source", conflicting_fact["event_id_hash"],
                     conflicting_fact["revision"], conflicting_fact["kind"],
                     conflicting_fact["time_ns"], conflicting_fact["payload_json"],
                     "0" * 64, conflicting_fact["native_session_id"],
                     conflicting_fact["native_worker_id"], conflicting_fact["native_turn_id"],
                     conflicting_fact["native_operation_id"], "", facts_lease["policy_epoch"], 1),
                )
                db.commit()
            run_client(facts_request_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                facts_cursor = db.execute(
                    "SELECT cursor FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()[0]
                facts_count = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='facts-source'").fetchone()[0]
                first_fact_count = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 "
                    "WHERE source_id='facts-source' AND event_id_hash=?",
                    (first_fact_hash,)).fetchone()[0]
                facts_receipts = db.execute(
                    "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                    (facts_request["request_id"],)).fetchone()[0]
                check((facts_cursor, facts_count, first_fact_count, facts_receipts) == ("", 1, 0, 0),
                      "late second-fact conflict did not roll back the first insert")
                removed = db.execute(
                    "DELETE FROM plugin_evidence_v1 WHERE source_id='facts-source' "
                    "AND event_id_hash=? AND record_digest=?",
                    (conflicting_fact["event_id_hash"], "0" * 64),
                ).rowcount
                check(removed == 1, "could not remove the fixture's conflicting seed row")
                db.commit()

            run_client(facts_request_path, socket_path, uid=client_uid, gid=client_gid,
                       drop_reply=True)
            deadline = time.monotonic() + 5
            while True:
                with closing(sqlite3.connect(clone)) as db:
                    facts_cursor = db.execute(
                        "SELECT cursor FROM plugin_source_cursor_v1 "
                        "WHERE source_id='facts-source'").fetchone()[0]
                    facts_receipt_count = db.execute(
                        "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                        (facts_request["request_id"],)).fetchone()[0]
                if facts_cursor == "facts-cursor-one" and facts_receipt_count == 1:
                    break
                check(time.monotonic() < deadline and service.poll() is None,
                      "lost fact ACK did not durably commit cursor and receipt")
                time.sleep(0.02)
            facts_replay = run_client(
                facts_request_path, socket_path, uid=client_uid, gid=client_gid,
                reply_path=admit_reply_path)
            check(facts_replay["result"]["kind"] == "fact_admission"
                  and facts_replay["result"]["disposition"] == "replay",
                  "same-process generic fact replay failed")

            service.send_signal(signal.SIGTERM)
            _, stderr = service.communicate(timeout=5)
            check(service.returncode == 0, f"service shutdown failed: {stderr}")
            service = start_service()
            replay = run_client(request_path, socket_path, uid=client_uid, gid=client_gid,
                                reply_path=admit_reply_path)
            check(replay["result"]["disposition"] == "replay",
                  "restart admission replay was not durable")
            facts_replay = run_client(
                facts_request_path, socket_path, uid=client_uid, gid=client_gid,
                reply_path=admit_reply_path)
            check(facts_replay["result"]["kind"] == "fact_admission"
                  and facts_replay["result"]["disposition"] == "replay",
                  "restart generic fact replay was not durable")

            exercise_codex_fact_path(
                root=root, codex_binary=codex_service_binary, clone=clone, oracle=oracle,
                receiver=receiver, socket_path=socket_path,
                service_uid=service_uid, service_gid=service_gid,
                client_uid=client_uid, client_gid=client_gid)

            with closing(sqlite3.connect(clone)) as db, closing(sqlite3.connect(oracle)) as py:
                rust_state = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='batch-source'").fetchone()
                py_state = py.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='batch-source'").fetchone()
                check(rust_state == py_state, "Rust cursor/policy state differs from Python")
                projection = ("event_id_hash,revision,kind,time_ns,payload_json,record_digest,"
                              "native_session_id,native_worker_id,native_turn_id,native_operation_id,"
                              "member_id,policy_epoch")
                rust_rows = db.execute(
                    f"SELECT {projection} FROM plugin_evidence_v1 WHERE source_id=? "
                    "ORDER BY event_id_hash,revision", ("batch-source",)).fetchall()
                py_rows = py.execute(
                    f"SELECT {projection} FROM plugin_evidence_v1 WHERE source_id=? "
                    "ORDER BY event_id_hash,revision", ("batch-source",)).fetchall()
                check(rust_rows == py_rows, "Rust evidence differs from Python Store")
                check(len(rust_rows) == len(rows), "admission evidence count mismatch")
                dirty = db.execute("SELECT dirty_seq FROM plugin_evidence_dirty_v1 "
                                   "WHERE source_id='batch-source'").fetchone()
                py_dirty = py.execute("SELECT dirty_seq FROM plugin_evidence_dirty_v1 "
                                      "WHERE source_id='batch-source'").fetchone()
                check(dirty == py_dirty, "Rust evidence dirty sequence differs from Python")
                receipt_count = db.execute(
                    "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                    (request["request_id"],)).fetchone()[0]
                check(receipt_count == 1, "admission replay receipt was duplicated")

            # Exercise the exact shared cursor ceiling across Python request
            # construction, the Unix service transport, Rust storage, and reply.
            long_activation = {"version": 2, "request_id": "8" * 32,
                               "receiver_id": receiver,
                               "operation": {"op": "activate_source",
                                             "plugin_id": "fixture.plugin",
                                             "source_id": "long-source",
                                             "generation": "a" * 64}}
            long_activation_path = root / "long-activation.json"
            long_activation_path.write_text(json.dumps(long_activation))
            long_activation_path.chmod(0o644)
            long_activated = run_client(long_activation_path, socket_path, uid=client_uid,
                                        gid=client_gid, reply_path=client_results / "long-activation-reply.json")
            check(long_activated["result"]["disposition"] == "committed",
                  "long-cursor source activation failed")
            long_cursor = "x" * 800
            long_batch = new_batch("long-source", long_cursor)
            long_request = request_for(long_activated["result"]["lease"], long_batch,
                                       "9" * 32, "")
            long_path = root / "long-admit.json"
            long_path.write_text(json.dumps(long_request))
            long_path.chmod(0o644)
            long_reply = run_client(long_path, socket_path, uid=client_uid, gid=client_gid,
                                    reply_path=client_results / "long-admit-reply.json")
            check(long_reply["result"]["cursor"] == long_cursor,
                  "800-byte cursor did not round-trip through service reply")
            with closing(sqlite3.connect(clone)) as db, closing(sqlite3.connect(oracle)) as py:
                stored_long_cursor = db.execute(
                    "SELECT cursor FROM plugin_source_cursor_v1 WHERE source_id='long-source'"
                ).fetchone()[0]
                check(stored_long_cursor == long_cursor,
                      "800-byte cursor was not stored exactly")

                facts_rust_state = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                facts_py_state = py.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                check(facts_rust_state == facts_py_state,
                      "Rust mixed-fact cursor/policy state differs from Python")
                facts_rust_rows = db.execute(
                    f"SELECT {projection} FROM plugin_evidence_v1 WHERE source_id=? "
                    "ORDER BY event_id_hash,revision", ("facts-source",)).fetchall()
                facts_py_rows = py.execute(
                    f"SELECT {projection} FROM plugin_evidence_v1 WHERE source_id=? "
                    "ORDER BY event_id_hash,revision", ("facts-source",)).fetchall()
                check(facts_rust_rows == facts_py_rows,
                      "Rust message/tool/check/usage facts differ from Python Store")
                check(len(facts_rust_rows) == 4,
                      "mixed-fact batch did not admit all four fact kinds")
                facts_dirty = db.execute(
                    "SELECT dirty_seq FROM plugin_evidence_dirty_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                facts_py_dirty = py.execute(
                    "SELECT dirty_seq FROM plugin_evidence_dirty_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                check(facts_dirty == facts_py_dirty,
                      "Rust fact dirty sequence differs from Python")
                facts_content_batch = db.execute(
                    "SELECT cursor,refs_digest FROM plugin_content_batch_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                facts_py_content_batch = py.execute(
                    "SELECT cursor,refs_digest FROM plugin_content_batch_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                check(facts_content_batch == facts_py_content_batch,
                      "Rust empty-content batch receipt differs from Python")
                facts_content_status = db.execute(
                    "SELECT cursor,reason,refs_count,policy_epoch "
                    "FROM plugin_content_status_v1 WHERE source_id='facts-source'").fetchone()
                facts_py_content_status = py.execute(
                    "SELECT cursor,reason,refs_count,policy_epoch "
                    "FROM plugin_content_status_v1 WHERE source_id='facts-source'").fetchone()
                check(facts_content_status == facts_py_content_status,
                      "Rust content status differs from Python")
                facts_receipt_count = db.execute(
                    "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                    (facts_request["request_id"],)).fetchone()[0]
                check(facts_receipt_count == 1,
                      "fact replay receipt was duplicated")

            marker_batch = new_fact_batch(
                "facts-source", "facts-cursor-two", suffix="marker")
            marker_request = request_for(
                facts_lease, marker_batch, "b" * 32,
                "facts-cursor-one", fact_kinds=True)
            marker_path = root / "marker-facts.json"
            marker_path.write_text(json.dumps(marker_request))
            marker_path.chmod(0o644)
            with closing(sqlite3.connect(clone)) as db:
                db.execute(
                    "CREATE TRIGGER delete_clone_marker_during_fact AFTER INSERT "
                    "ON plugin_evidence_v1 WHEN NEW.source_id='facts-source' "
                    "BEGIN DELETE FROM schema_meta "
                    "WHERE key='storage_activation_clone_v1'; END")
                db.commit()
            run_client(marker_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                marker_cursor = db.execute(
                    "SELECT cursor FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()[0]
                marker_evidence = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 "
                    "WHERE source_id='facts-source'").fetchone()[0]
                marker_receipt = db.execute(
                    "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                    (marker_request["request_id"],)).fetchone()[0]
                clone_marker = db.execute(
                    "SELECT value FROM schema_meta "
                    "WHERE key='storage_activation_clone_v1'").fetchone()
                check((marker_cursor, marker_evidence, marker_receipt, clone_marker) ==
                      ("facts-cursor-one", 4, 0, ("enabled",)),
                      "marker-changing trigger was not rejected before fact writes")
                db.execute("DROP TRIGGER delete_clone_marker_during_fact")
                db.commit()

            # Revoke through Python against the clone while the Rust service is
            # idle, then prove the previously issued Rust lease is stale. The
            # Python oracle receives the same policy transition.
            stale_batch = new_batch("batch-source", "cursor-two")
            stale_request = request_for(lease, stale_batch, "3" * 32, "cursor-one")
            stale_path = root / "stale.json"
            stale_path.write_text(json.dumps(stale_request))
            stale_path.chmod(0o644)
            clone_store = Store(clone)
            set_source_capture(clone_store, source_id="batch-source", enabled=False)
            clone_store.close()
            oracle_store = Store(oracle)
            set_source_capture(oracle_store, source_id="batch-source", enabled=False)
            oracle_store.close()
            with closing(sqlite3.connect(clone)) as db:
                before_stale = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='batch-source'").fetchone()
                before_evidence = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='batch-source'").fetchone()[0]
            run_client(stale_path, socket_path, uid=client_uid, gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                after_stale = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='batch-source'").fetchone()
                after_evidence = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='batch-source'").fetchone()[0]
                check((after_stale, after_evidence) == (before_stale, before_evidence),
                      "stale lease admission changed revoked source state")

            stale_facts_batch = new_fact_batch(
                "facts-source", "facts-cursor-two", suffix="two")
            stale_facts_request = request_for(
                facts_lease, stale_facts_batch, "a" * 32,
                "facts-cursor-one", fact_kinds=True)
            stale_facts_path = root / "stale-facts.json"
            stale_facts_path.write_text(json.dumps(stale_facts_request))
            stale_facts_path.chmod(0o644)
            clone_store = Store(clone)
            set_source_capture(clone_store, source_id="facts-source", enabled=False)
            clone_store.close()
            oracle_store = Store(oracle)
            set_source_capture(oracle_store, source_id="facts-source", enabled=False)
            oracle_store.close()
            with closing(sqlite3.connect(clone)) as db:
                facts_before_stale = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                facts_evidence_before_stale = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='facts-source'").fetchone()[0]
            run_client(stale_facts_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                facts_after_stale = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                facts_evidence_after_stale = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='facts-source'").fetchone()[0]
                check((facts_after_stale, facts_evidence_after_stale) ==
                      (facts_before_stale, facts_evidence_before_stale),
                      "stale fact lease changed revoked source state")

            with closing(sqlite3.connect(clone)) as db, closing(sqlite3.connect(oracle)) as py:
                rust_state = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='batch-source'").fetchone()
                py_state = py.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='batch-source'").fetchone()
                # Policy and random lease epochs are unaffected by admission.
                check(rust_state == py_state, "Rust cursor/policy state differs from Python")
                rust_count = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='batch-source'").fetchone()[0]
                py_count = py.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='batch-source'").fetchone()[0]
                check(rust_count == py_count == 0, "revocation left source evidence")
                facts_rust_state = db.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                facts_py_state = py.execute(
                    "SELECT cursor,prior_cursor,last_batch_digest,dirty_seq,policy_epoch,"
                    "capture_enabled,content_fingerprint FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()
                check(facts_rust_state == facts_py_state,
                      "fact-source revocation state differs from Python")
                facts_rust_count = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='facts-source'").fetchone()[0]
                facts_py_count = py.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='facts-source'").fetchone()[0]
                check(facts_rust_count == facts_py_count == 0,
                      "revocation left generic fact evidence")

            # Replacing the fixed database inode while the service is alive
            # must prevent a new, otherwise valid activation from mutating it.
            replacement = data / "replacement.db"
            with closing(sqlite3.connect(clone)) as source_db, \
                    closing(sqlite3.connect(replacement)) as replacement_db:
                source_db.backup(replacement_db)
            os.chown(replacement, service_uid, service_gid)
            replacement.chmod(0o600)
            Path(f"{clone}-wal").unlink(missing_ok=True)
            Path(f"{clone}-shm").unlink(missing_ok=True)
            os.replace(replacement, clone)
            replacement_activation = {
                "version": 2, "request_id": "e" * 32, "receiver_id": receiver,
                "operation": {"op": "activate_source", "plugin_id": "fixture.plugin",
                              "source_id": "replacement-source", "generation": "e" * 64},
            }
            replacement_request_path = root / "replacement-activation.json"
            replacement_request_path.write_text(json.dumps(replacement_activation))
            replacement_request_path.chmod(0o644)
            run_client(replacement_request_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                replacement_source_count = db.execute(
                    "SELECT COUNT(*) FROM plugin_source_cursor_v1 "
                    "WHERE source_id='replacement-source'").fetchone()[0]
                check(replacement_source_count == 0,
                      "service mutated a replaced fixed-path database inode")

            # The exact receipt and cursor survive service restart; all test
            # mutations stayed on the detached clone.
            check(canonical_state() == canonical_before, "canonical database changed")
            print("PASS detached Store lifecycle/fact admission, parity, replay, fencing, and replacement guard")
        finally:
            if service.poll() is None:
                service.send_signal(signal.SIGTERM)
            try:
                _, stderr = service.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                service.kill()
                _, stderr = service.communicate(timeout=5)
            check(service.returncode == 0
                  or "scratch database identity changed" in stderr,
                  f"service failed outside the expected database replacement fence: {stderr}")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path)
    parser.add_argument("--codex-bin", type=Path)
    parser.add_argument("--client", action="store_true")
    parser.add_argument("--socket", type=Path)
    parser.add_argument("--request", type=Path)
    parser.add_argument("--reply", type=Path)
    parser.add_argument("--reject", action="store_true")
    parser.add_argument("--drop-reply", action="store_true")
    args = parser.parse_args()
    if args.client:
        child_request(args)
    else:
        check(args.codex_bin is not None, "--codex-bin is required for the Codex integration proof")
        main(args.bin.resolve(strict=True), args.codex_bin.resolve(strict=True))
