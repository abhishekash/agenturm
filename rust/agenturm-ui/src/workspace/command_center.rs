//! Full-width task navigation; inspectors open through the normal history stack.
use super::*;

const FILTERS: [Filter; 5] = [
    Filter::All,
    Filter::Attention,
    Filter::Active,
    Filter::Finished,
    Filter::Inactive,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Grouping {
    #[default]
    Project,
    Source,
    None,
}

impl Grouping {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Project => Self::Source,
            Self::Source => Self::None,
            Self::None => Self::Project,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Source => "Source",
            Self::None => "None",
        }
    }
}

pub(super) fn project(row: &RecordedSession) -> &str {
    row.project_path
        .as_deref()
        .filter(|p| row.project_origin.is_some() && !p.trim().is_empty())
        .unwrap_or("Project unavailable")
}

pub(super) fn provider(row: &RecordedSession) -> &str {
    match source_family(&row.harness_id) {
        "harness.codex" => "Codex",
        "harness.claude" => "Claude Code",
        "harness.pi" => "Pi",
        "harness.kiro" => "Kiro",
        "harness.opencode" => "OpenCode",
        source => source.strip_prefix("harness.").unwrap_or(source),
    }
}

pub(super) fn name(row: &RecordedSession) -> &str {
    row.task_title
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Name unavailable")
}

fn supporting_context(row: &RecordedSession, grouping: Grouping, duplicate: bool) -> String {
    let mut parts = vec![provider(row).to_owned()];
    if let Some(value) = row
        .native_context
        .as_deref()
        .and_then(|context| context.agent_name.as_deref())
        .filter(|value| !value.is_empty())
    {
        parts.push(visible_text(value));
    }
    if duplicate {
        parts.push("Same name".into());
    }
    if let Some(value) = row
        .native_context
        .as_deref()
        .and_then(|context| context.agent_role.as_deref())
        .filter(|value| !value.is_empty())
    {
        parts.push(visible_text(value));
    }
    if grouping != Grouping::Project && row.project_origin.is_some() {
        let path = project(row);
        let short = std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path);
        parts.push(visible_text(short));
    }
    parts.join(" · ")
}

pub(super) fn group(row: &RecordedSession, grouping: Grouping) -> &str {
    match grouping {
        Grouping::Project => project(row),
        Grouping::Source => &row.harness_id,
        Grouping::None => "All tasks",
    }
}

fn filter_lines(agents: &AgentList, width: u16) -> Vec<Line<'static>> {
    let mut chips = FILTERS
        .iter()
        .map(|filter| {
            let count = agents.catalog.as_ref().map_or("—".into(), |catalog| {
                catalog
                    .sessions
                    .iter()
                    .filter(|row| filter.accepts(row))
                    .count()
                    .to_string()
            });
            (
                format!("{} {count}", filter.label()),
                if *filter == agents.filter {
                    selection_style()
                } else {
                    label_style()
                },
            )
        })
        .collect::<Vec<_>>();
    if let Some(catalog) = &agents.catalog {
        let unknown = catalog
            .sessions
            .iter()
            .filter(|row| !FILTERS[1..].iter().any(|f| f.accepts(row)))
            .count();
        if unknown != 0 {
            chips.push((format!("Unknown {unknown}"), label_style()));
        }
    }
    let mut lines = Vec::new();
    let mut spans = vec![Span::raw(" ")];
    let mut occupied = 1;
    for (text, style) in chips {
        let cells = text.cell_width() + 1;
        if occupied + cells > width && occupied > 1 {
            lines.push(Line::from(spans));
            spans = vec![Span::raw(" ")];
            occupied = 1;
        }
        occupied += cells;
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    lines.push(Line::from(spans));
    lines
}

fn status(row: &RecordedSession) -> &str {
    if row.pending_requests.iter().any(|request| !request.seen) {
        return "Needs you";
    }
    match row.state.as_deref() {
        Some("failed") => "Error",
        Some("interrupted" | "waiting" | "blocked") => "Needs you",
        Some("running" | "started" | "restored") => "Working",
        Some("completed" | "ended") => "Ready",
        Some("paused") => "Paused",
        Some("idle" | "stopped" | "cancelled" | "closed") => "Inactive",
        Some(_) | None => "Unknown",
    }
}

