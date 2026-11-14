"""Fresh source access and frozen old requests across scoped Feed revocation."""
from copy import deepcopy
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from textual.widgets import RichLog
from observatory.ui_findings import capture_reference_context,finding_capture_allowed
from tests.test_feed_capture_safety import settled,PRIVATE,PUBLIC
from tests.test_finding_sources_ui import Harness as FindingHarness
from tests.test_ui_investigations import Harness as InvestigationHarness
from tests.test_investigation_cli import match
from observatory.investigation import decode_reference


def owner_fixture():
    api,feed=settled();owner=SimpleNamespace(feed=feed,data=feed.snapshot)
    actor=api.sessions[0]
    scope={key:actor[key] for key in ('service','agent_id','session_id','trace_id')}
    item=dict(scope,id='f'*16,receiver_id=feed.snapshot.receiver_id)
    row=match('exact',**scope,receiver_id=feed.snapshot.receiver_id)
    ref=decode_reference(row['source_reference'])
    source_scope={key:ref.get(key) for key in ('service','agent_id','session_id','trace_id','span_id','turn_id')}
    finding=dict(receiver_id=feed.snapshot.receiver_id,finding_id=item['id'],finding_scope=scope,
        sources=[dict(type='log',id=ref['source_id'],scope=source_scope,record={'output':PRIVATE})])
    exact=dict(receiver_id=feed.snapshot.receiver_id,event_id=ref['event_id'],source_reference=ref,
        sources=deepcopy(finding['sources']))
    return owner,api,item,row,finding,exact


def regrant(owner,api):
    api.deny_a=True;api.ready=False;owner.feed.refresh()
    api.deny_a=False;api.ready=True;owner.feed.refresh()
    assert not owner.feed.snapshot.error


class CaptureReferenceModels(unittest.TestCase):
    def test_fresh_published_finding_allowed_old_snapshot_blocked_and_sibling_unchanged(self):
        owner,api,item,_,_,_=owner_fixture();old=owner.data;regrant(owner,api)
        self.assertFalse(finding_capture_allowed(item,owner.feed.snapshot,published_snapshot=old))
        self.assertTrue(finding_capture_allowed(item,owner.feed.snapshot,published_snapshot=owner.feed.snapshot))
        sibling=dict(api.sessions[1],receiver_id=owner.feed.snapshot.receiver_id)
        self.assertTrue(finding_capture_allowed(sibling,owner.feed.snapshot,published_snapshot=old))

    def test_request_context_is_detached_policy_metadata_only(self):
        owner,api,*_=owner_fixture();regrant(owner,api);context=capture_reference_context(owner.feed.snapshot)
        self.assertNotIn(PRIVATE,repr(context));self.assertNotIn(PUBLIC,repr(context))
        before=repr(context);owner.feed.snapshot.local_status['capture_policy_revisions'].clear()
        self.assertEqual(repr(context),before)


