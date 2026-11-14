"""Real authenticated receiver reads and metadata writes without a TUI import."""
import argparse
from contextlib import redirect_stdout,redirect_stderr
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import threading
import time
import unittest
from unittest.mock import patch
from http.server import ThreadingHTTPServer

from observatory.attention_cli import add_attention_parser,collect_snapshot,run_watch
from observatory.attention_runner import inventory_ready
from observatory.core import Store,normalize_native
from observatory.native_lifecycle import (enqueue,flush,normalize_event,post_event,validate_binding,verify_binding)
from observatory.server import handler_for
from observatory.terminal import Api
from observatory.workflow_store import WorkflowStore
from observatory.agent_hierarchy import canonical_agent_id
from tests.test_native_lifecycle import binding,event


class AttentionCLIReceiverTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'receiver.db');self.addCleanup(self.store.close)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),handler_for(self.store,'secret'))
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.addCleanup(self.server.server_close);self.addCleanup(self.server.shutdown)
        self.endpoint='http://127.0.0.1:'+str(self.server.server_port)
        self.api=Api(self.endpoint,token='secret');self.state=Path(self.tmp.name)/'workflow.db'
        self.actor=canonical_agent_id('SDK','fixture','worker');self.trace='a'*32
        self.store.ingest([normalize_native(dict(trace_id=self.trace,span_id='1'*16,name='Bash',
            start_unix_nano=time.time_ns()-1000,end_unix_nano=time.time_ns(),status='OK',events=[],
            attributes={'agent.id':self.actor,'session.id':'session','tool.name':'Bash','gen_ai.tool.type':'shell',
                'process.command_line':'pytest tests','process.exit.code':0},
            resource_attributes={'observatory.source_id':'source'}),'codex')])

    def args(self,desktop=False):
        return argparse.Namespace(endpoint=self.endpoint,state=self.state,interval=1,desktop=desktop,once=True)

    def native_binding(self):
        return validate_binding(dict(binding(),endpoint=self.endpoint,receiver_id=self.store.receiver_id,agent_id=self.actor))

    def native_event(self,**changes):
        return dict(event(),receiver_id=self.store.receiver_id,agent_id=self.actor,**changes)

    def test_actual_watch_baseline_then_native_request_delivers_once_without_tui(self):
        output=io.StringIO();self.assertEqual(run_watch(self.args(),api=self.api,stdout=output),0)
        first=json.loads(output.getvalue());self.assertTrue(first['ready'])
        configured=self.native_binding();verify_binding(self.api,configured)
        workflow=WorkflowStore(self.state)
        payload=self.native_event(time_unix_nano=time.time_ns(),request_id='native-request')
        enqueue(workflow,configured,normalize_event(payload,configured))
        self.assertEqual(flush(workflow,configured,lambda log:post_event(self.api,configured,log)),1)
        logs=self.store.logs();self.assertEqual(len(logs),1)
        self.assertEqual(logs[0]['attributes']['observatory.native.request_id'],'native-request')
        calls=[];notify=lambda *values:calls.append(values) or 'Desktop notification delivered'
        output=io.StringIO();self.assertEqual(run_watch(self.args(True),api=self.api,notify=notify,stdout=output),0)
        pending=json.loads(output.getvalue())['pending'];self.assertEqual(len([row for row in pending if row['kind']=='approval']),1)
        self.assertEqual(len(calls),1)
        self.assertEqual(run_watch(self.args(True),api=self.api,notify=notify,stdout=io.StringIO()),0)
        self.assertEqual(len(calls),1)
        # A receiver-scoped write is rejected before it reaches another DB namespace.
        with self.assertRaises(Exception):post_event(self.api,dict(configured,receiver_id='foreign'),logs[0])
        self.assertEqual(len(self.store.logs()),1)

    def test_incomplete_migration_or_namespace_read_never_marks_inventory_ready(self):
        healthy=collect_snapshot(self.api);self.assertTrue(inventory_ready(healthy))
        original=self.api.get
        def changed(path):
            value=original(path)
            if path.startswith('/api/sessions'):value['receiver_id']='foreign'
            return value
        with patch.object(self.api,'get',changed):
            capture=collect_snapshot(self.api);self.assertFalse(inventory_ready(capture))
            self.assertIn('sessions',capture.read_errors)

    def test_subprocess_headless_reader_does_not_import_textual(self):
        script='''import argparse,sys
from observatory.attention_cli import add_attention_parser,run_attention_command
parser=argparse.ArgumentParser();sub=parser.add_subparsers(dest="command");add_attention_parser(sub)
args=parser.parse_args(sys.argv[1:]);result=run_attention_command(args)
assert not any(name=="textual" or name.startswith("textual.") for name in sys.modules)
raise SystemExit(result)
'''
        result=subprocess.run([sys.executable,'-c',script,'attention','watch','--endpoint',self.endpoint,
            '--state',str(self.state),'--once'],env={**os.environ,'OBSERVATORY_TOKEN':'secret'},capture_output=True,text=True,timeout=15)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(json.loads(result.stdout)['ready'])
        self.assertNotIn('secret',result.stdout)

    def test_missing_state_status_is_read_only_and_does_not_claim_live_success(self):
        from observatory.attention_cli import run_attention_command
        path=Path(self.tmp.name)/'missing.db';out=io.StringIO()
        args=argparse.Namespace(attention_command='status',state=path,receiver=None)
        with redirect_stdout(out):self.assertEqual(run_attention_command(args),0)
        payload=json.loads(out.getvalue());self.assertFalse(payload['state_exists']);self.assertIsNone(payload['ready'])
        self.assertFalse(payload['live_read_performed']);self.assertFalse(path.exists())


if __name__=='__main__':unittest.main()
