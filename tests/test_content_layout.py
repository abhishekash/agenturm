"""Evidence is a reading surface: hierarchy, whitespace and responsive reflow."""
import json
import time
import unittest
from unittest.mock import patch

from rich.console import Console, Group
from rich.padding import Padding
from rich.text import Text
from textual.widgets import TabbedContent

from observatory.ui_content import ContentLog, ReadingMarkdown, metadata_table, prose, report_document, write_section
from observatory.ui_export import ExportScreen
from observatory.ui_monitoring import UsageScreen
from observatory.terminal_ui import ObservatoryApp
from tests.test_terminal_search import ManualFeed


MESSAGE = '''I’d start with **ClickUp Free Forever** for two people.

Use these options:

- Compare the free plans
  - Keep the workspace limits in mind
- Import one board first

| Workflow | Pick | Free limits |
| --- | --- | --- |
| Spreadsheet boards | **Airtable** | 1,000 records per base |
| Task deadlines | **Asana Personal** | Two people and unlimited tasks |

[ClickUp pricing](https://clickup.com/pricing?workspace=two&view=free)

```python
if ready:
    import_board("日本語")
```
END_OF_MESSAGE'''


def message_feed():
    feed = ManualFeed()
    event = {**feed.api.activity[0], 'source_id': 'formatted-message', 'domain': 'message',
             'action': 'assistant', 'subject': MESSAGE[:400], 'detail': MESSAGE,
             'time_unix_nano': time.time_ns()}
    feed.api.activity = [event]
    feed.refresh()
    return feed, event


