//! Persistent, scrollable contextual keyboard reference.
//!
//! The shell opens one overlay from the shared action inventory. Opening help
//! never mutates the underlying view, so drafts, selections and scroll survive
//! `F1` and return to the exact same control.

use crate::{
    HELP_CLOSE_HINT, heading_style, label_style, panel, surface, theme, visible_text,
    wrap_display_width,
};
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
        self.viewport_height.saturating_sub(2).max(1)
    }

    fn wrapped_len(&self) -> usize {
        self.lines
            .iter()
            .map(|line| {
                if line.heading || line.text.is_empty() {
                    1
                } else {
                    wrap_display_width(&line.text, self.viewport_width.max(20)).len()
                }
            })
            .sum()
    }

    fn clamp(&mut self) {
        let max = self
            .wrapped_len()
            .saturating_sub(self.viewport_height.saturating_sub(2).max(1));
        self.scroll = self.scroll.min(max);
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        if !self.open {
            return;
        }
        let area = frame.area();
        self.viewport_height = usize::from(area.height);
        self.viewport_width = usize::from(area.width.saturating_sub(4));
        self.clamp();
        if area.width < 20 || area.height < 6 {
            frame.render_widget(
                Paragraph::new("Terminal too small · widen for help · F1 / Esc return")
                    .centered()
                    .style(theme::foreground(theme::palette().primary)),
                area,
            );
            return;
        }
        frame.render_widget(Clear, area);
        let [body, footer] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
        let block = panel(format!(" Help · {} ", visible_text(&self.title)))
            .border_style(theme::foreground(theme::palette().primary));
        let inner = block.inner(body);
        frame.render_widget(block, body);
        let width = inner.width.max(1);
        let mut lines = Vec::new();
        for line in &self.lines {
            if line.heading {
                lines.push(Line::styled(line.text.clone(), heading_style()));
            } else if line.text.is_empty() {
                lines.push(Line::from(""));
            } else {
                for chunk in wrap_display_width(&line.text, usize::from(width)) {
                    lines.push(Line::from(chunk));
                }
            }
        }
        let max = lines.len().saturating_sub(usize::from(inner.height).max(1));
        let scroll = self.scroll.min(max) as u16;
        frame.render_widget(
            Paragraph::new(lines)
                .style(surface(theme::palette().surface))
                .scroll((scroll, 0)),
            inner,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("↑↓ / PgUp/PgDn Read", label_style()),
                Span::raw("  ·  "),
                Span::styled(HELP_CLOSE_HINT, theme::muted()),
            ]))
            .centered(),
            footer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

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
