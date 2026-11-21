//! Generic terminal rendering for kernel snapshots.
//!
//! The UI consumes data-only panes and never switches on extension IDs. It
//! owns terminal lifecycle and keyboard state, while the app supplies an
//! immutable snapshot prepared outside the render loop.

#![forbid(unsafe_code)]

use agenturm_kernel::{MetricRow, Snapshot};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::io;
use unicode_width::UnicodeWidthChar;

const MIN_WIDTH: u16 = 38;
const MIN_HEIGHT: u16 = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pane {
    title: String,
    rows: Vec<MetricRow>,
}

/// Keyboard and selection state for a rendered kernel snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UiModel {
    panes: Vec<Pane>,
    selected: usize,
    scroll: usize,
    scroll_limit: usize,
    viewport_width: u16,
    viewport_height: u16,
    observations: usize,
    truncated: bool,
    help: bool,
}

impl UiModel {
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        let mut panes: Vec<_> = snapshot
            .panes
            .into_values()
            .map(|pane| Pane {
                title: visible_text(&pane.title),
                rows: pane.rows,
            })
            .collect();
        if !snapshot.metrics.is_empty() {
            panes.push(Pane {
                title: "Metrics".into(),
                rows: snapshot
                    .metrics
                    .into_iter()
                    .flat_map(|(extension_id, rows)| {
                        rows.into_iter().map(move |row| MetricRow {
                            label: format!("{} · {}", extension_id.as_str(), row.label),
                            value: row.value,
                        })
                    })
                    .collect(),
            });
        }
        if !snapshot.failures.is_empty() {
            panes.push(Pane {
                title: "Attention".into(),
                rows: snapshot
                    .failures
                    .into_iter()
                    .map(|failure| MetricRow {
                        label: format!("{} · {:?}", failure.extension_id.as_str(), failure.stage),
                        value: failure.message,
                    })
                    .collect(),
            });
        }
        Self {
            panes,
            selected: 0,
            scroll: 0,
            scroll_limit: 0,
            viewport_width: 0,
            viewport_height: 0,
            observations: snapshot.observations.len(),
            truncated: snapshot.observations_truncated,
            help: false,
        }
    }

    pub fn selected_title(&self) -> Option<&str> {
        self.panes
            .get(self.selected)
            .map(|pane| pane.title.as_str())
    }

    pub fn select_next(&mut self) {
        if !self.panes.is_empty() {
            self.selected = (self.selected + 1) % self.panes.len();
            self.scroll = 0;
            self.update_scroll_limit();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.panes.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.panes.len() - 1);
            self.scroll = 0;
            self.update_scroll_limit();
        }
    }

    pub fn set_viewport(&mut self, width: u16, height: u16) {
        self.viewport_width = width;
        self.viewport_height = height;
        self.update_scroll_limit();
    }

    fn update_scroll_limit(&mut self) {
        let Some(pane) = self.panes.get(self.selected) else {
            self.scroll = 0;
            self.scroll_limit = 0;
            return;
        };
        let visible_lines = usize::from(self.viewport_height.saturating_sub(7).max(1));
        let pane_width = if self.viewport_width < 64 {
            self.viewport_width
        } else {
            self.viewport_width.saturating_sub(22)
        };
        self.scroll_limit = pane_lines(pane, pane_width.saturating_sub(2))
            .len()
            .saturating_sub(visible_lines)
            .min(usize::from(u16::MAX));
        self.scroll = self.scroll.min(self.scroll_limit);
    }

    fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1).min(self.scroll_limit);
    }

    fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    /// Applies one key press. Returns true when the user requests exit.
    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => true,
            KeyCode::Right | KeyCode::Tab => {
                self.select_next();
                false
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.select_previous();
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_down();
                false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_up();
                false
            }
            KeyCode::Char('?') => {
                self.help = !self.help;
                false
            }
            _ => false,
        }
    }
}

