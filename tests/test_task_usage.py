import unittest
import tempfile
from pathlib import Path
from observatory.core import Store, normalize_native

from textual.widgets import Static
from observatory.terminal_ui import ObservatoryApp
from observatory.ui_monitoring import session_usage, task_run_usage, task_usage_summary, UsageScreen
from tests.test_terminal_search import ManualFeed


class TaskUsageTests(unittest.TestCase):
    def test_latest_run_usage_is_not_the_session_total(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'runs.db')
            try:
                for i,tokens,cost in ((1,100,.2),(2,700,.7)):
                    store.ingest([normalize_native({'trace_id':f'{i:032x}','span_id':f'{i:016x}',
                        'name':'task','start_unix_nano':i*100,'end_unix_nano':i*100+10,'status':'OK',
                        'attributes':{'session.id':'session-a','gen_ai.usage.input_tokens':tokens,
                                      'gen_ai.usage.output_tokens':0,'llm.cost.total':cost}})])
                session=store.sessions()[0]
                self.assertEqual(session['input_tokens'],800)
                latest=task_run_usage([session],{'session_id':'session-a'})
                self.assertEqual(latest['input_tokens'],700)
                self.assertEqual(latest['known_cost_usd'],.7)
                self.assertEqual(latest['trace_id'],f'{2:032x}')
                self.assertEqual(task_run_usage([{'session_id':'session-a','input_tokens':800}],
                                                {'session_id':'session-a'}),{})
            finally:store.close()

    def test_totals_exclude_cache_double_counting_and_show_cost(self):
        usage={'trace_count':2,'input_tokens':2000,'output_tokens':500,
               'cache_read_input_tokens':1500,'cache_write_input_tokens':100,
               'known_cost_usd':.25,'cost_complete':True}
        text,details=task_usage_summary(usage,'Fix checkout',identity='session-a')
        self.assertIn('Task cost $0.2500',text)
        self.assertIn('Tokens 2,500',text)
        self.assertIn('Cached input 1,500 · 75% of input · Cache write 100',text)
        self.assertIn('Session: session-a',details)
        self.assertNotIn('Avg/task',text)
        self.assertIn('Input: 2,000',details)
        self.assertIn('Input 2,000 · Output 500',text)

    def test_zero_unknown_partial_and_log_only_are_distinct(self):
        text,_=task_usage_summary({'trace_count':1,'cost_complete':True,'known_cost_usd':0,
                                   'input_tokens':0,'output_tokens':0,'cache_read_input_tokens':0,
                                   'cache_read_input_tokens_reported':False})
        self.assertIn('cost $0.0000',text)
        self.assertIn('Tokens 0',text)
        self.assertIn('Cached input Not reported',text)
        self.assertNotIn('Cache write',text)
        self.assertNotIn('% of input',text)
        text,details=task_usage_summary({'input_tokens':10,'output_tokens':0,
            'cache_write_input_tokens':0,'cache_write_input_tokens_reported':True})
        self.assertIn('Cache write 0',text)
        self.assertIn('Cache write: 0',details)
        text,details=task_usage_summary({'cache_write_input_tokens':0,
                                       'cache_write_input_tokens_reported':False})
        self.assertNotIn('Cache write',text)
        self.assertNotIn('Cache write:',details)
        text,_=task_usage_summary({'trace_count':0,'cost_complete':True,'known_cost_usd':0})
        self.assertIn('cost Not reported',text)
        text,_=task_usage_summary({'trace_count':3,'unknown_cost_runs':2,'cost_complete':False,
                                   'known_cost_usd':.2,'usage_complete':False,'input_tokens':10,
                                   'output_tokens':5,'cache_read_input_tokens':5})
        self.assertIn('$0.2000+',text)
        self.assertIn('Tokens 15+',text)
        self.assertNotIn('% of input',text)

    def test_shared_session_id_does_not_mix_workers(self):
        rows=[{'agent_id':'a','session_id':'shared','input_tokens':1},
              {'agent_id':'b','session_id':'shared','input_tokens':2}]
        self.assertEqual(session_usage(rows,{'agent_id':'b','session_id':'shared'})['input_tokens'],2)
        self.assertEqual(session_usage(rows,{'agent_id':'c','session_id':'shared'}),{})
        self.assertEqual(session_usage(rows,{'session_id':'shared'}),{})
        untagged=[{'session_id':'shared','input_tokens':99}]
        self.assertEqual(session_usage(untagged,{'agent_id':'b','session_id':'shared'}),{})
        self.assertEqual(session_usage(untagged,{'agent_id':'legacy','agent_id_inferred':True,
                                               'session_id':'shared'})['input_tokens'],99)


