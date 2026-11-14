"""Explicit Git binding, immutable draft approval and bounded overlap evidence."""
from copy import deepcopy
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from observatory.review_continuity import (bind_workspace,capture_review_workspace,freeze_workspace,
    editable_draft,overlap_observations,_safe_git,source_scope,policy_checker)
from observatory.saved_tasks import TaskStore,TaskError,TaskConflict,render_handoff
from observatory.ui_saved_tasks import capture_selected_run
from tests.test_turn_task_continuity import turn_capture


def projection():return turn_capture()[2]


class ReviewContinuityTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name);self.repo=self.root/'checkout';self.repo.mkdir()
        self.git('init','-q');self.git('config','user.name','Fixture');self.git('config','user.email','fixture@example.invalid')
        (self.repo/'file.py').write_text('original\n');self.git('add','.');self.git('commit','-qm','base')

    def git(self,*args):return subprocess.run(['git','-C',str(self.repo),*args],capture_output=True,text=True,check=True).stdout.strip()

    def frozen(self,p=None):
        p=p or projection();return freeze_workspace(p,capture_review_workspace(p,bind_workspace(p,str(self.repo))))

    def test_receiver_cwd_and_loopback_do_not_select_a_local_checkout(self):
        owner,item,p=turn_capture();owner.feed.api.endpoint='http://127.0.0.1:4318'
        item['working_directory']=str(self.repo)
        context=capture_selected_run(owner,item_override=item,review_snapshot=p)
        self.assertEqual(context['working_directory'],str(self.repo));self.assertEqual(context['local_workspace'],'')
        frozen=self.frozen(p)
        explicit=capture_selected_run(owner,item_override=item,review_snapshot=frozen)
        self.assertEqual(explicit['local_workspace'],str(self.repo))
        with self.assertRaises(TaskError):bind_workspace(p,'')

    def test_mapping_and_saved_base_are_exact_scope_owned(self):
        p=projection();binding=bind_workspace(p,str(self.repo));first=capture_review_workspace(p,binding)
        base=first['snapshot']['base_commit']
        (self.repo/'file.py').write_text('committed later\n');self.git('add','.');self.git('commit','-qm','later')
        (self.repo/'staged.txt').write_text('staged\n');self.git('add','staged.txt')
        (self.repo/'file.py').write_text('worktree\n');(self.repo/'untracked.txt').write_text('UNTRACKED_PRIVATE_CONTENT')
        later=capture_review_workspace(p,binding,first)
        self.assertEqual(later['snapshot']['base_commit'],base);self.assertNotEqual(later['snapshot']['head'],base)
        self.assertIn('worktree',later['snapshot']['diff']);self.assertNotIn('UNTRACKED_PRIVATE_CONTENT',json.dumps(later))
        changed=deepcopy(p);changed['scope']['agent_id']='other'
        with patch('observatory.review_continuity.capture_workspace') as capture:
            with self.assertRaises(TaskError):capture_review_workspace(changed,binding)
            capture.assert_not_called()

    def test_multiline_and_quoted_credentials_disable_safe_equivalence(self):
        cases=['-----BEGIN PRIVATE KEY-----\nPRIVATE_KEY_BYTES\n-----END PRIVATE KEY-----\n',
               '+{"password":"PRIVATE_PASSWORD_VALUE"}\n',
               '+{"api\\u005fkey":"PRIVATE_PASSWORD_VALUE"}\n']
        for diff in cases:
            value=_safe_git(dict(diff=diff,stat='',files=[],branch='main',workspace='/work',repo_root='/work',
                base_commit='a'*40,head='a'*40,complete=True,fingerprint='raw-secret-hash',warnings=[]))
            self.assertNotIn('PRIVATE_KEY_BYTES',json.dumps(value));self.assertNotIn('PRIVATE_PASSWORD_VALUE',json.dumps(value))
            self.assertNotIn('fingerprint',value);self.assertFalse(value['complete'])
        value=_safe_git(dict(diff='safe',stat='',files=[dict(path='api_key=PRIVATE_PASSWORD_VALUE')],branch='main',complete=True,warnings=[]))
        self.assertNotIn('PRIVATE_PASSWORD_VALUE',json.dumps(value));self.assertNotIn('fingerprint',value)

    def test_drafting_is_not_a_saved_task_and_approval_preserves_authored_fields(self):
        store=TaskStore(self.root/'tasks.db')
        task=store.create('Human title',str(self.repo),'Human notes')
        task=store.update_brief(task['id'],{'intent':'Human intent'})
        task=store.add_decision(task['id'],kind='rejected',text='Rejected approach',rationale='Human reason')
        before=deepcopy(task);draft=editable_draft(self.frozen(),task)
        self.assertIn('REJECTED: Rejected approach',draft['text'])
        self.assertEqual(store.get(task['id']),before)
        with self.assertRaises(TaskError):store.approve_continuity(task['id'],draft,expected_revision=task['revision'])
        approved=store.approve_continuity(task['id'],draft,text='Reviewed continuation',expected_revision=task['revision'],source_allowed=True)
        for key in ('title','workspace','notes','brief','decisions'):self.assertEqual(approved[key],before[key])
        self.assertEqual(approved['continuity_drafts'][0]['text'],'Reviewed continuation')
        self.assertEqual(approved['continuity_drafts'][0]['review_snapshot'],draft['review'])
        with self.assertRaises(TaskConflict):store.approve_continuity(task['id'],draft,expected_revision=before['revision'],source_allowed=True)
        self.assertEqual(len(store.get(task['id'])['continuity_drafts']),1)
        offline=render_handoff(approved,'json')
        self.assertIn('Human notes',offline);self.assertNotIn('Reviewed continuation',offline)
        self.assertNotIn('workspace_observation',offline)
        online=render_handoff(approved,source_policy=lambda scope:True)
        self.assertIn('Reviewed continuation',online);self.assertIn('Human notes',online)

    def test_new_task_approval_is_atomic_and_keeps_whole_turn(self):
        store=TaskStore(self.root/'tasks.db');draft=editable_draft(self.frozen())
        with self.assertRaises(TaskError):store.create('New',continuity_draft=draft)
        self.assertEqual(store.count(),0)
        task=store.create('New',continuity_draft=draft,source_allowed=True)
        self.assertEqual(task['notes'],'');self.assertEqual(len(task['continuity_drafts']),1)
        self.assertEqual(task['links'][0]['turn_id'],draft['scope']['turn_id'])
        self.assertEqual(len(task['links'][0]['checks']),2)
        self.assertEqual(TaskStore(store.path).get(task['id'])['continuity_drafts'],task['continuity_drafts'])

    def pair(self):
        left=self.frozen();right=deepcopy(left);right['scope']['agent_id']='other'
        for row in right['checks']:row['source']['agent_id']='other'
        right['workspace_observation']['binding']['scope']=source_scope(right)
        left['edits']=[dict(path='new.py',from_path='old.py',action='rename',source={'time_unix_nano':10})]
        right['edits']=[dict(path='old.py',action='edit',source={'time_unix_nano':30})]
        check=deepcopy(left['checks'][0]);check.update(command='pytest',completed_ns=20,result='passed');check['source']['time_unix_nano']=20
        left['checks']=[check];right['checks']=[]
        return left,right

    def test_overlap_and_chronology_are_symmetric_and_do_not_infer_historical_branch(self):
        left,right=self.pair();forward=overlap_observations([left,right]);back=overlap_observations([right,left])
        self.assertEqual(len(forward['observations']),2);self.assertEqual(len(back['observations']),2)
        self.assertTrue(all(row['branch_basis']=='historical branch/workspace unverified' for row in forward['observations']))
        left['edits'][0]['recorded_branch']='one';left['checks'][0]['recorded_branch']='one';right['edits'][0]['recorded_branch']='two'
        self.assertEqual(overlap_observations([left,right])['observations'],[])
        right['edits'][0]['effect']='attempt; effect unknown'
        self.assertEqual(overlap_observations([left,right])['observations'],[])

    def test_different_worktrees_unknown_mapping_and_clipped_inputs_stay_disclosed(self):
        left,right=self.pair();right['workspace_observation']['binding']['workspace']='/different-worktree'
        self.assertEqual(overlap_observations([left,right])['observations'],[])
        right['workspace_observation']['binding']['workspace']=str(self.repo)
        right['workspace_observation']['binding']['kind']='loopback-guess'
        self.assertEqual(overlap_observations([left,right])['observations'],[])
        many=[deepcopy(left) for _ in range(21)]
        result=overlap_observations(many)
        self.assertTrue(result['partial']);self.assertLessEqual(len(result['observations']),100)

    def test_policy_checker_rejects_metadata_only_and_receiver_replacement(self):
        p=projection()
        class Api:
            health=dict(receiver_id='receiver',local_agents={})
            def get(self,path):return self.health
        api=Api();check=policy_checker(api,p);self.assertTrue(check())
        api.health=dict(receiver_id='receiver',local_agents={'capture_content':False})
        with self.assertRaises(TaskError):check()
        api.health=dict(receiver_id='other',local_agents={})
        with self.assertRaises(TaskError):check()


if __name__=='__main__':unittest.main()