/// Runs the terminal UI and restores the terminal on every return path.
pub fn run(snapshot: Snapshot) -> io::Result<()> {
    let mut model = UiModel::from_snapshot(snapshot);
    ratatui::run(|terminal| {
        loop {
            let area = terminal.size()?;
            model.set_viewport(area.width, area.height);
            terminal.draw(|frame| draw(frame, &model))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && model.handle_key(key.code)
            {
                break Ok(());
            }
        }
    })
}

fn draw(frame: &mut Frame, model: &UiModel) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window to inspect Agenturm")
                .centered()
                .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
    let mut header = vec![Span::styled(
        " Agenturm ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    header.push(Span::raw(format!(" · {} observations", model.observations)));
    if model.truncated {
        header.push(Span::styled(
            " · showing newest history",
            Style::default().fg(Color::Yellow),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(header)).block(Block::default().borders(Borders::BOTTOM)),
        rows[0],
    );

    if model.panes.is_empty() {
        frame.render_widget(
            Paragraph::new("No panes are registered yet.\nAdd a UI contribution to inspect data.")
                .block(Block::bordered().title("Overview"))
                .wrap(Wrap { trim: true }),
            rows[1],
        );
    } else {
        render_panes(frame, model, rows[1]);
    }

    let footer = if model.help {
        " ←/→ or Tab switch panes · ↑/↓ or j/k scroll · ? close help · q quit "
    } else {
        " ←/→ or Tab switch panes · ↑/↓ scroll · ? help · q quit "
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(Style::default().fg(Color::DarkGray))
            .centered(),
        rows[2],
    );
}

fn render_panes(frame: &mut Frame, model: &UiModel, area: Rect) {
    if area.width < 64 {
        render_selected_pane(frame, model, area);
        return;
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(1)])
        .split(area);
    let items = model
        .panes
        .iter()
        .map(|pane| ListItem::new(pane.title.as_str()))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::bordered().title("Views"))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    state.select(Some(model.selected));
    frame.render_stateful_widget(list, columns[0], &mut state);
    render_selected_pane(frame, model, columns[1]);
}

fn render_selected_pane(frame: &mut Frame, model: &UiModel, area: Rect) {
    let pane = &model.panes[model.selected];
    let content_width = area.width.saturating_sub(2);
    frame.render_widget(
        Paragraph::new(pane_lines(pane, content_width))
            .block(Block::bordered().title(pane.title.as_str()))
            .scroll((model.scroll as u16, 0)),
        area,
    );
}

fn pane_lines(pane: &Pane, width: u16) -> Vec<Line<'static>> {
    if pane.rows.is_empty() {
        return vec![Line::from(Span::styled(
            "No data in this view yet",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let width = usize::from(width.max(1));
    let mut output = Vec::new();
    for row in &pane.rows {
        let text = format!("{}  {}", visible_text(&row.label), visible_text(&row.value));
        for (index, chunk) in wrap_display_width(&text, width).into_iter().enumerate() {
            if index == 0 {
                output.push(Line::from(Span::styled(
                    chunk,
                    Style::default().fg(Color::Magenta),
                )));
            } else {
                output.push(Line::from(chunk));
            }
        }
    }
    output
}

fn visible_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            output.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
        } else {
            output.push(character);
        }
    }
    output
}

