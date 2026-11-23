"""Verify generic local OTLP error logs on a disposable history."""

import argparse
from contextlib import closing
from copy import deepcopy
import json
import os
from pathlib import Path
import secrets
import sqlite3
import struct
import subprocess
import tempfile
import time

from validate_rust_broker_otlp import field, post
from validate_rust_host_history import read_line
from validate_rust_remote_history import stop

COLLECTOR = "otlp.log-acceptance"
BODY = "private-error-log-acceptance-marker"
SOURCE_SESSION = "error-session-acceptance"
TRACE_ID = "ab" * 16
SPAN_ID = "cd" * 8


def value(key, text):
    return {"key": key, "value": {"stringValue": text}}


def envelope(*records):
    return {"resourceLogs": [{"scopeLogs": [{"logRecords": records}]}]}


def metadata_record(at):
    return {
        "timeUnixNano": "0",
        "observedTimeUnixNano": str(at),
        "severityNumber": 17,
        "severityText": "ignored severity label",
        "eventName": "ordinary SDK log",
        "traceId": TRACE_ID,
        "spanId": SPAN_ID,
        "body": {"stringValue": BODY},
    }


def protobuf_record(at):
    record = bytes([11 << 3 | 1]) + struct.pack("<Q", at)
    record += bytes([2 << 3, 17])
    record += field(9, bytes.fromhex(TRACE_ID)) + field(10, bytes.fromhex(SPAN_ID))
    record += field(5, field(1, BODY.encode()))
    return field(1, field(2, field(2, record)))


def exact_rows(app, db, cwd, env):
    result = subprocess.run(
        [app, "query", "--db", db, "--harness-id", COLLECTOR, "--limit", "20"],
        cwd=cwd, env=env, capture_output=True, text=True, check=True, timeout=15,
    )
    assert BODY not in result.stdout and BODY not in result.stderr
    return json.loads(result.stdout)["observations"]


