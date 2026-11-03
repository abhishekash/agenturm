import unittest

from textual.widgets import Static
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_monitoring import session_usage, task_usage_summary, UsageScreen
from tests.test_terminal_search import ManualFeed


class TaskUsageTests(unittest.TestCase):
    def test_totals_exclude_cache_double_counting_and_show_cost(self):
        usage={'trace_count':2,'input_tokens':2000,'output_tokens':500,
               'cache_read_input_tokens':1500,'cache_write_input_tokens':100,
               'known_cost_usd':.25,'cost_complete':True}
        text,details=task_usage_summary(usage,'Fix checkout',identity='session-a')
        self.assertIn('Task cost $0.2500',text)
        self.assertIn('Tokens 2,500',text)
        self.assertIn('read 1,500 · write 100 · hit 75%',text)
        self.assertIn('Session: session-a',details)
        self.assertNotIn('Avg/task',text)
        self.assertIn('Input: 2,000',details)

    def test_zero_unknown_partial_and_log_only_are_distinct(self):
        text,_=task_usage_summary({'trace_count':1,'cost_complete':True,'known_cost_usd':0,
                                   'input_tokens':0,'output_tokens':0,'cache_read_input_tokens':0,
                                   'cache_read_input_tokens_reported':False})
        self.assertIn('cost $0.0000',text)
        self.assertIn('Tokens 0',text)
        self.assertIn('Cache read Not reported',text)
        self.assertNotIn('hit 0%',text)
        text,_=task_usage_summary({'trace_count':0,'cost_complete':True,'known_cost_usd':0})
        self.assertIn('cost Not reported',text)
        text,_=task_usage_summary({'trace_count':3,'unknown_cost_runs':2,'cost_complete':False,
                                   'known_cost_usd':.2,'usage_complete':False,'input_tokens':10,
                                   'output_tokens':5,'cache_read_input_tokens':5})
        self.assertIn('$0.2000+',text)
        self.assertIn('Tokens 15+',text)
        self.assertNotIn('hit ',text)

    def test_shared_session_id_does_not_mix_workers(self):
        rows=[{'agent_id':'a','session_id':'shared','input_tokens':1},
              {'agent_id':'b','session_id':'shared','input_tokens':2}]
        self.assertEqual(session_usage(rows,{'agent_id':'b','session_id':'shared'})['input_tokens'],2)
        self.assertEqual(session_usage(rows,{'agent_id':'c','session_id':'shared'}),{})
        self.assertEqual(session_usage(rows,{'session_id':'shared'}),{})


class TaskUsageJourney(unittest.IsolatedAsyncioTestCase):
    async def test_selection_inspector_modal_and_trace_use_the_same_task(self):
        feed=ManualFeed()
        summaries=[{'session_id':'session-1','trace_count':2,'input_tokens':2000,'output_tokens':500,
                    'cache_read_input_tokens':1500,'cache_write_input_tokens':100,
                    'known_cost_usd':.25,'cost_complete':True},
                   {'session_id':'session-2','trace_count':1,'input_tokens':800,'output_tokens':100,
                    'cache_read_input_tokens':360,'cache_write_input_tokens':20,
                    'known_cost_usd':1,'cost_complete':True}]
        get=feed.api.get
        feed.api.get=lambda path: {'sessions':summaries} if path.split('?')[0]=='/api/sessions' else get(path)
        feed.api.traces[0].update(input_tokens=10,output_tokens=2,cache_read_input_tokens=8,
                                  cache_write_input_tokens=1,known_cost_usd=.03,cost_complete=True)
        feed.extra_cache=None;feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause();app.set_agent_filter('all');app.select_agent('source-1');await pilot.pause()
            bar=lambda:'\n'.join(app.query_one('#usage-summary',Static).render_line(i).text for i in range(2))
            self.assertIn('$0.2500',bar());self.assertIn('2,500',bar());self.assertIn('75%',bar())
            app.select_agent('source-2');await pilot.pause()
            self.assertIn('$1.0000',bar());self.assertIn('Tokens 900',bar());self.assertIn('45%',bar())
            self.assertNotIn('$0.2500',bar())
            await pilot.press('e');await pilot.pause()
            self.assertIn('Task cost $1.0000',app.query_one('#agent-detail').plain_text)
            await pilot.press('u');await pilot.pause()
            self.assertIsInstance(app.screen,UsageScreen)
            self.assertIn('Session: session-2',app.screen.details)
            self.assertIn('Input: 800',app.screen.details)
            self.assertIn('Cache write: 20',app.screen.details)
            await pilot.press('escape','escape');await pilot.resize_terminal(60,20);await pilot.pause()
            self.assertIn('Task cost $1.0000',bar());self.assertIn('Cache read 360',bar())
            self.assertIn('hit 45%',bar())
            await pilot.press('4');await pilot.pause()
            self.assertIn('$0.0300',bar());self.assertIn('Tokens 12',bar())
            await pilot.press('u');await pilot.pause()
            self.assertIn('Trace: trace-1',app.screen.details)
            self.assertIn('this recorded run',app.screen.details)
            log=app.screen.query_one('#usage-log')
            self.assertEqual(log.scroll_offset.y,0)
            self.assertIn('SELECTED TASK USAGE',log.render_line(0).text)
            self.assertFalse(log.show_horizontal_scrollbar)
