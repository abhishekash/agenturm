//! Terminal presentation of text already delivered under a content grant.

use super::*;
use crate::workbench::{PromptKind, PromptReceiver};

const DISPLAY_LIFETIME: Duration = Duration::from_secs(25);
const TAB_COLUMNS: usize = 8;

pub(crate) struct PromptScreen {
    text: String,
    kind: PromptKind,
    readable_start: usize,
    hidden_context: usize,
    original: bool,
    readable_row: usize,
    original_row: usize,
    column: usize,
    max_row: usize,
    max_column: usize,
    pub(crate) acknowledge: Option<mpsc::Sender<()>>,
    pub(crate) active: Arc<Mutex<bool>>,
    expires: Instant,
}

impl PromptScreen {
    pub(crate) fn new(text: String, receiver: PromptReceiver, now: Instant) -> Self {
        let (readable, hidden_context) = if receiver.kind == PromptKind::User {
            readable_request(&text)
        } else {
            (text.as_str(), 0)
        };
        Self {
            readable_start: text.len() - readable.len(),
            text,
            kind: receiver.kind,
            hidden_context,
            original: false,
            readable_row: 0,
            original_row: 0,
            column: 0,
            max_row: 0,
            max_column: 0,
            acknowledge: Some(receiver.acknowledgement),
            active: receiver.active,
            expires: now + DISPLAY_LIFETIME,
        }
    }

    pub(crate) fn expired(&self, now: Instant) -> bool {
        now >= self.expires
    }

    pub(crate) fn expires_at(&self) -> Instant {
        self.expires
    }

    pub(crate) fn key(&mut self, code: KeyCode) {
        if code == KeyCode::Char('z') && self.kind == PromptKind::User {
            self.original = !self.original;
            return;
        }
        let row = if self.original {
            &mut self.original_row
        } else {
            &mut self.readable_row
        };
        match code {
            KeyCode::Down | KeyCode::Char('j') => *row = row.saturating_add(1).min(self.max_row),
            KeyCode::Up | KeyCode::Char('k') => *row = row.saturating_sub(1),
            KeyCode::Home => {
                *row = 0;
                if self.original {
                    self.column = 0;
                }
            }
            KeyCode::Left if self.original => self.column = self.column.saturating_sub(TAB_COLUMNS),
            KeyCode::Right if self.original => {
                self.column = self.column.saturating_add(TAB_COLUMNS).min(self.max_column);
            }
            _ => {}
        }
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        let footer_lines = if self.kind == PromptKind::User { 2 } else { 1 };
        let [main, foot] = Layout::vertical([Constraint::Min(1), Constraint::Length(footer_lines)])
            .areas(frame.area());
        let title = match (self.kind, self.original) {
            (PromptKind::User, true) if main.width >= 51 => {
                "Granted prompt · captured text · original layout"
            }
            (PromptKind::User, true) => "Granted prompt · original layout",
            (PromptKind::User, false) => "Granted prompt · readable",
            (kind, _) => kind.title(),
        };
        let block = panel(title);
        let interior = block.inner(main);
        let width = usize::from(interior.width);
        let height = usize::from(interior.height);
        if width == 0 || height == 0 {
            frame.render_widget(block, main);
            return;
        }

        let text = if self.original {
            &self.text
        } else if self.readable_start == self.text.len() && self.hidden_context != 0 {
            "This request contains runtime context only. Press z for the captured text."
        } else {
            &self.text[self.readable_start..]
        };
        let lines = text.split('\n').map(display_line);
        let lines = if self.original {
            lines.collect::<Vec<_>>()
        } else {
            lines.flat_map(|line| wrap_line(&line, width)).collect()
        };
        self.max_row = lines.len().saturating_sub(height);
        let row = if self.original {
            &mut self.original_row
        } else {
            &mut self.readable_row
        };
        *row = (*row).min(self.max_row);
        let row = *row;
        let column = if self.original {
            self.max_column = lines
                .iter()
                .map(|line| usize::from(line.as_str().cell_width()))
                .max()
                .unwrap_or_default()
                .saturating_sub(width);
            self.column = self.column.min(self.max_column);
            self.column
        } else {
            0
        };
        let visible = lines.iter().skip(row).take(height).map(|line| {
            Line::from(if self.original {
                slice_line(line, column, width)
            } else {
                line.clone()
            })
        });
        frame.render_widget(
            Paragraph::new(visible.collect::<Vec<_>>()).block(block),
            main,
        );

        let keys = match (self.kind, self.original) {
            (PromptKind::User, true) => "z Readable · ←→ pan · ↑↓ scroll · Esc close",
            (PromptKind::User, false) => "z Original layout · ↑↓ scroll · Esc close",
            _ => "↑↓ scroll · Esc close · closes after 25s",
        };
        let status = if self.kind == PromptKind::User && self.original {
            format!("Column {} · closes after 25s", column + 1)
        } else if self.hidden_context == 1 {
            "1 runtime context block hidden · closes after 25s".into()
        } else if self.hidden_context > 1 {
            format!(
                "{} runtime context blocks hidden · closes after 25s",
                self.hidden_context
            )
        } else {
            "Closes after 25s".into()
        };
        frame.render_widget(
            Paragraph::new(if self.kind == PromptKind::User {
                vec![Line::from(keys), Line::from(status)]
            } else {
                vec![Line::from(keys)]
            }),
            foot,
        );
    }
}

