//! Searchable cockpit command palette and the shared agent-session chooser.
//!
//! The palette is data-only: it never reads kernel state. Callers supply
//! entries with stable keys and interpret the returned key. This keeps the
//! shell generic and lets the command-center, inspector and workflow lanes add
//! commands without touching the input loop.

use crate::{heading_style, label_style, panel, selection_style, surface, theme, visible_text};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Commands,
    Sessions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) group: String,
    pub(crate) enabled: bool,
}

impl Entry {
    pub(crate) fn command(
        key: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
        group: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            detail: detail.into(),
            group: group.into(),
            enabled: true,
        }
    }

    pub(crate) fn unavailable(
        key: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
        group: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            detail: detail.into(),
            group: group.into(),
            enabled: false,
        }
    }

    fn haystack(&self) -> String {
        format!("{} {} {} {}", self.label, self.detail, self.group, self.key).to_lowercase()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Open,
    Help,
    Run(String),
    Cancel,
}

#[derive(Default)]
pub(crate) struct Palette {
    open: bool,
    kind: Option<Kind>,
    query: String,
    entries: Vec<Entry>,
    visible: Vec<usize>,
    selection: usize,
}

impl Palette {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn kind(&self) -> Option<Kind> {
        self.kind
    }

    #[allow(dead_code)]
    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    #[allow(dead_code)]
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub(crate) fn selected(&self) -> Option<&Entry> {
        self.visible
            .get(self.selection)
            .and_then(|index| self.entries.get(*index))
    }

    pub(crate) fn open_commands(&mut self, entries: Vec<Entry>) {
        self.open_with(Kind::Commands, entries);
    }

    pub(crate) fn open_sessions(&mut self, entries: Vec<Entry>) {
        self.open_with(Kind::Sessions, entries);
    }

    /// Selects an entry by stable key when it is visible. Missing keys keep the
    /// first deterministic row.
    pub(crate) fn select_key(&mut self, key: &str) {
        if let Some(position) = self
            .visible
            .iter()
            .position(|index| self.entries[*index].key == key)
        {
            self.selection = position;
        }
    }

    fn open_with(&mut self, kind: Kind, entries: Vec<Entry>) {
        self.open = true;
        self.kind = Some(kind);
        self.query.clear();
        self.entries = entries;
        self.selection = 0;
        self.refresh();
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.kind = None;
        self.entries.clear();
        self.visible.clear();
        self.selection = 0;
    }

