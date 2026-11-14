"""Actual Agents inspector frames prioritize recorded work over completion claims."""
import time
from pathlib import Path
import unittest
from textual.widgets import TabbedContent
from observatory.terminal_ui import ObservatoryApp,sources
from tests.test_terminal_search import ManualFeed


class OverviewJourney(unittest.IsolatedAsyncioTestCase):
    async def test_recorded_work_and_partial_coverage_at_all_supported_frames(self):
        for size in ((120,36),(80,24),(60,18)):
            feed=ManualFeed();agent=sources(feed.snapshot)[0]['agent_id']
            entry=feed.api.sessions[0];entry.update(agent_id=agent,initial_task='Repair retry handling',latest_trace_id='a'*32)
            now=time.time_ns()
            def row(identity,**values):
                return dict(source_id=identity,receiver_id='receiver',agent_id=agent,service='codex',session_id='session-1',
                    trace_id='a'*32,time_unix_nano=now,**values)
            feed.api.activity=[row('edit',domain='file',action='update',subject='retry.py',result='success'),
                row('test-file',domain='file',action='add',subject='tests/test_retry.py',result='success'),
                row('pass',domain='shell',action='execute',subject='pytest tests -q',result='success',
                    attributes={'process.command_line':'pytest tests -q','process.exit.code':0}),
                row('failed',domain='shell',action='execute',subject='pytest tests/test_retry.py',result='error',
                    attributes={'process.command_line':'pytest tests/test_retry.py','process.exit.code':1,'stderr':'Expected retry count 3, got 2'}),
                row('claim',domain='message',action='assistant',subject='All tests pass',result='observed')]
            original=feed.api.get
            def get(path):
                response=original(path)
                return {**response,'receiver_id':'receiver'} if path.startswith('/api/health') else response
            feed.api.get=get;feed.refresh()
            app=ObservatoryApp(feed,persist_preferences=False)
            async with app.run_test(size=size) as pilot:
                await pilot.pause();app.action_all();app.select_agent('source-1');await pilot.pause()
                if size[1]>=30:
                    preview=app.query_one('#agent-detail').plain_text
                    self.assertIn('2 file operations',preview)
                    self.assertIn('1 passed',preview);self.assertIn('1 failed',preview)
                    self.assertLess(preview.casefold().index('recorded work'),preview.casefold().index('agent reported'))
                await pilot.press('e');await pilot.pause()
                text=app.query_one('#agent-detail').plain_text
                self.assertIn('2 file operations',text)
                self.assertIn('1 passed',text);self.assertIn('1 failed',text)
                self.assertIn('tests/test_retry.py',text)
                self.assertIn('whole-run capture',text)
                self.assertIn('recent validation',text.casefold())
                Path('/tmp/agenturm-overview-'+str(size[0])+'.svg').write_text(app.export_screenshot())
                await pilot.press('escape');await pilot.pause()
                self.assertEqual(app.selected_source,'source-1')


if __name__=='__main__':unittest.main()
