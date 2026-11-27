//! Searchable cockpit command palette and the shared agent-session chooser.
//!
//! The palette is data-only: it never reads kernel state. Callers supply
//! entries with stable keys and interpret the returned key. This keeps the
//! shell generic and lets the command-center, inspector and workflow lanes add
//! commands without touching the input loop.

use crate::{panel, selection_style, surface, theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

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
            .filter(|(_, entry)| words.iter().all(|word| entry.matches_word(word)))
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
        // Border + search + count + one result + two detail rows + hint.
        if width < 20 || height < 8 {
            theme::resize_notice(frame, "22×10 for the palette");
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
        let [search, count, list, detail, hint] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner);

        let search_line = if self.query.is_empty() {
            theme::clip_cells(" Search  type to search…", usize::from(search.width))
        } else {
            format!(
                " Search  {}▏",
                theme::tail_cells(&self.query, usize::from(search.width.saturating_sub(10)))
            )
        };
        frame.render_widget(
            Paragraph::new(search_line).style(surface(theme::palette().panel)),
            search,
        );
        let session_count = self
            .entries
            .iter()
            .filter(|entry| entry.key != "session|all")
            .count();
        let count_text = match self.kind {
            Some(Kind::Sessions) => format!(
                " {} session{} · {} visible",
                session_count,
                if session_count == 1 { "" } else { "s" },
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
        let label_width = usize::from(list.width.saturating_sub(2));
        let show_detail = inner.width >= 54;
        let items = self.visible.iter().filter_map(|index| {
            let entry = self.entries.get(*index)?;
            let style = if entry.enabled {
                StyleText::normal()
            } else {
                StyleText::unavailable()
            };
            let label = if entry.enabled {
                entry.label.clone()
            } else {
                format!("[Unavailable] {}", entry.label)
            };
            let group = format!("  ·  {}", crate::visible_text(&entry.group));
            let show_group = show_detail
                && !entry.group.is_empty()
                && UnicodeWidthStr::width(crate::visible_text(&label).as_str())
                    + UnicodeWidthStr::width(group.as_str())
                    <= label_width;
            let mut item = ListItem::new(Line::from(vec![
                Span::styled(theme::clip_cells(&label, label_width), style.label),
                Span::styled(
                    if show_group { group } else { String::new() },
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
                    "Nothing available in this context."
                } else {
                    "No matches · Ctrl+A"
                })
                .style(theme::muted()),
                Rect::new(list.x, list.y, list.width, 1),
            );
        }

        let detail_lines = self.selected().map_or_else(
            || vec![String::new()],
            |entry| {
                let status = if entry.enabled {
                    entry.detail.clone()
                } else {
                    format!("Unavailable · {}", entry.detail)
                };
                let mut lines = theme::wrap_cells(&status, usize::from(detail.width));
                // Reserve both rows for the reason, not a duplicate of the
                // selected label. Disclose clipping rather than silently hiding it.
                if lines.len() > 2 {
                    lines.truncate(2);
                    lines[1] =
                        theme::clip_cells(&format!("{}…", lines[1]), usize::from(detail.width));
                }
                lines
            },
        );
        frame.render_widget(
            Paragraph::new(detail_lines.into_iter().map(Line::from).collect::<Vec<_>>())
                .style(theme::foreground(theme::palette().text)),
            detail,
        );
        frame.render_widget(
            Paragraph::new(match (self.kind, hint.width) {
                (_, 0..=35) => "Esc cancel · Enter · ↑↓",
                (Some(Kind::Sessions), _) => "Esc cancel · Enter apply · ↑↓ browse · F1 help",
                _ => "Esc cancel · Enter run · ↑↓ choose · F1 help",
            })
            .style(theme::muted())
            .centered(),
            hint,
        );
    }
}

impl Entry {
    fn matches_word(&self, query: &str) -> bool {
        self.haystack()
            .split_whitespace()
            .any(|word| fuzzy_subsequence(query, word))
    }
}

fn fuzzy_subsequence(query: &str, candidate: &str) -> bool {
    let mut chars = candidate.chars();
    query
        .chars()
        .all(|wanted| chars.any(|actual| actual == wanted))
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

    fn text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn selected_long_label_does_not_hide_the_unavailable_reason() {
        let label = format!("界e\u{301}👩🏽‍💻 {}", "long-label-".repeat(14));
        let mut palette = Palette::default();
        palette.open_commands(vec![Entry::unavailable(
            "one",
            &label,
            "Needs a selected session",
            "GROUP",
        )]);
        for (width, height) in [(120, 32), (80, 24), (60, 18), (22, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| palette.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let rendered = text(&terminal);
            assert!(rendered.contains("[Unavailable]"), "{width}: {rendered}");
            assert!(rendered.contains("Unavailable ·"), "{width}: {rendered}");
            if width >= 60 {
                assert!(
                    rendered.contains("Needs a selected session"),
                    "{width}: {rendered}"
                );
                assert!(rendered.contains("界"));
                assert!(rendered.contains("e\u{301}"));
                assert!(rendered.contains("👩🏽‍💻"));
            }
            assert!(rendered.contains('…'), "long labels disclose clipping");
            assert!(rendered.contains("Esc cancel"));
            let marker = buffer
                .content()
                .iter()
                .find(|cell| cell.symbol() == "›")
                .unwrap();
            assert!(marker.modifier.contains(ratatui::style::Modifier::BOLD));
            if !crate::colors() {
                assert!(marker.modifier.contains(ratatui::style::Modifier::REVERSED));
                assert!(
                    buffer
                        .content()
                        .iter()
                        .all(|cell| cell.fg == ratatui::style::Color::Reset
                            && cell.bg == ratatui::style::Color::Reset)
                );
            }
            assert_eq!(palette.key(key(KeyCode::Enter)), Outcome::Open);
            assert_eq!(palette.selected().unwrap().key, "one");
        }
    }

    #[test]
    fn resize_keeps_selected_identity_and_the_editing_end_of_the_query_visible() {
        let mut palette = Palette::default();
        let query = format!("{}界e\u{301}👩🏽‍💻TAIL", "query".repeat(17));
        palette.open_commands(vec![
            Entry::command("first", "First", &query, "GROUP"),
            Entry::command("exact", "Exact selected session", &query, "GROUP"),
        ]);
        palette.select_key("exact");
        for ch in query.chars() {
            palette.key(key(KeyCode::Char(ch)));
        }
        for (width, height) in [(120, 32), (80, 24), (60, 18), (21, 9), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| palette.draw(frame)).unwrap();
            assert_eq!(palette.selected().unwrap().key, "exact");
            assert_eq!(palette.query(), query);
            if width >= 60 {
                assert!(
                    text(&terminal).contains("TAIL▏"),
                    "search cursor and suffix must be visible"
                );
            } else {
                assert!(text(&terminal).contains("Esc back"));
            }
        }
    }

    #[test]
    fn empty_and_no_match_states_keep_cancel_and_clear_reachable() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut palette = Palette::default();
            palette.open_commands(vec![]);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| palette.draw(frame)).unwrap();
            assert!(text(&terminal).contains("Nothing available"));
            assert!(!text(&terminal).contains('›'));
            palette.open_commands(entries());
            palette.key(key(KeyCode::Char('!')));
            terminal.draw(|frame| palette.draw(frame)).unwrap();
            assert!(text(&terminal).contains("No matches"));
            assert!(text(&terminal).contains("Ctrl+A"));
            assert!(text(&terminal).contains("Esc cancel"));
            assert!(palette.selected().is_none());
            assert_eq!(palette.key(key(KeyCode::Enter)), Outcome::Open);
            palette.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
            assert_eq!(palette.selected().unwrap().key, "go.agents");
        }
    }

    #[test]
    fn session_count_excludes_only_an_explicit_all_sessions_entry() {
        let mut palette = Palette::default();
        let session = Entry::command(
            "session|source|exact",
            "Exact session",
            "Recorded identity",
            "SESSIONS",
        );
        for entries in [
            vec![session.clone()],
            vec![
                Entry::command("session|all", "All sessions", "Clear scope", "SESSIONS"),
                session,
            ],
        ] {
            palette.open_sessions(entries);
            let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
            terminal.draw(|frame| palette.draw(frame)).unwrap();
            assert!(text(&terminal).contains("1 session ·"));
            assert!(!text(&terminal).contains("0 sessions"));
        }
    }

    #[test]
    fn minimum_palette_scrolls_to_the_selected_result_and_below_floor_clears() {
        let mut palette = Palette::default();
        palette.open_commands(entries());
        palette.select_key("view.review");
        let mut terminal = Terminal::new(TestBackend::new(22, 10)).unwrap();
        terminal.draw(|frame| palette.draw(frame)).unwrap();
        assert!(text(&terminal).contains("› [Unavailable]"));
        for (width, height) in [(21, 10), (22, 9), (19, 5), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    frame.render_widget(Paragraph::new("OLD CONTENT"), frame.area());
                    palette.draw(frame);
                })
                .unwrap();
            assert!(!text(&terminal).contains("OLD CONTENT"));
            if width >= 16 {
                assert!(text(&terminal).contains("Esc back"));
            }
            assert_eq!(palette.selected().unwrap().key, "view.review");
        }
        assert_eq!(palette.key(key(KeyCode::Esc)), Outcome::Cancel);
    }

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
    fn fuzzy_search_stays_within_a_single_command_word() {
        let mut palette = Palette::default();
        palette.open_commands(vec![
            Entry::command(
                "themes",
                "Appearance themes",
                "Preview and apply colors",
                "SYSTEM",
            ),
            Entry::command(
                "today",
                "Today",
                "Show current work and recent outcomes",
                "TIME",
            ),
            Entry::command("history", "History", "Show retained sessions", "TIME"),
            Entry::command(
                "refresh",
                "Refresh sessions",
                "Reload the current page",
                "LIVE",
            ),
        ]);
        for character in "theme".chars() {
            palette.key(key(KeyCode::Char(character)));
        }
        assert_eq!(palette.selected().unwrap().key, "themes");
        assert_eq!(palette.visible.len(), 1);
        palette.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        for character in "thms".chars() {
            palette.key(key(KeyCode::Char(character)));
        }
        assert_eq!(palette.selected().unwrap().key, "themes");
        assert_eq!(palette.visible.len(), 1);
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