    /// Applies one key. The palette owns Escape, all printable text, arrows and
    /// Enter while open; the enclosing screen never sees these keys.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        if !self.open {
            return Outcome::Open;
        }
        match key.code {
            KeyCode::Char('p' | 'P') if key.modifiers == KeyModifiers::CONTROL => {
                self.close();
                Outcome::Cancel
            }
            KeyCode::Esc => {
                self.close();
                Outcome::Cancel
            }
            KeyCode::F(1) => Outcome::Help,
            KeyCode::Up | KeyCode::BackTab => {
                self.move_by(-1);
                Outcome::Open
            }
            KeyCode::Down | KeyCode::Tab => {
                self.move_by(1);
                Outcome::Open
            }
            KeyCode::PageUp => {
                self.move_by(-6);
                Outcome::Open
            }
            KeyCode::PageDown => {
                self.move_by(6);
                Outcome::Open
            }
            KeyCode::Home => {
                self.selection = 0;
                Outcome::Open
            }
            KeyCode::End => {
                self.selection = self.visible.len().saturating_sub(1);
                Outcome::Open
            }
            KeyCode::Enter => self
                .selected()
                .filter(|entry| entry.enabled)
                .map(|entry| Outcome::Run(entry.key.clone()))
                .unwrap_or(Outcome::Open),
            KeyCode::Char('a' | 'A') if key.modifiers == KeyModifiers::CONTROL => {
                self.query.clear();
                self.refresh();
                Outcome::Open
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refresh();
                Outcome::Open
            }
            KeyCode::Char(ch)
                if !ch.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(ch) = crate::keymap::text_key(key)
                    && self.query.len() + ch.len_utf8() <= 128
                {
                    self.query.push(ch);
                    self.refresh();
                }
                Outcome::Open
            }
            _ => Outcome::Open,
        }
    }

    fn move_by(&mut self, amount: isize) {
        if self.visible.is_empty() {
            self.selection = 0;
            return;
        }
        let last = self.visible.len() - 1;
        self.selection = if amount.is_negative() {
            self.selection.saturating_sub(amount.unsigned_abs())
        } else {
            (self.selection + amount as usize).min(last)
        };
    }

    fn refresh(&mut self) {
        let previous_key = self.selected().map(|entry| entry.key.clone());
        let query = self.query.to_lowercase();
        let words = query.split_whitespace().collect::<Vec<_>>();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                let haystack = entry.haystack();
                words.iter().all(|word| haystack.contains(word))
            })
            .map(|(index, _)| index)
            .collect();
        self.selection = previous_key
            .and_then(|key| {
                self.visible
                    .iter()
                    .position(|index| self.entries[*index].key == key)
            })
            .unwrap_or(0)
            .min(self.visible.len().saturating_sub(1));
    }

    pub(crate) fn draw(&self, frame: &mut Frame) {
        if !self.open {
            return;
        }
        let area = frame.area();
        let width = area.width.saturating_sub(2).min(88);
        let height = area.height.saturating_sub(2).min(24);
        if width < 20 || height < 6 {
            frame.render_widget(
                Paragraph::new("Terminal too small · widen for the palette · Esc cancel")
                    .centered()
                    .style(theme::foreground(theme::palette().primary)),
                area,
            );
            return;
        }
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let title = match self.kind {
            Some(Kind::Sessions) => " Choose agent session ",
            _ => " Command palette ",
        };
        let block = panel(title).border_style(theme::foreground(theme::palette().primary));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.height < 4 || inner.width < 10 {
            return;
        }
        let [search, count, list, detail, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner);

        let search_line = if self.query.is_empty() {
            " Search  type a destination, session or action…".to_owned()
        } else {
            format!(" Search  {}▏", visible_text(&self.query))
        };
        frame.render_widget(
            Paragraph::new(search_line)
                .style(surface(theme::palette().panel))
                .wrap(Wrap { trim: false }),
            search,
        );
        let count_text = match self.kind {
            Some(Kind::Sessions) => format!(
                " {} session{} · {} visible",
                self.entries.len().saturating_sub(1),
                if self.entries.len() == 2 { "" } else { "s" },
                self.visible.len()
            ),
            _ => format!(
                " {} action{} · {} visible",
                self.entries.len(),
                if self.entries.len() == 1 { "" } else { "s" },
                self.visible.len()
            ),
        };
        frame.render_widget(Paragraph::new(count_text).style(theme::muted()), count);

        let mut state = ListState::default()
            .with_selected((!self.visible.is_empty()).then_some(self.selection));
        let show_detail = inner.width >= 54;
        let items = self.visible.iter().filter_map(|index| {
            let entry = self.entries.get(*index)?;
            let style = if entry.enabled {
                StyleText::normal()
            } else {
                StyleText::unavailable()
            };
            let mut item = ListItem::new(Line::from(vec![
                Span::styled(entry.label.clone(), style.label),
                Span::styled(
                    if show_detail && !entry.group.is_empty() {
                        format!("  ·  {}", entry.group)
                    } else {
                        String::new()
                    },
                    theme::muted(),
                ),
            ]));
            if !entry.enabled {
                item = item.style(theme::foreground(theme::palette().muted));
            }
            Some(item)
        });
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("› ")
                .highlight_style(selection_style())
                .style(surface(theme::palette().surface)),
            list,
            &mut state,
        );
        if self.visible.is_empty() {
            frame.render_widget(
                Paragraph::new(if self.query.is_empty() {
                    " Nothing available in this context."
                } else {
                    " No matches · clear the search with Ctrl+A."
                })
                .style(theme::muted()),
                Rect::new(list.x, list.y, list.width, 1),
            );
        }

        let detail_lines = self.selected().map_or_else(
            || vec![Line::from("")],
            |entry| {
                let status = if entry.enabled {
                    entry.detail.clone()
                } else {
                    format!("Unavailable · {}", entry.detail)
                };
                vec![
                    Line::styled(entry.label.clone(), heading_style()),
                    Line::from(format!(" {}  {}", status, entry.group)).style(label_style()),
                ]
            },
        );
        frame.render_widget(
            Paragraph::new(detail_lines).wrap(Wrap { trim: false }),
            detail,
        );
        frame.render_widget(
            Paragraph::new(match self.kind {
                Some(Kind::Sessions) => "↑↓ browse · Enter apply · Esc cancel · type to search",
                _ => "↑↓ choose · Enter run · Esc cancel · type to search",
            })
            .style(theme::muted())
            .centered(),
            hint,
        );
    }
}