def transfer(app, root, env, capture):
    db = root / "history.sqlite"
    command = [app, "serve-traces", "--listen", "127.0.0.1:0", "--db", db,
               "--source-id", COLLECTOR]
    token = None
    if capture:
        token_file = root / "producer-token"
        token = secrets.token_hex(32)
        with os.fdopen(os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as stream:
            stream.write(token + "\n")
        command += ["--payload-scope", "standalone", "--token-file", token_file,
                    "--capture-log-text"]
    process = subprocess.Popen(command, cwd=root, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True)
    try:
        traces = read_line(process).split()[-1]
        url = traces.replace("/v1/traces", "/v1/logs")
        at = time.time_ns() - 100_000_000
        ordinary = metadata_record(at)
        activity = {
            "eventName": "agent.activity",
            "timeUnixNano": str(at),
            "severityNumber": 17,
            "attributes": [value("session.id", SOURCE_SESSION),
                           value("agenturm.log.id", "explicit-activity-id")],
            "body": {"stringValue": "legacy-metadata-only-batch-member"},
        }
        informational = {"severityNumber": 13, "body": {"stringValue": BODY}}
        source_body = json.dumps(envelope(ordinary, activity, informational)).encode()
        if capture:
            assert post(url, source_body, "application/json")[0] == 401
        status, response = post(url, source_body, "application/json", token)
        assert status == 200, ("mixed log admission", status)
        assert BODY.encode() not in response
        assert json.loads(response)["partialSuccess"]["rejectedLogRecords"] == "1"
        assert post(url, source_body, "application/json", token)[0] == 200
        assert post(url, protobuf_record(at), "application/x-protobuf", token) == (200, b"")

        if capture:
            different_body = deepcopy(ordinary)
            different_body["body"] = {"stringValue": "divergent capture marker"}
            status, response = post(url, json.dumps(envelope(activity, different_body)).encode(),
                                    "application/json", token)
            assert status in (409, 503), ("private replay conflict", status)
            assert b"divergent capture marker" not in response and BODY.encode() not in response
        else:
            different_body = deepcopy(ordinary)
            different_body["body"] = {"stringValue": "ignored anonymous replay body"}
            assert post(url, json.dumps(envelope(different_body)).encode(),
                        "application/json") == (200, b"{}")
    finally:
        stop(process)
    rows = exact_rows(app, db, root, env)
    assert len(rows) == 2
    by_kind = {row["observation"]["kind"]: row for row in rows}
    ordinary_row = by_kind["otlp_error_log"]
    assert ordinary_row["event_id"].startswith("otlp-error-log:")
    assert not ordinary_row["identity"].get("session_id")
    assert ordinary_row["observed_at_unix_ns"] == str(at)
    assert ordinary_row["observation"] == {
        "kind": "otlp_error_log", "severity_number": 17, "observed_time_unix_ns": at,
        "trace_id": TRACE_ID, "span_id": SPAN_ID,
    }
    assert by_kind["otlp_activity_log"]["identity"]["session_id"] == SOURCE_SESSION
    with closing(sqlite3.connect(db.as_uri() + "?mode=ro", uri=True)) as history:
        saved = history.execute(
            "SELECT event_id, payload_json FROM agenturm_telemetry_payloads"
        ).fetchall()
        if capture:
            assert len(saved) == 1 and saved[0][0] == ordinary_row["event_id"]
            assert json.loads(saved[0][1]) == {"text": BODY}
        else:
            assert saved == []
    print(f"generic error logs {'opt-in text' if capture else 'metadata-only'} witness passed")


def actual_sdk(app, root, env):
    import logging
    import requests
    from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
    from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
    from opentelemetry.sdk._logs.export import SimpleLogRecordProcessor
    from opentelemetry.sdk.resources import Resource

    db = root / "history.sqlite"
    process = subprocess.Popen(
        [app, "serve-traces", "--listen", "127.0.0.1:0", "--db", db,
         "--source-id", COLLECTOR],
        env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        url = read_line(process).split()[-1].replace("/v1/traces", "/v1/logs")
        with requests.Session() as session:
            session.trust_env = False
            exporter = OTLPLogExporter(
                endpoint=url, session=session, timeout=2,
                headers={"Content-Type": "application/x-protobuf"},
            )
            provider = LoggerProvider(resource=Resource.create({}), shutdown_on_exit=False)
            provider.add_log_record_processor(SimpleLogRecordProcessor(exporter))
            logger = logging.getLogger("agenturm-ordinary-log-acceptance")
            handler = LoggingHandler(logger_provider=provider)
            logger.addHandler(handler)
            logger.propagate = False
            try:
                logger.error(BODY)
                assert provider.force_flush()
            finally:
                logger.removeHandler(handler)
                provider.shutdown()
    finally:
        stop(process)
    rows = exact_rows(app, db, root, env)
    assert len(rows) == 1
    row = rows[0]
    assert row["identity"]["scope"] == "source"
    assert row["observation"]["kind"] == "otlp_error_log"
    assert row["observation"]["severity_number"] == 17
    assert row["observation"]["source_time_unix_ns"] > 0
    assert row["observation"]["observed_time_unix_ns"] > 0
    print("actual cached OpenTelemetry SDK error log witness passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agenturm", type=Path, required=True)
    parser.add_argument("--actual-sdk", action="store_true")
    args = parser.parse_args()
    app = args.agenturm.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="agenturm-generic-logs-") as directory:
        root = Path(directory)
        home = root / "home"
        home.mkdir(mode=0o700)
        env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(home / "config"),
               "XDG_STATE_HOME": str(home / "state"), "XDG_DATA_HOME": str(home / "data")}
        for capture in (False, True):
            workspace = root / ("capture" if capture else "metadata")
            workspace.mkdir(mode=0o700)
            transfer(app, workspace, env, capture)
        if args.actual_sdk:
            workspace = root / "sdk"
            workspace.mkdir(mode=0o700)
            actual_sdk(app, workspace, env)


if __name__ == "__main__":
    main()
