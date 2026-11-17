"""An attention count must lead to the owning agent, reason and next action."""
import time
import unittest
from dataclasses import replace

from textual.widgets import DataTable, Input, Select, Static, TabbedContent
from observatory.terminal_ui import ObservatoryApp, agent_status
from tests.test_agent_hierarchy_ui import HierarchyFeed, agent_node


def attention_feed():
    feed=HierarchyFeed()
    now=time.time_ns()
    rows=[agent_node('root',None,'Working parent','running',stamp=now),
          agent_node('approval','root','Approve the deployment','waiting',waiting='Approval',stamp=now),
          agent_node('reply',None,'Choose retry policy','waiting',waiting='User input',stamp=now),
          agent_node('failure',None,'Repair checkout','failed',stamp=now),
          agent_node('uncertain',None,'Waiting reason absent','waiting',stamp=now),
          agent_node('done',None,'Completed task','completed',stamp=now)]
    feed.snapshot=replace(feed.snapshot,local_status={'sessions':rows},activity=[])
    return feed


class AttentionLabels(unittest.TestCase):
    def test_reason_is_visible_without_opening_details(self):
        for item,label in (({'state':'Waiting','waiting_for':'Approval'},'Needs approval'),
                ({'state':'Waiting','waiting_for':'User input'},'Needs reply'),
                ({'state':'Waiting'},'Check status'),
                ({'state':'Failed'},'Run failed'),
                ({'state':'Possibly stalled'},'Check progress')):
            with self.subTest(label=label):
                self.assertEqual(agent_status(item).plain,label)


class AttentionNavigation(unittest.IsolatedAsyncioTestCase):
    async def test_header_click_finds_hidden_child_clears_filter_and_shows_reason(self):
        for size in ((120,36),(80,24),(60,18)):
            with self.subTest(size=size):
                feed=attention_feed();app=ObservatoryApp(feed,persist_preferences=False)
                async with app.run_test(size=size) as pilot:
                    await pilot.pause();app.refresh_timer.stop()
                    app.set_agent_filter('active')
                    app.query_one('#search-agents',Input).value='Working parent'
                    app.collapsed_agents.add(app.source_map['s-root']['agent_id'])
                    app.set_session_scope('s-root')
                    await pilot.pause()
                    if size[1]>24:
                        await pilot.click('#stat-attention')
                    else:
                        # The compact header hides summary counters; the same
                        # action remains reachable through ! and the picker.
                        app.query_one('#agent-table',DataTable).focus()
                        await pilot.press('!')
                    await pilot.pause()
                    self.assertEqual(app.agent_filter,'attention')
                    self.assertEqual(app.scope_value,'all')
                    self.assertEqual(app.search_queries['agents'],'')
                    self.assertEqual(app.query_one('#agent-filter',Select).value,'attention')
                    self.assertEqual(set(app.visible_agent_rows),
                        {'s-root','s-approval','s-reply','s-failure','s-uncertain'})
                    self.assertNotIn('s-done',app.visible_agent_rows)
                    self.assertEqual(app.selected_source,'s-approval')
                    self.assertEqual(app.summary_values['attention'][0],4)
                    if size[1]>24:
                        self.assertIn('4 needs attention',app.query_one('#stat-attention',Static).render().plain)
                    table=app.query_one('#agent-table',DataTable)
                    self.assertEqual(table.get_row('s-approval')[1].plain,'Needs approval')
                    self.assertEqual(table.get_row('s-failure')[1].plain,'Run failed')
                    self.assertTrue(table.has_focus)
                    await pilot.press('enter');await pilot.pause()
                    text=' '.join(app.query_one('#agent-detail').plain_text.split())
                    self.assertIn('why attention',text.casefold())
                    self.assertIn('next step',text.casefold())
                    self.assertIn('Codex terminal',text)
                    self.assertIn('w',text)

    async def test_shortcut_from_other_view_and_status_checks_do_not_invent_a_request(self):
        app=ObservatoryApp(attention_feed(),persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            await pilot.press('2','!');await pilot.pause()
            self.assertEqual(app.query_one('#pages',TabbedContent).active,'agents')
            self.assertEqual(app.agent_filter,'attention')
            app._focus_source_row('s-uncertain')
            await pilot.press('enter');await pilot.pause()
            text=' '.join(app.query_one('#agent-detail').plain_text.split())
            self.assertIn('no approval or reply request',text.casefold())
            self.assertIn('d',text)
            self.assertEqual(app.query_one('#agent-context',Static).render().plain.split(' · ')[1],'Check status')

    async def test_shortcut_is_literal_while_typing_and_empty_queue_is_explicit(self):
        feed=attention_feed();app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(80,24)) as pilot:
            await pilot.pause();app.refresh_timer.stop()
            await pilot.press('/','!');await pilot.pause()
            self.assertEqual(app.query_one('#search-agents',Input).value,'!')
            self.assertEqual(app.agent_filter,'today')
            feed.snapshot=replace(feed.snapshot,updated=time.time(),
                local_status={'sessions':[feed.snapshot.local_status['sessions'][-1]]})
            app.query_one('#agent-table',DataTable).focus()
            await pilot.press('!');await pilot.pause()
            self.assertEqual(app.visible_agent_rows,[])
            self.assertIn('No attention items',app.query_one('#search-status-agents',Static).render().plain)
            self.assertNotIn('Start an agent',repr(app.inspector_payloads.get('agent-detail')))
