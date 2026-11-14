"""Exact-source keyboard flow, stale responses and capture revocation."""
import asyncio
from types import SimpleNamespace
import threading
import unittest

from textual.app import App
from textual.widgets import RichLog,Static
from observatory.ui_findings import FindingSourceScreen


class Harness(App):
    def __init__(self,owner,item):super().__init__();self.owner=owner;self.item=item
    def compose(self):yield Static('Findings')
    def on_mount(self):self.push_screen(FindingSourceScreen(self.owner,self.item))


class FindingSourceJourneys(unittest.IsolatedAsyncioTestCase):
    def fixture(self,gate=None):
        scope=dict(service='sdk',agent_id='worker',session_id='session')
        snapshot=SimpleNamespace(receiver_id='receiver',local_status={})
        response=dict(receiver_id='receiver',finding_id='a'*16,finding_scope=dict(scope),sources=[dict(type='log',id='source',scope=scope,
            record={'output':'\n'.join('Recorded line '+str(i) for i in range(150))},complete=True)],coverage='Exact source')
        class Api:
            def get(self,path):
                if gate:gate.wait(5)
                return response
        owner=SimpleNamespace(feed=SimpleNamespace(snapshot=snapshot,api=Api()))
        item=dict(id='a'*16,receiver_id='receiver',**scope)
        return owner,item,response

    async def loaded(self,pilot,app):
        for _ in range(30):
            await pilot.pause(.03)
            if isinstance(app.screen,FindingSourceScreen) and app.screen.sources:return
        self.fail('Exact source did not load')

    async def test_source_modal_scroll_keys_and_return_at_supported_sizes(self):
        for size in ((120,36),(80,24),(60,18)):
            owner,item,_=self.fixture();app=Harness(owner,item)
            async with app.run_test(size=size) as pilot:
                await self.loaded(pilot,app)
                log=app.screen.query_one(RichLog)
                self.assertTrue(log.has_focus)
                await pilot.press('end');await pilot.pause()
                self.assertGreater(log.scroll_y,0)
                await pilot.press('home');await pilot.pause()
                self.assertEqual(log.scroll_y,0)
                await pilot.press('down');await pilot.pause()
                self.assertGreater(log.scroll_y,0)
                await pilot.press('tab');await pilot.press('shift+tab')
                await pilot.press('escape');await pilot.pause()
                self.assertNotIsInstance(app.screen,FindingSourceScreen)

    async def test_policy_revoke_clears_loaded_content_and_late_response_is_discarded(self):
        owner,item,_=self.fixture();app=Harness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await self.loaded(pilot,app)
            owner.feed.snapshot.local_status={'sessions':[dict(service='sdk',agent_id='worker',session_id='session',capture_content=False)]}
            app.screen.check_policy()
            self.assertEqual(app.screen.sources,[])
            self.assertFalse(app.screen.query_one(RichLog).lines)
        gate=threading.Event();owner,item,response=self.fixture(gate);app=Harness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();screen=app.screen
            owner.feed.snapshot.receiver_id='replacement'
            gate.set();await pilot.pause(.1)
            self.assertEqual(screen.sources,[])
            self.assertFalse(screen.query_one(RichLog).lines)
            await pilot.press('escape')

    async def test_dismissed_modal_cannot_publish_a_late_response(self):
        gate=threading.Event();owner,item,_=self.fixture(gate);app=Harness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();screen=app.screen
            await pilot.press('escape');gate.set();await pilot.pause(.1)
            self.assertNotIsInstance(app.screen,FindingSourceScreen)
            self.assertEqual(screen.sources,[])

    async def test_finding_deny_still_applies_when_source_owner_is_missing(self):
        gate=threading.Event();owner,item,response=self.fixture(gate)
        response['sources'][0]['scope']={}
        app=Harness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause()
            owner.feed.snapshot.local_status={'sessions':[dict(service='sdk',agent_id='worker',session_id='session',capture_content=False)]}
            gate.set();await pilot.pause(.1)
            self.assertEqual(app.screen.sources,[])
            self.assertFalse(app.screen.query_one(RichLog).lines)

    async def test_wrong_finding_response_cannot_publish(self):
        owner,item,response=self.fixture();response['finding_id']='b'*16
        app=Harness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause(.1)
            self.assertEqual(app.screen.sources,[])
            self.assertFalse(app.screen.query_one(RichLog).lines)

    async def test_actual_findings_source_key_does_not_stack_and_export_rechecks_policy(self):
        from dataclasses import replace
        from textual.widgets import Button,Input,TabbedContent
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_export import ExportScreen
        from tests.test_terminal_search import ManualFeed
        from tempfile import TemporaryDirectory
        from pathlib import Path
        feed=ManualFeed()
        owner,item,response=self.fixture()
        feed.api.findings=[dict(item,kind='span_error',title='Recorded failure',
            severity='high',detail='PRIVATE_FINDING',evidence=[dict(type='log',id='c'*64)])]
        original=feed.api.get
        feed.api.get=lambda path:response if path.endswith('/source') else {'ok':True,'receiver_id':'receiver'} if path.startswith('/api/health') else original(path)
        feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.action_page('findings');await pilot.pause()
            await pilot.press('s');await self.loaded(pilot,app)
            depth=len(app.screen_stack)
            await pilot.press('s');await pilot.pause()
            self.assertEqual(len(app.screen_stack),depth)
            await pilot.press('escape');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'findings')
            app.action_export();await pilot.pause()
            self.assertIsInstance(app.screen,ExportScreen)
            with TemporaryDirectory() as directory:
                path=Path(directory)/'finding.json'
                app.screen.query_one('#export-path',Input).value=str(path)
                feed.snapshot=replace(feed.snapshot,local_status={'capture_content':False})
                self.assertFalse(app.screen.can_write())
                self.assertEqual(app.screen.content,'')
                self.assertFalse(app.screen.query_one('#export-preview',RichLog).lines)
                feed.snapshot=replace(feed.snapshot,local_status={})
                self.assertFalse(app.screen.can_write())
                app.screen.confirm();await pilot.pause()
                self.assertFalse(path.exists())

    async def test_changed_finding_owner_cannot_publish_under_frozen_selection(self):
        owner,item,response=self.fixture();response['finding_scope']['agent_id']='foreign'
        app=Harness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause(.1)
            self.assertEqual(app.screen.sources,[])
            self.assertFalse(app.screen.query_one(RichLog).lines)


if __name__=='__main__':unittest.main()
