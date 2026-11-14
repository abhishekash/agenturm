"""Headless egress cannot discard a frozen cockpit capture generation."""
import argparse
import gc
from contextlib import redirect_stdout,redirect_stderr
from copy import deepcopy
from io import StringIO
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
import warnings
from unittest.mock import patch

from observatory.review_continuity import policy_checker,editable_draft,content_allowed
from observatory.saved_tasks import TaskStore,TaskError
from observatory.task_cli import add_task_parser,run_task_command
from tests.test_turn_task_continuity import turn_capture

TOKEN='feed-safety-v1:0:123456789:'+('a'*64)

class AllowedAPI:
    def get(self,path):
        if path!='/api/health':raise AssertionError(path)
        return dict(receiver_id='receiver',local_agents=dict(capture_content=True,sessions=[dict(
            service='opencode',agent_id='worker',session_id='session',capture_content=True)]))


class HeadlessCaptureBindingTests(unittest.TestCase):
    def projection(self,bound):
        p=deepcopy(turn_capture()[2])
        if bound:p['window']['capture_revision']=TOKEN
        return p

    def test_checker_rejects_unknown_client_generation_but_new_headless_evidence_works(self):
        api=AllowedAPI()
        self.assertTrue(policy_checker(api,self.projection(False))())
        with self.assertRaises(TaskError):policy_checker(api,self.projection(True))()
        malformed=self.projection(True);malformed['window']['capture_revision']='feed-safety-v1:broken'
        with self.assertRaises(TaskError):policy_checker(api,malformed)()

    def test_unusable_frozen_projection_withholds_instead_of_crashing_policy_factory(self):
        p=self.projection(False);p['window']['capture_revision']=False
        def policy(scope):return True
        policy.for_projection=lambda projection:(lambda scope:policy_checker(AllowedAPI(),projection)())
        self.assertFalse(content_allowed(p,policy))
        policy.for_projection=lambda projection:policy_checker(AllowedAPI(),projection)
        self.assertFalse(content_allowed(p,policy))

    def test_actual_cli_stream_and_file_handoff_withhold_old_bound_draft_and_keep_authored_notes(self):
        with TemporaryDirectory()as directory:
            root=Path(directory);store=TaskStore(root/'tasks.db')
            tasks=[]
            for bound in (True,False):
                draft=editable_draft(self.projection(bound));draft['text']='PRIVATE_DERIVED_DRAFT'
                tasks.append((bound,store.create('Human title',notes='HUMAN_AUTHORED_NOTES',continuity_draft=draft,source_allowed=True)))
            parser=argparse.ArgumentParser();sub=parser.add_subparsers();task_parser=add_task_parser(sub)
            for bound,task in tasks:
                for export in (False,True):
                    arguments=['task','--tasks-db',str(store.path),'handoff',task['id'],'--endpoint','http://127.0.0.1:14318','--format','json']
                    destination=root/('bound'if bound else 'headless')
                    if export:arguments+=['--output',str(destination)+'.json']
                    args=parser.parse_args(arguments);out=StringIO();err=StringIO()
                    # Collect earlier fixtures' unreachable objects before
                    # treating stderr as this command's output. Their warnings
                    # remain visible to the test runner. Warnings from this
                    # invocation, including deferred connection cleanup, fail.
                    gc.collect()
                    with warnings.catch_warnings(record=True) as command_warnings:
                        warnings.simplefilter('always',ResourceWarning)
                        with patch('observatory.task_cli._source_api',return_value=AllowedAPI()),redirect_stdout(out),redirect_stderr(err):
                            code=run_task_command(args,task_parser)
                        gc.collect()
                    self.assertEqual([warnings.formatwarning(w.message,w.category,w.filename,w.lineno)
                        for w in command_warnings],[])
                    self.assertEqual((code,err.getvalue()),(0,''))
                    text=''.join(destination.with_suffix(suffix).read_text()for suffix in ('.md','.html','.json'))if export else out.getvalue()
                    self.assertIn('HUMAN_AUTHORED_NOTES',text)
                    if bound:self.assertNotIn('PRIVATE_DERIVED_DRAFT',text)
                    else:self.assertIn('PRIVATE_DERIVED_DRAFT',text)