class TaskUsageJourney(unittest.IsolatedAsyncioTestCase):
    async def test_selection_inspector_modal_and_trace_use_the_same_task(self):
        feed=ManualFeed()
        summaries=[{'session_id':'session-1','trace_count':2,'input_tokens':2000,'output_tokens':500,
                    'cache_read_input_tokens':1500,'cache_write_input_tokens':100,
                    'known_cost_usd':.25,'cost_complete':True},
                   {'session_id':'session-2','trace_count':1,'input_tokens':800,'output_tokens':100,
                    'cache_read_input_tokens':360,'cache_write_input_tokens':20,
                    'known_cost_usd':1,'cost_complete':True}]
        for index,row in enumerate(summaries,1):
            row['latest_run']={k:v for k,v in row.items() if k not in {'trace_count','session_id'}}
            row['latest_run']['trace_id']=f'run-{index}'
            row['known_cost_usd']=99
            row['input_tokens']=999999
        get=feed.api.get
        feed.api.get=lambda path: {'sessions':summaries} if path.split('?')[0]=='/api/sessions' else get(path)
        feed.api.traces[0].update(input_tokens=10,output_tokens=2,cache_read_input_tokens=8,
                                  cache_write_input_tokens=1,known_cost_usd=.03,cost_complete=True)
        feed.sessions_attempted=None;feed.refresh()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,40)) as pilot:
            await pilot.pause();app.set_agent_filter('all');app.select_agent('source-1');await pilot.pause()
            bar=lambda:' '.join(app.query_one('#agent-detail').plain_text.split())
            self.assertFalse(app.query_one('#usage-summary').display)
            row=app.query_one('#agent-table').get_row('source-1')[0].plain
            self.assertNotIn('tok ·',row)
            self.assertNotIn('cache',row.lower())
            app.set_session_mode('details');await app.workers.wait_for_complete();await pilot.pause()
            self.assertIn('$0.2500',bar());self.assertIn('2,500',bar());self.assertIn('75%',bar())
            app.select_agent('source-2');await pilot.pause()
            self.assertIn('$1.0000',bar());self.assertIn('Tokens 900',bar());self.assertIn('45%',bar())
            self.assertNotIn('$0.2500',bar())
            self.assertIn('Task cost $1.0000',bar())
            await pilot.press('u');await pilot.pause()
            self.assertIsInstance(app.screen,UsageScreen)
            self.assertIn('Trace: run-2',app.screen.details)
            self.assertNotIn('999,999',app.screen.details)
            self.assertIn('Input: 800',app.screen.details)
            self.assertIn('Cache write: 20',app.screen.details)
            await pilot.press('escape','escape');await pilot.resize_terminal(60,20);await pilot.pause()
            await pilot.press('e');await pilot.pause()
            self.assertIn('Task cost $1.0000',bar());self.assertIn('Cached input 360',bar())
            self.assertIn('45% of input',bar())
            await pilot.press('escape');await pilot.pause()
            await pilot.press('4');await pilot.pause()
            await pilot.press('u');await pilot.pause()
            self.assertIn('Trace: trace-1',app.screen.details)
            self.assertIn('Cost: $0.0300',app.screen.details)
            self.assertIn('Tokens used: 12',app.screen.details)
            self.assertIn('this recorded run',app.screen.details)
            log=app.screen.query_one('#usage-log')
            self.assertEqual(log.scroll_offset.y,0)
            self.assertIn('SELECTED TASK USAGE',log.render_line(0).text)
            self.assertFalse(log.show_horizontal_scrollbar)
