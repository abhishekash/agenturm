//! Session inspector sections: Overview, Prompts and Details.
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

/// The three discoverable inspector sections. `Prompts` enters the shared
/// prompt navigator rather than reimplementing a reader here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Tab {
    #[default]
    Overview,
    Prompts,
    Details,
}

impl Tab {
    const ORDER: [Tab; 3] = [Tab::Overview, Tab::Prompts, Tab::Details];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Prompts => "Prompts",
            Self::Details => "Details",
        }
    }

    pub(super) fn from_view(view: SessionView) -> Option<Self> {
        match view {
            SessionView::Overview => Some(Self::Overview),
            SessionView::Details => Some(Self::Details),
            _ => None,
        }
    }

    pub(super) fn view(self) -> Option<SessionView> {
        match self {
            Self::Overview => Some(SessionView::Overview),
            Self::Details => Some(SessionView::Details),
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
    ui: &crate::UiModel,
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

    // One focus stop for the tab strip. The active section is always visible;
    // the focused strip adds a marker so `Enter read` is not a guess.
    let mut line = Vec::new();
    for tab in Tab::ORDER {
        let active = tab == state.tab;
        let mut style = if active {
            selection_style()
        } else {
            Style::default()
        };
        let marker = if active && state.tabs_focused {
            "›"
        } else {
            " "
        };
        if active && !state.tabs_focused {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        line.push(Span::styled(format!(" {marker}{} ", tab.label()), style));
        if tab == Tab::Prompts {
            line.push(Span::styled("▸request nav", theme::muted()));
        }
        line.push(Span::raw("  "));
    }
    line.push(Span::styled(
        if state.tabs_focused {
            "READ: Enter"
        } else {
            "READ: content"
        },
        theme::muted(),
    ));
    frame.render_widget(Paragraph::new(Line::from(line)), tabs);

    crate::render_selected_pane(frame, ui, body);

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
                keymap::id::BACK,
                keymap::id::HELP,
            ])
        )
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
