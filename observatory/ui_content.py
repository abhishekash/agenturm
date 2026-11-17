"""Shared reading layout. Presentation only: raw evidence remains the copy source."""
from __future__ import annotations

import re

from rich.console import Group
from rich.cells import cell_len
from rich.markdown import CodeBlock, Heading, Markdown, TableElement
from rich.padding import Padding
from rich.syntax import Syntax
from rich.table import Table
from rich.text import Text
from rich.theme import Theme
from textual.widgets import RichLog

from .themes import PALETTES, themed_style


class ReadingHeading(Heading):
    LEVEL_ALIGN = {f'h{level}': 'left' for level in range(1, 7)}


class ReadingCode(CodeBlock):
    def __rich_console__(self, console, options):
        # No nested box or extra left padding; source indentation owns the code.
        yield Syntax(self.text.plain.rstrip('\n'), self.lexer_name,
                     theme=self.theme, word_wrap=True, background_color='default')


class ReadingTable(TableElement):
    def __rich_console__(self, console, options):
        if options.max_width >= 80:
            yield from super().__rich_console__(console, options)
            return
        headers = self.header.row.cells if self.header and self.header.row else []
        for index, row in enumerate(self.body.rows if self.body else []):
            if index:
                yield Text('')
            for column, entry in enumerate(row.cells):
                label = headers[column].content.copy() if column < len(headers) else Text(f'Column {column + 1}')
                label.stylize('bold')
                yield label
                yield Padding(entry.content, (0, 0, 0, 2), expand=False)


class ReadingMarkdown(Markdown):
    elements = {**Markdown.elements, 'heading_open': ReadingHeading,
                'table_open': ReadingTable, 'fence': ReadingCode, 'code_block': ReadingCode}

    def __init__(self, text, theme='Current'):
        self.colors = PALETTES.get(theme, PALETTES['Current'])
        super().__init__(text, code_theme='friendly' if theme == 'Bright' else 'monokai')

    def __rich_console__(self, console, options):
        colors = self.colors
        styles = {f'markdown.h{level}': f'bold {colors["label"]}' for level in range(1, 7)}
        styles.update({'markdown.link_url': f'underline {colors["secondary"]}',
                       'markdown.code': colors['secondary'], 'markdown.table.border': colors['border'],
                       'markdown.table.header': f'bold {colors["label"]}'})
        with console.use_theme(Theme(styles)):
            yield from super().__rich_console__(console, options)


_MARKDOWN = re.compile(r'(?m)^ {0,3}(?:#{1,6} |[-*+] |\d+[.)] |```|~~~|> |\|)|\*\*\S|~~\S|(?<!\w)[*_]\S[^\n]*?[*_](?!\w)|\[[^\]\n]+\]\([^\n]+\)|`[^`\n]+`')


def prose(value, theme='Current'):
    """Interpret only recognizable narrative markup, never arbitrary log output."""
    text = str(value)
    if text.startswith(('diff --git ', '--- a/')):
        return Syntax(text, 'diff', word_wrap=True,
                      theme='friendly' if theme == 'Bright' else 'monokai',
                      background_color=PALETTES.get(theme, PALETTES['Current'])['surface'])
    # Keep large/HTML-bearing captures literal rather than dropping HTML tokens
    # or making Markdown parsing an unbounded cost on routine selection.
    if len(text) <= 131072 and _MARKDOWN.search(text) and not re.search(r'</?[A-Za-z][^>]*>', text):
        return ReadingMarkdown(text, theme)
    return Text(text)


def write_body(log, content):
    log.write(Padding(content, (0, 0, 0, 2), expand=False))


def write_section(log, label, content, *, first=False):
    if not first:
        log.write(Text(''))
    style = themed_style(log, 'bold #c5adff') if getattr(log, 'is_mounted', False) else 'bold #c5adff'
    log.write(Text(label, style=style))
    write_body(log, content)


def metadata_table(fields, width, style='dim'):
    table = Table.grid(padding=(0, 2), expand=True)
    table.add_column(style=style, width=min(max((cell_len(str(label)) for label, _ in fields), default=0),
                                          max(8, width // 3)), overflow='fold')
    table.add_column(ratio=1, overflow='fold')
    for label, value in fields:
        table.add_row(Text(str(label)), value if isinstance(value, Text) else
                      Text(str('Not reported' if value is None or value == '' else value)))
    return table


def report_document(text, headings, *, narrative=(), theme='Current'):
    """Inset known generated report sections; leave captured body text literal."""
    blocks = []
    body = []
    inset = False
    heading = None
    def flush():
        if body:
            value = '\n'.join(body).strip('\n')
            content = prose(value, theme) if heading in narrative else Text(value)
            blocks.append(Padding(content, (0, 0, 0, 2), expand=False) if inset else content)
            body.clear()
    for line in text.splitlines():
        if any(line == heading or line.startswith(heading + ' · ') for heading in headings):
            flush()
            if blocks:
                blocks.append(Text(''))
            blocks.append(Text(line, style='bold'))
            inset = True
            heading = line
        else:
            body.append(line)
    flush()
    return Group(*blocks)


class ContentLog(RichLog):
    """Modal reading surfaces reflow retained renderables at their actual width."""
    DEFAULT_CSS = 'ContentLog { overflow-x: hidden; }'

    def __init__(self, *args, **kwargs):
        self._content_blocks = []
        self._content_width = None
        self._reflow_pending = False
        super().__init__(*args, **kwargs)

    def write(self, content, *args, **kwargs):
        # These finite modal documents replace content via clear(), including
        # policy revocation. Never retain an old document outside that lifecycle.
        self._content_blocks.append(content)
        width = self.scrollable_content_region.width
        if not self._size_known or not width:
            # Own the deferred document, not RichLog's DeferredRender queue:
            # its resize handler calls self.write() again, duplicating retained
            # blocks and replaying the obsolete width.
            self.schedule_reflow()
            return self
        if args:
            args = (width, *args[1:])
            kwargs.pop('width', None)
        else:
            kwargs['width'] = width
        self._content_width = width
        return super().write(content, *args, **kwargs)

    def clear(self):
        self._content_blocks.clear()
        self._content_width = None
        if self.is_mounted and self in self.screen.selections:
            self.screen.clear_selection()
        return super().clear()

    def on_resize(self):
        self.reflow_when_ready()

    def on_show(self):
        self.reflow_when_ready()

    def reflow_when_ready(self):
        # These are the log's own layout events, not the app's earlier resize.
        # Rebuild before its next paint when geometry is already valid. A
        # deferred-only reveal can otherwise leave a blank first frame.
        if self._size_known and self.scrollable_content_region.width:
            self.reflow_content()
        else:
            self.schedule_reflow()

    def schedule_reflow(self):
        if not self._reflow_pending:
            self._reflow_pending = True
            self.call_after_refresh(self.reflow_content)

    def reflow_content(self):
        self._reflow_pending = False
        width = self.scrollable_content_region.width
        if not width or width == self._content_width or not self._content_blocks:
            return
        position = self.scroll_offset
        blocks = list(self._content_blocks)
        self.clear()
        for block in blocks:
            self.write(block)
        self.call_after_refresh(self.scroll_to, x=0, y=position.y, animate=False)
