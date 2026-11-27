//! Session inspector sections and scoped evidence readers.
//!
//! The inspector is a shell for lane-owned models. This module owns only the
//! section tab strip and the inspector frame: the Overview and Details content
//! arrives as generic panes from the application, and the Prompts section
//! delegates to the UI-04 prompt navigator through the existing
//! `WorkCommand::Prompts` request. Nothing here reads provider data or captured
//! text.

use super::command_center;
use super::{RecordedSession, SessionView, age};
use crate::{MIN_HEIGHT, MIN_WIDTH, keymap, label_style, selection_style, theme, visible_text};
use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// The discoverable inspector sections. `Prompts` enters the shared
/// prompt navigator rather than reimplementing a reader here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Tab {
    #[default]
    Overview,
    Prompts,
    Details,
    Activity,
    Mcp,
    Tools,
}

impl Tab {
    const ORDER: [Tab; 6] = [
        Tab::Overview,
        Tab::Prompts,
        Tab::Details,
        Tab::Activity,
        Tab::Mcp,
        Tab::Tools,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Prompts => "Prompts",
            Self::Details => "Details",
            Self::Activity => "Activity",
            Self::Mcp => "MCP",
            Self::Tools => "Tools",
        }
    }

    pub(super) fn from_view(view: SessionView) -> Option<Self> {
        match view {
            SessionView::Overview => Some(Self::Overview),
            SessionView::Details => Some(Self::Details),
            SessionView::Activity => Some(Self::Activity),
            SessionView::Mcp => Some(Self::Mcp),
            SessionView::Tools => Some(Self::Tools),
            _ => None,
        }
    }

    pub(super) fn view(self) -> Option<SessionView> {
        match self {
            Self::Overview => Some(SessionView::Overview),
            Self::Details => Some(SessionView::Details),
            Self::Activity => Some(SessionView::Activity),
            Self::Mcp => Some(SessionView::Mcp),
            Self::Tools => Some(SessionView::Tools),
            Self::Prompts => None,
        }
    }

    pub(super) fn next(self) -> Self {
        self.step(1)
    }

    pub(super) fn previous(self) -> Self {
        self.step(-1)
    }

    fn step(self, direction: isize) -> Self {
        let index = Self::ORDER
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default() as isize;
        let next = (index + direction).rem_euclid(Self::ORDER.len() as isize);
        Self::ORDER[next as usize]
    }
}

/// Inspector-local focus. `tabs_focused` means Left/Right choose a section and
/// Enter/Down enters it; false means the content reader owns Up/Down/scroll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct State {
    pub(super) tab: Tab,
    pub(super) tabs_focused: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tab: Tab::Overview,
            tabs_focused: true,
        }
    }
}

impl State {
    pub(super) fn focused(view: SessionView) -> Self {
        Self {
            tab: Tab::from_view(view).unwrap_or_default(),
            tabs_focused: true,
        }
    }
}

pub(super) fn draw(
    frame: &mut Frame,
    session: &RecordedSession,
    page: &mut super::PageView,
    state: State,
    has_newer_page: bool,
    pending: bool,
    message: Option<&str>,
) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window to inspect Agenturm")
                .centered()
                .style(theme::foreground(theme::palette().primary)),
            area,
        );
        return;
    }
    let [head, tabs, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);

    let name = visible_text(&command_center::name(session));
    let project = project_label(session);
    let state_label = session.state.as_deref().unwrap_or("unknown");
    let mut context = format!(
        " {} · {} · recorded {} · {}",
        visible_text(session.provider_label()),
        visible_text(&project),
        visible_text(state_label),
        age(session.updated_at_unix_ns),
    );
    if let Some(worker) = session.worker_id.as_deref() {
        context.push_str(&format!(" · worker {}", visible_text(worker)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" Agenturm · {name}"),
                theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   {context}"),
                if context.cell_width() + name.cell_width() + 14 <= area.width {
                    label_style()
                } else {
                    theme::muted()
                },
            ),
        ]))
        .block(Block::default().borders(Borders::BOTTOM)),
        head,
    );

    draw_tabs(frame, tabs, state);

    let ui = &page.ui;
    let content_hint = if let Some(table) = page.operation_table.active(ui) {
        table.draw(frame, body, ui.selected_title().unwrap_or("Tools"));
        Some(table.key_hint().to_owned())
    } else if let Some(table) = ui.selected_evidence() {
        let visible = ui.evidence_visible();
        if let Some(inspection) = &ui.evidence_inspection {
            table.draw_expanded_filtered(frame, body, Some(&visible), ui.scroll, inspection);
        } else {
            table.draw_with_inspector_filtered(frame, body, Some(&visible), ui.scroll, true);
        }
        Some("↑↓ select · Enter inspect · / filter · Esc back".to_owned())
    } else {
        crate::render_selected_pane(frame, ui, body);
        None
    };

    let hints = if state.tabs_focused {
        format!(
            "↑↓ scroll · {}",
            keymap::hint(&[
                keymap::id::INSPECTOR_SECTIONS,
                keymap::id::INSPECT,
                keymap::id::BACK,
                keymap::id::HELP,
            ])
        )
    } else {
        format!(
            "↑↓ scroll · {}",
            keymap::hint(&[
                keymap::id::INSPECTOR_SECTIONS,
                keymap::id::PROMPTS,
                keymap::id::SESSION_ACTIVITY,
                keymap::id::REVIEW,
                keymap::id::TRACES,
                keymap::id::SESSION_USAGE,
                keymap::id::SESSION_METRICS,
                keymap::id::BACK,
                keymap::id::HELP,
            ])
        )
    };
    let hints = if !state.tabs_focused {
        content_hint.map_or(hints.clone(), |content| {
            format!("{content} · Ctrl+←/→ sections")
        })
    } else {
        hints
    };
    let mut footer_lines = Vec::new();
    if let Some(error) = message {
        footer_lines.push(Line::styled(visible_text(error), label_style()));
    } else if pending {
        footer_lines.push(Line::styled("Loading this section…", label_style()));
    }
    let mut pages = String::new();
    if has_newer_page {
        pages.push_str("[ newer · ");
    }
    if ui.older_cursor().is_some() {
        pages.push_str("] older · ");
    }
    if !pages.is_empty() {
        pages.push_str("l latest · ");
    }
    footer_lines.push(Line::from(format!("{pages}{hints}")).style(theme::muted()));
    frame.render_widget(
        Paragraph::new(footer_lines).wrap(Wrap { trim: true }),
        footer,
    );
}

