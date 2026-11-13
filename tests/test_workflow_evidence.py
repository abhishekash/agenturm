import json
import asyncio
import threading
import time
import unittest
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace
from observatory.workflow_evidence import (ExecutionScope, ReviewHistory, ReviewCache, scope_for, check_kind, render_review, MAX_BYTES)
from observatory.feed_requests import shared_get, RequestBatch


def event(identity='one', **kw):
    return dict(id=identity, service='codex', session_id='session', agent_id='worker', trace_id='run',
                time_unix_nano=10, domain='shell', action='execute', subject='pytest tests -q',result='success',**kw)


class EvidenceTests(unittest.TestCase):
    def setUp(self):self.scope=ExecutionScope('receiver','codex','worker','session','run',kind='run')

    def test_scope_never_inherits_untagged_sibling_other_producer_or_run(self):
        selected=event();history=ReviewHistory(self.scope)
        history.accept({'activity':[selected,*[{**selected,'id':key,field:value} for key,field,value in (
            ('sibling','agent_id','sibling'),('untagged','agent_id',None),('producer','service','claude'),('run','trace_id','other'))]]})
        self.assertEqual(len(history.events),1)
        other=ExecutionScope('other','codex','worker','session','run',kind='run')
        self.assertNotEqual(self.scope.scope_key,other.scope_key)
        legacy=ExecutionScope('receiver','codex',session_id='session',kind='legacy_session')
        self.assertFalse(legacy.matches(selected));self.assertTrue(legacy.matches({**selected,'agent_id':None}))

    def test_stable_anchor_is_not_invented_and_endpoint_credentials_are_excluded(self):
        item=dict(service='codex',session_id='session',agent_id='worker',stamp=999,task='title',working_directory='/tmp')
        scope=scope_for(item,endpoint='https://name:secret@host:8080/path?token=secret')
        self.assertFalse(scope.persistent);self.assertNotIn('secret',scope.receiver)
        self.assertTrue(scope.endpoint_fallback)
        self.assertTrue(scope_for({**item,'trace_id':'trace'},SimpleNamespace(receiver_id='stable')).persistent)

    def test_checks_are_recorded_results_and_later_edits_weaken_freshness(self):
        rows=[event(),{**event('unknown'),'time_unix_nano':11,'result':'started'},
              {**event('claim'),'domain':'message','action':'assistant','subject':'Tests passed'},
              {**event('edit'),'time_unix_nano':12,'domain':'file','action':'edit','subject':'retry.py'}]
        history=ReviewHistory(self.scope);history.accept({'activity':rows,'next_cursor':'older'})
        projection=history.projection({'state':'Completed','task':'Fix retries'})
        self.assertEqual([check['result'] for check in projection['checks']],['passed','unknown'])
        self.assertEqual(projection['checks'][0]['freshness'],'later recorded edits')
        self.assertEqual(projection['window']['coverage'],'partial')
        self.assertEqual(len(projection['checks']),2)
        self.assertIn('Agent reported: Completed',render_review(projection))
        self.assertIsNone(check_kind('echo pytest'))
        self.assertIsNone(check_kind('pytest || true'))
        self.assertEqual(check_kind('.venv/bin/python -m unittest discover'),'unittest')

    def test_bounds_freeze_source_and_cache_selection_count(self):
        history=ReviewHistory(self.scope)
        row=event();history.accept({'activity':[row]});row['subject']='changed'
        self.assertEqual(next(iter(history.events.values()))['subject'],'pytest tests -q')
        huge={**event('huge'),'detail':'x'*(MAX_BYTES+1)}
        history.accept({'activity':[huge],'next_cursor':'more'})
        self.assertLessEqual(history.payload_bytes,MAX_BYTES)
        self.assertIsNone(history.cursor);self.assertTrue(history.limited)
        cache=ReviewCache()
        for revision in range(8):cache.get(self.scope,revision)
        self.assertEqual(len(cache.entries),4)

    def test_shared_read_limit_includes_batch_and_direct_review(self):
        class Api:
            active=0;maximum=0
            lock=threading.Lock()
            def get(self,path):
                with self.lock:self.active+=1;self.maximum=max(self.maximum,self.active)
                time.sleep(.01)
                with self.lock:self.active-=1
                return {}
        api=Api();batch=RequestBatch(api,threading.Event());batch.prefetch([str(i) for i in range(8)])
        with ThreadPoolExecutor(max_workers=8) as pool:list(pool.map(lambda n:shared_get(api,str(n)),range(8)))
        batch.close();self.assertLessEqual(api.maximum,4)

    def test_usage_is_exact_run_only_and_new_native_start_does_not_inherit_old_trace(self):
        item=dict(service='codex',agent_id='worker',session_id='session',run_start_ns=20,latest_trace_id='old')
        scope=scope_for(item,SimpleNamespace(receiver_id='receiver'))
        self.assertEqual(scope.run_id,'');self.assertEqual(scope.run_anchor,'start:20')
        ended=scope_for({**item,'outcome_time_ns':30},SimpleNamespace(receiver_id='receiver'))
        self.assertTrue(ended.matches({**event(),'time_unix_nano':25}))
        self.assertFalse(ended.matches({**event(),'time_unix_nano':35}))
        history=ReviewHistory(self.scope)
        projection=history.projection({'_review_usage':{'trace_id':'other','input_tokens':999}})
        self.assertEqual(projection['usage']['values'],{})
        projection=history.projection({'_review_usage':{'trace_id':'run','input_tokens':10,'known_cost_usd':0,'cost_complete':False,'agent_count':1,'agent_count_complete':True,
            'review_owner':{'producer':'codex','agent_id':'worker','session_id':'session'}}})
        self.assertEqual(projection['usage']['values']['input_tokens'],10)
        self.assertFalse(projection['usage']['values']['cost_complete'])

    def test_cancelled_waiter_never_performs_a_request(self):
        class Api:
            calls=0
            def get(self,path):self.calls+=1;return {}
        api=Api();shared_get(api,'initialize');slots=api._agenturm_read_slots
        for _ in range(4):slots.acquire()
        cancel=threading.Event()
        try:
            with ThreadPoolExecutor(max_workers=1) as pool:
                waiting=pool.submit(shared_get,api,'cancelled',cancelled=cancel.is_set)
                cancel.set()
                with self.assertRaises(InterruptedError):waiting.result(timeout=2)
        finally:
            for _ in range(4):slots.release()
        self.assertEqual(api.calls,1)

    def test_receiver_response_replacement_is_rejected_without_mutating_preview(self):
        history=ReviewHistory(self.scope)
        history.accept({'activity':[event()], 'receiver_id':'receiver'})
        self.assertTrue(history.receiver_confirmed)
        with self.assertRaisesRegex(ValueError,'Receiver identity changed'):
            history.accept({'activity':[{**event('private'),'subject':'pytest OTHER_RECEIVER'}],'receiver_id':'replacement'})
        self.assertEqual(len(history.events),1)
        self.assertNotIn('OTHER_RECEIVER',json.dumps(history.projection({})))

    def test_identical_review_reads_share_one_inflight_request(self):
        class Api:
            calls=0
            def get(self,path):
                self.calls+=1;time.sleep(.05);return {'activity':[]}
        api=Api()
        with ThreadPoolExecutor(max_workers=8) as pool:
            replies=list(pool.map(lambda _:shared_get(api,'same-review',review=True),range(8)))
        self.assertEqual(api.calls,1)
        self.assertTrue(all(reply=={'activity':[]} for reply in replies))

    def test_deduplicated_transport_timeout_is_propagated_to_every_reader(self):
        class Api:
            calls=0
            def get(self,path):
                self.calls+=1;time.sleep(.05);raise TimeoutError('transport timed out')
        api=Api()
        with ThreadPoolExecutor(max_workers=4) as pool:
            futures=[pool.submit(shared_get,api,'same-review',review=True) for _ in range(4)]
            for future in futures:
                with self.assertRaisesRegex(TimeoutError,'transport timed out'):future.result(timeout=2)
        self.assertEqual(api.calls,1)


