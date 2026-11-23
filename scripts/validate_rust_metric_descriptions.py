"""Verify authenticated metric descriptions without rewriting existing points."""
import argparse
from pathlib import Path
import http.client
import json
import struct
import os
import re
import secrets
import select
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument("--app", type=Path, required=True)
args = parser.parse_args()
checkout = args.checkout.resolve(strict=True)
app = args.app.resolve(strict=True)
fixture=checkout/'rust/agenturm-app/src/trace_receiver/metrics/fixtures'
source='otlp.metrics-final-proof'

with tempfile.TemporaryDirectory(prefix='agenturm-metrics-http-proof-') as folder:
    root=Path(folder)
    token_path=root/'token'
    secret=secrets.token_hex(32)
    token_path.write_text(secret+'\n'); token_path.chmod(0o600)
    db=root/'history.sqlite'
    authority={'HOME':str(root),'XDG_CONFIG_HOME':str(root/'config'),'XDG_STATE_HOME':str(root/'state'),'PATH':os.defpath}
    proc=subprocess.Popen([app,'serve-traces','--listen','127.0.0.1:0','--db',db,
                           '--source-id',source,'--payload-scope','standalone','--token-file',token_path],
                          cwd=root,env=authority,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    try:
        ready=select.select([proc.stdout],[],[],10)[0]
        assert ready, 'Receiver did not report startup'
        started=proc.stdout.readline()
        match=re.fullmatch(r'Listening for OTLP traces at http://127\.0\.0\.1:(\d+)/v1/traces\n',started)
        assert match, 'Receiver did not publish its bound loopback socket'
        port=int(match[1]); assert port > 0
        def post(raw,media,token=None):
            conn=http.client.HTTPConnection('127.0.0.1',port,timeout=10)
            headers={'Content-Type':media}
            if token is not None: headers['Authorization']='Bearer '+token
            try:
                conn.request('POST','/v1/metrics',body=raw,headers=headers)
                response=conn.getresponse()
                return response.status,response.read()
            finally:
                conn.close()
        def state():
            with sqlite3.connect(db.as_uri()+'?mode=ro',uri=True,timeout=2) as store:
                rows = store.execute(
                    "SELECT event_id,event_digest,kind_json,h.sequence "
                    "FROM agenturm_observations JOIN agenturm_history_batches h USING(request_id) "
                    "WHERE source_id=? ORDER BY event_id",
                    [source],
                ).fetchall()
                cursor, = store.execute(
                    "SELECT cursor FROM agenturm_source_state WHERE source_id=?", [source]
                ).fetchone()
                version, = store.execute("PRAGMA user_version").fetchone()
                assert version == 21
                points = tuple(row for row in rows if json.loads(row[2])["kind"] == "metric_point")
                descriptors = tuple(row for row in rows if json.loads(row[2])["kind"] == "metric_descriptor")
                return points, descriptors, cursor

        def json_post(payload):
            return post(json.dumps(payload).encode(), 'application/json', secret)

        def encode_field(tag, value):
            assert len(value) < 128
            return bytes([tag << 3 | 2, len(value)]) + value

        def described_proto(name, description, timestamp):
            field = encode_field
            point = bytes([3 << 3 | 1]) + struct.pack("<Q", timestamp)
            point += bytes([6 << 3 | 1]) + struct.pack("<q", 12)
            metric = field(1, name.encode()) + field(2, description.encode()) + field(5, field(1, point))
            return field(1, field(2, field(2, metric)))

        def described_json(name, description, timestamp):
            return {"resourceMetrics": [{"scopeMetrics": [{"metrics": [
                {"name": name, "description": description,
                 "gauge": {"dataPoints": [{"timeUnixNano": str(timestamp), "asInt": "12"}]}}
            ]}]}]}

        body = (fixture/'sdk-metrics.json').read_bytes()
        encoded = (fixture/'sdk-metrics.pb').read_bytes()
        assert post(body, 'application/json')[0] == 401
        assert post(body, 'application/json', '0'*64)[0] == 401
        assert not db.exists()
        assert post(body, 'application/json', secret) == (200, b'{}')
        before = state()
        assert len(before[0]) == 7 and not before[1]
        assert post(encoded, 'application/x-protobuf', secret) == (200, b'')
        assert state() == before
        assert post(b'{ malformed', 'application/json', secret)[0] == 400
        assert state() == before

        changed = json.loads(body)
        metric = changed["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]
        assert metric["name"] == "agents.gauge"
        metric["description"] = "Archived request duration"
        assert json_post(changed) == (200, b'{}')
        first = state()
        assert first[0] == before[0]
        assert len(first[1]) == 2
        first_ids = {row[0] for row in first[0]}
        for row in first[1]:
            descriptor = json.loads(row[2])
            assert descriptor["description"] == "Archived request duration"
            assert descriptor["point_event_id"] in first_ids
            assert row[3] > max(point[3] for point in first[0])
        assert json_post(changed) == (200, b'{}') and state() == first

        name, description, timestamp = "crosswire.gauge", "Other instrument description", time.time_ns()
        assert post(described_proto(name, description, timestamp), 'application/x-protobuf', secret) == (200, b'')
        crosswire = state()
        assert len(crosswire[0]) == 8 and len(crosswire[1]) == 3
        assert json_post(described_json(name, description, timestamp)) == (200, b'{}')
        assert state() == crosswire

        assert json_post(described_json(name, "Updated instrument description", timestamp)) == (200, b'{}')
        revised = state()
        assert len(revised[1]) == 4 and revised[0] == crosswire[0]
        assert max(revised[1], key=lambda row: row[3])[2].find("Updated instrument description") >= 0
        assert json_post(described_json(name, "Authorization: dummy credential", timestamp)) == (200, b'{}')
        safe = state()
        assert safe[:2] == revised[:2]
        print("Authenticated /v1/metrics preserved seven old points; JSON/protobuf descriptor replay, description-only admission, revision and unsafe text omission passed")
    finally:
        if proc.poll() is None:
            proc.send_signal(signal.SIGINT)
            try: proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill(); proc.wait(timeout=5)