/// Match only complete leading runtime envelopes recognized by the Python reader.
/// Any context in the middle of the user's request remains part of the request.
fn readable_request(text: &str) -> (&str, usize) {
    let mut remaining = text.trim_start();
    let mut blocks = 0;
    while let Some(rest) = remove_context(remaining) {
        remaining = rest.trim_start();
        blocks += 1;
    }
    if blocks == 0 {
        (text, 0)
    } else {
        (remaining, blocks)
    }
}

fn remove_context(text: &str) -> Option<&str> {
    let (candidate, repository_header) =
        if let Some(header) = text.strip_prefix("# AGENTS.md instructions for ") {
            let (path, remainder) = header.split_once('\n')?;
            if path.is_empty() {
                return None;
            }
            (remainder.trim_start(), true)
        } else {
            (text, false)
        };
    let (open, content) = candidate.strip_prefix('<')?.split_once('>')?;
    let tag = open
        .split(|character: char| character.is_ascii_whitespace())
        .next()?;
    if !matches!(
        tag,
        "environment_context"
            | "environment_details"
            | "skills_instructions"
            | "user_instructions"
            | "developer_instructions"
            | "system_instructions"
            | "system-reminder"
            | "INSTRUCTIONS"
            | "skill"
            | "local-command-caveat"
            | "local-command-stdout"
            | "local-command-stderr"
    ) || open.trim_ascii_end().ends_with('/')
        || (repository_header && tag != "INSTRUCTIONS")
    {
        return None;
    }
    let closing = format!("</{tag}>");
    let (body, tail) = content.split_once(&closing)?;
    // A same-name opener makes the first close ambiguous. Preserve that text.
    let nested = format!("<{tag}");
    if body.match_indices(&nested).any(|(index, _)| {
        body[index + nested.len()..]
            .starts_with(|character: char| character == '>' || character.is_ascii_whitespace())
    }) {
        return None;
    }
    Some(tail)
}

fn display_line(line: &str) -> String {
    let mut expanded = String::new();
    let mut column = 0;
    for (index, segment) in line.split('\t').enumerate() {
        if index != 0 {
            let spaces = TAB_COLUMNS - column % TAB_COLUMNS;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        }
        let segment = visible_text(segment);
        column += usize::from(segment.as_str().cell_width());
        expanded.push_str(&segment);
    }
    expanded
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let span = Span::raw(line);
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut columns = 0;
    let mut word_break = None;
    for item in span.styled_graphemes(Style::default()) {
        let cells = usize::from(item.symbol.cell_width());
        if columns + cells > width && !current.is_empty() {
            if let Some((byte, previous_columns)) = word_break.take() {
                let tail = current.split_off(byte);
                rows.push(std::mem::replace(&mut current, tail));
                columns -= previous_columns;
            }
            if columns + cells > width && !current.is_empty() {
                rows.push(std::mem::take(&mut current));
                columns = 0;
            }
        }
        current.push_str(item.symbol);
        columns += cells;
        if item.symbol.chars().all(char::is_whitespace) {
            word_break = Some((current.len(), columns));
        }
    }
    rows.push(current);
    rows
}

