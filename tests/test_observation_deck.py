"""Website-inspired layout without losing real session state or navigation."""
import time
import unittest

from textual.widgets import DataTable, TabbedContent

from observatory.terminal_ui import ObservatoryApp, agent_status, sources, task_text
from observatory.themes import PALETTES
from tests.test_terminal_search import ManualFeed


class ObservationDeckTests(unittest.IsolatedAsyncioTestCase):
    async def test_wide_session_inspector_is_beside_evidence_and_toolbar_is_below_tabs(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause()
            results = app.query_one('#results-agents')
            inspector = app.query_one('#agent-bottom')
            self.assertEqual(results.region.y, inspector.region.y)
            self.assertGreaterEqual(inspector.region.x, results.region.right)
            self.assertGreaterEqual(inspector.size.width, 36)
            controls = app.query_one('#controls')
            tabs = app.query_one('#pages > Tabs')
            self.assertGreaterEqual(controls.region.y, tabs.region.bottom)
            self.assertGreaterEqual(results.region.y, controls.region.bottom)
            self.assertLessEqual(results.region.y,12)
            self.assertEqual(app.query_one('#search-agents').size.height,1)
            self.assertEqual(controls.size.height,1)
            self.assertEqual([str(c.label) for c in app.query_one('#agent-table', DataTable).columns.values()],
                             ['Agent / project', 'Current task', 'Status', 'Updated'])
            self.assertFalse(app.query_one('#agent-table').zebra_stripes)
            self.assertNotIn('session-1', app.query_one('#agent-detail').plain_text)
            self.assertNotIn('LATEST RECORDED EVENT', app.query_one('#agent-detail').plain_text)
            await pilot.click('#expand-agents')
            await pilot.pause()
            self.assertFalse(app.query_one('#results-agents').display)
            self.assertTrue(inspector.display)
            self.assertIn('session-1', app.query_one('#agent-detail').plain_text)
            await pilot.press('escape')
            await pilot.pause()
            self.assertTrue(app.query_one('#results-agents').display)

    async def test_work_and_attention_cards_do_not_count_normal_tool_waits_as_action(self):
        feed = ManualFeed()
        now = time.time_ns()
        for entry in feed.snapshot.local_status['sessions']:
            entry['last_record_ns'] = now
        feed.snapshot.local_status['sessions'][0].update(run_state='running', waiting_for='Tool response: build')
        feed.snapshot.local_status['sessions'][1].update(run_state='waiting', waiting_for='Approval')
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            work = app.query_one('#stat-sessions').render().plain
            attention = app.query_one('#stat-attention').render().plain
            self.assertEqual(work.splitlines()[0],'1')
            self.assertEqual(attention.splitlines()[0],'1')
            self.assertEqual(app.query_one('#stat-sessions').border_title,'WORKING AGENTS')
            self.assertEqual(app.query_one('#stat-attention').border_title,'NEEDS ATTENTION')
            self.assertIn('Codex 1', work)
            self.assertEqual(len(app.query('#stat-processes')), 0)
            await pilot.press('2')
            await pilot.pause()
            self.assertEqual(app.query_one('#pages', TabbedContent).active, 'activity')
            self.assertTrue(app.query_one('#activity-inspector').display)

    async def test_narrow_stacks_and_compact_keeps_inspection_reachable(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            results = app.query_one('#results-agents')
            inspector = app.query_one('#agent-bottom')
            self.assertGreaterEqual(inspector.region.y, results.region.bottom)
            await pilot.resize_terminal(80, 24)
            await pilot.pause()
            self.assertFalse(inspector.display)
            self.assertTrue(results.display)
            await pilot.press('e')
            await pilot.pause()
            self.assertTrue(inspector.display)
            self.assertIn('session-1', app.query_one('#agent-detail').plain_text)

    async def test_selection_marker_tracks_keyboard_refresh_and_resize_without_erasing_status_colors(self):
        feed=ManualFeed()
        now=time.time_ns()
        feed.snapshot.local_status['sessions'][0].update(run_state='running',last_record_ns=now)
        feed.snapshot.local_status['sessions'][1].update(run_state='completed',last_record_ns=now)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            table=app.query_one('#agent-table',DataTable)
            self.assertTrue(table.get_row('source-1')[0].plain.startswith('▸ '))
            self.assertFalse(table.get_row('source-2')[0].plain.startswith('▸ '))
            await pilot.press('down')
            await pilot.pause()
            self.assertEqual(app.selected_source,'source-2')
            self.assertTrue(table.get_row('source-2')[0].plain.startswith('▸ '))
            self.assertFalse(table.get_row('source-1')[0].plain.startswith('▸ '))
            app.last_update=-1
            app.refresh_data()
            await pilot.resize_terminal(80,24)
            await pilot.resize_terminal(180,48)
            await pilot.pause()
            self.assertEqual(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value,'source-2')
            self.assertTrue(table.get_row('source-2')[0].plain.startswith('▸ '))
            for theme,colors in PALETTES.items():
                app.theme=theme
                await pilot.pause()
                segments=[segment for row in range(table.size.height) for segment in table.render_line(row)]
                working=next(segment for segment in segments if '● Working' in segment.text)
                completed=next(segment for segment in segments if '✓ Completed' in segment.text)
                self.assertEqual(working.style.color.get_truecolor().hex,colors['success'])
                self.assertEqual(completed.style.color.get_truecolor().hex,colors['muted'])

    async def test_compact_navigation_and_back_button_are_not_clipped(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause()
            tabs=app.query_one('#pages > Tabs')
            findings=app.query_one('#--content-tab-findings')
            self.assertTrue(tabs.region.contains_region(findings.region))
            app.remember_location()
            await pilot.pause()
            back=app.query_one('#history-back')
            self.assertEqual(back.size.height,1)
            self.assertIn('Back',back.render_line(0).text)
            self.assertTrue(app.query_one('#controls').region.contains_region(back.region))

    async def test_inspector_rewraps_after_its_actual_width_changes(self):
        feed=ManualFeed()
        title='Sharpen the terminal layout and preserve keyboard navigation'
        feed.snapshot.local_status['sessions'][0]['task']=title
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            log=app.query_one('#agent-detail')
            self.assertNotIn(title,[line.text.rstrip() for line in log.lines])
            await pilot.resize_terminal(120,40)
            await pilot.pause()
            self.assertIn(title,[line.text.rstrip() for line in log.lines])
            self.assertIn('Shell command · git',log.plain_text)
            self.assertLessEqual(len(log.lines),log.scrollable_content_region.height)
            await pilot.resize_terminal(180,48)
            await pilot.pause()
            self.assertNotIn(title,[line.text.rstrip() for line in log.lines])

    def test_status_shapes_separate_work_history_and_explicit_requests(self):
        self.assertEqual(agent_status({'state':'Working','active':True}).plain,'● Working')
        self.assertEqual(agent_status({'state':'Working (quiet)','active':True}).plain,'◌ Quiet work')
        self.assertEqual(agent_status({'state':'Completed'}).plain,'✓ Completed')
        self.assertEqual(agent_status({'state':'Waiting','waiting_for':'Approval'}).plain,'! NEEDS APPROVAL')
        self.assertEqual(agent_status({'state':'Failed'}).plain,'× Failed')
        self.assertEqual(agent_status({'state':'Unknown'}).plain,'· Unknown')

    async def test_long_commands_and_output_are_never_in_the_default_overview(self):
        feed=ManualFeed()
        event=feed.snapshot.activity[0]
        event.update(domain='shell',subject='curl -fsSL --secret-argument '+('x'*2000),
                     detail='RAW_RESPONSE_MARKER '+('body{color:red}'*1000))
        feed.snapshot.local_status['sessions'][0]['task']='Inspect the website performance'
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            log=app.query_one('#agent-detail')
            text=log.plain_text
            self.assertIn('Shell command · curl',text)
            self.assertNotIn('--secret-argument',text)
            self.assertNotIn('RAW_RESPONSE_MARKER',text)
            self.assertLess(len(log.lines),14)
            self.assertFalse(app.query_one('#agent-actions').display)
            self.assertFalse(app.query_one('#agent-summary').display)
            await pilot.press('e')
            await pilot.pause()
            self.assertIn('RAW_RESPONSE_MARKER',log.plain_text)
            await pilot.press('escape')
            await pilot.pause()
            self.assertNotIn('RAW_RESPONSE_MARKER',log.plain_text)

    def test_hidden_reviews_are_not_reintroduced_from_stored_otlp_summaries(self):
        feed=ManualFeed()
        native={'source_id':'review','session_id':'review-1','runtime':'Codex',
                'service':'codex-review','model':'codex-auto-review','task':'Approval review'}
        feed.snapshot.local_status['sessions'].append(native)
        feed.snapshot.sessions=[
            {'session_id':'review-1','services':['codex-review'],'models':['codex-auto-review']},
            {'session_id':'review-2','services':['codex-review'],'models':['codex-auto-review']},
            {'session_id':'user-session','services':['codex'],'latest_task':'A real user task'},
        ]
        hidden=sources(feed.snapshot)
        self.assertFalse(any(row.get('session_id') in {'review-1','review-2'} for row in hidden))
        shown=sources(feed.snapshot,include_reviews=True)
        self.assertEqual(sum(row.get('session_id')=='review-1' for row in shown),1)
        self.assertTrue(next(row for row in shown if row.get('session_id')=='review-2')['review'])
        self.assertTrue(any(row.get('session_id')=='user-session' for row in hidden))

    def test_transport_replies_get_a_short_label_without_changing_the_underlying_task(self):
        task='<send_user_message_question_reply> [{"questionItemId":"private-id","answer":"yes"}]'
        item={'task':task}
        preview=task_text(item)
        self.assertEqual(preview.plain,'Reply to input request')
        self.assertTrue(preview.no_wrap)
        self.assertEqual(item['task'],task)
