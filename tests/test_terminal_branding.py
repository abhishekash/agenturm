"""Header identity, summary, and controls stay visible at supported sizes."""
import unittest

from rich.cells import cell_len

from observatory.branding import NAME, TAGLINE, masthead
from observatory.terminal_ui import ObservatoryApp
from observatory.themes import PALETTES
from textual.widgets import TabbedContent
from tests.test_terminal_search import ManualFeed


class BrandingTest(unittest.IsolatedAsyncioTestCase):
    async def test_name_is_visible_at_wide_and_compact_sizes(self):
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(160, 48)) as pilot:
            for size in ((160, 48), (120, 40), (80, 24)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                brand = app.query_one('#brand')
                rendered = ' '.join(brand.render_line(row).text for row in range(brand.size.height))
                self.assertIn('Agenturm', rendered)
                self.assertNotIn('♜', rendered)
                self.assertEqual(brand.content_size.height, 1)
                self.assertTrue(app.query_one('#masthead').region.contains_region(brand.region))
                self.assertLessEqual(app.query_one('#brand-lockup').region.right,
                                     app.query_one('#connection').region.x)
                self.assertNotIn(TAGLINE, rendered)

    async def test_half_size_overview_and_separately_framed_navigation(self):
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(160, 48)) as pilot:
            for size in ((160, 48), (120, 40), (80, 24)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                masthead = app.query_one('#masthead')
                self.assertEqual(masthead.region.height,max(4,(size[1]//4-2)//2))
                tabs=app.query_one('#pages > Tabs')
                self.assertEqual(tabs.region.height,3)
                self.assertEqual(tabs.region.y,masthead.region.bottom)
                self.assertEqual(tabs.border_title,'Views · 1–7 to switch')
                self.assertEqual(app.query_one('#results-agents').region.y,masthead.region.height+4)
                self.assertEqual(app.query_one('#brand').size.height, 1)
                for name in ('brand','summary','connection','pause'):
                    self.assertTrue(masthead.region.contains_region(app.query_one('#'+name).region))
                self.assertFalse(app.query('#view-guide'),'Switching help belongs in the Views frame only')
            for key,page,table in (('2','activity','activity-table'),('3','mcp','mcp-table'),
                                   ('4','traces','trace-table'),('1','agents','agent-table')):
                await pilot.press(key)
                await pilot.pause()
                self.assertEqual(app.query_one('#pages',TabbedContent).active,page)
                self.assertEqual(app.focused,app.query_one('#'+table))
                self.assertEqual(app.query_one('#results-'+page).region.y,8)
                self.assertTrue(app.query_one('#summary').display)

    async def test_keyboard_hints_are_a_compact_bottom_footer(self):
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(160, 48)) as pilot:
            for size in ((160, 48), (120, 40), (100, 40), (80, 24)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                hints = app.query_one('#shortcuts')
                self.assertTrue(hints.display, f'{size}: hints should be visible')
                self.assertGreaterEqual(hints.region.y, app.query_one('#pages').region.bottom)
                rendered = '\n'.join(hints.render_line(row).text for row in range(hints.size.height))
                for label in ('Move', 'Inspect', 'Help', 'Quit', 'Search'):
                    self.assertIn(label, rendered, f'{size}: missing {label}')
                if size[0] >= 140:
                    for label in ('Expand', 'Palette', 'Diagnostics'):
                        self.assertIn(label, rendered, f'{size}: missing {label}')
                for label in ('views', 'trace', 'copy', 'pause', 'export', 'session activity',
                              'reset', 'reviews', 'newer', 'older', 'latest', 'close'):
                    self.assertNotIn(label, rendered, f'{size}: crowded legend still shows {label}')

    async def test_bottom_footer_is_removed(self):
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause()
            self.assertEqual(len(app.query('Footer')), 0)

    async def test_masthead_is_left_aligned_like_the_observation_deck(self):
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(200, 50)) as pilot:
            for width, height in ((200, 50), (120, 40)):
                await pilot.resize_terminal(width, height)
                await pilot.pause()
                masthead = app.query_one('#masthead')
                lockup = app.query_one('#brand-lockup')
                self.assertLessEqual(lockup.region.x,masthead.region.x+2)
                self.assertEqual(len(app.query('#masthead-left')),0)
                connection = app.query_one('#connection')
                self.assertLess(connection.region.right, masthead.region.right)
                self.assertEqual(app.query_one('#pause').region.right, masthead.region.right - 2)

    async def test_summary_and_controls_fit_without_clipping(self):
        feed = ManualFeed()
        feed.api.endpoint = 'http://127.0.0.1:4318/' + 'long-endpoint/' * 12
        feed.snapshot.totals.update(input_tokens=25_600_000, output_tokens=72_500)
        feed.snapshot.mcp_summary.update(calls=145, errors=28)
        app = ObservatoryApp(feed)
        async with app.run_test(size=(160, 48)) as pilot:
            for size in ((160, 48), (120, 40), (100, 40), (80, 40), (80, 24), (160, 48)):
                await pilot.resize_terminal(*size)
                await pilot.pause()
                summary = app.query_one('#summary')
                stats = [app.query_one('#stat-sessions'),app.query_one('#stat-attention')]
                for stat in stats:
                    self.assertTrue(summary.region.contains_region(stat.region))
                    text = stat.render()
                    lines = text.plain.splitlines()[:stat.content_size.height]
                    for line in lines:
                        self.assertLessEqual(cell_len(line), stat.content_size.width,
                                             f'{size}: {stat.id}: {line}')
                self.assertEqual(len({stat.region.y for stat in stats}), 1)
                for name in ('scope', 'more-views', 'connection'):
                    widget = app.query_one('#' + name)
                    if not widget.display:
                        continue
                    self.assertTrue(app.screen.region.contains_region(widget.region), f'{size}: {name}')
                scope_label = app.query_one('#scope SelectCurrent #label')
                self.assertEqual(scope_label.size.height, 1)
                self.assertEqual(app.query_one('#scope').value,'all')
                self.assertFalse(app.query_one('#history-back').display)
                self.assertIn('127.0.0.1:4318', app.query_one('#connection').tooltip)
                table = app.query_one('#agent-table')
                visible = table.region.intersection(app.query_one('#results-agents').content_region)
                self.assertGreaterEqual(visible.height, 3, f'{size}: session rows must remain visible')

    def test_masthead_is_one_line_even_in_demo_mode(self):
        for compact in (False, True):
            for demo in (False, True):
                label = masthead(compact=compact, demo=demo).plain
                self.assertIn('Agenturm', label)
                self.assertNotIn('\n', label)

    async def test_wordmark_colors_follow_the_theme(self):
        app = ObservatoryApp(ManualFeed())
        async with app.run_test(size=(160, 48)) as pilot:
            for size in ((160, 48), (80, 24)):
                await pilot.resize_terminal(*size)
                for name, colors in PALETTES.items():
                    app.theme = name
                    await pilot.pause()
                    brand = app.query_one('#brand')
                    name_segment = next(segment for segment in brand.render_line(0)
                                        if 'Agenturm' in segment.text)
                    self.assertEqual(name_segment.style.color.get_truecolor().hex, colors['text'])
                    self.assertNotIn(TAGLINE, brand.render_line(0).text)

    def test_palette_has_sufficient_contrast_in_every_theme(self):
        def luminance(color):
            channels = [int(color[index:index + 2], 16) / 255 for index in (1, 3, 5)]
            linear = [value / 12.92 if value <= .04045 else ((value + .055) / 1.055) ** 2.4
                      for value in channels]
            return sum(value * weight for value, weight in zip(linear, (.2126, .7152, .0722)))

        for name, colors in PALETTES.items():
            for role in ('text', 'muted', 'secondary', 'primary'):
                with self.subTest(theme=name, role=role):
                    values = sorted((luminance(colors[role]), luminance(colors['canvas'])))
                    self.assertGreaterEqual((values[1] + .05) / (values[0] + .05), 4.5)

    def test_wordmark_is_bounded_standard_terminal_text(self):
        self.assertEqual(masthead().plain.splitlines(), [f'{NAME}  │  {TAGLINE}'])
        self.assertEqual(masthead(compact=True).plain, NAME)
