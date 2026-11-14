"""Keyboard Git mapping and explicit reviewed-draft approval, with policy fences."""
import asyncio
from dataclasses import replace
from copy import deepcopy
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import threading
import time
import unittest
from unittest.mock import patch

from textual.app import App
from textual.widgets import Button,Input,RichLog,TextArea
from observatory.saved_tasks import TaskStore
from observatory.ui_review import ReviewScreen,WorkspaceMappingScreen
from observatory.ui_saved_tasks import TaskEditor,HandoffScreen,task_policy_signature,projection_policy_state
from observatory.workflow_evidence import ReviewCache,ExecutionScope
from observatory.terminal_ui import ObservatoryApp
from tests.test_turn_task_continuity import turn_capture


class Harness(ObservatoryApp):
    def __init__(self,owner,item):
        super().__init__(owner.feed,persist_preferences=False,task_store_path=owner.task_store_path)
        self.item=item;self.review_cache=owner.review_cache
    def on_mount(self):
        super().on_mount();self.refresh_timer.stop();self.push_screen(ReviewScreen(self,self.item))


class ReviewContinuityJourney(unittest.IsolatedAsyncioTestCase):
    def setup_owner(self,path):
        owner,item,p=turn_capture();owner.task_store_path=path
        item['_review_usage']=deepcopy(owner.feed.snapshot.sessions[0]['latest_run']['turn_usage'])
        owner.notify=lambda *args,**kwargs:None;owner.copy_to_clipboard=lambda text:None
        owner.review_cache=ReviewCache();owner.last_review={}
        scope=ExecutionScope(**p['scope'])
        owner.review_cache.get(scope,None,True).accept(dict(receiver_id='receiver',activity=owner.feed.snapshot.activity))
        return owner,item

    async def ready(self,pilot,predicate):
        end=time.monotonic()+8
        while not predicate() and time.monotonic()<end:await pilot.pause(.04)
        self.assertTrue(predicate())

    def repo(self,root):
        repo=root/'checkout';repo.mkdir()
        for args in (['init','-q'],['config','user.name','Fixture'],['config','user.email','fixture@example.invalid']):
            subprocess.run(['git','-C',str(repo),*args],check=True,capture_output=True)
        (repo/'a.py').write_text('before\n')
        subprocess.run(['git','-C',str(repo),'add','.'],check=True,capture_output=True)
        subprocess.run(['git','-C',str(repo),'commit','-qm','base'],check=True,capture_output=True)
        (repo/'a.py').write_text('after\n')
        return repo

    async def test_mapping_and_readable_git_receipt_at_review_floor(self):
        with TemporaryDirectory() as directory:
            root=Path(directory);repo=self.repo(root);owner,item=self.setup_owner(root/'tasks.db')
            item['working_directory']='/remote/unproven'
            app=Harness(owner,item)
            async with app.run_test(size=(60,18)) as pilot:
                await pilot.pause();screen=app.screen
                await pilot.press('g');await pilot.pause()
                self.assertIsInstance(app.screen,WorkspaceMappingScreen)
                self.assertEqual(app.screen.query_one(Input).value,'')
                app.screen.query_one(Input).value=str(repo)
                await pilot.press('ctrl+s')
                await self.ready(pilot,lambda:app.screen is screen and not screen.git_busy and screen.workspace_observation is not None)
                self.assertEqual(screen.projection['workspace_observation']['binding']['workspace'],str(repo))
                self.assertIn('+after',screen.projection['workspace_observation']['snapshot']['diff'])
                for button in screen.query('#review-controls Button'):
                    self.assertTrue(button.parent.region.contains_region(button.region),str((button.id,button.region,button.parent.region)))
                self.assertIn('FROZEN LOCAL GIT','\n'.join(line.text for line in screen.query_one(RichLog).lines))

    async def test_draft_edit_then_approve_creates_only_the_reviewed_version(self):
        with TemporaryDirectory() as directory:
            root=Path(directory);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            async with app.run_test(size=(60,20)) as pilot:
                await pilot.pause();await pilot.press('d')
                await self.ready(pilot,lambda:isinstance(app.screen,TaskEditor))
                editor=app.screen
                self.assertIn('CONTINUITY DRAFT',editor.query_one(TextArea).text)
                self.assertEqual(TaskStore(root/'tasks.db').count(),0)
                editor.query_one(TextArea).load_text('Reviewed continuation: rerun the affected checks.')
                await pilot.press('ctrl+s')
                await self.ready(pilot,lambda:app.screen is not editor)
                store=TaskStore(root/'tasks.db');task=store.get(store.list()[0]['id'])
                self.assertEqual(task['notes'],'')
                self.assertEqual(task['continuity_drafts'][0]['text'],'Reviewed continuation: rerun the affected checks.')
                self.assertEqual(len(task['links'][0]['checks']),2)
                self.assertTrue(task['links'][0]['turn_id'])

    async def test_source_revocation_blocks_draft_approval_without_task_mutation(self):
        with TemporaryDirectory() as directory:
            root=Path(directory);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();await pilot.press('d');await self.ready(pilot,lambda:isinstance(app.screen,TaskEditor))
                editor=app.screen
                owner.feed.snapshot=replace(owner.feed.snapshot,local_status={'capture_content':False})
                await pilot.press('ctrl+s');await self.ready(pilot,lambda:not editor.busy)
                self.assertIs(app.screen,editor);self.assertEqual(TaskStore(root/'tasks.db').count(),0)
                self.assertIn('Source capture',str(editor.query_one('#task-status').content))

    async def test_late_git_result_cannot_restore_revoked_content(self):
        gate=threading.Event();started=threading.Event()
        with TemporaryDirectory() as directory:
            root=Path(directory);repo=self.repo(root);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            from observatory.review_continuity import capture_review_workspace
            def capture(*args):started.set();gate.wait(5);return capture_review_workspace(*args)
            with patch('observatory.review_continuity.capture_review_workspace',capture):
                async with app.run_test(size=(80,24)) as pilot:
                    await pilot.pause();screen=app.screen
                    screen.workspace_selected(str(repo));await self.ready(pilot,started.is_set)
                    owner.feed.snapshot=replace(owner.feed.snapshot,local_status={'capture_content':False})
                    screen.apply_capture_policy();gate.set();await pilot.pause(.2)
                    self.assertIsNone(screen.workspace_observation)
                    self.assertNotIn('workspace_observation',screen.projection)
                    self.assertFalse(screen.git_busy)

    def peer_projection(self,p):
        p=deepcopy(p)
        p['parallel_path_observations']=dict(observations=[dict(paths=['PEER_PRIVATE_PATH'],agents=['worker','peer'],
            branch_basis='unknown',observation='Recorded overlap',sources=[dict(service='opencode',agent_id='peer',
            session_id='peer-session',trace_id='b'*32)])])
        return p

    def revoke_peer(self,owner,revision=2):
        owner.feed.snapshot=replace(owner.feed.snapshot,local_status={'sessions':[dict(service='opencode',
            agent_id='peer',session_id='peer-session',capture_content=False,capture_revision=revision)]})

    def test_all_source_policy_signature_tracks_peer_revision_and_receiver(self):
        from observatory.review_continuity import editable_draft
        with TemporaryDirectory() as directory:
            owner,item,p=turn_capture();p=self.peer_projection(p)
            task=TaskStore(Path(directory)/'tasks.db').create('Authored task',notes='Authored notes',
                continuity_draft=editable_draft(p),source_allowed=True)
            initial=task_policy_signature(owner,task)
            self.assertTrue(projection_policy_state(owner,p)[0])
            self.revoke_peer(owner)
            self.assertNotEqual(initial,task_policy_signature(owner,task))
            self.assertFalse(projection_policy_state(owner,p)[0])
            denied=task_policy_signature(owner,task)
            self.revoke_peer(owner,3);self.assertNotEqual(denied,task_policy_signature(owner,task))
            owner.feed.snapshot=replace(owner.feed.snapshot,receiver_id='replacement')
            self.assertFalse(projection_policy_state(owner,p)[0])
            # Parallel-only linked reviews use the same full contributor fence.
            linked={'links':[{'review_snapshot':p}]}
            replaced=task_policy_signature(owner,linked)
            owner.feed.snapshot=replace(owner.feed.snapshot,receiver_id='receiver',local_status={})
            self.assertNotEqual(replaced,task_policy_signature(owner,linked))

    async def test_handoff_peer_only_revoke_clears_copy_immediately_and_keeps_authored_notes(self):
        from observatory.review_continuity import editable_draft
        with TemporaryDirectory() as directory:
            root=Path(directory);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            p=self.peer_projection(turn_capture()[2])
            task=TaskStore(root/'tasks.db').create('Authored title',notes='Authored notes',
                continuity_draft=editable_draft(p),source_allowed=True)
            copies=[];app.copy_to_clipboard=copies.append
            async with app.run_test(size=(100,32)) as pilot:
                await pilot.pause();screen=HandoffScreen(app,task);app.push_screen(screen)
                await self.ready(pilot,lambda:'PEER_PRIVATE_PATH' in screen.markdown and not screen.busy)
                self.revoke_peer(app)
                # Call the actual button handler before its periodic policy check.
                screen.buttons(Button.Pressed(screen.query_one('#handoff-copy',Button)))
                self.assertEqual(copies,[])
                self.assertNotIn('PEER_PRIVATE_PATH',screen.markdown)
                await self.ready(pilot,lambda:'Authored notes' in screen.markdown and not screen.busy)
                self.assertNotIn('PEER_PRIVATE_PATH',screen.markdown)
                screen.buttons(Button.Pressed(screen.query_one('#handoff-copy',Button)))
                self.assertIn('Authored notes',copies[-1]);self.assertNotIn('PEER_PRIVATE_PATH',copies[-1])

    async def test_metadata_only_receipt_preserves_safe_copy(self):
        with TemporaryDirectory() as directory:
            root=Path(directory);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            copies=[];app.copy_to_clipboard=copies.append
            async with app.run_test(size=(100,32)) as pilot:
                await pilot.pause();screen=app.screen
                app.feed.snapshot=replace(app.feed.snapshot,local_status={'capture_content':False})
                screen.action_copy()
                await self.ready(pilot,lambda:bool(copies))
                self.assertNotIn('PEER_PRIVATE_PATH',copies[-1])
                self.assertIn('metadata-only',copies[-1].lower())

    async def test_peer_revoke_during_receipt_format_discards_late_result(self):
        gate=threading.Event();started=threading.Event()
        from observatory.work_receipts import freeze_receipt
        def blocked(p):started.set();gate.wait(5);return freeze_receipt(p)
        with TemporaryDirectory() as directory:
            root=Path(directory);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            copies=[];app.copy_to_clipboard=copies.append
            with patch('observatory.work_receipts.freeze_receipt',blocked):
                async with app.run_test(size=(100,32)) as pilot:
                    await pilot.pause();screen=app.screen
                    screen.projection=self.peer_projection(screen.projection)
                    screen.start_receipt(False);await self.ready(pilot,started.is_set)
                    self.revoke_peer(app);gate.set();await pilot.pause(.3)
                    self.assertEqual(copies,[])

    async def test_export_valid_is_receiver_aware_and_sticky_after_peer_revoke(self):
        from observatory.work_receipts import freeze_receipt
        from observatory.ui_export import ReceiptExportScreen
        with TemporaryDirectory() as directory:
            root=Path(directory);owner,item=self.setup_owner(root/'tasks.db');app=Harness(owner,item)
            async with app.run_test(size=(100,32)) as pilot:
                await pilot.pause();screen=app.screen;p=self.peer_projection(screen.projection)
                state=projection_policy_state(app,p)
                screen.receipt_ready(freeze_receipt(p),True,state,p)
                await pilot.pause();export=app.screen;self.assertIsInstance(export,ReceiptExportScreen)
                self.assertTrue(export.valid())
                self.revoke_peer(app);self.assertFalse(export.valid())
                app.feed.snapshot=replace(app.feed.snapshot,local_status={})
                self.assertFalse(export.valid(),'revoked generation must stay invalid after restoration')
                app.pop_screen();await pilot.pause()
                screen.receipt_ready(freeze_receipt(p),True,projection_policy_state(app,p),p)
                await pilot.pause();export=app.screen;self.assertTrue(export.valid())
                app.feed.snapshot=replace(app.feed.snapshot,receiver_id='replacement')
                self.assertFalse(export.valid())
                app.feed.snapshot=replace(app.feed.snapshot,receiver_id='receiver')
                self.assertFalse(export.valid())


if __name__=='__main__':unittest.main()
