"""Authenticated receiver policy checks around draft artifacts and approval."""
import argparse
from contextlib import redirect_stdout,redirect_stderr
from io import StringIO
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest

from observatory.task_cli import add_task_parser,run_task_command
from observatory.saved_tasks import TaskStore
from observatory.terminal import LocalReceiver
from tests.test_turn_task_continuity import turn_capture


class ContinuityCLI(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=Path(self.tmp.name)
        self.receiver=LocalReceiver(self.root/'events.db',0,'fixture-token',local_agents=False)
        self.addCleanup(self.receiver.close)
        self.endpoint='http://127.0.0.1:'+str(self.receiver.server.server_port)
        p=turn_capture()[2];p['scope']['receiver']=self.receiver.store.receiver_id
        self.review=self.root/'review.json';self.review.write_text(json.dumps(p))
        self.tasks=self.root/'tasks.db'

    def run_cli(self,*arguments):
        parser=argparse.ArgumentParser();task_parser=add_task_parser(parser.add_subparsers(dest='command'))
        args=parser.parse_args(['task','--tasks-db',str(self.tasks),*arguments])
        out=StringIO();error=StringIO()
        with redirect_stdout(out),redirect_stderr(error):code=run_task_command(args,task_parser)
        return code,out.getvalue(),error.getvalue()

    def test_draft_artifact_is_editable_but_task_is_written_only_after_approval(self):
        output=self.root/'draft.json'
        code,out,error=self.run_cli('draft','--review-file',str(self.review),'--endpoint',self.endpoint,'--token','fixture-token','--output',str(output))
        self.assertEqual(code,0,error);self.assertFalse(self.tasks.exists())
        self.assertTrue(output.with_suffix('.md').exists());self.assertTrue(output.with_suffix('.html').exists())
        draft=json.loads(output.read_text());draft['text']='Reviewed CLI continuation';output.write_text(json.dumps(draft))
        code,out,error=self.run_cli('approve-draft',str(output),'--endpoint',self.endpoint,'--token','fixture-token','--json')
        self.assertEqual(code,0,error);task=json.loads(out)
        self.assertEqual(task['notes'],'');self.assertEqual(task['continuity_drafts'][0]['text'],'Reviewed CLI continuation')
        self.assertEqual(len(task['links'][0]['checks']),2)
        code,offline,error=self.run_cli('show',task['id'],'--json')
        self.assertEqual(code,0,error);self.assertNotIn('Reviewed CLI continuation',offline)
        self.assertEqual(self.run_cli('draft','--review-file',str(self.review),'--endpoint',self.endpoint,'--token','fixture-token','--output',str(output))[0],2)

    def test_auth_receiver_and_capture_denials_produce_no_artifact_or_task(self):
        output=self.root/'denied.json'
        for token in ('wrong-token',):
            code,_,_=self.run_cli('draft','--review-file',str(self.review),'--endpoint',self.endpoint,'--token',token,'--output',str(output))
            self.assertEqual(code,2);self.assertFalse(output.exists());self.assertFalse(self.tasks.exists())
        self.receiver.store.local_agent_monitor=SimpleNamespace(status=lambda:dict(capture_content=False))
        code,_,error=self.run_cli('draft','--review-file',str(self.review),'--endpoint',self.endpoint,'--token','fixture-token','--output',str(output))
        self.assertEqual(code,2,error);self.assertFalse(output.exists());self.assertFalse(self.tasks.exists())
        p=json.loads(self.review.read_text());p['scope']['receiver']='different';self.review.write_text(json.dumps(p))
        self.assertEqual(self.run_cli('draft','--review-file',str(self.review),'--endpoint',self.endpoint,'--token','fixture-token')[0],2)


if __name__=='__main__':unittest.main()
