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
        .unwrap_or("No recorded project")
}

pub(super) fn provider(row: &RecordedSession) -> &str {
    // Presentation fallback only: the leading source family names the adapter,
    // and a per-file or variant suffix does not change the human label. Nothing
    // here routes or selects evidence.
    let source = source_family(&row.harness_id);
    let family = source.strip_prefix("harness.").unwrap_or(source);
    let root = family.split('.').next().unwrap_or(family);
    match root {
        "codex" => "Codex",
        "claude" => "Claude Code",
        "pi" => "Pi",
        "kiro" => "Kiro",
        "opencode" => "OpenCode",
        other => other,
    }
}

pub(super) fn name(row: &RecordedSession) -> std::borrow::Cow<'_, str> {
    if let Some(title) = row
        .task_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return title.into();
    }
    if let Some(nickname) = row
        .native_context
        .as_deref()
        .and_then(|context| context.agent_name.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        return nickname.into();
    }
    // Never render a literal "Name unavailable". Derive the most specific
    // human context the record actually has: provider, role and project.
    let provider = provider(row);
    let provider = if provider.trim().is_empty() {
        "Agent"
    } else {
        provider.trim()
    };
    let mut label = if let Some(worker) = row.worker_id.as_deref().filter(|w| !w.trim().is_empty())
    {
        format!("{provider} worker {worker}")
    } else {
        format!("{provider} session")
    };
    if let Some(role) = row
        .native_context
        .as_deref()
        .and_then(|context| context.agent_role.as_deref())
        .filter(|value| !value.trim().is_empty() && *value != provider)
    {
        label.push_str(&format!(" · {role}"));
    }
    if row.project_origin.is_some()
        && let Some(project) = row
            .project_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_name())
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
    {
        label.push_str(&format!(" · {project}"));
    }
    label.into()
}