pub(super) fn draw(
    frame: &mut Frame,
    agents: &mut AgentList,
    pending: bool,
    message: Option<&str>,
) {
    let full = frame.area();
    let area = if full.width >= 80 {
        full.inner(ratatui::layout::Margin::new(1, 0))
    } else {
        full
    };
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window"),
            area,
        );
        return;
    }
    let lines = filter_lines(agents, area.width);
    let [brand, filters, navigation, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(lines.len() as u16),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);
    let [identity, appearance] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(17)]).areas(brand);
    frame.render_widget(
        Paragraph::new("F2 Themes")
            .right_aligned()
            .style(label_style()),
        appearance,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Agenturm command center", heading_style()),
            Span::styled(
                format!("   Group: {} · G", agents.grouping.label()),
                label_style(),
            ),
        ])),
        identity,
    );
    frame.render_widget(
        Paragraph::new(lines).style(surface(theme::palette().surface)),
        filters,
    );
    let navigation_text = if agents.editing || !agents.search.is_empty() {
        format!(
            " / {}{}",
            visible_text(&agents.search),
            if agents.editing { "▏" } else { "" }
        )
    } else if area.width >= 110 {
        format!(" {}", keymap::navigation_hints(keymap::NAVIGATION))
    } else {
        format!(
            " {} · F1 more",
            keymap::navigation_hints(&keymap::NAVIGATION[..5])
        )
    };
    frame.render_widget(
        Paragraph::new(navigation_text).style(label_style()),
        navigation,
    );
    if agents.help {
        let help = format!(
            "{}\n{} · H live Host · U Usage\nTab / Shift+Tab Status filter · / Search · a Reset\nCtrl+F Investigate retained evidence\nEnter Inspect · Esc Back · Ctrl+R / V Review · P Prompts\nX Mark request seen · v Capture availability\nF2 Themes · Ctrl+P Appearance
G Group project/source/none · f Tree/flat · o Sort\n←/→ Collapse/expand · p Pause · Y To review\nAlt+↑/↓ Parent/child · Alt+→ Needs attention\n[ Newer · ] Show more · l Latest · R Refresh\nb Tasks · w Workflow · c Copy · x Export · q Quit",
            keymap::navigation_hints(&keymap::NAVIGATION[..5]),
            keymap::navigation_hints(&keymap::NAVIGATION[5..])
        );
        frame.render_widget(
            Paragraph::new(help)
                .wrap(Wrap { trim: false })
                .block(panel(HELP_CLOSE_HINT)),
            area,
        );
        return;
    }
    let sessions = agents
        .catalog
        .as_ref()
        .map(|catalog| catalog.sessions.as_slice());
    let mut rows = Vec::new();
    let mut selected_display = None;
    let mut previous_project = None;
    let task_width = body.width.saturating_sub(26);
    for (position, item) in agents.visible.iter().enumerate() {
        let Some(row) = sessions.and_then(|rows| rows.get(item.index)) else {
            continue;
        };
        let group_name = group(row, agents.grouping);
        if previous_project != Some(group_name) {
            let count = agents
                .visible
                .iter()
                .filter(|item| {
                    sessions.is_some_and(|rows| {
                        rows.get(item.index)
                            .is_some_and(|row| group(row, agents.grouping) == group_name)
                    })
                })
                .count();
            rows.push(
                Row::new([
                    format!(
                        " {}  {count}",
                        roster_cell(&visible_text(group_name), task_width)
                    ),
                    String::new(),
                    String::new(),
                ])
                .style(label_style()),
            );
            previous_project = Some(group_name);
        }
        if !agents.show_more_selected && agents.selection.selected() == Some(position) {
            selected_display = Some(rows.len());
        }
        let marker = if item.has_children {
            if agents.collapsed.contains(&session_key(row)) {
                "▸"
            } else {
                "▾"
            }
        } else if Filter::Attention.accepts(row) {
            "!"
        } else if Filter::Active.accepts(row) {
            "●"
        } else {
            "○"
        };
        let indent = "  ".repeat(item.depth.min(4));
        let text = format!("{indent}{marker} {}", visible_text(name(row)));
        let duplicate = row.task_title.as_ref().is_some_and(|title| {
            agents
                .visible
                .iter()
                .filter_map(|item| sessions.and_then(|rows| rows.get(item.index)))
                .filter(|other| {
                    other
                        .task_title
                        .as_ref()
                        .is_some_and(|other| other.eq_ignore_ascii_case(title))
                })
                .count()
                > 1
        });
        let supporting = supporting_context(row, agents.grouping, duplicate);
        rows.push(
            Row::new(vec![
                Text::from(vec![
                    Line::styled(roster_cell(&text, task_width), heading_style()),
                    Line::styled(
                        roster_cell(&format!("{indent}  {supporting}"), task_width),
                        label_style(),
                    ),
                ]),
                Text::styled(status(row), theme::status(status(row))),
                Text::styled(age(row.updated_at_unix_ns), theme::muted()),
            ])
            .height(2)
            .style(theme::row(position)),
        );

    }
    let has_more = agents
        .catalog
        .as_ref()
        .is_some_and(|catalog| catalog.older.is_some());
    if has_more {
        if agents.show_more_selected {
            selected_display = Some(rows.len());
        }
        rows.push(Row::new(["  Show more · Enter / ]", "", ""]).style(label_style()));
    }
    let mut selection = TableState::default()
        .with_selected(selected_display)
        .with_offset(agents.display_offset);
    frame.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Fill(1),
                Constraint::Length(11),
                Constraint::Length(9),
            ],
        )
        .header(Row::new([" Tasks", "Status", "Updated"]).style(heading_style()))
        .column_spacing(1)
        .highlight_symbol("› ")
        .row_highlight_style(selection_style()),
        body,
        &mut selection,
    );
    agents.display_offset = selection.offset();
    if agents.visible.is_empty() {
        let empty = match sessions {
            None if pending => "Loading session evidence…",
            None => "Session history unavailable · R retry",
            Some([]) => "No sessions collected yet · R refresh",
            Some(_) => "No tasks match this filter · a reset",
        };
        frame.render_widget(
            Paragraph::new(empty).wrap(Wrap { trim: false }),
            Rect::new(
                body.x + 1,
                body.y + 1,
                body.width.saturating_sub(2),
                body.height.saturating_sub(1),
            ),
        );
    }
    let identity = agents.current().map_or_else(
        || {
            if agents.show_more_selected {
                "Show more · next recorded page".into()
            } else {
                "Counts cover the loaded evidence page".into()
            }
        },
        |row| {
            format!(
                "{} · {} · recorded {}",
                visible_text(name(row)),
                supporting_context(row, agents.grouping, false),
                visible_text(recorded_state(row))
            )
        },
    );
    let connection = if agents.paused {
        "Paused · p resume"
    } else if pending {
        "Loading…"
    } else {
        "Loaded page · R refresh"
    };
    let detail = message.map(visible_text).unwrap_or_else(|| {
        agents
            .current()
            .filter(|row| row.project_origin.is_some() && row.project_path.is_some())
            .map_or_else(
                || connection.into(),
                |row| {
                    format!(
                        "{} · {}",
                        match row.project_origin {
                            Some(ProjectOrigin::OperatorTaskWorkspace) => "Operator task workspace",
                            Some(ProjectOrigin::NativeRecordedWorkspace) =>
                                "Native session workspace",
                            None => "Project unavailable",
                        },
                        visible_text(project(row))
                    )
                },
            )
    });
    let hints = if agents.editing {
        "Enter / Esc done · Backspace edit"
    } else {
        "Tab filter · Enter inspect · / search · F1 help · q quit"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(roster_cell(&identity, area.width), label_style()),
            Line::from(hints),
            Line::from(detail),
        ]),
        footer,
    );
}