class CaptureReferenceJourneys(unittest.IsolatedAsyncioTestCase):
    async def until(self,pilot,predicate):
        end=time.monotonic()+5
        while not predicate() and time.monotonic()<end:await pilot.pause(.03)
        self.assertTrue(predicate())

    async def test_fresh_finding_source_loads_after_regrant_but_paused_old_does_not(self):
        owner,api,item,_,finding,_=owner_fixture();old=owner.data;calls=[]
        api.data_hook=lambda path:(calls.append(path)or deepcopy(finding)) if path.endswith('/source') else None
        regrant(owner,api);owner.data=owner.feed.snapshot
        app=FindingHarness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await self.until(pilot,lambda:bool(app.screen.sources))
            self.assertTrue(app.screen.query_one(RichLog).lines)
            self.assertIsNotNone(app.screen.item['capture_safety_revision'])
        count=len(calls);owner.data=old;app=FindingHarness(owner,item)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause(.1)
            self.assertEqual(app.screen.sources,[]);self.assertFalse(app.screen.query_one(RichLog).lines)
            self.assertEqual(len(calls),count)

    async def test_late_finding_deny_and_reallow_cannot_bind_completion_to_new_revision(self):
        owner,api,item,_,finding,_=owner_fixture();entered=threading.Event();release=threading.Event()
        def source(path):
            if path.endswith('/source'):
                entered.set();release.wait(3);return deepcopy(finding)
        api.data_hook=source;app=FindingHarness(owner,item)
        try:
            async with app.run_test(size=(80,24)) as pilot:
                await self.until(pilot,entered.is_set)
                # No UI yield between denial and regrant: the old callback's
                # own binding, not a timer observing the denied state, fences it.
                regrant(owner,api);release.set();await pilot.pause(.15)
                self.assertEqual(app.screen.sources,[]);self.assertFalse(app.screen.query_one(RichLog).lines)
        finally:release.set()

    async def test_fresh_investigation_and_exact_source_after_regrant(self):
        owner,api,_,row,_,exact=owner_fixture();regrant(owner,api);owner.data=owner.feed.snapshot
        with patch('observatory.ui_investigations.collect_investigation',return_value=dict(receiver_id=owner.feed.snapshot.receiver_id,matches=[row])),patch('observatory.ui_investigations.collect_source',return_value=exact):
            app=InvestigationHarness(owner,kind='command',query='pytest')
            async with app.run_test(size=(80,24)) as pilot:
                await pilot.pause();screen=app.screen;screen.action_search()
                await self.until(pilot,lambda:bool(screen.rows) and not screen.busy)
                self.assertIsNotNone(screen.references[0]['capture_safety_revision'])
                await pilot.press('enter');await self.until(pilot,lambda:bool(screen.sources) and not screen.busy)
                self.assertTrue(screen.query_one(RichLog).lines)
                self.assertNotIn('capture_safety_revision',decode_reference(screen.selected_reference))

    async def test_late_query_and_exact_source_keep_original_binding_across_rapid_reallow(self):
        for phase in ('query','source'):
            with self.subTest(phase=phase):
                owner,api,_,row,_,exact=owner_fixture();entered=threading.Event();release=threading.Event()
                def collect(**kwargs):
                    if phase=='query':entered.set();release.wait(3)
                    return dict(receiver_id=owner.feed.snapshot.receiver_id,matches=[deepcopy(row)])
                def source(*args):entered.set();release.wait(3);return deepcopy(exact)
                def query(*args,**kwargs):return collect(**kwargs)
                try:
                    with patch('observatory.ui_investigations.collect_investigation',side_effect=query),patch('observatory.ui_investigations.collect_source',side_effect=source):
                        app=InvestigationHarness(owner,kind='command',query='pytest')
                        async with app.run_test(size=(80,24)) as pilot:
                            await pilot.pause();screen=app.screen;screen.action_search()
                            if phase=='source':
                                await self.until(pilot,lambda:bool(screen.rows) and not screen.busy);await pilot.press('enter')
                            await self.until(pilot,entered.is_set)
                            regrant(owner,api);release.set();await pilot.pause(.15)
                            self.assertEqual(screen.rows,[]);self.assertEqual(screen.sources,[])
                            self.assertTrue(screen.invalidated);self.assertFalse(screen.query_one(RichLog).lines)
                finally:release.set()

    async def test_sibling_query_stays_valid_when_only_other_actor_changes_generation(self):
        owner,api,_,_,_,_=owner_fixture();sibling=api.sessions[1]
        row=match('sibling',**{key:sibling[key]for key in ('service','agent_id','session_id','trace_id')},receiver_id=owner.feed.snapshot.receiver_id)
        entered=threading.Event();release=threading.Event()
        def query(*args,**kwargs):
            entered.set();release.wait(3);return dict(receiver_id=owner.feed.snapshot.receiver_id,matches=[row])
        try:
            with patch('observatory.ui_investigations.collect_investigation',side_effect=query):
                app=InvestigationHarness(owner,kind='command',query='pytest')
                async with app.run_test(size=(80,24)) as pilot:
                    await pilot.pause();screen=app.screen;screen.action_search();await self.until(pilot,entered.is_set)
                    regrant(owner,api);release.set()
                    await self.until(pilot,lambda:bool(screen.rows) and not screen.busy)
                    self.assertFalse(screen.invalidated);self.assertEqual(screen.rows[0]['event_id'],'sibling')
        finally:release.set()