fn supporting_context(row: &RecordedSession, grouping: Grouping, duplicate: bool) -> String {
    if row
        .task_title
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && row
            .native_context
            .as_deref()
            .and_then(|context| context.agent_name.as_deref())
            .is_none_or(|value| value.trim().is_empty())
    {
        return String::new();
    }
    let mut parts = vec![provider(row).to_owned()];
    if let Some(value) = row
        .native_context
        .as_deref()
        .and_then(|context| context.agent_name.as_deref())
        .filter(|value| !value.is_empty())
        && name(row).as_ref() != value
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
    let loaded = agents.catalog.is_some();
    let sessions = agents
        .catalog
        .as_ref()
        .map(|catalog| catalog.sessions.as_slice())
        .unwrap_or(&[]);
    let count = |value: usize| {
        if loaded {
            value.to_string()
        } else {
            "—".to_owned()
        }
    };
    let scoped = |row: &RecordedSession| agents.in_identity_scope(row);
    let today = sessions
        .iter()
        .filter(|row| scoped(row))
        .filter(|row| TimeScope::Today.accepts(row, agents.day_start()))
        .count();
    let history = sessions.iter().filter(|row| scoped(row)).count();
    let in_time = |row: &RecordedSession| agents.in_session_scope(row);
    let mut chips: Vec<(String, ratatui::style::Style)> = Vec::new();
    let time_chip = |label: &str, value: String, active: bool| {
        (
            format!("{label} {value}"),
            if active {
                selection_style()
            } else {
                label_style()
            },
        )
    };
    chips.push(time_chip(
        "Today",
        count(today),
        agents.time_scope == TimeScope::Today,
    ));
    chips.push(time_chip(
        "History",
        count(history),
        agents.time_scope == TimeScope::All,
    ));
    for filter in FILTERS {
        let matching = sessions
            .iter()
            .filter(|row| in_time(row) && filter.accepts(row))
            .count();
        chips.push((
            format!("{} {}", filter.label(), count(matching)),
            if filter == agents.filter {
                selection_style()
            } else {
                label_style()
            },
        ));
    }
    if agents.filter == Filter::ToReview {
        let matching = sessions
            .iter()
            .filter(|row| in_time(row) && Filter::ToReview.accepts(row))
            .count();
        chips.push((format!("To review {}", count(matching)), selection_style()));
    }
    let errors = sessions
        .iter()
        .filter(|row| in_time(row) && errors_only(row))
        .count();
    chips.push((
        format!("Errors {}", count(errors)),
        if agents.errors_only {
            selection_style()
        } else {
            label_style()
        },
    ));
    let unknown = sessions
        .iter()
        .filter(|row| in_time(row))
        .filter(|row| !FILTERS[1..].iter().any(|f| f.accepts(row)))
        .count();
    if unknown != 0 {
        chips.push((format!("Unknown {}", count(unknown)), label_style()));
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
        Constraint::Length(4),
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
        // The shell owns the scrollable reference (`F1` / `?`). A direct render
        // of the command center keeps one deterministic pointer instead of a
        // second, drifting help table.
        frame.render_widget(
            Paragraph::new("F1 / ? opens the scrollable cockpit reference · Esc closes")
                .wrap(Wrap { trim: false })
                .block(panel(" Help ")),
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
        let text = format!("{indent}{marker} {}", visible_text(&name(row)));
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
                    Line::styled(
                        roster_cell(&text, task_width),
                        theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        roster_cell(&format!("{indent}  {supporting}"), task_width),
                        theme::muted(),
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
        .header(Row::new([" Tasks", "Status", "Updated↓"]).style(heading_style()))
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
            Some(_) if !agents.search.is_empty() => {
                "No sessions match this search · Enter / Esc keeps it · a resets"
            }
            Some(_) if agents.errors_only => "No recorded failures on this page · E clears",
            Some(_) if agents.time_scope == TimeScope::Today => {
                "Nothing pending today · O History shows earlier work"
            }
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
                format!(
                    "{} · {} · page counts",
                    agents.time_scope.label(),
                    if agents.errors_only {
                        "errors only"
                    } else {
                        "all recorded states"
                    }
                )
            }
        },
        |row| {
            format!(
                "{} · {} · recorded {}",
                visible_text(&name(row)),
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
    let complete = agents
        .catalog
        .as_ref()
        .is_some_and(|catalog| catalog.older.is_none());
    let coverage = if complete {
        "page counts · newest retained page"
    } else {
        "page counts · older sessions not loaded"
    };
    let mut scope_line = format!(
        "{} · {} · {} · recorded lifecycle, not live process state",
        agents.time_scope.label(),
        coverage,
        if agents.errors_only {
            "errors only"
        } else {
            "all states"
        }
    );
    if !agents.search.is_empty() {
        scope_line.push_str(&format!(
            " · {} match{} for /{}",
            agents.visible.len(),
            if agents.visible.len() == 1 { "" } else { "es" },
            visible_text(&agents.search)
        ));
    }
    if let Some((source, id)) = &agents.session_scope {
        let scoped_name = agents
            .catalog
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .sessions
                    .iter()
                    .find(|row| row.harness_id == *source && row.session_id == *id)
            })
            .map(|row| name(row).into_owned())
            .unwrap_or_else(|| "selected session".into());
        scope_line.push_str(&format!(
            " · scope {} · {}",
            if agents.subtree_scope {
                "exact agent + delegated subtree"
            } else {
                "exact agent"
            },
            visible_text(&scoped_name)
        ));
        if agents.subtree_unsupported > 0 {
            scope_line.push_str(&format!(
                " · {} relationship(s) excluded",
                agents.subtree_unsupported
            ));
        }
        if agents.subtree_outside_page > 0 {
            scope_line.push_str(&format!(
                " · {} parent(s) outside page",
                agents.subtree_outside_page
            ));
        }
    }
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
                            None => "No recorded project",
                        },
                        visible_text(project(row))
                    )
                },
            )
    });
    let hints = if agents.editing {
        "Enter / Esc done · Backspace edit"
    } else if !agents.search.is_empty() {
        "Enter / Esc search · a reset · q quit"
    } else if area.width >= 110 {
        "Tab filter · T Today · O History · E Errors · S subtree · Ctrl+P / : commands · Enter inspect · / search · F1 help"
    } else {
        "T Today · O History · E Errors · / search · Enter inspect"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(roster_cell(&identity, area.width), label_style()),
            Line::styled(roster_cell(&scope_line, area.width), theme::muted()),
            Line::from(hints),
            Line::from(detail),
        ]),
        footer,
    );
}