fn wrap_display_width(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if line_width + character_width > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line_width = 0;
        }
        line.push(character);
        line_width += character_width;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{ExtensionId, PaneModel};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::collections::BTreeMap;

    fn snapshot_with_panes() -> Snapshot {
        let pane = |title: &str, value: &str| PaneModel {
            title: title.into(),
            rows: vec![MetricRow {
                label: "Rows".into(),
                value: value.into(),
            }],
        };
        Snapshot {
            panes: BTreeMap::from([
                (
                    ExtensionId::parse("pane.activity").unwrap(),
                    pane("Activity", "3"),
                ),
                (
                    ExtensionId::parse("pane.usage").unwrap(),
                    pane("Usage", "2"),
                ),
            ]),
            observations: Vec::new(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn pane_navigation_and_row_scrolling_are_independent() {
        let mut model = UiModel::from_snapshot(snapshot_with_panes());
        model.set_viewport(40, 10);
        assert_eq!(model.selected_title(), Some("Activity"));
        model.select_previous();
        assert_eq!(model.selected_title(), Some("Usage"));
        assert!(!model.handle_key(KeyCode::Tab));
        assert_eq!(model.selected_title(), Some("Activity"));
        assert!(!model.handle_key(KeyCode::Down));
        assert_eq!(model.scroll, 0, "one-row pane cannot scroll");
        assert!(model.handle_key(KeyCode::Char('q')));
    }

    #[test]
    fn scrolling_stays_inside_the_active_pane_and_resets_on_switch() {
        let mut snapshot = snapshot_with_panes();
        snapshot
            .panes
            .get_mut(&ExtensionId::parse("pane.activity").unwrap())
            .unwrap()
            .rows = (0..4)
            .map(|index| MetricRow {
                label: format!("Row {index}"),
                value: index.to_string(),
            })
            .collect();
        let mut model = UiModel::from_snapshot(snapshot);
        model.set_viewport(40, 10);
        assert!(!model.handle_key(KeyCode::Down));
        assert_eq!(model.scroll, 1);
        assert!(!model.handle_key(KeyCode::Tab));
        assert_eq!(model.scroll, 0);
        assert_eq!(model.selected_title(), Some("Usage"));
    }

    #[test]
    fn viewport_scroll_limit_accounts_for_wrapped_text() {
        let mut snapshot = snapshot_with_panes();
        snapshot
            .panes
            .get_mut(&ExtensionId::parse("pane.activity").unwrap())
            .unwrap()
            .rows = (0..8)
            .map(|index| MetricRow {
                label: format!("Row {index}"),
                value: "A long value that must wrap in a narrow terminal to remain inspectable"
                    .into(),
            })
            .collect();
        let mut model = UiModel::from_snapshot(snapshot);
        model.set_viewport(40, 10);
        let limit = model.scroll_limit;
        assert!(limit > 8, "wrapped visual lines must be scrollable");
        for _ in 0..(limit + 4) {
            model.scroll_down();
        }
        assert_eq!(
            model.scroll, limit,
            "scroll stops at the final visible page"
        );
    }

    #[test]
    fn capability_rows_are_available_in_a_generic_metrics_view() {
        let mut snapshot = snapshot_with_panes();
        snapshot.metrics.insert(
            ExtensionId::parse("capability.prompt-quality").unwrap(),
            vec![MetricRow {
                label: "Prompt efficiency".into(),
                value: "0.82".into(),
            }],
        );
        let model = UiModel::from_snapshot(snapshot);
        let metrics = model
            .panes
            .iter()
            .find(|pane| pane.title == "Metrics")
            .unwrap();
        assert_eq!(
            metrics.rows[0].label,
            "capability.prompt-quality · Prompt efficiency"
        );
        assert_eq!(metrics.rows[0].value, "0.82");
    }

    #[test]
    fn untrusted_control_characters_in_extension_titles_are_visible_text() {
        let mut snapshot = snapshot_with_panes();
        snapshot
            .panes
            .get_mut(&ExtensionId::parse("pane.activity").unwrap())
            .unwrap()
            .title = "\u{1b}[31mActivity".into();
        let model = UiModel::from_snapshot(snapshot);
        assert_eq!(model.selected_title(), Some("\\u{1b}[31mActivity"));
    }

    #[test]
    fn renderer_handles_wide_narrow_and_too_small_terminals() {
        for (width, height) in [(100, 30), (50, 16), (20, 6)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut model = UiModel::from_snapshot(snapshot_with_panes());
            model.set_viewport(width, height);
            terminal.draw(|frame| draw(frame, &model)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            if width < MIN_WIDTH || height < MIN_HEIGHT {
                assert!(text.contains("Terminal too small"));
            } else {
                assert!(text.contains("Agenturm"));
                assert!(text.contains("Activity"));
            }
        }
    }
}
