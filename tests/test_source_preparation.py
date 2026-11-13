"""Reuse static source work without freezing health, ownership or consent."""
from copy import deepcopy
from dataclasses import replace
import asyncio
import gc
import threading
import weakref
import unittest
from unittest.mock import patch
from textual.screen import Screen

from observatory.terminal_ui import ObservatoryApp, SourcePreparation, prepare_sources, sources
from observatory.workflow_evidence import scope_for
from tests.test_agent_hierarchy_ui import HierarchyFeed, hierarchy_snapshot


class SourcePreparationTests(unittest.TestCase):
    def setUp(self):
        self.snapshot=hierarchy_snapshot()
        self.cache=SourcePreparation()

    def read(self,snapshot=None,**kwargs):
        return sources(snapshot or self.snapshot,preparation=self.cache,**kwargs)

    def test_shared_preparation_keeps_results_equivalent_and_consumers_detached(self):
        with patch('observatory.terminal_ui.prepare_sources',wraps=prepare_sources) as prepare:
            first=self.read()
            first[0]['task']='consumer decoration'
            first[0]['last_activity']['subject']='consumer nested change'
            second=self.read()
            self.assertEqual(prepare.call_count,1)
            self.assertEqual(second,sources(self.snapshot))
            self.assertNotEqual(second[0]['task'],'consumer decoration')
            self.assertNotEqual(second[0]['last_activity']['subject'],'consumer nested change')

    def test_clock_and_collector_health_are_live_on_cache_hit(self):
        now=self.snapshot.local_status['sessions'][0]['last_record_ns']
        with patch('observatory.terminal_ui.time.time_ns',return_value=now):
            rows=self.read()
            self.assertEqual(next(row for row in rows if row['id']=='s-root')['state'],'Working')
        with patch('observatory.terminal_ui.time.time_ns',return_value=now+301_000_000_000):
            rows=self.read()
            root=next(row for row in rows if row['id']=='s-root')
            self.assertEqual(root['state'],'Unconfirmed')
            self.assertFalse(root['active'])
            waiting=next(row for row in rows if row['id']=='s-c1')
            self.assertEqual(waiting['state'],'Waiting')
            self.assertTrue(waiting['owns_attention'])

    def test_each_nested_input_and_namespace_change_invalidates(self):
        changes=[
            lambda s:s.local_status['sessions'][0].update(initial_task='Original intent'),
            lambda s:s.local_status['sessions'][0]['last_activity'].update(subject='Changed nested action'),
            lambda s:s.activity[0].update(detail='Changed request detail'),
            lambda s:s.activity[0].update(agent_id='another owner'),
            lambda s:s.agents.append(dict(source_id='sdk',service='sdk',state='running')),
            lambda s:s.sessions.append(dict(session_id='otlp',latest_task='Observed task')),
            lambda s:setattr(s,'receiver_id','replacement'),
            lambda s:setattr(s,'view_scope',('new','scope')),
            lambda s:s.local_status['sessions'].pop(),
        ]
        for change in changes:
            with self.subTest(change=change):
                snapshot=hierarchy_snapshot();cache=SourcePreparation()
                with patch('observatory.terminal_ui.prepare_sources',wraps=prepare_sources) as prepare:
                    sources(snapshot,preparation=cache)
                    change(snapshot)
                    self.assertEqual(sources(snapshot,preparation=cache),sources(snapshot))
                    self.assertEqual(prepare.call_count,3)

    def test_capture_revocation_and_review_visibility_never_reuse_old_text(self):
        self.read()
        paused=deepcopy(self.snapshot)
        entry=self.snapshot.local_status['sessions'][1]
        self.assertTrue(self.snapshot.activity)
        entry.update(capture_content=False,task='',initial_task='',last_activity={})
        # Activity is deliberately stale: a failed read must not bypass policy.
        rows=self.read()
        root=next(row for row in rows if row['id']=='s-c1')
        self.assertFalse(root['capture_content'])
        self.assertFalse(root['task'])
        self.assertEqual(self.cache.entries,[])
        old=sources(paused,preparation=self.cache,policy_snapshot=self.snapshot)
        self.assertFalse(next(row for row in old if row['id']=='s-c1')['task'])
        self.assertEqual(self.cache.entries,[])
        # A policy from a different receiver must not rewrite a frozen receipt.
        other=replace(self.snapshot,receiver_id='other')
        old=sources(paused,preparation=self.cache,policy_snapshot=other)
        self.assertTrue(next(row for row in old if row['id']=='s-c1')['task'])
        entry.update(service='codex-review',internal=True)
        self.assertFalse(any(row['id']=='s-c1' for row in self.read()))
        self.assertTrue(any(row['id']=='s-c1' for row in self.read(include_reviews=True)))

    def test_live_and_paused_are_separate_and_cache_is_bounded(self):
        frozen=deepcopy(self.snapshot)
        with patch('observatory.terminal_ui.prepare_sources',wraps=prepare_sources) as prepare:
            self.read(frozen);self.read()
            self.read(frozen);self.read()
            self.assertEqual(prepare.call_count,2)
            self.read(replace(self.snapshot,receiver_id='other'))
            self.assertEqual(len(self.cache.entries),2)
            self.read(frozen)
            self.assertEqual(prepare.call_count,4)
        self.cache.clear()
        self.assertEqual(self.cache.entries,[])

    def test_oversized_custom_feeds_bypass_retention(self):
        self.snapshot.activity*=101
        with patch('observatory.terminal_ui.prepare_sources',wraps=prepare_sources) as prepare, \
                patch('observatory.terminal_ui.deepcopy',side_effect=AssertionError('Fresh result copied')):
            self.assertEqual(self.read(),self.read())
            self.assertEqual(prepare.call_count,2)
        self.assertEqual(self.cache.entries,[])

    def test_text_node_and_depth_budgets_are_explicit(self):
        self.assertFalse(self.cache.within_budget('x'*512001))
        self.assertFalse(self.cache.within_budget([None]*20001))
        value='nested'
        for _ in range(25):value=[value]
        self.assertFalse(self.cache.within_budget(value))
        self.snapshot.activity[0]['detail']='x'*512001
        self.read()
        self.assertEqual(self.cache.entries,[])

    def test_cache_does_not_retain_unrelated_snapshot_payloads(self):
        snapshot=hierarchy_snapshot()
        snapshot.mcp=[{'large unrelated payload':'x'*1000000}]
        reference=weakref.ref(snapshot)
        self.read(snapshot)
        del snapshot
        gc.collect()
        self.assertIsNone(reference())

    def test_stored_native_policy_alias_is_exact_to_receiver_producer_worker_session(self):
        live=hierarchy_snapshot();live.receiver_id='receiver'
        native=live.local_status['sessions'][1]
        native.update(capture_content=False,service='codex-review')
        def stored(**changes):
            row=dict(session_id=native['session_id'],agent_id=native['agent_id'],
                services=['codex'],initial_task='PRIVATE stored request')
            row.update(changes)
            return replace(live,local_status={},activity=[],sessions=[row])
        self.assertFalse(sources(stored(),policy_snapshot=live)[0]['task'])
        for changes in ({'session_id':'other'},{'agent_id':'other'},{'services':['claude']}):
            self.assertTrue(sources(stored(**changes),policy_snapshot=live)[0]['task'])
        other=replace(live,receiver_id='other')
        self.assertTrue(sources(stored(),policy_snapshot=other)[0]['task'])
        global_policy=replace(live,local_status={'capture_content':False})
        self.assertFalse(sources(stored(),policy_snapshot=global_policy)[0]['task'])