struct StyleText {
    label: ratatui::style::Style,
}

impl StyleText {
    fn normal() -> Self {
        Self {
            label: crate::theme::foreground(theme::palette().text),
        }
    }

    fn unavailable() -> Self {
        Self {
            label: theme::foreground(theme::palette().muted),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn entries() -> Vec<Entry> {
        vec![
            Entry::command("go.agents", "Agents", "Open the command center", "VIEWS"),
            Entry::command("cockpit.themes", "Themes", "Preview palettes", "SYSTEM"),
            Entry::unavailable(
                "view.review",
                "Review recorded work",
                "Needs a selected session",
                "REVIEW",
            ),
        ]
    }

    #[test]
    fn search_keeps_literal_text_and_preserves_the_selected_key() {
        let mut palette = Palette::default();
        palette.open_commands(entries());
        for ch in ['?', '[', '4'] {
            assert_eq!(palette.key(key(KeyCode::Char(ch))), Outcome::Open);
        }
        assert_eq!(palette.query(), "?[4");
        assert!(
            palette.selected().is_none(),
            "empty result stays open with no fabricated selection"
        );
        palette.key(key(KeyCode::Backspace));
        assert_eq!(palette.query(), "?[");
        palette.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(palette.query().is_empty());

        for ch in "them".chars() {
            palette.key(key(KeyCode::Char(ch)));
        }
        assert_eq!(palette.selected().unwrap().key, "cockpit.themes");
        assert_eq!(
            palette.key(key(KeyCode::Enter)),
            Outcome::Run("cockpit.themes".into())
        );
    }

    #[test]
    fn disabled_entries_stay_visible_and_do_not_run() {
        let mut palette = Palette::default();
        palette.open_commands(entries());
        palette.key(key(KeyCode::End));
        assert_eq!(palette.selected().unwrap().key, "view.review");
        assert_eq!(palette.key(key(KeyCode::Enter)), Outcome::Open);
        assert!(palette.is_open());
        assert_eq!(palette.key(key(KeyCode::Esc)), Outcome::Cancel);
        assert!(!palette.is_open());
        palette.open_commands(entries());
        assert_eq!(
            palette.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Outcome::Cancel
        );
        assert!(!palette.is_open());
    }

    #[test]
    fn session_escape_cancels_without_touching_the_enclosing_screen() {
        let mut palette = Palette::default();
        palette.open_sessions(vec![
            Entry::command(
                "session|all",
                "All sessions",
                "Clear only the session filter",
                "SESSIONS",
            ),
            Entry::command(
                "session|h|s",
                "Payments API",
                "Codex · /work/payments · h / s",
                "SESSIONS",
            ),
        ]);
        assert!(palette.is_open());
        assert_eq!(palette.key(key(KeyCode::F(1))), Outcome::Help);
        assert!(palette.is_open(), "help does not discard the chooser");
        assert_eq!(palette.key(key(KeyCode::Esc)), Outcome::Cancel);
        assert!(!palette.is_open());
    }

    #[test]
    fn palette_renders_results_detail_and_cancel_at_standard_and_compact_sizes() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut palette = Palette::default();
            palette.open_commands(entries());
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| palette.draw(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Command palette"), "{width}: {text}");
            assert!(text.contains("Agents"), "{width}: {text}");
            assert!(text.contains("Themes"), "{width}: {text}");
            assert!(text.contains("Esc cancel"), "{width}: {text}");
        }
    }
}
