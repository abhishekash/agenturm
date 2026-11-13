"""The session chooser uses task identity and retains exact evidence scope."""
import unittest

from textual.widgets import Button, Input, OptionList

from observatory.terminal_ui import ObservatoryApp
from observatory.ui_monitoring import SessionScopeScreen
from tests.test_agent_hierarchy_ui import HierarchyFeed


class SessionScopePickerTests(unittest.IsolatedAsyncioTestCase):
    async def test_empty_roster_still_fits_the_minimum_terminal(self):
        feed=HierarchyFeed();feed.snapshot.local_status['sessions']=[]
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            await pilot.click('#scope');await pilot.pause()
            self.assertEqual([entry['id'] for entry in app.screen.filtered],['all'])
            for selector in ('#session-scope-dialog','#session-scope-options','#session-scope-hint'):
                self.assertTrue(app.screen.region.contains_region(app.screen.query_one(selector).region))
            await pilot.press('escape');await pilot.pause()
            self.assertEqual(app.scope_value,'all')

    async def test_button_opens_readable_modal_and_keyboard_applies_exact_session(self):
        feed=HierarchyFeed();app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            await pilot.click('#scope');await pilot.pause()
            self.assertIsInstance(app.screen,SessionScopeScreen)
            choices=app.screen.query_one(OptionList)
            self.assertIs(app.screen.focused,app.screen.query_one(Input))
            self.assertGreater(app.screen.query_one('#session-scope-dialog').size.width,34)
            self.assertIn('Fix checkout regression',choices.get_option('s-root').prompt.plain)
            self.assertIn('checkout-api · Codex',choices.get_option('s-root').prompt.plain)
            await pilot.press('down');await pilot.pause()
            from observatory.themes import PALETTES
            for theme in ('Bright','Observation','Current'):
                app.theme=theme;await pilot.pause()
                self.assertEqual(choices.get_option_at_index(choices.highlighted).id,'s-root')
                self.assertIn(PALETTES[theme]['muted'],str(choices.get_option('s-root').prompt.spans[-1].style))
            self.assertIn('\n',app.screen.query_one('#session-scope-description').content)
            await pilot.press('enter');await pilot.pause()
            self.assertEqual(app.scope_value,'s-root')
            self.assertEqual(feed.session_id,'root')
            self.assertEqual(feed.scope_agent_id,app.source_map['s-root']['agent_id'])
            self.assertIn('Fix checkout regression',str(app.query_one('#scope',Button).label))
            self.assertEqual(app.focused.id,'scope')
            await pilot.click('#scope');await pilot.resize_terminal(60,18)
            await pilot.press('escape');await pilot.pause()
            self.assertTrue(app.screen.has_class('single-pane'))
            self.assertTrue(app.screen.region.contains_region(app.query_one('#scope').region))

    async def test_command_palette_chooser_returns_to_evidence_keyboard_focus(self):
        app=ObservatoryApp(HierarchyFeed(),persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            await pilot.press('2','ctrl+p');await pilot.pause()
            app.screen.query_one(Input).value='Choose agent session'
            await pilot.pause(.2);await pilot.press('down','enter');await pilot.pause()
            self.assertIsInstance(app.screen,SessionScopeScreen)
            await pilot.press('down','enter');await pilot.pause()
            self.assertEqual(app.scope_value,'s-root')
            self.assertEqual(app.focused.id,'activity-table')

    async def test_search_cancel_empty_and_reset_do_not_confuse_shared_session_workers(self):
        feed=HierarchyFeed()
        for row in feed.snapshot.local_status['sessions']:
            row['session_id']='shared-session'
            row['native_worker_id']=row['source_id']
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            app.select_agent('s-root');app.action_tree_left();await pilot.pause()
            self.assertNotIn('s-c1',app.visible_agent_rows)
            await pilot.click('#scope');await pilot.press('/');await pilot.pause()
            app.screen.query_one(Input).value='Inspect API logs';await pilot.pause()
            self.assertEqual(len(app.screen.filtered),1)
            await pilot.press('enter');await pilot.pause()
            self.assertEqual(app.scope_value,'s-c1')
            self.assertEqual(app.selected_source,'s-c1')
            self.assertIn('s-c1',app.visible_agent_rows)
            self.assertEqual(feed.scope_agent_id,app.source_map['s-c1']['agent_id'])
            self.assertEqual(feed.session_id,'shared-session')
            before=feed.refreshes
            await pilot.click('#scope');await pilot.press('/');await pilot.pause()
            app.screen.query_one(Input).value='no matching agent here';await pilot.pause()
            self.assertIsNone(app.screen.query_one(OptionList).highlighted)
            await pilot.press('enter');await pilot.pause()
            self.assertIsInstance(app.screen,SessionScopeScreen)
            await pilot.press('ctrl+a','r','o','o','t');await pilot.pause()
            self.assertEqual(app.screen.query_one(Input).value,'root')
            await pilot.press('escape');await pilot.pause()
            self.assertEqual(app.scope_value,'s-c1')
            self.assertEqual(feed.refreshes,before)
            await pilot.click('#scope');await pilot.press('/');await pilot.pause()
            app.screen.query_one(Input).value='All agent sessions';await pilot.pause()
            await pilot.press('down','enter');await pilot.pause()
            self.assertEqual(app.scope_value,'all')
            self.assertEqual((feed.session_id,feed.service,feed.scope_agent_id),('','',''))
            self.assertFalse(feed.scope_subtree)

    async def test_resizing_cancel_and_disappearing_choice_keep_scope_truthful(self):
        feed=HierarchyFeed();app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,40)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            app.set_session_scope('s-root')
            await pilot.click('#scope');await pilot.pause()
            for size in ((120,32),(80,24),(60,18),(160,40)):
                await pilot.resize_terminal(*size);await pilot.pause()
                for selector in ('#session-scope-dialog','#session-scope-search',
                                 '#session-scope-options','#session-scope-hint'):
                    widget=app.screen.query_one(selector)
                    self.assertTrue(app.screen.region.contains_region(widget.region),(size,selector))
                self.assertGreaterEqual(app.screen.query_one(OptionList).size.height,4)
            await pilot.press('/');app.screen.query_one(Input).value='Inspect API logs'
            await pilot.pause()
            feed.snapshot.local_status['sessions']=[row for row in feed.snapshot.local_status['sessions']
                                                     if row['source_id']!='s-c1']
            feed.snapshot.updated+=1
            await pilot.press('enter');await pilot.pause()
            self.assertEqual(app.scope_value,'s-root')
            self.assertEqual(feed.scope_agent_id,app.source_map['s-root']['agent_id'])


    async def test_large_roster_has_bounded_rows_searchable_full_text_and_stable_recent_order(self):
        from rich.cells import cell_len
        entries=[dict(id='all',task='All agent sessions',description='Clear the session filter')]
        for index in range(146):
            entries.append(dict(id=f's-{index:03}',task=('Long task 界 é 👩‍💻 '*80)+f' needle-{index}',
                                description=f'project · Codex · Worker {index:08}',
                                detail=f'Full task {index}\nSession full-session-{index}',stamp=index,
                                status='Running',updated='1m ago'))
        app=ObservatoryApp(HierarchyFeed(),persist_preferences=False)
        async with app.run_test(size=(120,36)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            app.push_screen(SessionScopeScreen(entries,'s-003'));await pilot.pause()
            screen=app.screen;choices=screen.query_one(OptionList)
            self.assertEqual([e['id'] for e in screen.filtered[:3]],['all','s-145','s-144'])
            self.assertIn('Current',choices.get_option('s-003').prompt.plain)
            for width,height in ((120,36),(80,24),(60,18)):
                await pilot.resize_terminal(width,height);await pilot.pause()
                for index in range(choices.option_count):
                    lines=choices.get_option_at_index(index).prompt.plain.splitlines()
                    self.assertEqual(len(lines),2)
                    self.assertTrue(all(cell_len(line)<=choices.content_size.width-4 for line in lines))
                self.assertGreaterEqual(choices.size.height,4)
                self.assertEqual(screen.filtered[choices.highlighted]['id'],'s-003')
            screen.query_one(Input).value='needle-145';await pilot.pause()
            self.assertEqual([e['id'] for e in screen.filtered],['s-145'])
            self.assertIs(screen.focused,screen.query_one(Input))
            screen.query_one(Input).value='';await pilot.pause()
            await pilot.press('up');await pilot.pause()
            self.assertEqual(screen.filtered[choices.highlighted]['id'],'s-004')
            self.assertEqual(app.scope_value,'all')
            await pilot.resize_terminal(50,14);await pilot.pause()
            self.assertTrue(screen.query_one('#session-scope-small').display)
            self.assertFalse(choices.display)
            await pilot.press('escape');await pilot.pause()
            self.assertNotIsInstance(app.screen,SessionScopeScreen)

    async def test_untitled_workers_keep_identity_visible_in_a_narrow_picker(self):
        feed=HierarchyFeed()
        for row in feed.snapshot.local_status['sessions']:
            row.update(task='',initial_task='',native_worker_id='worker-'+row['source_id'],
                       project='very-long-project-name-'*20)
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(60,18)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.action_session_scope();await pilot.pause()
            option=app.screen.query_one(OptionList).get_option('s-root').prompt.plain
            self.assertIn('Worker r-s-root',option.splitlines()[0])
            self.assertIn('Worker r-s-root',option.splitlines()[1])
            self.assertNotIn('Untitled',option)
            await pilot.press('escape');await pilot.pause()

    def test_unknown_timestamps_sort_last_with_deterministic_ties(self):
        screen=SessionScopeScreen([dict(id=key,stamp=stamp) for key,stamp in
                                   [('z',0),('b',8),('a',8),('all',0),('x',None)]])
        self.assertEqual([e['id'] for e in screen.entries],['all','a','b','x','z'])


if __name__=='__main__':unittest.main()
