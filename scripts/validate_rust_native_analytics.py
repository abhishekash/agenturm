"""Qualify Rust analytics with local fixture exports and 120, 80, and 60 column PTYs.

The witness uses disposable histories and runs without a model provider.
"""

import argparse
import copy
from datetime import datetime, timezone
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import time

from validate_rust_broker_otlp import post
from validate_rust_host_history import read_line
from validate_rust_real_session_roster import Cockpit
from validate_rust_remote_history import stop

TRACE = "ab" * 16
SPAN = "cd" * 8
PRIVATE = "PRIVATE_METRIC_PAYLOAD_MARKER"
SENTINEL = "PRIVATE_AGGREGATE_WITNESS_BODY"
SESSION = "trace-backed-fixture"
LOG_SESSION = "log-only-fixture"
DAY_NS = 86_400_000_000_000
SIZES = ((120, 32), (80, 24), (60, 18))
DEFAULT_FIXTURE = (
    Path(__file__).resolve().parent.parent
    / "rust/agenturm-app/src/trace_receiver/metrics/fixtures"
)


def wire_traces(at, trace_id, include_usage):
    return {"resourceSpans": [{"scopeSpans": [{"spans": [{
        "traceId": trace_id, "spanId": SPAN, "name": PRIVATE,
        "startTimeUnixNano": str(at - 1), "endTimeUnixNano": str(at),
        "status": {"code": 1},
        "attributes": [
            {"key": "session.id", "value": {"stringValue": "metrics-session"}},
        ] + ([
            {"key": "gen_ai.usage.input_tokens", "value": {"intValue": "143"}},
            {"key": "gen_ai.usage.output_tokens", "value": {"intValue": "0"}},
        ] if include_usage else []),
    }]}]}]}


