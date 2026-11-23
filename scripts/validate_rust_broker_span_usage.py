"""Exercise one maximum trace/usage/cost batch against packaged Linux binaries."""

import argparse
from collections import Counter
import grp
import json
import os
from pathlib import Path
import pwd
import stat
import subprocess
import tempfile
import time


SOURCE = "otlp.maximum-usage"
TARGET = "harness.fixture"
SESSION = "budget-session"
TURN = "budget-turn"
WORKER = "worker-budget"
TRACE = "31" * 16
PRIVATE = "synthetic-private-unprojected-maximum-input"


def as_user(uid, gid):
    def drop():
        os.setgroups([])
        os.setgid(gid)
        os.setuid(uid)
    return drop


def write_fixture(path, spans):
    path.write_text(
        json.dumps({"resourceSpans": [{"scopeSpans": [{"spans": spans}]}]},
                   separators=(",", ":")),
        encoding="ascii",
    )
    path.chmod(0o644)


def spans():
    end = time.time_ns()
    result = []
    for count in range(1, 1025):
        result.append({
            "traceId": TRACE, "spanId": f"{count:016x}",
            "name": PRIVATE, "startTimeUnixNano": str(end - 100),
            "endTimeUnixNano": str(end), "status": {"code": 1},
            "attributes": [
                {"key": key, "value": {kind: value}}
                for key, kind, value in [
                    ("session.id", "stringValue", SESSION),
                    ("agenturm.source.id", "stringValue", TARGET),
                    ("agenturm.turn.id", "stringValue", TURN),
                    ("agenturm.worker.id", "stringValue", WORKER),
                    ("llm.cost_usd", "intValue", "0"),
                    ("gen_ai.usage.input_tokens", "intValue", "0"),
                    ("llm.token_count.completion", "intValue", str(count)),
                    ("gen_ai.usage.cache_read.input_tokens", "intValue", "5"),
                    ("gen_ai.input.messages", "stringValue", PRIVATE),
                ]
            ],
        })
    return result