fn slice_line(line: &str, column: usize, width: usize) -> String {
    let span = Span::raw(line);
    let mut visible = String::new();
    let mut cursor = 0;
    let end = column + width;
    for item in span.styled_graphemes(Style::default()) {
        let next = cursor + usize::from(item.symbol.cell_width());
        if next > column && cursor < end {
            if cursor < column || next > end {
                let cells = next.min(end) - cursor.max(column);
                visible.extend(std::iter::repeat_n(' ', cells));
            } else {
                visible.push_str(item.symbol);
            }
        }
        if next >= end {
            break;
        }
        cursor = next;
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::prompt_channel;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn readable_omits_only_complete_leading_context_envelopes() {
        let capture = " \n<environment_context>fixture data</environment_context>\n\
            # AGENTS.md instructions for fixture/repo\n<INSTRUCTIONS>fixture rules</INSTRUCTIONS>\n\
            Write this request.\n<environment_context>literal example</environment_context>";
        let (body, hidden) = readable_request(capture);
        assert_eq!(hidden, 2);
        assert_eq!(
            body,
            "Write this request.\n<environment_context>literal example</environment_context>"
        );
        for text in [
            "<environment_context>truncated",
            "< environment_context>not a tag</environment_context>",
            "<environment_context / >self closing</environment_context>",
            "<environment_context-extra>similar tag</environment_context>",
            "<environment_context>ambiguous <environment_context>inner</environment_context>",
            "<project>arbitrary XML</project>",
            "# AGENTS.md instructions for fixture/repo\n<skill>separate tag</skill>",
            "Example:\n<system-reminder>inline</system-reminder>",
            "```xml\n<skill>in a fence</skill>\n```",
        ] {
            assert_eq!(readable_request(text), (text, 0));
        }
    }

    #[test]
    fn original_layout_preserves_tab_stops_and_clips_graphemes_at_cell_edges() {
        assert_eq!(display_line("界\tA\tB"), "界      A       B");
        assert_eq!(display_line("safe\x1b[31m"), r"safe\u{1b}[31m");
        assert_eq!(slice_line("界e\u{301}終", 1, 3), " e\u{301} ");
        assert_eq!(slice_line("界e\u{301}終", 2, 3), "e\u{301}終");
        assert_eq!(wrap_line("word another", 10), ["word ", "another"]);
    }

    fn draw(terminal: &mut Terminal<TestBackend>, prompt: &mut PromptScreen) -> String {
        let buffer = terminal.draw(|frame| prompt.draw(frame)).unwrap().buffer;
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn prompt_modes_render_and_scroll_at_wide_and_compact_sizes() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let (_delivery, receiver) = prompt_channel(PromptKind::User);
            let start = Instant::now();
            let text = format!(
                "<environment_context>FIXTURE CONTEXT</environment_context>\n\
                 Request\t{}TAIL_MARKER\n\n{}\nLAST_APPROVED_LINE",
                "界".repeat(70),
                (0..38)
                    .map(|i| format!("List item {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            let mut prompt = PromptScreen::new(text, receiver, start);
            let readable = draw(&mut terminal, &mut prompt);
            assert!(readable.contains("Granted prompt · readable"));
            assert!(readable.contains("1 runtime context block hidden"));
            assert!(readable.contains("TAIL_MARKER"));
            assert!(!readable.contains("FIXTURE CONTEXT"));

            prompt.key(KeyCode::Down);
            let moved_readable = draw(&mut terminal, &mut prompt);
            prompt.key(KeyCode::Char('z'));
            let original = draw(&mut terminal, &mut prompt);
            assert!(original.contains("captured text · original layout"));
            assert!(original.contains("FIXTURE CONTEXT"));
            assert!(!original.contains("TAIL_MARKER"));
            assert!(original.contains("Column 1"));
            for _ in 0..32 {
                prompt.key(KeyCode::Right);
            }
            assert!(draw(&mut terminal, &mut prompt).contains("TAIL_MARKER"));

            // The next layout clamps the existing horizontal position.
            terminal.backend_mut().resize(200, height);
            assert!(draw(&mut terminal, &mut prompt).contains("Column 1"));
            terminal.backend_mut().resize(width, height);
            assert!(draw(&mut terminal, &mut prompt).contains("FIXTURE CONTEXT"));
            for _ in 0..48 {
                prompt.key(KeyCode::Down);
            }
            assert!(draw(&mut terminal, &mut prompt).contains("LAST_APPROVED_LINE"));
            prompt.key(KeyCode::Home);
            assert_eq!(draw(&mut terminal, &mut prompt), original);
            prompt.key(KeyCode::Char('z'));
            assert_eq!(draw(&mut terminal, &mut prompt), moved_readable);
            assert!(!prompt.expired(start + Duration::from_millis(24_999)));
            assert!(prompt.expired(start + Duration::from_secs(25)));
        }
    }

    #[test]
    fn other_private_text_keeps_its_single_presentation() {
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        let (_delivery, receiver) = prompt_channel(PromptKind::Mcp);
        let mut prompt = PromptScreen::new(
            "<environment_context>shown literally</environment_context>".into(),
            receiver,
            Instant::now(),
        );
        let before = draw(&mut terminal, &mut prompt);
        assert!(before.contains("Granted MCP payload"));
        assert!(before.contains("shown literally"));
        assert!(!before.contains("Original layout"));
        prompt.key(KeyCode::Char('z'));
        assert_eq!(draw(&mut terminal, &mut prompt), before);
    }
}
