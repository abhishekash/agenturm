"""Root-only two-UID proof for feature-off real Store clone admission."""
from __future__ import annotations

from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from validate_rust_real_clone_activation import send
from validate_rust_storage_service_accounts import check, drop_to, identities, wait_socket


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


def main(binary: Path) -> None:
    check(sys.platform == "linux" and os.geteuid() == 0,
          "run as root on isolated Linux")
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

            # A failure on the second insert must roll back the first fact,
            # cursor, dirty marker and shared request receipt.
            second_fact_hash = facts_request["operation"]["records"][1]["event_id_hash"]
            with closing(sqlite3.connect(clone)) as db:
                db.execute(
                    "CREATE TRIGGER abort_second_fact BEFORE INSERT ON plugin_evidence_v1 "
                    f"WHEN NEW.source_id='facts-source' AND NEW.event_id_hash='{second_fact_hash}' "
                    "BEGIN SELECT RAISE(ABORT,'test second-fact abort'); END")
                db.commit()
            run_client(facts_request_path, socket_path, uid=client_uid,
                       gid=client_gid, reject=True)
            with closing(sqlite3.connect(clone)) as db:
                facts_cursor = db.execute(
                    "SELECT cursor FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()[0]
                facts_count = db.execute(
                    "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='facts-source'").fetchone()[0]
                facts_receipts = db.execute(
                    "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                    (facts_request["request_id"],)).fetchone()[0]
                check((facts_cursor, facts_count, facts_receipts) == ("", 0, 0),
                      "second-fact abort left partial transaction state")
                db.execute("DROP TRIGGER abort_second_fact")
                db.commit()

            run_client(facts_request_path, socket_path, uid=client_uid, gid=client_gid,
                       drop_reply=True)
            with closing(sqlite3.connect(clone)) as db:
                facts_cursor = db.execute(
                    "SELECT cursor FROM plugin_source_cursor_v1 "
                    "WHERE source_id='facts-source'").fetchone()[0]
                check(facts_cursor == "facts-cursor-one",
                      "lost fact ACK did not commit its cursor")
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
            with closing(sqlite3.connect(clone)) as db:
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
                      "marker trigger did not roll back the complete fact batch")
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
            check(service.returncode == 0, f"service failed: {stderr}")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path)
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
        main(args.bin.resolve(strict=True))
