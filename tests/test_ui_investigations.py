"""Typed search, exact source identity, policy fences and 60×18 keyboard journeys."""
import asyncio
from copy import deepcopy
from dataclasses import replace
from pathlib import Path
from tempfile import TemporaryDirectory
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from textual.app import App
from textual.widgets import Button,DataTable,Input,RichLog,Select,Static
from observatory.investigation import decode_reference
from observatory.ui_investigations import InvestigationsScreen,policy_state,source_references,source_lines
from tests.test_investigation_cli import Api,match


def source_for(row):
    ref=decode_reference(row['source_reference'])
    scope={key:ref[key] for key in ('service','agent_id','session_id','trace_id','span_id','turn_id')}
    return dict(receiver_id='receiver',event_id=ref['event_id'],source_reference=ref,sources=[dict(type=ref['source'],
        id=ref['source_id'],scope=scope,record={'output':'\n'.join('Recorded line '+str(i) for i in range(180))},complete=True)],coverage='Exact retained source only.')


class Harness(App):
    BINDINGS=[('c','root_copy','Copy'),('x','root_export','Export')]
    def __init__(self,owner,**kwargs):super().__init__();self.owner=owner;self.kwargs=kwargs;self.root_actions=[]
    def compose(self):yield Static('Activity')
    def on_mount(self):self.push_screen(InvestigationsScreen(self.owner,**self.kwargs))
    def action_root_copy(self):self.root_actions.append('copy')
    def action_root_export(self):self.root_actions.append('export')


class InvestigationHelpers(unittest.TestCase):
    def fixture(self):
        row=match();response=source_for(row)
        owner=SimpleNamespace(feed=SimpleNamespace(snapshot=SimpleNamespace(receiver_id='receiver',local_status={})))
        return owner,row,response

    def test_actual_source_all_six_scope_fields_and_original_namespace_are_pinned(self):
        owner,row,response=self.fixture();expected=decode_reference(row['source_reference'])
        self.assertTrue(policy_state(owner,'receiver',source_references(response,expected))[0])
        for key in ('service','agent_id','session_id','trace_id','span_id','turn_id'):
            changed=deepcopy(response);changed['sources'][0]['scope'][key]='foreign'
            with self.assertRaisesRegex(ValueError,'owner/execution'):source_references(changed,expected)
            changed=deepcopy(response);changed['sources'][0]['scope'].pop(key)
            with self.assertRaisesRegex(ValueError,'owner/execution'):source_references(changed,expected)
        changed=deepcopy(response);changed['receiver_id']='foreign'
        with self.assertRaisesRegex(ValueError,'reference changed'):source_references(changed,expected)
        refs=source_references(response,expected)
        owner.feed.snapshot.local_status={'sessions':[dict(service='sdk',agent_id='worker',session_id='session',capture_content=False)]}
        self.assertFalse(policy_state(owner,'receiver',refs)[0])
        owner.feed.snapshot.local_status={};owner.feed.snapshot.receiver_id='foreign'
        self.assertFalse(policy_state(owner,'receiver',refs)[0])

    def test_display_controls_and_field_work_are_bounded(self):
        _,_,response=self.fixture()
        response['sources'][0]['record']={'output':'\x1b]0;BAD\x07\n'+'x'*70000}
        lines=source_lines(response)
        self.assertNotIn('\x1b',str(lines));self.assertNotIn('\x07',str(lines))
        self.assertIn('Display limit',str(lines));self.assertLess(len(str(lines)),70000)


