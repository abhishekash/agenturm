"""Website-inspired layout without losing real session state or navigation."""
import time
import unittest

from textual.widgets import Button, DataTable, Select, TabbedContent

from observatory.terminal_ui import ObservatoryApp, agent_status, sources, task_text
from observatory.themes import PALETTES
from tests.test_terminal_search import ManualFeed


class ObservationDeckTests(unittest.IsolatedAsyncioTestCase):
    async def test_wide_session_inspector_is_beside_evidence_and_toolbar_is_below_tabs(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
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
            self.assertEqual(results.region.y,9)
            self.assertEqual(app.query_one('#search-agents').size.height,1)
            self.assertEqual(controls.size.height,1)
            self.assertEqual([str(c.label) for c in app.query_one('#agent-table', DataTable).columns.values()],
                             ['Task / source', 'State', 'Updated'])
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
            self.assertIn('1 working agent',work)
            self.assertIn('1 needs attention',attention)
            self.assertIn('Codex 1', app.query_one('#stat-sessions').tooltip)
            self.assertEqual(len(app.query('#stat-processes')), 0)
            await pilot.press('2')
            await pilot.pause()
            self.assertEqual(app.query_one('#pages', TabbedContent).active, 'activity')
            self.assertTrue(app.query_one('#activity-inspector').display)

    async def test_narrow_stacks_and_compact_keeps_inspection_reachable(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        async with app.run_test(size=(120, 40)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            results = app.query_one('#results-agents')
            inspector = app.query_one('#agent-bottom')
            self.assertEqual(inspector.region.y, results.region.y)
            await pilot.resize_terminal(100, 40)
            await pilot.pause()
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
            app.set_agent_filter('all')
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
            self.assertFalse(findings.display)
            self.assertTrue(app.query_one('#more-views').display)
            await pilot.press('7')
            await pilot.pause()
            self.assertEqual(app.query_one('#pages', TabbedContent).active,'findings')
            self.assertEqual(str(app.query_one('#more-views').value),'findings')
            app.remember_location()
            await pilot.pause()
            back=app.query_one('#history-back')
            self.assertEqual(back.size.height,1)
            self.assertIn('Back',back.render_line(0).text)
            self.assertTrue(app.query_one('#controls').region.contains_region(back.region))

    async def test_secondary_views_are_direct_tabs_or_a_labelled_visible_picker(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            picker=app.query_one('#more-views',Select)
            self.assertFalse(picker.display)
            for page in ('host','metrics','findings'):
                tab=app.query_one('#--content-tab-'+page)
                self.assertTrue(tab.display)
                self.assertTrue(app.screen.region.contains_region(tab.region))
                await pilot.click('#--content-tab-'+page)
                await pilot.pause()
                self.assertEqual(app.query_one('#pages',TabbedContent).active,page)

            await pilot.press('1')
            await pilot.resize_terminal(80,24)
            await pilot.pause()
            self.assertTrue(picker.display)
            self.assertIn('Other views',picker.query_one('SelectCurrent #label').render_line(0).text)
            self.assertTrue(app.screen.region.contains_region(picker.region))
            self.assertGreaterEqual(picker.region.x,app.query_one('#--content-tab-traces').region.right)
            await pilot.click('#more-views')
            await pilot.press('down','enter')
            await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'host')
            self.assertIn('5 Host',picker.query_one('SelectCurrent #label').render_line(0).text)
            self.assertTrue(picker.has_class('secondary-active'))

            # Very narrow terminals use one labelled selector for all seven
            # views, rather than letting an overflow control cover a tab.
            await pilot.resize_terminal(50,24)
            await pilot.pause()
            self.assertTrue(app.screen.region.contains_region(picker.region))
            await pilot.press('3')
            await pilot.pause()
            self.assertEqual(picker.value,'mcp')
            self.assertIn('Views: 3 MCP Calls',picker.query_one('SelectCurrent #label').render_line(0).text)
            await pilot.resize_terminal(160,48)
            await pilot.pause()
            self.assertFalse(picker.display)
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'mcp')

    async def test_enter_and_e_both_expand_the_selected_agent_inspector(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            await pilot.press('enter')
            await pilot.pause()
            self.assertFalse(app.query_one('#results-agents').display)
            self.assertTrue(app.query_one('#agent-bottom').display)
            await pilot.press('escape')
            await pilot.pause()
            self.assertTrue(app.query_one('#results-agents').display)
            await pilot.press('e')
            await pilot.pause()
            self.assertFalse(app.query_one('#results-agents').display)

    async def test_inspector_rewraps_after_its_actual_width_changes(self):
        feed=ManualFeed()
        title='Sharpen the terminal layout and preserve keyboard navigation'
        feed.snapshot.local_status['sessions'][0]['task']=title
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            log=app.query_one('#agent-detail')
            self.assertNotIn(title,[line.text.rstrip() for line in log.lines])
            await pilot.resize_terminal(120,40)
            await pilot.pause()
            self.assertIn(title,' '.join(line.text.rstrip() for line in log.lines))
            self.assertIn('Shell command · git',log.plain_text)
            self.assertLessEqual(len(log.lines),log.scrollable_content_region.height)
            await pilot.resize_terminal(180,48)
            await pilot.pause()
            self.assertNotIn(title,[line.text.rstrip() for line in log.lines])

    async def test_hidden_inspector_updates_reappear_at_the_same_width_without_character_wrapping(self):
        feed=ManualFeed()
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            app.set_agent_filter('all')
            await pilot.pause()
            log=app.query_one('#agent-detail')
            inspector=app.query_one('#agent-bottom')
            app.select_agent('source-1')
            width=log.scrollable_content_region.width
            for title in ('Best reliable mini PC under €300 in Germany','Inspect the failed web search'):
                inspector.display=False
                await pilot.pause()
                self.assertEqual(log.scrollable_content_region.width,0)
                previous=log.plain_text
                # A late selection or live update can reach an inspector after
                # its parent is hidden. Showing it again need not resize it.
                feed.snapshot.local_status['sessions'][0]['task']=title
                app.source_map[app.selected_source]['task']=title
                app.show_agent()
                self.assertEqual(log.plain_text,previous)
                inspector.display=True
                await pilot.pause()
                self.assertEqual(log.scrollable_content_region.width,width)
                self.assertIn(title,[line.text.rstrip() for line in log.lines])
                self.assertLessEqual(len(log.lines),log.scrollable_content_region.height)

    async def test_failed_session_inspector_survives_compact_and_tab_transitions(self):
        feed=ManualFeed()
        feed.snapshot.local_status['sessions']=[]
        feed.snapshot.sessions=[{'session_id':'failed-session','latest_status':'ERROR',
            'latest_task':'Best reliable mini PC under €300 in Germany',
            'last_unix_nano':time.time_ns(),'errors':1,'trace_count':1}]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            log=app.query_one('#agent-detail')
            for width,height in ((180,24),(180,48),(100,40),(180,48)):
                await pilot.resize_terminal(width,height)
                await pilot.pause()
                app.show_agent()
                if height>=30:
                    self.assertIn('Best reliable mini PC',' '.join(line.text.rstrip() for line in log.lines))
                    self.assertIn('Telemetry recorded 1 failed run(s)',log.plain_text)
                await pilot.press('2','1')
                await pilot.pause()
            self.assertIn('Best reliable mini PC under €300 in Germany',log.plain_text)

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
            app.set_agent_filter('all')
            await pilot.pause()
            log=app.query_one('#agent-detail')
            text=log.plain_text
            self.assertIn('Shell command · curl',text)
            self.assertNotIn('--secret-argument',text)
            self.assertNotIn('RAW_RESPONSE_MARKER',text)
            self.assertLessEqual(len(log.lines),16)
            self.assertFalse(app.query_one('#agent-actions').display)
            self.assertFalse(app.query_one('#agent-summary').display)
            await pilot.press('e')
            await pilot.pause()
            self.assertIn('RAW_RESPONSE_MARKER',log.plain_text)
            await pilot.press('escape')
            await pilot.pause()
            self.assertNotIn('RAW_RESPONSE_MARKER',log.plain_text)

    async def test_task_rows_filters_palette_theme_and_secondary_view_picker(self):
        feed=ManualFeed()
        now=time.time_ns()
        feed.snapshot.local_status['sessions'][0].update(
            task='',run_state='waiting',waiting_for='Approval',last_record_ns=now)
        feed.snapshot.local_status['sessions'][1].update(
            task='Repair keyboard navigation',run_state='running',last_record_ns=now)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            self.assertEqual(app.theme,'Observation')
            self.assertIn('Untitled session · Codex',app.query_one('#agent-table').get_row('source-1')[0].plain)
            self.assertIn('Repair keyboard navigation',app.query_one('#agent-table').get_row('source-2')[0].plain)
            self.assertEqual(app.query_one('#agent-table').get_row('source-1')[0].plain.count('\n'),1)
            app.query_one('#agent-filter',Select).value='active'
            await pilot.pause()
            self.assertEqual(app.agent_filter,'active')
            self.assertEqual(app.query_one('#agent-table').row_count,1)
            await pilot.press('ctrl+p')
            await pilot.pause()
            self.assertTrue(app.screen.__class__.__name__.endswith('CommandPalette'))
            await pilot.press('escape')
            await pilot.pause()
            app.action_page('metrics')
            await pilot.pause()
            self.assertEqual(app.query_one('#more-views').value,'metrics')

    async def test_recent_filter_shows_active_and_only_latest_finished_with_tab_navigation(self):
        feed=ManualFeed()
        now=time.time_ns()
        sessions=feed.snapshot.local_status['sessions']
        sessions[0].update(run_state='completed',last_record_ns=now-2_000_000)
        sessions[1].update(run_state='completed',last_record_ns=now-1_000_000)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            table=app.query_one('#agent-table',DataTable)
            self.assertEqual(table.row_count,1)
            expected=max(app.source_map.values(),key=lambda item:item['stamp'])['id']
            self.assertEqual(list(table.rows)[0].value,expected)
            sessions[0].update(run_state='running',last_record_ns=now)
            app.last_update=-1
            app.refresh_data()
            await pilot.pause()
            self.assertEqual(table.row_count,2)
            self.assertEqual(app.focused,table)
            await pilot.press('tab')
            await pilot.pause()
            self.assertNotEqual(app.focused,table)
            await pilot.press('shift+tab')
            await pilot.pause()
            self.assertEqual(app.focused,table)
            app.query_one('#agent-filter',Select).value='all'
            await pilot.pause()
            self.assertEqual(table.row_count,2)

    async def test_live_priority_changes_queue_order_until_explicit_refresh(self):
        feed=ManualFeed()
        now=time.time_ns()
        sessions=feed.snapshot.local_status['sessions']
        sessions[0].update(run_state='running',waiting_for='',last_record_ns=now+1)
        sessions[1].update(run_state='completed',waiting_for='',last_record_ns=now+2)
        feed.snapshot.sessions=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.refresh_timer.stop()
            app.set_agent_filter('all')
            await pilot.pause()
            table=app.query_one('#agent-table')
            before=[key.value for key in table.rows]
            self.assertEqual(before,['source-1','source-2'])
            # A terminated session must stay below working work even when its
            # timestamp is newer.  When it starts again, the list freezes until
            # the operator applies the queued order.
            sessions[1].update(run_state='running',waiting_for='',last_record_ns=now+3)
            app.last_update=-1
            app.refresh_data()
            await pilot.pause()
            self.assertEqual([key.value for key in table.rows],before)
            self.assertTrue(app.query_one('#refresh-order').display)
            await pilot.click('#refresh-order')
            await pilot.pause()
            self.assertEqual(list(table.rows)[0].value,'source-2')
            self.assertFalse(app.query_one('#refresh-order').display)

    async def test_updated_sort_keeps_working_first_and_terminated_last(self):
        feed=ManualFeed()
        now=time.time_ns()
        base=feed.snapshot.local_status['sessions'][0]
        specs=[('source-1','Working','running',now+3,''),
               ('source-2','Waiting','waiting',now+2,'Approval'),
               ('source-3','Completed','completed',now+4,''),
               ('source-4','Error','error',now+1,'')]
        feed.snapshot.local_status['sessions']=[
            {**base,'source_id':sid,'session_id':'session-'+sid[-1],'service':'codex',
             'runtime':'Codex','task':state,'run_state':run_state,
             'waiting_for':waiting,'last_record_ns':stamp}
            for sid,state,run_state,stamp,waiting in specs
        ]
        feed.snapshot.sessions=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            table=app.query_one('#agent-table')
            self.assertEqual([key.value for key in table.rows],
                             ['source-1','source-2','source-3','source-4'])
            self.assertIn('Updated ↓',str(app.query_one('#agent-sort',Button).label))
            table.move_cursor(row=2)
            app.select_agent('source-3')
            await pilot.pause()
            await pilot.click('#agent-sort')
            await pilot.pause()
            self.assertIn('Updated ↑',str(app.query_one('#agent-sort',Button).label))
            self.assertEqual([key.value for key in table.rows],
                             ['source-1','source-2','source-4','source-3'])
            self.assertEqual(app.selected_source,'source-3')
            self.assertEqual(table.cursor_row,3)
            await pilot.press('o')
            await pilot.pause()
            self.assertIn('Updated ↓',str(app.query_one('#agent-sort',Button).label))
            self.assertEqual([key.value for key in table.rows],
                             ['source-1','source-2','source-3','source-4'])
            self.assertEqual(app.selected_source,'source-3')
            self.assertEqual(table.cursor_row,2)

    async def test_sort_control_is_available_on_agents(self):
        app=ObservatoryApp(ManualFeed(),persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            sort=app.query_one('#agent-sort',Button)
            self.assertTrue(sort.display)
            await pilot.resize_terminal(80,24)
            await pilot.pause()
            self.assertTrue(sort.display)
            self.assertTrue(app.screen.region.contains_region(sort.region))
            await pilot.press('2')
            await pilot.pause()
            self.assertFalse(sort.display)

    async def test_sessions_arriving_after_empty_start_are_not_dropped_by_order_freeze(self):
        feed=ManualFeed()
        initial=feed.snapshot.local_status['sessions']
        feed.snapshot.local_status['sessions']=[]
        feed.snapshot.sessions=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            await pilot.pause()
            table=app.query_one('#agent-table')
            self.assertEqual(table.row_count,0)
            feed.snapshot.local_status['sessions']=initial
            app.last_update=-1
            app.refresh_data()
            await pilot.pause()
            self.assertEqual(table.row_count,2)
            self.assertFalse(app.query_one('#refresh-order').display)

    async def test_default_inspector_exposes_recorded_user_request_and_latest_action(self):
        feed=ManualFeed()
        now=time.time_ns()
        feed.snapshot.activity.append({
            'id':'user-request','source_id':'user-request','service':'codex','session_id':'session-1',
            'time_unix_nano':now,'domain':'message','action':'user',
            'subject':'Implement a retry guard for the payment request',
            'detail':'Implement a retry guard for the payment request and preserve the original response.',
        })
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,48)) as pilot:
            await pilot.pause()
            app.set_agent_filter('all')
            app.select_agent('source-1')
            await pilot.pause()
            detail=app.query_one('#agent-detail').plain_text
            self.assertIn('LATEST RECORDED REQUEST',detail)
            self.assertIn('Implement a retry guard for the payment request',detail)
            self.assertIn('LATEST RECORDED ACTION',detail)
            self.assertIn('Activity · s',app.query_one('#agent-open').label.plain)
            await pilot.press('e')
            await pilot.pause()
            expanded=app.query_one('#agent-detail').plain_text
            self.assertIn('Recorded User Prompts',expanded)
            self.assertIn('preserve the original response',' '.join(expanded.split()))
            self.assertIn('Recent Recorded Activity',expanded)
            self.assertIn('git status --short',expanded)

    async def test_running_agents_keep_all_prompts_activity_and_mcp_calls_visible(self):
        feed=ManualFeed()
        now=time.time_ns()
        sessions=feed.snapshot.local_status['sessions']
        running=[
            ('session-1','source-1','codex','Codex','Build the billing dashboard'),
            ('session-2','source-2','claude','Claude','Review the deployment plan'),
        ]
        for session_id,source_id,service,runtime,task in running:
            entry=next(item for item in sessions if item.get('source_id')==source_id)
            entry.update(session_id=session_id,service=service,runtime=runtime,task=task,
                         run_state='running',waiting_for='',last_record_ns=now)
        third=dict(sessions[1])
        third.update(source_id='source-3',session_id='session-3',service='opencode',runtime='OpenCode',
                     task='Investigate the flaky test',run_state='running',waiting_for='',last_record_ns=now)
        sessions.append(third)
        feed.snapshot.mcp=[]

        for index,(session_id,source_id,service,runtime,task) in enumerate(running+[
                ('session-3','source-3','opencode','OpenCode','Investigate the flaky test')]):
            feed.snapshot.activity.extend([
                {'id':f'{source_id}-prompt-1','source_id':f'{source_id}-prompt-1','session_id':session_id,
                 'service':service,'time_unix_nano':now+index*10+1,'domain':'message','action':'user',
                 'subject':f'{runtime} first prompt · {task}'},
                {'id':f'{source_id}-prompt-2','source_id':f'{source_id}-prompt-2','session_id':session_id,
                 'service':service,'time_unix_nano':now+index*10+2,'domain':'message','action':'user',
                 'subject':f'{runtime} follow-up prompt · keep going'},
                {'id':f'{source_id}-action','source_id':f'{source_id}-action','session_id':session_id,
                 'service':service,'time_unix_nano':now+index*10+3,'domain':'shell','action':'run',
                 'subject':f'Current work for {runtime}', 'result':'running'},
            ])
            feed.snapshot.mcp.append({
                'id':f'{source_id}-mcp','source_id':f'{source_id}-mcp','session_id':session_id,
                'service':service,'time_unix_nano':now+index*10+4,'server':'workspace-tools',
                'tool':f'{runtime.lower()}_lookup','method':'tools/call','status':'OK','duration_ms':12,
                'arguments':{'task':task},'output':{'state':'in progress'},'source':'test',
            })
        feed.snapshot.mcp_summary.update(calls=3,tools=3,errors=0)
        feed.snapshot.mcp_results=list(feed.snapshot.mcp)
        feed.snapshot.updated+=1

        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,48)) as pilot:
            await pilot.pause()
            app.last_update=-1
            app.refresh_data()
            table=app.query_one('#agent-table',DataTable)
            self.assertEqual(table.row_count,3,'Recent must include every currently running session')
            self.assertEqual({app.table_data['agent-table'][key.value]['session_id']
                              for key in table.rows},{'session-1','session-2','session-3'})

            app.action_expand()
            await pilot.pause()
            for session_id,source_id,service,runtime,task in running+[
                    ('session-3','source-3','opencode','OpenCode','Investigate the flaky test')]:
                app.select_agent(source_id)
                text=app.query_one('#agent-detail').plain_text
                self.assertIn(f'{runtime} first prompt · {task}',text)
                self.assertIn(f'{runtime} follow-up prompt · keep going',text)
                self.assertIn(f'Current work for {runtime}',text)

            app.action_page('mcp')
            await pilot.pause()
            mcp=app.query_one('#mcp-table',DataTable)
            self.assertEqual(mcp.row_count,3)
            self.assertEqual({call['session_id'] for call in app.table_data['mcp-table'].values()},
                             {'session-1','session-2','session-3'})
            for _,_,_,runtime,_ in running+[
                    ('session-3','source-3','opencode','OpenCode','Investigate the flaky test')]:
                self.assertTrue(any(call['tool']==f'{runtime.lower()}_lookup'
                                    for call in app.table_data['mcp-table'].values()))

            await pilot.resize_terminal(80,24)
            await pilot.press('1')
            await pilot.pause()
            self.assertEqual(app.query_one('#agent-table').row_count,3)
            await pilot.press('enter')
            await pilot.pause()
            detail=' '.join(app.query_one('#agent-detail').plain_text.split())
            self.assertIn('OpenCode first prompt · Investigate the flaky test',detail)
            self.assertIn('OpenCode follow-up prompt · keep going',detail)
            self.assertIn('Current work for OpenCode',detail)

    async def test_compact_overview_leaves_full_height_agent_and_inspector_frames(self):
        feed=ManualFeed()
        now=time.time_ns()
        sessions=feed.snapshot.local_status['sessions']
        for index in range(12):
            if index < len(sessions):
                entry=sessions[index]
            else:
                entry=dict(sessions[0])
                sessions.append(entry)
            entry.update(source_id=f'source-{index+1}',session_id=f'session-{index+1}',
                         service='codex',runtime='Codex',task=f'Work item {index+1}',
                         run_state='running',waiting_for='',last_record_ns=now+index)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(180,60)) as pilot:
            await pilot.pause()
            table=app.query_one('#agent-table',DataTable)
            results=app.query_one('#results-agents')
            self.assertEqual(table.row_count,12,'The view retains all agents in its scrollable list')
            inspector=app.query_one('#agent-bottom')
            self.assertEqual(results.region.y,10)
            self.assertEqual(results.region.y,inspector.region.y)
            self.assertEqual(results.region.bottom,inspector.region.bottom)
            self.assertEqual(results.region.bottom,app.query_one('#shortcuts').region.y)
            self.assertGreater(results.region.height,34,'No artificial cap or empty space below the list')
            self.assertTrue(app.query_one('#masthead').region.contains_region(
                app.query_one('#brand').region))
            table.focus()
            await pilot.press(*['down']*11)
            await pilot.pause()
            self.assertEqual(table.cursor_row,11)

            # Resizing trades header space against the whole workspace, not
            # just the agent list. All 12 sessions remain in the table.
            await pilot.resize_terminal(180,40)
            await pilot.pause()
            self.assertEqual(results.region.y,8)
            self.assertEqual(results.region.bottom,inspector.region.bottom)
            self.assertEqual(table.row_count,12)

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