class ContentLayoutTests(unittest.IsolatedAsyncioTestCase):
    async def test_modal_documents_reflow_and_clearing_forgets_old_evidence(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        literal = '    SOURCE_INDENT\n**literal evidence, not a heading**\n' + 'readable text ' * 50 + 'TAIL'
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause(); app.refresh_timer.stop()
            app.push_screen(UsageScreen(details=literal)); await pilot.pause()
            log = app.screen.query_one(ContentLog)
            wide_lines = len(log.lines)
            await pilot.resize_terminal(60, 18); await pilot.pause()
            text = '\n'.join(line.text.rstrip() for line in log.lines)
            self.assertIn('    SOURCE_INDENT', text)
            self.assertIn('**literal evidence, not a heading**', text)
            self.assertIn('TAIL', text)
            self.assertGreater(len(log.lines), wide_lines)
            self.assertFalse(log.show_horizontal_scrollbar)
            log.clear(); log.write(Text('Content withheld'))
            await pilot.resize_terminal(120, 40); await pilot.pause()
            self.assertEqual('\n'.join(line.text.rstrip() for line in log.lines), 'Content withheld')
            self.assertEqual(len(log._content_blocks), 1)
            log.display = False; await pilot.pause()
            log.clear(); write_section(log, 'Updated source', Text('    NEW_SOURCE'), first=True)
            await pilot.pause()
            log.display = True; await pilot.pause()
            self.assertEqual('\n'.join(line.text.rstrip() for line in log.lines),
                             'Updated source\n      NEW_SOURCE')
            self.assertNotIn('Content withheld', str(log._content_blocks))
            app.pop_screen(); await pilot.pause()
            raw = json.dumps({'message': MESSAGE}, ensure_ascii=False, indent=2)
            app.push_screen(ExportScreen('activity', {}, {}, content=raw)); await pilot.pause()
            preview = app.screen.query_one(ContentLog)
            self.assertIn('**ClickUp', ''.join(line.text for line in preview.lines))
            await pilot.resize_terminal(80, 24); await pilot.pause()
            self.assertIn('**ClickUp', ''.join(line.text for line in preview.lines))
            self.assertEqual(app.screen.content, raw)

    async def test_message_is_not_a_duplicate_title_and_markdown_is_readable(self):
        feed, event = message_feed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause(); app.refresh_timer.stop()
            app.action_page('activity'); await pilot.pause()
            log = app.query_one('#activity-detail')
            self.assertTrue(log.plain_text.startswith('Agent message\n'), log.plain_text[:300])
            self.assertEqual(log.plain_text.count('I’d start with'), 1)
            self.assertNotIn('**', log.plain_text)
            self.assertNotIn('| ---', log.plain_text)
            self.assertIn('Airtable', log.plain_text)
            self.assertIn('Recorded content\n  I’d start with', log.plain_text)
            self.assertIn('    import_board', log.plain_text)
            links = {segment.style.link for line in log.lines for segment in line
                     if segment.style and segment.style.link}
            self.assertIn('https://clickup.com/pricing?workspace=two&view=free', links)
            with patch.object(app, 'copy_to_clipboard') as copy:
                app.action_copy()
            self.assertIn(MESSAGE, copy.call_args.args[0])
            self.assertEqual(feed.api.activity[0]['detail'], event['detail'])

    async def test_all_inspectors_indent_and_wrap_fields_and_structured_evidence(self):
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        async with app.run_test(size=(160, 48)) as pilot:
            await pilot.pause(); app.refresh_timer.stop()
            for page, name in (('activity', 'activity-detail'), ('mcp', 'mcp-detail'),
                               ('traces', 'span-info'), ('host', 'host-detail'),
                               ('metrics', 'metrics-detail'), ('findings', 'finding-detail'),
                               ('agents', 'agent-detail')):
                app.action_page(page); await pilot.pause()
                value = {'outer': {'items': [{'name': 'first', 'active': False}]}}
                app.inspect(name, 'Selected evidence', [('Owner', 'codex'),
                            ('Context', 'wide context ' * 14 + 'FIELD_END')],
                            [('Arguments', value), ('Literal output', '    keep indentation\nnext line')])
                log = app.query_one('#' + name)
                text = log.plain_text
                self.assertIn('FIELD_END', text, name)
                self.assertIn('Arguments\n  {\n    "outer": {', text, name)
                self.assertIn('Literal output\n      keep indentation\n  next line', text, name)
                self.assertFalse(log.show_horizontal_scrollbar, name)

    async def test_message_reflows_on_expand_resize_and_hidden_tab_reentry(self):
        feed, _ = message_feed()
        app = ObservatoryApp(feed, persist_preferences=False)
        async with app.run_test(size=(180, 48)) as pilot:
            await pilot.pause(); app.refresh_timer.stop()
            app.action_page('activity'); await pilot.pause()
            selected = app.selected_activity
            app.action_expand(); await pilot.pause()
            log = app.query_one('#activity-detail')
            wide_lines = len(log.lines)
            for size in ((120, 40), (80, 24), (60, 18)):
                await pilot.resize_terminal(*size); await pilot.pause()
                self.assertIn('END_OF_MESSAGE', log.plain_text)
                self.assertNotIn('| ---', log.plain_text)
                self.assertFalse(log.show_horizontal_scrollbar)
                self.assertLessEqual(max(line.cell_length for line in log.lines),
                                     log.scrollable_content_region.width)
                self.assertEqual(app.selected_activity, selected)
            self.assertGreater(len(log.lines), wide_lines)
            self.assertIn('Workflow\n    Spreadsheet boards', log.plain_text)
            app.action_page('agents'); await pilot.pause()
            feed.api.activity[0]['detail'] = '**Replacement message**\n\nREPLACEMENT_END'
            feed.refresh(); app.last_update = -1; app.refresh_data(); await pilot.pause()
            app.action_page('activity'); await pilot.pause()
            if not app.query_one('#activity').has_class('expanded'):
                app.action_expand(); await pilot.pause()
            self.assertIn('REPLACEMENT_END', log.plain_text)
            self.assertNotIn('END_OF_MESSAGE', log.plain_text)
            self.assertEqual(app.query_one('#pages', TabbedContent).active, 'activity')


class ContentRenderableTests(unittest.TestCase):
    def render(self, content, width=58):
        console = Console(width=width, color_system=None)
        return '\n'.join(''.join(segment.text for segment in line).rstrip()
                         for line in console.render_lines(content, console.options, pad=False))

    def test_markdown_tables_lists_and_code_keep_their_hierarchy(self):
        for theme in ('Current', 'Observation', 'Bright', 'Dark', 'Solarized'):
            text = self.render(Padding(ReadingMarkdown(MESSAGE, theme), (0, 0, 0, 2)))
            self.assertIn('  Workflow\n    Spreadsheet boards', text)
            self.assertIn('  Pick\n    Airtable', text)
            self.assertIn('  if ready:\n      import_board("日本語")', text)
            self.assertIn('Keep the workspace limits in mind', text)
            self.assertNotIn('**', text)
            self.assertNotIn('```', text)

    def test_literal_source_and_original_whitespace_are_not_markdown(self):
        raw = '<environment_context>**Do not hide this**</environment_context>\n    indentation'
        self.assertIsInstance(prose(raw), Text)
        self.assertEqual(prose(raw).plain, raw)
        self.assertEqual(prose('    indented output\n  more output').plain,
                         '    indented output\n  more output')

    def test_inline_emphasis_and_fenced_json_are_not_fragmented_into_raw_log_sections(self):
        self.assertIsInstance(prose('Read *carefully* and _keep context_.'), ReadingMarkdown)
        app = ObservatoryApp(ManualFeed(), persist_preferences=False)
        blocks = []
        class Capture:
            def write(self, block): blocks.append(block)
        value = 'Captured **configuration**:\n\n```json\n{\n  "enabled": false\n}\n```'
        app.write_evidence(Capture(), 'recorded_content', value)
        rendered = self.render(Group(*blocks))
        self.assertIn('Recorded content\n  Captured configuration:', rendered)
        self.assertIn('    "enabled": false', rendered)
        self.assertNotIn('```', rendered)

    def test_metadata_continuations_stay_in_the_value_column(self):
        text = self.render(metadata_table([('Agent', 'codex'), ('Context', 'keep this ' * 12 + 'END')], 40), 40)
        lines = text.splitlines()
        self.assertIn('END', text)
        self.assertTrue(all(line.startswith(' ' * 9) for line in lines[2:]), text)
        self.assertNotIn('…', text)

    def test_report_sections_and_source_fields_have_hanging_insets(self):
        report = report_document('Summary\n\nNOTES\n    preserved indent\nnext line\n\nWORKSPACE\n/project', ('NOTES', 'WORKSPACE'))
        text = self.render(report)
        self.assertIn('NOTES\n      preserved indent\n  next line', text)
        self.assertIn('WORKSPACE\n  /project', text)
        blocks = []
        class Capture:
            def write(self, block): blocks.append(block)
        write_section(Capture(), 'Exact source', Text('    literal'), first=True)
        self.assertEqual(self.render(Group(*blocks)),
                         'Exact source\n      literal')


if __name__ == '__main__':
    unittest.main()