class InvestigationJourneys(unittest.IsolatedAsyncioTestCase):
    async def test_actual_monitor_shortcut_and_palette_action_open_one_screen(self):
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed
        feed=ManualFeed();app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.query_one('#agent-table').focus()
            await pilot.press('ctrl+f');await pilot.pause()
            self.assertIsInstance(app.screen,InvestigationsScreen)
            self.assertTrue(app.screen.query_one('#investigation-query',Input).has_focus)
            depth=len(app.screen_stack);app.action_investigate()
            self.assertEqual(len(app.screen_stack),depth)
            await pilot.press('escape');await pilot.pause()
            self.assertNotIsInstance(app.screen,InvestigationsScreen)

    def fixture(self,rows=12):
        api=Api();page=[match(str(i)) for i in range(rows)];api.pages=[dict(matches=page,next_cursor=None)]
        api.source=source_for(page[0])
        owner=SimpleNamespace(feed=SimpleNamespace(snapshot=SimpleNamespace(receiver_id='receiver',local_status={}),api=api))
        return owner,api

    async def ready(self,pilot,predicate):
        end=time.monotonic()+6
        while not predicate() and time.monotonic()<end:await pilot.pause(.03)
        self.assertTrue(predicate())

    async def search(self,pilot,app):
        app.screen.query_one('#investigation-query',Input).value='pytest'
        await pilot.press('enter');await self.ready(pilot,lambda:bool(app.screen.rows) and not app.screen.busy)

    async def test_floor_query_navigation_source_and_back_without_modal_stacking(self):
        owner,api=self.fixture();app=Harness(owner,kind='command')
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();screen=app.screen
            self.assertTrue(screen.query_one('#investigation-query',Input).has_focus)
            await self.search(pilot,app);table=screen.query_one(DataTable)
            self.assertGreaterEqual(table.region.height,5)
            for identity in ('investigation-kind','investigation-query','investigation-search','investigation-more','investigation-back'):
                widget=screen.query_one('#'+identity)
                self.assertTrue(screen.region.contains_region(widget.region),(identity,widget.region))
            await pilot.press('end');self.assertEqual(table.cursor_row,11)
            await pilot.press('home');self.assertEqual(table.cursor_row,0)
            await pilot.press('pagedown');self.assertGreater(table.cursor_row,0)
            await pilot.press('home','enter');await self.ready(pilot,lambda:bool(screen.sources) and not screen.busy)
            depth=len(app.screen_stack);log=screen.query_one(RichLog)
            self.assertTrue(log.has_focus);self.assertTrue(screen.source_mode)
            self.assertFalse(screen.query_one('#investigation-form').display)
            await pilot.press('end');await pilot.pause();self.assertGreater(log.scroll_y,0)
            await pilot.press('home');await pilot.pause();self.assertEqual(log.scroll_y,0)
            await pilot.press('down');await pilot.pause();self.assertGreater(log.scroll_y,0)
            await pilot.press('c','x');self.assertEqual(app.root_actions,[])
            await pilot.press('escape');self.assertIs(app.screen,screen);self.assertFalse(screen.source_mode)
            self.assertEqual(screen.query_one('#investigation-query',Input).value,'pytest')
            self.assertEqual(len(app.screen_stack),depth)
            await pilot.press('escape');self.assertIsNot(app.screen,screen)

    async def test_typing_filters_and_paging_are_bounded(self):
        owner,api=self.fixture();api.pages=[dict(matches=[match(str(page*50+i)) for i in range(50)],next_cursor='cursor'+str(page)) for page in range(5)]
        app=Harness(owner,kind='command')
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();screen=app.screen;query=screen.query_one('#investigation-query',Input)
            await pilot.press('c','x');self.assertEqual(query.value,'cx');self.assertEqual(app.root_actions,[])
            await self.search(pilot,app)
            for expected in (100,150,200,250):
                screen.action_more();await self.ready(pilot,lambda:len(screen.rows)==expected and not screen.busy)
            self.assertEqual(screen.pages,5);self.assertTrue(screen.query_one('#investigation-more',Button).disabled)
            before=len(api.paths);screen.action_more();await pilot.pause();self.assertEqual(len(api.paths),before)
            screen.buttons(Button.Pressed(screen.query_one('#investigation-toggle-filters',Button)));await pilot.pause()
            self.assertTrue(screen.query_one('#investigation-filters').display)
            self.assertGreaterEqual(screen.query_one(DataTable).region.height,3)
            query.value='new query';await pilot.pause();self.assertEqual(screen.rows,[])

    async def test_peer_revoke_clears_loaded_source_and_restoration_does_not_revive(self):
        owner,api=self.fixture();app=Harness(owner,kind='command')
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();await self.search(pilot,app);screen=app.screen
            await pilot.press('enter');await self.ready(pilot,lambda:bool(screen.sources))
            owner.feed.snapshot.local_status={'sessions':[dict(service='sdk',agent_id='worker',session_id='session',capture_content=False)]}
            screen.check_policy();self.assertEqual(screen.rows,[]);self.assertEqual(screen.sources,[])
            self.assertFalse(screen.query_one(RichLog).lines);self.assertTrue(screen.invalidated)
            owner.feed.snapshot.local_status={};screen.check_policy();self.assertEqual(screen.sources,[])
            self.assertEqual(screen.query_one('#investigation-query',Input).value,'pytest')
            await pilot.press('c','x');self.assertEqual(app.root_actions,[])

    async def test_policy_revision_and_cumulative_payload_require_a_fresh_search(self):
        owner,api=self.fixture();app=Harness(owner,kind='command')
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();await self.search(pilot,app);screen=app.screen
            owner.feed.snapshot.local_status={'sessions':[dict(service='sdk',agent_id='worker',session_id='session',capture_content=True,capture_revision=3)]}
            screen.check_policy();self.assertTrue(screen.invalidated);self.assertEqual(screen.rows,[])
            screen.check_policy();self.assertEqual(screen.rows,[])
        owner,api=self.fixture();row=match('first');row['source_command']='x'*900000
        second=match('second');second['source_command']='x'*1400000
        api.pages=[dict(matches=[row],next_cursor='next'),dict(matches=[second],next_cursor=None)]
        app=Harness(owner,kind='command')
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();await self.search(pilot,app);screen=app.screen
            screen.action_more();await self.ready(pilot,lambda:not screen.busy)
            self.assertEqual([row['event_id'] for row in screen.rows],['first'])
            self.assertTrue(screen.query_one('#investigation-more',Button).disabled)
            self.assertIn('2 MiB',str(screen.query_one('#investigation-status',Static).content))

    async def test_receiver_swap_and_source_identity_drift_never_publish(self):
        for change in ('receiver','owner','execution'):
            owner,api=self.fixture();app=Harness(owner,kind='command')
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();await self.search(pilot,app);screen=app.screen
                if change=='receiver':api.receiver='replacement'
                else:api.source['sources'][0]['scope']['agent_id' if change=='owner' else 'trace_id']='FOREIGN'
                await pilot.press('enter');await self.ready(pilot,lambda:not screen.busy)
                self.assertEqual(screen.sources,[]);self.assertFalse(screen.query_one(RichLog).lines)
                self.assertIn('unavailable',str(screen.query_one('#investigation-status',Static).content))

    async def test_late_search_and_source_cannot_publish_after_cancel_revoke_or_close(self):
        for stage in ('search','source'):
            gate=threading.Event();started=threading.Event();owner,api=self.fixture();original=api.get
            def get(path):
                if path.startswith('/api/investigations/source?') if stage=='source' else path.startswith('/api/investigations?'):
                    started.set();gate.wait(5)
                return original(path)
            api.get=get;app=Harness(owner,kind='command')
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();screen=app.screen
                screen.query_one('#investigation-query',Input).value='pytest';await pilot.press('enter')
                if stage=='source':await self.ready(pilot,lambda:bool(screen.rows) and not screen.busy);await pilot.press('enter')
                await self.ready(pilot,started.is_set)
                owner.feed.snapshot.local_status={'capture_content':False};screen.check_policy();gate.set();await pilot.pause(.2)
                self.assertEqual(screen.rows,[]);self.assertEqual(screen.sources,[])
                self.assertFalse(screen.query_one(RichLog).lines)
        gate=threading.Event();started=threading.Event();owner,api=self.fixture();original=api.get
        def get(path):
            if path.startswith('/api/investigations?'):started.set();gate.wait(5)
            return original(path)
        api.get=get;app=Harness(owner,kind='command')
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();old=app.screen;old.query_one('#investigation-query',Input).value='pytest';await pilot.press('enter')
            await self.ready(pilot,started.is_set);await pilot.press('ctrl+c','escape');gate.set();await pilot.pause(.2)
            self.assertIsNot(app.screen,old);self.assertEqual(old.rows,[])
            app.push_screen(InvestigationsScreen(owner,kind='instruction'));await pilot.pause()
            self.assertEqual(app.screen.rows,[])

    async def test_api_unavailable_is_explicit_and_actual_app_floor_stays_usable(self):
        from observatory.terminal_ui import ObservatoryApp
        from tests.test_terminal_search import ManualFeed
        owner,api=self.fixture();api.get=lambda path:dict(receiver_id='receiver')
        app=Harness(owner,kind='instruction')
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.screen.query_one('#investigation-query',Input).value='keep API';await pilot.press('enter')
            await self.ready(pilot,lambda:not app.screen.busy)
            self.assertEqual(app.screen.rows,[])
            self.assertIn('unavailable',str(app.screen.query_one('#investigation-status',Static).content))
        owner,api=self.fixture();feed=ManualFeed();feed.refresh();feed.snapshot=replace(feed.snapshot,receiver_id='receiver')
        base=feed.api.get
        feed.api.get=lambda path:api.get(path) if path=='/api/health' or path.startswith('/api/investigations') else base(path)
        class Actual(ObservatoryApp):
            def on_mount(self):
                super().on_mount();self.refresh_timer.stop();self.push_screen(InvestigationsScreen(self,kind='command'))
        actual=Actual(feed,persist_preferences=False)
        async with actual.run_test(size=(60,18)) as pilot:
            await pilot.pause();await self.search(pilot,actual);screen=actual.screen
            self.assertGreaterEqual(screen.query_one(DataTable).region.height,5)
            for identity in ('investigation-search','investigation-more','investigation-toggle-filters','investigation-back'):
                self.assertTrue(screen.region.contains_region(screen.query_one('#'+identity).region),identity)
            await pilot.press('enter');await self.ready(pilot,lambda:bool(screen.sources))
            await pilot.press('escape');self.assertIs(actual.screen,screen)
