"""Selected-session reads remain bounded, asynchronous and exactly attributed."""
import threading
import time
import unittest
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from observatory.terminal_ui import ObservatoryApp, sources
from tests.test_agent_hierarchy_ui import HierarchyFeed, agent_node
from tests.test_terminal_search import ManualFeed


class SessionEvidenceTests(unittest.IsolatedAsyncioTestCase):
    async def test_quiet_worker_gets_exact_history_outside_global_window(self):
        feed=HierarchyFeed()
        worker=feed.snapshot.local_status['sessions'][1]
        worker['session_id']='shared'
        feed.snapshot.activity=[]
        calls=[]
        def get(path):
            q=parse_qs(urlsplit(path).query);calls.append(q)
            base=dict(service='codex',session_id='shared',time_unix_nano=time.time_ns(),domain='message',action='assistant')
            return {'activity':[dict(base,id='own',agent_id=worker['agent_id'],subject='Own recorded progress'),
                                dict(base,id='sibling',agent_id='sibling',subject='SIBLING DATA'),
                                dict(base,id='untagged',subject='PARENT DATA'),
                                dict(base,id='service',agent_id=worker['agent_id'],service='claude',subject='OTHER SERVICE')],
                    'next_cursor':'older'}
        feed.api=SimpleNamespace(get=get,endpoint='http://example.invalid')
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(150,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.select_agent(worker['source_id']);app.action_expand()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(calls[0]['agent_id'],[worker['agent_id']])
            self.assertEqual(calls[0]['session_id'],['shared'])
            self.assertEqual(calls[0]['subtree'],['0'])
            self.assertEqual(calls[0]['limit'],['100'])
            text=app.query_one('#agent-detail').plain_text
            self.assertIn('Own recorded progress',text)
            for other in ('SIBLING DATA','PARENT DATA','OTHER SERVICE'):self.assertNotIn(other,text)
            self.assertIn('earlier records',app.query_one('#agent-evidence-status').render().plain)
            for _ in range(5):app.show_agent()
            self.assertEqual(len(calls),1)
            # A new source revision triggers one update, not a request per repaint.
            worker['last_record_ns']+=1
            app.last_update=-1;app.refresh_data()
            await app.workers.wait_for_complete();await pilot.pause()
            self.assertEqual(len(calls),2)

    async def test_slow_evidence_cannot_publish_into_new_selection_and_retry_retains_data(self):
        feed=ManualFeed()
        original=feed.api.get
        started,release=threading.Event(),threading.Event()
        self.addCleanup(release.set)
        def slow(path):
            q=parse_qs(urlsplit(path).query)
            if urlsplit(path).path=='/api/activity' and q.get('session_id')==['session-1']:
                started.set();release.wait(5)
                return {'activity':[dict(feed.api.activity[0],domain='message',action='assistant',subject='STALE FIRST SESSION')]}
            return original(path)
        feed.api.get=slow
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            app.select_agent('source-1');app.action_expand();await pilot.pause()
            self.assertTrue(started.is_set())
            app.select_agent('source-2');await pilot.pause()
            self.assertNotIn('STALE FIRST SESSION',app.query_one('#agent-detail').plain_text)
            release.set();await app.workers.wait_for_complete();await pilot.pause()
            self.assertNotIn('STALE FIRST SESSION',app.query_one('#agent-detail').plain_text)
            self.assertEqual(app.session_evidence.scope[1],'session-2')

    def test_title_fallback_isolates_services_and_workers(self):
        feed=HierarchyFeed()
        a=agent_node('a',None,'','running')
        b=agent_node('b',None,'','running')
        a['session_id']=b['session_id']='shared'
        feed.snapshot.local_status['sessions']=[a,b]
        base=dict(session_id='shared',domain='message',action='user',time_unix_nano=time.time_ns())
        feed.snapshot.activity=[dict(base,service='codex',agent_id=a['agent_id'],subject='Task A'),
                                dict(base,service='codex',agent_id=b['agent_id'],subject='Task B'),
                                dict(base,service='claude',subject='WRONG TASK')]
        rows={row['source_id']:row for row in sources(feed.snapshot)}
        self.assertEqual(rows['s-a']['task'],'Task A')
        self.assertEqual(rows['s-b']['task'],'Task B')


if __name__=='__main__':unittest.main()