def metric_admission(app, root, env, db, fixture, source):
    process = subprocess.Popen(
        [app, "serve-traces", "--listen", "127.0.0.1:0", "--db", db, "--source-id", source],
        env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        trace_url = read_line(process).split()[-1]
        metric_url = trace_url.replace("/v1/traces", "/v1/metrics")
        binary = (fixture / "sdk-metrics.pb").read_bytes()
        encoded = (fixture / "sdk-metrics.json").read_bytes()
        assert post(metric_url, binary, "application/x-protobuf") == (200, b"")
        code, value = post(metric_url, encoded, "application/json")
        assert code == 200 and json.loads(value) == {}, (code, value)
        changed = copy.deepcopy(json.loads(encoded))
        changed["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"][0]["asInt"] = "17"
        code, _ = post(metric_url, json.dumps(changed).encode(), "application/json")
        assert code in (409, 503), ("metric replay conflict", code)
        recent = time.time_ns() - 2_000_000_000
        for at, trace_id, include_usage in (
            (14, TRACE, False),
            (recent - DAY_NS, "12" * 16, True),
            (recent, "34" * 16, True),
        ):
            export = wire_traces(at, trace_id, include_usage)
            code, _ = post(trace_url, json.dumps(export).encode(), "application/json")
            assert code == 200, ("adjacent trace admission", code)
    finally:
        stop(process)
    result = subprocess.run(
        [app, "query", "--db", db, "--harness-id", source, "--limit", "20"],
        env=env, cwd=root, check=True, capture_output=True, text=True, timeout=12,
    )
    assert PRIVATE not in result.stdout
    assert result.stdout.count('"metric_point"') == 7, "both metric encodings must replay exact rows"
    assert result.stdout.count('"span_token_usage"') == 2, "two distinct days of reported counts"
    return {datetime.fromtimestamp(at // 1_000_000_000, timezone.utc).date().isoformat()
            for at in (recent - DAY_NS, recent)}


def selection(frame):
    return next(((index, line) for index, line in enumerate(frame.splitlines()) if "›" in line), None)


def day_values(frame, value):
    pattern = re.compile(rf"^│(\d{{4}}-\d{{2}}-\d{{2}})[†‡* ]+{re.escape(value)} +█* +│")
    return {match[1] for line in frame.splitlines() if (match := pattern.match(line))}


def browse_metrics(app, root, env, db, size, reported_days):
    width, height = size
    with Cockpit(app, env, root, width, height, db=db) as tui:
        tui.markers("command center", "Agenturm command center")
        tui.key(b"6")
        tui.markers("metric catalog", "OTLP series", "agents.gauge", "runs.duration")
        for _ in range(10):
            frame = tui.screen.contents()
            preview = "Selected series" in frame
            # Only the gauge with point attributes carries the exemplar.
            selected = selection(frame)
            if selected is not None and "agents.gauge" in selected[1]:
                tui.key(b"\r")
                tui.markers("metric points", "Metric points", "Enter details")
                tui.key(b"\r")
                tui.markers("gauge details", "Metric point", "Esc points")
                if "Exemplar 1/1" in tui.screen.contents():
                    break
                tui.key(b"\x1b")
                tui.markers("return to points", "Metric points", "Enter details")
                tui.key(b"\x1b")
                tui.markers("return to series", "OTLP series", "Enter points")
            tui.key(b"j")
            tui.until(
                lambda screen: selection(screen) is not None and selection(screen) != selected,
                "advance to another retained metric series",
            )
        else:
            raise AssertionError("fixture gauge containing exact exemplar was unreachable")
        assert not preview or width >= 100
        tui.key(b"\r")
        tui.markers("same-source exact exemplar", "Exemplar trace", TRACE, SPAN, "metrics-session")
        for phase, title, hint in [
            ("return to point", "Metric point", "Esc points"),
            ("return to points", "Metric points", "Enter details"),
            ("return to series", "OTLP series", "Enter points"),
            ("return to agents", "Agenturm command center", "Enter inspect"),
        ]:
            tui.key(b"\x1b")
            tui.markers(phase, title, hint)
        tui.key(b"U")
        tui.markers("seven day Usage", "7 UTC days", "Native usage reports", "Unreported")
        tui.key(b"s")
        tui.until(
            lambda frame: "Span reports may overlap" in frame and "Input tokens" in frame
            and day_values(frame, "143") == reported_days,
            "two exact input-token report dates",
        )
        tui.key(b"t")
        tui.until(
            lambda frame: "Output tokens" in frame and "Span reports may overlap" in frame
            and day_values(frame, "0") == reported_days
            and len(day_values(frame, "Unreported")) == 5,
            "two explicit zero output-token dates and five unreported dates",
        )
        assert PRIVATE not in tui.screen.contents()
        tui.quit()

def pairs(**entries):
    result = []
    for key, value in entries.items():
        kind = "intValue" if isinstance(value, int) else "stringValue"
        result.append({"key": key, "value": {kind: str(value)}})
    return result


def attributed(session, amounts, currency=None, cost=None):
    values = {"session.id": session, **amounts}
    if currency is not None:
        values["agenturm.reported_cost.currency"] = currency
        values["agenturm.reported_cost.amount"] = str(cost)
    return values


def span(identifier, name, at, parent=None, *, attrs, status=1):
    record = {
        "traceId": TRACE, "spanId": identifier * 16, "name": name,
        "startTimeUnixNano": str(at - 100), "endTimeUnixNano": str(at),
        "status": {"code": status}, "attributes": pairs(**attrs),
    }
    if parent is not None:
        record["parentSpanId"] = parent * 16
    return record


def traces(at):
    observations = [
        span("1", "agent.run", at - DAY_NS, attrs=attributed(
            SESSION, {"gen_ai.usage.input_tokens": 400}, "USD", "1000"
        )),
        span("2", SENTINEL, at - DAY_NS, "1", attrs=attributed(
            SESSION, {"gen_ai.usage.input_tokens": 100, "gen_ai.usage.output_tokens": 8},
            "USD", "99",
        )),
        span("3", SENTINEL, at, "2", attrs=attributed(
            SESSION, {"gen_ai.usage.input_tokens": 3}, "EUR", "0.0025"
        ), status=2),
        span("4", SENTINEL, at + 1, "1", attrs=attributed(
            SESSION, {"gen_ai.usage.input_tokens": 0}, "USD", "0"
        )),
    ]
    return {"resourceSpans": [{"scopeSpans": [{"spans": observations}]}]}


def log_record(session, id_value, at, input_count, severity, currency=None, cost=None):
    attrs = attributed(
        session, {"observatory.record_id": id_value, "gen_ai.usage.input_tokens": input_count},
        currency, cost,
    )
    return {"eventName": "agent.activity", "timeUnixNano": str(at),
            "severityNumber": severity, "attributes": pairs(**attrs),
            "body": {"stringValue": SENTINEL}}


def usage_admission(app, root, db, env, timestamp):
    process = subprocess.Popen(
        [app, "serve-traces", "--listen", "127.0.0.1:0", "--db", db, "--source-id", "otlp.usage-fixture"],
        env=env, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        trace_url = read_line(process).split()[-1]
        log_url = trace_url.replace("/v1/traces", "/v1/logs")
        body = json.dumps(traces(timestamp)).encode()
        code, response = post(trace_url, body, "application/json")
        assert code == 200 and json.loads(response) == {}, ("trace ingestion", code)
        logs = {"resourceLogs": [{"scopeLogs": [{"logRecords": [
            log_record(SESSION, "same-session", timestamp, 99, 17),
            log_record(LOG_SESSION, "only-log", timestamp, 7, 17, "USD", "1.25"),
        ]}]}]}
        for _ in range(2):
            code, response = post(log_url, json.dumps(logs).encode(), "application/json")
            assert code == 200 and SENTINEL.encode() not in response, ("log ingestion", code)
    finally:
        stop(process)


def browse_usage(app, root, db, env, size, at):
    width, height = size
    day = datetime.fromtimestamp(at // 1_000_000_000, timezone.utc).date().isoformat()
    with Cockpit(app, env, root, width, height, db=db) as tui:
        tui.markers("command center", "Agenturm command center")
        tui.key(b"U")
        tui.markers("Usage", "7 UTC days", "Native usage reports")
        def watch(mode, label, amount):
            tui.until(
                lambda frame: mode in frame and label in frame and day in day_values(frame, amount)
                and SENTINEL not in frame,
                f"{mode} {label} exact day value",
            )
        tui.key(b"s")
        tui.markers("raw span mode", "Span reports may overlap")
        tui.key(b"s")
        trace_mode = "Retained trace summaries"
        watch(trace_mode, "Input tokens", "3")
        tui.key(b"t")
        watch(trace_mode, "Output tokens", "8")
        for label, amount in (
            ("Recorded traces", "1"),
            ("Reported error traces", "1"),
            ("Traces with cost gaps", "1"),
            ("EUR reported cost", "0.0025"),
        ):
            tui.key(b"n")
            watch(trace_mode, label, amount)
        tui.key(b"C")
        watch(trace_mode, "USD reported cost", "0")
        tui.key(b"s")
        log_mode = "Recorded log-only activity"
        tui.markers("log mode", log_mode)
        for label, amount in (("Activity logs", "1"), ("Reported error logs", "1"), ("USD reported cost", "1.25")):
            tui.key(b"n")
            watch(log_mode, label, amount)
        tui.quit()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--fixture", default=DEFAULT_FIXTURE, type=Path,
                        help="Directory with sdk-metrics.json and sdk-metrics.pb")
    parser.add_argument("--only", choices=("metrics", "usage", "all"), default="all")
    opts = parser.parse_args()
    app = opts.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="agenturm-native-analytics-") as path:
        root = Path(path)
        env = {**os.environ, "HOME": str(root), "XDG_STATE_HOME": str(root / "state"),
               "XDG_CONFIG_HOME": str(root / "config"), "TERM": "xterm-256color", "NO_COLOR": "1"}
        if opts.only in ("metrics", "all"):
            db = root / "metric-history.sqlite"
            fixture = opts.fixture.resolve(strict=True)
            reported_days = metric_admission(app, root, env, db, fixture, "otlp.metric-acceptance")
            for size in SIZES:
                browse_metrics(app, root, env, db, size, reported_days)
            print("PASS: metric wire forms, immutable replay, exact exemplar and reported zeros at 120/80/60")
        if opts.only in ("usage", "all"):
            db = root / "usage-history.sqlite"
            now = time.time_ns() - 3_000_000_000
            usage_admission(app, root, db, env, now)
            for size in SIZES:
                browse_usage(app, root, db, env, size, now)
            print("PASS: distinct trace counts, per-currency reported costs and log overlap at 120/80/60")


if __name__ == "__main__":
    main()
