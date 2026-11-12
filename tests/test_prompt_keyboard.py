"""Choose every captured request and read it without mouse or direct actions."""
import unittest

from textual.widgets import OptionList, Static

from observatory.terminal_ui import ObservatoryApp
from tests.test_inspector_layout import ManualFeed, prompt


class PromptKeyboardTests(unittest.IsolatedAsyncioTestCase):
    def app(self):
        feed=ManualFeed()
        feed.api.activity.extend([dict(prompt(i),detail=f'Request body {i}\n'+('Line of recorded instructions.\n'*90))
                                  for i in range(1,9)])
        feed.refresh()
        return ObservatoryApp(feed,persist_preferences=False)

    async def open_prompts(self,app,pilot,*,enter_section=True):
        await pilot.pause();app.refresh_timer.stop();app.set_agent_filter('all')
        app.select_agent('source-1');app.query_one('#agent-table').focus()
        await pilot.press('enter','right')
        if enter_section:await pilot.press('enter')
        await app.workers.wait_for_complete();await pilot.pause()

    async def test_wide_arrows_choose_every_prompt_enter_reads_left_returns(self):
        app=self.app()
        async with app.run_test(size=(120,32)) as pilot:
            await self.open_prompts(app,pilot,enter_section=False)
            self.assertEqual(app.focused.id,'agent-modes')
            keys=list(app.session_prompts.events)
            keys.sort(key=lambda key:app.session_prompts.events[key]['time_unix_nano'])
            self.assertGreaterEqual(len(keys),8)
            self.assertEqual(app.session_prompts.selected,keys[-1])
            await pilot.press('up');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-prompts')
            self.assertEqual(app.session_prompts.selected,keys[-2])
            await pilot.press('end');await pilot.pause()
            self.assertIn('↑↓ Choose',app.query_one('#shortcuts',Static).render().plain)
            await pilot.press('home');await pilot.pause()
            self.assertEqual(app.session_prompts.selected,keys[0])
            for expected in keys[1:]:
                await pilot.press('down');await pilot.pause()
                self.assertEqual(app.session_prompts.selected,expected)
            await pilot.press('enter','pagedown');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-detail')
            reader=app.query_one('#agent-detail')
            self.assertGreater(reader.scroll_y,0)
            before=reader.scroll_y
            await pilot.press('up');await pilot.pause()
            self.assertLess(reader.scroll_y,before)
            self.assertEqual(app.session_prompts.selected,keys[-1])
            self.assertIn('← Prompts',app.query_one('#shortcuts',Static).render().plain)
            await pilot.press('left','up');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-prompts')
            self.assertEqual(app.session_prompts.selected,keys[-2])
            await pilot.press('right','[',']');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-detail')
            self.assertEqual(app.session_prompts.selected,keys[-2])
            await pilot.press('escape');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-table')

    async def test_compact_native_chooser_and_resize_keep_navigation_focus(self):
        app=self.app()
        async with app.run_test(size=(120,32)) as pilot:
            await self.open_prompts(app,pilot)
            latest=app.session_prompts.selected
            for size,target in (((80,24),'agent-prompt-select'),((120,32),'agent-prompts'),
                                ((60,18),'agent-prompt-select')):
                await pilot.resize_terminal(*size);await pilot.pause()
                self.assertEqual(app.focused.id,target)
                self.assertEqual(app.session_prompts.selected,latest)
                table=app.query_one('#agent-prompts')
                self.assertTrue(all(isinstance(row.key.value,str) for row in table.ordered_rows))
            await pilot.press('enter','enter');await pilot.pause()
            self.assertEqual(app.session_prompts.selected,latest)
            self.assertEqual(app.focused.id,'agent-detail')
            await pilot.press('left');await pilot.pause()
            await pilot.press('up');await pilot.pause()
            self.assertTrue(app.query_one('#agent-prompt-select').expanded)
            overlay=app.query_one('#agent-prompt-select').query_one(OptionList)
            self.assertEqual(overlay.virtual_size.height,overlay.option_count,
                             'Each request needs one readable row in the compact chooser')
            self.assertIn('↑↓ Choose',app.query_one('#shortcuts',Static).render().plain)
            self.assertNotIn('Scroll',app.query_one('#shortcuts',Static).render().plain)
            await pilot.press('up','enter');await pilot.pause()
            self.assertNotEqual(app.session_prompts.selected,latest)
            self.assertEqual(app.focused.id,'agent-detail')
            await pilot.press('left','space','end','enter');await pilot.pause()
            self.assertEqual(app.session_prompts.selected,latest)
            await pilot.press('left','right');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-detail')
            self.assertFalse(app.query_one('#agent-prompt-select').expanded)
            await pilot.resize_terminal(120,32);await pilot.pause()
            self.assertEqual(app.focused.id,'agent-detail')

    async def test_section_shortcuts_land_on_prompt_navigation(self):
        app=self.app()
        async with app.run_test(size=(80,24)) as pilot:
            await self.open_prompts(app,pilot)
            self.assertEqual(app.focused.id,'agent-prompt-select')
            await pilot.press('right','ctrl+right');await pilot.pause()
            self.assertEqual(app.session_mode,'details')
            self.assertEqual(app.focused.id,'agent-detail')
            await pilot.press('ctrl+left');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-prompt-select')
            await pilot.press('right','m','m','m');await pilot.pause()
            self.assertEqual(app.session_mode,'prompts')
            self.assertEqual(app.focused.id,'agent-prompt-select')
            await pilot.press('space','escape');await pilot.pause()
            self.assertTrue(app.inspector_open())
            self.assertFalse(app.query_one('#agent-prompt-select').expanded)
            await pilot.resize_terminal(120,32);await pilot.pause()
            self.assertEqual(app.focused.id,'agent-prompts')
            await pilot.press('f1');await pilot.pause()
            await pilot.resize_terminal(60,18);await pilot.pause()
            await pilot.press('escape');await pilot.pause()
            self.assertEqual(app.focused.id,'agent-prompt-select')

    async def test_lost_prompt_focus_returns_to_navigation_across_queued_reflows(self):
        app=self.app()
        async with app.run_test(size=(80,24)) as pilot:
            await self.open_prompts(app,pilot)
            # A hidden navigator can leave focus invalid during a resize. More
            # than one reflow may run before Textual applies deferred focus.
            app.screen.set_focus(None)
            for _ in range(3):app.reflow_inspectors()
            await pilot.pause()
            self.assertEqual(app.focused.id,'agent-prompt-select')
            await pilot.press('right')
            await pilot.resize_terminal(120,32);await pilot.pause()
            self.assertEqual(app.focused.id,'agent-detail')