class ReviewJourney(unittest.IsolatedAsyncioTestCase):
    async def test_refresh_commits_matching_native_outcome_and_owned_usage(self):
        from tests.test_terminal_search import ManualFeed
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        feed=ManualFeed();feed.request_refresh=lambda:None
        feed.snapshot.receiver_id='receiver';feed.snapshot.review_trace_filter=True
        native=dict(source_id='current',service='codex',session_id='session',agent_id='worker',trace_id='run',
            runtime='Codex',task='Fix retries',run_state='running',last_record_ns=time.time_ns(),capture_content=True)
        usage=dict(trace_id='run',agent_id='worker',agent_count=1,agent_count_complete=True,input_tokens=100,
            review_owner=dict(producer='codex',agent_id='worker',session_id='session'))
        feed.snapshot.local_status={'sessions':[native]}
        feed.snapshot.sessions=[dict(agent_id='worker',session_id='session',services=['codex'],latest_run=usage)]
        original=feed.api.get
        feed.api.get=lambda path: {'activity':[event()],'receiver_id':'receiver'} if 'limit=200' in path else original(path)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.push_screen(ReviewScreen(app,{**native,'state':'Working','_review_usage':dict(usage)}))
            await app.workers.wait_for_complete();await pilot.pause();screen=app.screen
            self.assertEqual(screen.projection['usage']['values']['input_tokens'],100)
            native['run_state']='completed';usage['input_tokens']=500;feed.snapshot.updated=time.time()
            await pilot.press('R');await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(screen.projection['reported_outcome'],'Completed')
            self.assertEqual(screen.projection['usage']['values']['input_tokens'],500)
            native['trace_id']='new';native['run_state']='running';usage['trace_id']='new'
            await pilot.press('R');await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(screen.projection['scope']['run_id'],'run')
            self.assertEqual(screen.projection['reported_outcome'],'Completed')
            self.assertIn('previous snapshot',' '.join(screen.projection['window']['reasons']))

    async def test_policy_change_during_formatting_revokes_copy_and_frozen_export_button(self):
        from tests.test_terminal_search import ManualFeed
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        from observatory.ui_export import ReceiptExportScreen
        from observatory.work_receipts import freeze_receipt
        from unittest.mock import patch
        from pathlib import Path
        from tempfile import TemporaryDirectory
        from textual.widgets import Input
        feed=ManualFeed();feed.request_refresh=lambda:None
        feed.snapshot.receiver_id='receiver';feed.snapshot.review_trace_filter=True
        native=dict(source_id='current',service='codex',runtime='Codex',agent_id='worker',session_id='session',trace_id='new',capture_content=True)
        feed.snapshot.local_status={'sessions':[native]}
        original=feed.api.get
        feed.api.get=lambda path: {'activity':[{**event(),'subject':'pytest PRIVATE'}],'receiver_id':'receiver'} if 'limit=200' in path else original(path)
        app=ObservatoryApp(feed,persist_preferences=False);started=threading.Event();release=threading.Event()
        def slow_format(projection):
            started.set();release.wait(5);return freeze_receipt(projection)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.push_screen(ReviewScreen(app,{**native,'trace_id':'run','task':'PRIVATE_TASK'}))
            await app.workers.wait_for_complete();await pilot.pause();screen=app.screen
            self.assertIn('PRIVATE',screen.frozen_json())
            with patch('observatory.work_receipts.freeze_receipt',side_effect=slow_format),patch.object(app,'copy_to_clipboard') as copy:
                await pilot.press('c');self.assertTrue(await asyncio.to_thread(started.wait,2))
                native['capture_content']=False;release.set()
                await app.workers.wait_for_complete();await pilot.pause();copy.assert_not_called()
                await pilot.press('j');await pilot.pause()
                self.assertNotIn('PRIVATE',copy.call_args.args[0])
                self.assertNotIn('PRIVATE',str(screen.query_one('#review-heading').content))
            with TemporaryDirectory() as directory:
                # Exercise the real inherited Button.Pressed route, not a direct method call.
                target=Path(directory)/'blocked.md'
                app.push_screen(ReceiptExportScreen(freeze_receipt(screen.projection),valid=lambda:False))
                await pilot.pause();app.screen.query_one('#export-path',Input).value=str(target)
                preview=app.screen.query_one('#export-preview')
                self.assertEqual(preview.scroll_y,0)
                self.assertFalse(preview.show_horizontal_scrollbar)
                self.assertIn('# Agenturm execution evidence',preview.lines[0].text)
                for widget in app.screen.query('#export-dialog > *'):
                    self.assertTrue(app.screen.region.contains_region(widget.region),widget.id)
                await pilot.click('#export-confirm');await app.workers.wait_for_complete();await pilot.pause()
                self.assertFalse(target.exists());self.assertFalse(target.with_suffix('.html').exists())

    async def test_lazy_paging_frozen_copy_export_floor_and_return(self):
        from tests.test_terminal_search import ManualFeed
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        from observatory.ui_export import ExportScreen
        from unittest.mock import patch
        feed=ManualFeed();feed.snapshot.receiver_id='receiver';feed.snapshot.review_trace_filter=True
        original=feed.api.get;requests=[]
        def get(path):
            if '/api/activity?' not in path or 'limit=200' not in path:return original(path)
            requests.append(path)
            return {'activity':[event('old' if 'cursor=older' in path else 'new')], 'next_cursor':None if 'cursor=older' in path else 'older'}
        feed.api.get=get
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            self.assertEqual(requests,[])
            item=dict(id='source-1',service='codex',session_id='session',agent_id='worker',trace_id='run',task='Fix retries',state='Completed')
            app.push_screen(ReviewScreen(app,item));await app.workers.wait_for_complete();await pilot.pause()
            screen=app.screen;self.assertIsInstance(screen,ReviewScreen)
            self.assertEqual(len(requests),1);self.assertIn('trace_id=run',requests[0])
            started_snapshot=screen.started_snapshot;feed.snapshot.updated+=1
            await pilot.click('#review-earlier');await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(len(screen.history.events),2)
            self.assertEqual(screen.started_snapshot,started_snapshot)
            self.assertIn('newer snapshot available',str(screen.query_one('#review-status').content))
            with patch.object(app,'copy_to_clipboard') as copy:
                await pilot.press('c');await pilot.pause();frozen=copy.call_args.args[0]
            await pilot.press('x');await pilot.pause();self.assertIsInstance(app.screen,ExportScreen)
            self.assertEqual(app.screen.content,frozen)
            await pilot.press('escape');await pilot.resize_terminal(60,18);await pilot.pause()
            self.assertTrue(app.screen.query_one('#review-body').display)
            self.assertTrue(app.screen.region.contains_region(app.screen.query_one('#review-body').region))
            self.assertFalse(app.screen.query_one('#review-body').show_horizontal_scrollbar)
            for button in app.screen.query('#review-controls Button'):
                self.assertTrue(app.screen.region.contains_region(button.region),button.id)
            await pilot.resize_terminal(45,15);await pilot.pause();self.assertTrue(app.screen.has_class('too-small'))
            await pilot.press('escape');await pilot.pause();self.assertNotIsInstance(app.screen,ReviewScreen)

    async def test_metadata_only_never_reuses_cached_content_or_fetches_it(self):
        from tests.test_terminal_search import ManualFeed
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        feed=ManualFeed();feed.snapshot.receiver_id='receiver';feed.snapshot.review_trace_filter=True
        app=ObservatoryApp(feed,persist_preferences=False)
        item=dict(id='source-1',service='codex',session_id='session',agent_id='worker',trace_id='run',state='Completed')
        scope=scope_for(item,feed.snapshot,feed.api.endpoint)
        cached=app.review_cache.get(scope);cached.accept({'activity':[{**event(),'subject':'pytest PRIVATE'}]})
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop();before=len(feed.api.requests)
            app.push_screen(ReviewScreen(app,{**item,'capture_content':False}));await app.workers.wait_for_complete();await pilot.pause()
            self.assertNotIn('PRIVATE',app.screen.frozen_json())
            self.assertEqual(len(feed.api.requests),before)
            self.assertTrue(app.screen.query_one('#review-excerpt').disabled)

    async def test_closed_selection_result_cannot_replace_new_review(self):
        from tests.test_terminal_search import ManualFeed
        from observatory.terminal_ui import ObservatoryApp
        from observatory.ui_review import ReviewScreen
        feed=ManualFeed();feed.snapshot.receiver_id='receiver';feed.snapshot.review_trace_filter=True
        original=feed.api.get;started=threading.Event();release=threading.Event()
        def get(path):
            if 'limit=200' not in path:return original(path)
            if 'trace_id=old' in path:
                started.set();release.wait(timeout=5)
                return {'activity':[{**event(),'trace_id':'old','subject':'pytest OLD_PRIVATE'}]}
            return {'activity':[{**event(),'trace_id':'new','subject':'pytest new'}]}
        feed.api.get=get;app=ObservatoryApp(feed,persist_preferences=False)
        item=dict(id='source-1',service='codex',session_id='session',agent_id='worker',state='Completed')
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.push_screen(ReviewScreen(app,{**item,'trace_id':'old'}));await pilot.pause()
            self.assertTrue(await asyncio.to_thread(started.wait,2))
            await pilot.press('escape');await pilot.pause()
            app.push_screen(ReviewScreen(app,{**item,'trace_id':'new'}));await pilot.pause()
            release.set();await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(app.last_review['projection']['scope']['run_id'],'new')
            self.assertNotIn('OLD_PRIVATE',app.screen.frozen_json())