def run_packaged(binaries):
    occupied_uids = {account.pw_uid for account in pwd.getpwall()}
    occupied_gids = {account.gr_gid for account in grp.getgrall()}
    broker, collector, reader = [uid for uid in range(45000, 46000)
                                 if uid not in occupied_uids][:3]
    client_gid = next(gid for gid in range(45000, 46000) if gid not in occupied_gids)
    app = binaries / "agenturm"
    service_binary = binaries / "agenturm-storage-service"
    with tempfile.TemporaryDirectory(prefix="agenturm-broker-maximum-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        data = root / "private"
        transport = root / "transport"
        for path, uid, gid, mode in [
            (data, broker, broker, 0o700),
            (transport, broker, client_gid, 0o2710),
        ]:
            path.mkdir()
            os.chown(path, uid, gid)
            path.chmod(mode)
        socket = transport / "agenturm-native-storage-v1.sock"
        config = root / "broker.json"
        config.write_text(json.dumps({
            "version": 1, "socket": str(socket), "receiver_id": "receiver:maximum-usage",
            "service_uid": broker, "service_gid": broker,
        }))
        config.chmod(0o644)

        def cli(uid, verb, *args, succeeds=True):
            result = subprocess.run(
                [app, "broker", verb, "--broker-config", config, *args],
                cwd=root, capture_output=True, text=True, timeout=20,
                preexec_fn=as_user(uid, client_gid),
            )
            assert (result.returncode == 0) == succeeds, (verb, result.returncode, result.stderr)
            assert PRIVATE not in result.stdout and PRIVATE not in result.stderr
            return result.stdout

        def page(*options):
            return json.loads(cli(reader, "query", "--harness-id", SOURCE,
                                  "--session-id", SESSION, "--limit", "1024", *options))

        rows = spans()
        source_file = root / "traces.json"
        malformed_file = root / "invalid.json"
        write_fixture(source_file, rows)
        invalid = [*rows[:-1], {**rows[-1], "spanId": "bad-span-id"}]
        write_fixture(malformed_file, invalid)
        with subprocess.Popen(
            [service_binary, "--data-root", data, "--transport-dir", transport,
             "--receiver-id", "receiver:maximum-usage", "--client-uid", str(collector),
             "--client-gid", str(client_gid), "--native-reader", f"{reader}:{client_gid}",
             "--native-sources", ",".join([SOURCE, TARGET])],
            cwd=root, preexec_fn=as_user(broker, broker),
        ) as service:
            try:
                deadline = time.monotonic() + 10
                while not socket.exists() or stat.S_IMODE(socket.stat().st_mode) != 0o660:
                    assert service.poll() is None and time.monotonic() < deadline, "Broker did not start"
                    time.sleep(0.04)

                assert page()["observations"] == []
                cli(collector, "import-traces", "--file", malformed_file, "--source-id", SOURCE,
                    succeeds=False)
                assert page()["observations"] == []
                cli(reader, "import-traces", "--file", source_file, "--source-id", SOURCE,
                    succeeds=False)
                assert page()["observations"] == []

                receipt = cli(collector, "import-traces", "--file", source_file, "--source-id", SOURCE)
                assert "Imported 4096 observations" in receipt, receipt
                first = observed = page()
                snapshot = first["next_before"]["snapshot_id"]
                seen = {}
                pages = 0
                while True:
                    pages += 1
                    assert pages <= 4
                    for row in observed["observations"]:
                        assert row["harness_id"] == SOURCE
                        assert row["identity"] == {
                            "scope": "session", "session_id": SESSION, "worker_id": WORKER,
                            "turn_id": TURN, "operation_id": None,
                        }
                        fact = row["observation"]
                        kind = fact["kind"]
                        identity = fact if kind in {"trace_span", "trace_span_summary"} else fact["span"]
                        assert identity["trace_id"] == TRACE
                        span = identity["span_id"]
                        assert (kind, span) not in seen
                        seen[kind, span] = fact
                    position = observed["next_before"]
                    if position is None:
                        break
                    assert position["snapshot_id"] == snapshot
                    observed = page(
                        "--snapshot-id", position["snapshot_id"],
                        "--before-timestamp-ns", position["observed_at_unix_ns"],
                        "--before-harness-id", position["harness_id"],
                        "--before-event-id", position["event_id"],
                        "--before-revision", str(position["revision"]),
                    )
                assert Counter(kind for kind, _ in seen) == {
                    "trace_span": 1024, "span_token_usage": 1024, "reported_cost": 1024, "trace_span_summary": 1024,
                }
                for (_, span), fact in seen.items():
                    if fact["kind"] == "trace_span":
                        assert fact["target_source_id"] == TARGET
                    elif fact["kind"] == "reported_cost":
                        assert (fact["currency"], fact["nanounits"]) == ("USD", "0")
                    elif fact["kind"] == "trace_span_summary":
                        assert fact["agent_run"] is False and fact["reported_error"] is False
                    else:
                        assert fact["target_source_id"] == TARGET
                        assert fact["input_tokens"] == 0
                        assert fact["output_tokens"] == int(span, 16)
                        assert fact["cache_read_input_tokens"] == 5
                        assert "total_tokens" not in fact and "reasoning_output_tokens" not in fact
                replay = cli(collector, "import-traces", "--file", source_file, "--source-id", SOURCE)
                assert "Imported 0 observations" in replay, replay
                assert page() == first
            finally:
                if service.poll() is None:
                    service.terminate()
                    try:
                        assert service.wait(timeout=10) == 0
                    except subprocess.TimeoutExpired:
                        service.kill()
                        service.wait()
                        raise
        print(
            f"Packaged broker committed 3,072 rows from 1,024 spans in a {source_file.stat().st_size}-byte "
            f"export. Reader verified all kinds in {pages} pages. Malformed and reader writes left no rows. Replay was a no-op."
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    options = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("Run in a disposable Linux guest with separate UID support")
    run_packaged(options.bin_dir.resolve(strict=True))