class CapturePolicyUITests(unittest.IsolatedAsyncioTestCase):
    async def test_unrelated_modal_uses_uncached_single_consumer_projection(self):
        app=ObservatoryApp(HierarchyFeed(),persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.source_preparation.clear()
            with patch('observatory.terminal_ui.prepare_sources',wraps=prepare_sources) as prepare:
                app.workflow_items();app.workflow_items()
                self.assertEqual(prepare.call_count,1)
                app.push_screen(Screen());await pilot.pause()
                app.workflow_items();app.workflow_items()
                self.assertEqual(prepare.call_count,3)
                self.assertEqual(app.source_preparation.entries,[])
                app.pop_screen();await pilot.pause()

    async def test_inflight_ai_summary_rechecks_before_dispatch_and_publication(self):
        for stage in ('activity','provider'):
            with self.subTest(stage=stage):
                snapshot=hierarchy_snapshot();snapshot.receiver_id='receiver'
                root=snapshot.local_status['sessions'][0];root['capture_content']=True
                feed=HierarchyFeed(snapshot)
                started,release=threading.Event(),threading.Event()
                self.addCleanup(release.set)
                def fetch(path):
                    if stage=='activity':started.set();release.wait(5)
                    return {'activity':[]}
                def generate(*args):
                    if stage=='provider':started.set();release.wait(5)
                    return 'PRIVATE late summary'
                feed.api.get=fetch
                app=ObservatoryApp(feed,persist_preferences=False)
                with patch('observatory.ai_summary.generate_summary',side_effect=generate) as provider:
                    async with app.run_test(size=(120,36)) as pilot:
                        await pilot.pause();app.refresh_timer.stop()
                        item=next(row for row in app.workflow_items() if row['id']==root['source_id'])
                        app.summarize_agent(item,object(),scope_for(item,snapshot,feed.api.endpoint))
                        self.assertTrue(await asyncio.to_thread(started.wait,3))
                        root['capture_content']=False
                        app.refresh_data();app.select_agent('s-c2')
                        release.set()
                        await app.workers.wait_for_complete();await pilot.pause()
                        self.assertNotIn(root['source_id'],app.ai_summaries)
                        self.assertEqual(provider.call_count,0 if stage=='activity' else 1)

    async def test_paused_inspector_revocation_discards_stale_and_inflight_content(self):
        snapshot=hierarchy_snapshot()
        root=snapshot.local_status['sessions'][0]
        root.update(initial_task='PRIVATE original request',capture_content=True)
        event=dict(id='private',source_id='private',agent_id=root['agent_id'],
            session_id=root['session_id'],service=root['service'],domain='message',
            action='user',subject='PRIVATE request body',time_unix_nano=root['last_record_ns'])
        snapshot.activity.append(event)
        feed=HierarchyFeed(snapshot)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            app.select_agent(root['source_id']);app.set_session_mode('prompts')
            await pilot.pause()
            self.assertIn('PRIVATE',app.query_one('#agent-detail').plain_text)
            old_history=app.session_prompts;old_evidence=app.session_evidence
            app.action_pause();await pilot.pause()
            live=deepcopy(snapshot);live.updated+=1
            live.local_status['sessions'][0].update(capture_content=False,task='',initial_task='',last_activity={})
            # Keep the stale Activity buffer, as happens after a partial timeout.
            feed.snapshot=live
            app.refresh_data();await pilot.pause()
            self.assertFalse(app.source_map[root['source_id']]['task'])
            self.assertEqual(app.session_prompts.events,{})
            self.assertEqual(app.session_evidence.events,[])
            app.finish_session_prompts(old_history,None,{'activity':[event]},'')
            app.finish_session_evidence(old_evidence,{'activity':[event]},'')
            for mode in ('overview','prompts','details'):
                app.set_session_mode(mode);await pilot.pause()
                self.assertNotIn('PRIVATE',app.query_one('#agent-detail').plain_text,mode)
                self.assertNotIn('PRIVATE',app.query_one('#agent-heading').tooltip,mode)
            self.assertTrue(feed.paused)


if __name__=='__main__':unittest.main()
