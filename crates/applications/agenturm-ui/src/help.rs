//! Persistent, scrollable contextual keyboard reference.
//!
//! The shell opens one overlay from the shared action inventory. Opening help
//! never mutates the underlying view, so drafts, selections and scroll survive
//! `F1` and return to the exact same control.

use crate::{HELP_CLOSE_HINT, heading_style, label_style, panel, surface, theme, visible_text};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelpLine {
    pub(crate) text: String,
    pub(crate) heading: bool,
}

#[derive(Default)]
pub(crate) struct Help {
    open: bool,
    title: String,
    lines: Vec<HelpLine>,
    scroll: usize,
    viewport_height: usize,
    viewport_width: usize,
}

impl Help {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    #[allow(dead_code)]
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    #[allow(dead_code)]
    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }

    #[allow(dead_code)]
    pub(crate) fn lines(&self) -> &[HelpLine] {
        &self.lines
    }

    pub(crate) fn open(
        &mut self,
        title: impl Into<String>,
        sections: Vec<(&'static str, Vec<String>)>,
    ) {
        self.open = true;
        self.title = title.into();
        self.scroll = 0;
        self.lines.clear();
        for (heading, entries) in sections {
            if !self.lines.is_empty() {
                self.lines.push(HelpLine {
                    text: String::new(),
                    heading: false,
                });
            }
            self.lines.push(HelpLine {
                text: heading.to_owned(),
                heading: true,
            });
            self.lines.extend(entries.into_iter().map(|text| HelpLine {
                text,
                heading: false,
            }));
        }
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.scroll = 0;
    }

    /// Returns true when the key was consumed by the help overlay.
    pub(crate) fn key(&mut self, key: KeyEvent) -> bool {
        if !self.open {
            return false;
        }
        let code = crate::keymap::plain(key).unwrap_or(KeyCode::Null);
        match code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?') => self.close(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.page()),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(self.page()),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = self.wrapped_len(),
            _ => {}
        }
        self.clamp();
        true
    }

    fn page(&self) -> usize {
        self.viewport_height.max(1)
    }

    fn chunks(&self, line: &HelpLine) -> Vec<String> {
        theme::wrap_cells(&line.text, self.viewport_width.max(1))
    }

    fn wrapped_len(&self) -> usize {
        self.lines.iter().map(|line| self.chunks(line).len()).sum()
    }

    /// Logical line and byte offset within its escaped display text. Reflow
    /// keeps this anchor instead of treating a wrapped row as a stable identity.
    fn anchor(&self) -> (usize, usize) {
        let mut row = 0;
        for (index, line) in self.lines.iter().enumerate() {
            let mut byte = 0;
            for chunk in self.chunks(line) {
                if row == self.scroll {
                    return (index, byte);
                }
                byte += chunk.len();
                row += 1;
            }
        }
        (0, 0)
    }

    fn restore_anchor(&mut self, anchor: (usize, usize)) {
        let mut row = 0;
        for (index, line) in self.lines.iter().enumerate() {
            let mut byte = 0;
            let chunks = self.chunks(line);
            for (part, chunk) in chunks.iter().enumerate() {
                if index == anchor.0 && (byte + chunk.len() > anchor.1 || part + 1 == chunks.len())
                {
                    self.scroll = row;
                    return;
                }
                byte += chunk.len();
                row += 1;
            }
        }
    }

    fn clamp(&mut self) {
        let max = self
            .wrapped_len()
            .saturating_sub(self.viewport_height.max(1));
        self.scroll = self.scroll.min(max);
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        if !self.open {
            return;
        }
        let area = frame.area();
        if area.width < 20 || area.height < 6 {
            theme::resize_notice(frame, "20×6 for help");
            return;
        }
        frame.render_widget(Clear, area);
        let [body, footer] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
        let block = panel(format!(" Help · {} ", visible_text(&self.title)))
            .border_style(theme::foreground(theme::palette().primary));
        let inner = block.inner(body);
        frame.render_widget(block, body);
        let anchor = self.anchor();
        let width_changed = self.viewport_width != usize::from(inner.width);
        self.viewport_width = usize::from(inner.width);
        self.viewport_height = usize::from(inner.height);
        if width_changed {
            self.restore_anchor(anchor);
        }
        self.clamp();
        let mut lines = Vec::new();
        for line in &self.lines {
            for chunk in self.chunks(line) {
                lines.push(if line.heading {
                    Line::styled(chunk, heading_style())
                } else {
                    Line::from(chunk)
                });
            }
        }
        frame.render_widget(
            // Skip in usize space; Paragraph's u16 scroll would wrap long help.
            Paragraph::new(lines.into_iter().skip(self.scroll).collect::<Vec<_>>())
                .style(surface(theme::palette().surface)),
            inner,
        );
        frame.render_widget(
            Paragraph::new(if footer.width >= 56 {
                Line::from(vec![
                    Span::styled("↑↓ / PgUp/PgDn Read", label_style()),
                    Span::raw("  ·  "),
                    Span::styled(HELP_CLOSE_HINT, theme::muted()),
                ])
            } else if footer.width >= 26 {
                Line::from("Esc / F1 close · ↑↓ read")
            } else {
                Line::from("Esc / F1 close")
            })
            .centered(),
            footer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn long_help() -> Help {
        let mut help = Help::default();
        let mut entries = (0..50)
            .map(|index| {
                format!(
                    "Evidence {index}: {}",
                    "界e\u{301}👩🏽‍💻 retained evidence ".repeat(6)
                )
            })
            .collect::<Vec<_>>();
        entries.push("LAST HELP ENTRY".into());
        help.open("Long reference", vec![("EVIDENCE", entries)]);
        help
    }

    #[test]
    fn end_reaches_the_actual_last_body_row_at_each_size() {
        let key = |code| KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        for (width, height) in [(120, 32), (80, 24), (60, 18), (20, 6)] {
            let mut help = long_help();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| help.draw(frame)).unwrap();
            assert_eq!(help.viewport_width, usize::from(width - 2));
            assert_eq!(help.viewport_height, usize::from(height - 3));
            help.key(key(KeyCode::End));
            terminal.draw(|frame| help.draw(frame)).unwrap();
            assert_eq!(help.scroll(), help.wrapped_len() - help.viewport_height);
            let last_row: String = (1..width - 1)
                .map(|x| terminal.backend().buffer()[(x, height - 3)].symbol())
                .collect();
            assert!(last_row.contains("LAST HELP ENTRY"), "{width}: {last_row}");
            assert!(text(&terminal).contains("Esc"));
            help.key(key(KeyCode::Home));
            terminal.draw(|frame| help.draw(frame)).unwrap();
            assert_eq!(help.scroll(), 0);
            assert!(text(&terminal).contains("EVIDENCE"));
            assert!(text(&terminal).contains("界"));
            assert!(text(&terminal).contains("e\u{301}"));
            assert!(text(&terminal).contains("👩🏽‍💻"));
        }
    }

    #[test]
    fn help_reflow_preserves_the_logical_reading_anchor_and_too_small_is_non_destructive() {
        let mut help = long_help();
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        terminal.draw(|frame| help.draw(frame)).unwrap();
        help.restore_anchor((15, 120));
        for (width, height) in [(80, 24), (60, 18), (19, 5), (60, 18), (120, 32)] {
            let before = help.anchor();
            let before_scroll = help.scroll();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| help.draw(frame)).unwrap();
            if width < 20 {
                assert_eq!(help.scroll(), before_scroll);
                assert_eq!(help.anchor(), before);
                assert!(text(&terminal).contains("Esc back"));
            } else {
                let after = help.anchor();
                assert_eq!(after.0, before.0, "retain the logical evidence line");
                assert!(after.1 <= before.1);
                let chunks = help.chunks(&help.lines[after.0]);
                let mut byte = 0;
                let chunk = chunks
                    .iter()
                    .find(|chunk| {
                        if byte == after.1 {
                            true
                        } else {
                            byte += chunk.len();
                            false
                        }
                    })
                    .unwrap();
                assert!(before.1 < after.1 + chunk.len());
            }
        }
        help.key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!help.is_open());
    }

    #[test]
    fn headings_wrap_and_terminal_controls_are_displayed_as_text() {
        let mut help = Help::default();
        help.open(
            "Safe \x1b title",
            vec![(
                "A LONG EVIDENCE HEADING THAT MUST WRAP IN A COMPACT READER WITHOUT HIDING ITS LAST WORD",
                vec!["literal \x1b[31m, 界, e\u{301}, 👩🏽‍💻".into()],
            )],
        );
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal.draw(|frame| help.draw(frame)).unwrap();
        let rendered = text(&terminal);
        assert!(rendered.contains("LAST WORD"));
        assert!(rendered.contains("\\u{1b}[31m"));
        assert!(!rendered.contains('\x1b'));
        if !crate::colors() {
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .all(|cell| cell.fg == ratatui::style::Color::Reset
                        && cell.bg == ratatui::style::Color::Reset)
            );
        }
    }

    fn help() -> Help {
        let mut help = Help::default();
        help.open(
            "Session inspector",
            vec![("PAGES", vec!["↑↓ scroll the selected pane".into()])],
        );
        help
    }

    #[test]
    fn help_opens_scrolls_and_returns_without_mutating_anything_else() {
        let mut help = help();
        assert!(help.is_open());
        let key = |code| KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        assert!(help.key(key(KeyCode::Down)));
        assert!(help.scroll() >= 1);
        assert!(help.key(key(KeyCode::PageDown)));
        assert!(help.key(key(KeyCode::Home)));
        assert_eq!(help.scroll(), 0);
        assert!(help.key(key(KeyCode::Char('?'))));
        assert!(!help.is_open());
    }

    #[test]
    fn help_is_readable_at_standard_and_compact_sizes() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut help = help();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| help.draw(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Help · Session inspector"), "{width}: {text}");
            assert!(text.contains("↑↓ / PgUp/PgDn Read"), "{width}: {text}");
            assert!(text.contains("F1 or ? close help"), "{width}: {text}");
        }
    }
}