/// Renders the section tab strip with the UI-04 prompt navigator in the
/// body. The navigator owns its own reader, chooser and status line.
pub(super) fn draw_prompts(
    frame: &mut Frame,
    session: &RecordedSession,
    state: State,
    navigator: &mut crate::workspace::prompt::PromptNavigator,
    pending: bool,
    message: Option<&str>,
) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window to inspect Agenturm")
                .centered()
                .style(theme::foreground(theme::palette().primary)),
            area,
        );
        return;
    }
    let [head, tabs, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);
    let name = visible_text(&command_center::name(session));
    let context = format!(
        " {} · {} · recorded {} · {}",
        visible_text(session.provider_label()),
        visible_text(&project_label(session)),
        visible_text(session.state.as_deref().unwrap_or("unknown")),
        age(session.updated_at_unix_ns),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" Agenturm · {name}"),
                theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("   {context}"), label_style()),
        ]))
        .block(Block::default().borders(Borders::BOTTOM)),
        head,
    );
    draw_tabs(frame, tabs, state);
    navigator.set_layout(area.width);
    navigator.draw(frame, body);
    let mut lines = Vec::new();
    if let Some(error) = message {
        lines.push(Line::styled(visible_text(error), label_style()));
    } else if pending
        && matches!(
            navigator.status(),
            super::prompt::PromptStatus::Loading { .. }
        )
    {
        lines.push(Line::styled("Loading this prompt page…", label_style()));
    }
    lines.push(Line::from(navigator.hints_for(area.width)));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), footer);
}

fn draw_tabs(frame: &mut Frame, area: ratatui::layout::Rect, state: State) {
    let active_index = Tab::ORDER
        .iter()
        .position(|tab| *tab == state.tab)
        .unwrap_or(0);
    let mut line = Vec::new();
    let mut used = 0;
    for (index, tab) in Tab::ORDER.into_iter().enumerate() {
        let active = tab == state.tab;
        let mut style = if active {
            selection_style()
        } else {
            Style::default()
        };
        if active && !state.tabs_focused {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        let marker = if active && state.tabs_focused {
            "›"
        } else {
            " "
        };
        let span = Span::styled(format!("{marker}{}  ", tab.label()), style);
        if index > active_index && used + span.width() > usize::from(area.width) {
            break;
        }
        used += span.width();
        line.push(span);
        while used > usize::from(area.width) && line.len() > 1 {
            used -= line.remove(0).width();
        }
    }
    frame.render_widget(Paragraph::new(Line::from(line)), area);
}

fn project_label(session: &RecordedSession) -> String {
    if session.project_origin.is_none() {
        return "No recorded project".into();
    }
    let path = session.project_path.as_deref().unwrap_or("");
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn active_inspector_section_stays_visible_at_the_minimum_width() {
        for tab in Tab::ORDER {
            let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 1)).unwrap();
            terminal
                .draw(|frame| {
                    draw_tabs(
                        frame,
                        frame.area(),
                        State {
                            tab,
                            tabs_focused: true,
                        },
                    )
                })
                .unwrap();
            let text = terminal.backend().to_string();
            assert!(text.contains(&format!("›{}", tab.label())), "{text}");
        }
    }
}
