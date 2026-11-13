"""Readable agent identities, hierarchy and status without changing incidents."""
import time
import unittest

from observatory.agent_hierarchy import tree_prefix
from observatory.terminal_ui import ObservatoryApp, sources, task_preview
from observatory.ui_workflow import workflow_reasons
from tests.test_agent_hierarchy_ui import HierarchyFeed, agent_node


class AgentsReadabilityTests(unittest.IsolatedAsyncioTestCase):
    def test_old_truncated_scaffolding_falls_back_to_a_real_recorded_request(self):
        feed=HierarchyFeed()
        root=feed.snapshot.local_status['sessions'][0]
        poisoned=('<local-command-caveat>The command below was run directly in Claude Code, '
                  'not sent to you as a request. '+('Boilerplate. '*100))[:500]
        root.update(initial_task=poisoned,task=poisoned)
        feed.snapshot.activity.append(dict(id='real',source_id='real',agent_id=root['agent_id'],
            session_id=root['session_id'],service=root['service'],domain='message',action='user',
            time_unix_nano=1,detail='Fix checkout retries.'))
        item=next(row for row in sources(feed.snapshot) if row['id']==root['source_id'])
        self.assertEqual(task_preview(item),'Fix checkout retries.')
        feed.snapshot.activity=[]
        item=next(row for row in sources(feed.snapshot) if row['id']==root['source_id'])
        self.assertEqual(task_preview(item),'')

    def test_missing_status_is_not_a_workflow_incident_or_input_request(self):
        feed=HierarchyFeed()
        for state in ('Unknown','Unconfirmed','Monitor delayed','Catching up','Spawn recorded','Connected'):
            item=dict(agent_node('unknown',None,'Inspect checkout'),state=state,active=False)
            self.assertEqual(workflow_reasons(item,{},feed.snapshot,{}),[],state)

    def test_deeper_guides_have_matching_ascii_width_and_continuations(self):
        for depth in range(9):
            for last in (True,False):
                prefix=tree_prefix(depth,last=last,ancestor_last=[False]*9)
                ascii_prefix=tree_prefix(depth,last=last,ancestor_last=[False]*9,ascii_only=True)
                self.assertEqual(len(prefix),depth*4)
                self.assertEqual(len(ascii_prefix),len(prefix))
                guide=tree_prefix(depth,last=last,ancestor_last=[False]*9,continuation=True)
                self.assertEqual(len(guide),len(prefix))
                self.assertNotIn('├',guide)
                self.assertNotIn('└',guide)

    async def test_scan_labels_tree_alignment_refresh_and_narrow_keyboard_navigation(self):
        feed=HierarchyFeed();now=time.time_ns()
        missing=agent_node('missing',None,'Inspect missing lifecycle',state='unknown',stamp=now)
        feed.snapshot.local_status['sessions'].append(missing)
        root=feed.snapshot.local_status['sessions'][0]
        feed.snapshot.local_status['sessions'].append(dict(root,source_id='z-root-copy'))
        app=ObservatoryApp(feed,persist_preferences=False)
        async with app.run_test(size=(160,42)) as pilot:
            await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
            table=app.query_one('#agent-table')
            self.assertTrue(table.zebra_stripes)
            self.assertEqual(table.row_count,6)
            self.assertIn('3 working agents',app.query_one('#stat-sessions').render().plain)
            root=table.get_row('s-root')[0].plain.splitlines()
            child=table.get_row('s-c2')[0].plain.splitlines()
            self.assertEqual(child[0].index('Review payment flow')-root[0].index('Fix checkout regression'),4)
            self.assertEqual(child[1].index('Codex'),child[0].index('Review payment flow'))
            self.assertNotIn('parent not reported','\n'.join(str(c) for row in table.rows for c in table.get_row(row)))
            self.assertNotIn('Agent: ',child[1])
            self.assertEqual(table.get_row('s-root')[1].plain,'In progress')
            self.assertEqual(table.get_row('s-c1')[1].plain,'Needs attention')
            self.assertEqual(table.get_row('s-c3')[1].plain,'Ready for review')
            self.assertEqual(table.get_row('s-missing')[1].plain,'Status unknown')
            self.assertIn('1 needs attention',app.query_one('#stat-attention').render().plain)
            self.assertIn('status check',app.query_one('#stat-attention').tooltip)
            app.select_agent('s-missing');await pilot.press('e');await pilot.pause()
            self.assertIn('Task outcome was not reported',app.query_one('#agent-detail').plain_text)
            self.assertFalse(app.query_one('#agent-reviewed').display)
            await pilot.press('escape');app.select_agent('s-root');table.focus()
            app.action_tree_left();await pilot.pause()
            self.assertIn('2 active',table.get_row('s-root')[0].plain)
            self.assertIn('1 attention',table.get_row('s-root')[0].plain)
            await pilot.press('right');await pilot.pause()
            self.assertIn('s-c1',app.visible_agent_rows)
            for size in ((120,36),(80,24),(60,18),(60,20),(160,42)):
                await pilot.resize_terminal(*size);await pilot.pause()
                self.assertEqual(app.selected_source,'s-root')
                self.assertTrue(app.screen.region.contains_region(table.region),size)
                self.assertGreater(table.size.height,3,size)
                self.assertEqual(table.ordered_rows[0].height,3 if size[1]>=24 else 2)
                await pilot.press('down');await pilot.pause()
                self.assertNotEqual(app.selected_source,'s-root')
                await pilot.press('up');await pilot.pause()
                self.assertEqual(app.selected_source,'s-root')
            # Dynamic uncertainty can resolve without rebuilding relationships.
            missing['run_state']='completed';feed.snapshot.updated+=1
            app.refresh_data();await pilot.pause()
            self.assertEqual(table.get_row('s-missing')[1].plain,'Ready for review')
            self.assertIn('1 needs attention',app.query_one('#stat-attention').render().plain)


if __name__=='__main__':unittest.main()
