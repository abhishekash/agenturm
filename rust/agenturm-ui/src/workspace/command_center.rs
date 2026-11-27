//! Full-width task navigation; inspectors open through the normal history stack.
use super::*;

const FILTERS: [Filter; 5] = [
    Filter::All,
    Filter::Attention,
    Filter::Active,
    Filter::Finished,
    Filter::Inactive,
];
const COMMAND_CENTER_MIN_WIDTH: u16 = 60;
const COMMAND_CENTER_MIN_HEIGHT: u16 = 18;

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
    let mut label = if row
        .worker_id
        .as_deref()
        .is_some_and(|worker| !worker.trim().is_empty())
    {
        format!("{provider} worker")
    } else {
        format!("{provider} session")
    };
    if let Some(role) = row
        .native_context
        .as_deref()
        .and_then(|context| context.agent_role.as_deref())
        .filter(|value| {
            !value.trim().is_empty()
                && *value != provider
                && !(row.worker_id.is_some() && value.eq_ignore_ascii_case("worker"))
        })
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
    let has_specific_name = row
        .task_title
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || row
            .native_context
            .as_deref()
            .and_then(|context| context.agent_name.as_deref())
            .is_some_and(|value| !value.trim().is_empty());
    let mut parts = Vec::new();
    if has_specific_name
        && let Some(value) = row
            .native_context
            .as_deref()
            .and_then(|context| context.agent_name.as_deref())
            .filter(|value| !value.is_empty())
        && name(row).as_ref() != value
    {
        parts.push(visible_text(value));
    }
    if row
        .task_title
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && let Some(activity) = row
            .native_context
            .as_deref()
            .and_then(|context| context.last_activity.as_deref())
            .filter(|value| !value.trim().is_empty())
    {
        parts.push(visible_text(activity));
    }
    if duplicate {
        parts.push("Same name".into());
    }
    if has_specific_name
        && let Some(value) = row
            .native_context
            .as_deref()
            .and_then(|context| context.agent_role.as_deref())
            .filter(|value| !value.is_empty())
    {
        parts.push(visible_text(value));
    }
    if has_specific_name && grouping != Grouping::Project && row.project_origin.is_some() {
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
    let mut time = Vec::new();
    let time_chip = |label: &str, value: String, active: bool| {
        (
            if active {
                format!(" [{label} {value}] ")
            } else {
                format!(" {label} {value} ")
            },
            if active {
                active_control_style()
            } else {
                theme::muted()
            },
        )
    };
    time.push(time_chip(
        "T Today",
        count(today),
        agents.time_scope == TimeScope::Today,
    ));
    time.push(time_chip(
        "O History",
        count(history),
        agents.time_scope == TimeScope::All,
    ));
    let mut spans = Vec::new();
    for (text, style) in time {
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    let time_width = spans.iter().map(Span::width).sum::<usize>();
    let grouping = format!("G Group: {}", agents.grouping.label());
    let gap = usize::from(width).saturating_sub(time_width + grouping.len() + 1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(grouping, theme::muted()));
    let mut lines = vec![Line::from(spans)];
    let mut chips = Vec::new();
    // Small terminals show the active state explicitly. Tab still cycles the
    // complete filter inventory; zero-value counters never crowd out tasks.
    let filters = if width < 90 {
        vec![agents.filter]
    } else {
        let mut filters = FILTERS.to_vec();
        if agents.filter == Filter::ToReview {
            filters.push(Filter::ToReview);
        }
        filters
    };
    for filter in filters {
        let matching = sessions
            .iter()
            .filter(|row| in_time(row) && filter.accepts(row))
            .count();
        chips.push((
            if filter == agents.filter {
                format!(" [{} {}] ", filter.label(), count(matching))
            } else {
                format!(" {} {} ", filter.label(), count(matching))
            },
            if filter == agents.filter {
                active_control_style()
            } else {
                theme::muted()
            },
        ));
    }
    if width < 90 {
        chips.push((" Tab change ".into(), theme::muted()));
    }
    let errors = sessions
        .iter()
        .filter(|row| in_time(row) && errors_only(row))
        .count();
    if errors > 0 || agents.errors_only {
        chips.push((
            format!(" E Errors {} ", count(errors)),
            if agents.errors_only {
                active_control_style()
            } else {
                theme::status("Error")
            },
        ));
    }
    let mut spans = vec![Span::styled(" State ", theme::muted())];
    let mut occupied = 7;
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
    if area.width < COMMAND_CENTER_MIN_WIDTH || area.height < COMMAND_CENTER_MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · need 60×18 or larger · q quit").centered(),
            area,
        );
        return;
    }
    let lines = filter_lines(agents, area.width);
    // 120 columns is wide enough for the 64/36 master-detail split while
    // preserving a useful task column. Keep the inspector out of short
    // terminals, where it would compete with the roster for vertical space.
    let show_task_detail = area.width >= 118 && area.height >= 24;
    let [brand, navigation, filters, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(lines.len() as u16),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);
    let [identity, appearance] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(17)]).areas(brand);
    frame.render_widget(
        Paragraph::new("F2 Themes  ? Help")
            .right_aligned()
            .style(theme::muted()),
        appearance,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Agenturm",
                theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" command center", theme::muted()),
        ])),
        identity,
    );
    frame.render_widget(
        Paragraph::new(lines).style(surface(theme::palette().surface)),
        filters,
    );
    let mut navigation_spans = Vec::new();
    let shown = if area.width >= 100 { 7 } else { 3 };
    for (index, (key, _, label)) in keymap::NAVIGATION[..shown].iter().enumerate() {
        if index == 3 {
            navigation_spans.push(Span::styled(" │ ", theme::muted()));
        } else if index > 0 {
            navigation_spans.push(Span::raw("  "));
        }
        navigation_spans.push(Span::styled(
            if index == 0 {
                format!(" [{key} {label}] ")
            } else {
                format!(" {key} {label} ")
            },
            if index == 0 {
                active_control_style()
            } else {
                theme::muted()
            },
        ));
    }
    if shown < 7 {
        navigation_spans.push(Span::styled("   : More views", theme::muted()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(navigation_spans)).block(contour(" Workspace ")),
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
    let (roster_area, detail_area) = if show_task_detail {
        let [roster, _, detail] = Layout::horizontal([
            Constraint::Fill(2),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body);
        (roster, Some(detail))
    } else {
        (body, None)
    };
    let has_more = agents
        .catalog
        .as_ref()
        .is_some_and(|catalog| catalog.older.is_some());
    let mixed_providers = sessions.is_some_and(|rows| {
        rows.first()
            .is_some_and(|first| rows.iter().any(|row| provider(row) != provider(first)))
    });
    let roster_title = sessions
        .and_then(|rows| rows.first())
        .filter(|_| !mixed_providers)
        .map_or_else(
            || " Tasks ".to_owned(),
            |row| format!(" Tasks · {} ", provider(row)),
        );
    let roster_panel = contour(&roster_title).title_bottom(Line::styled(
        if agents.editing || !agents.search.is_empty() {
            format!(
                " / {}{} ",
                visible_text(&agents.search),
                if agents.editing { "▏" } else { "" }
            )
        } else {
            format!(
                " {} shown{} · recorded states ",
                agents.visible.len(),
                if has_more { " · more available" } else { "" }
            )
        },
        theme::muted(),
    ));
    let table_area = roster_panel.inner(roster_area);
    frame.render_widget(roster_panel, roster_area);
    let show_provider = mixed_providers;
    // In a narrow mixed-provider list, identity wins over recency. The exact
    // update time remains available in the task inspector.
    let show_updated = !mixed_providers || table_area.width >= 72;
    let mut rows = Vec::new();
    let mut selected_display = None;
    let mut previous_project = None;
    let fixed_width = 13 + if show_provider { 11 } else { 0 } + if show_updated { 9 } else { 0 };
    let task_width = table_area.width.saturating_sub(fixed_width);
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
                    roster_cell(
                        &format!(" {} · {count}", group_label(group_name, agents)),
                        task_width,
                    ),
                    String::new(),
                    String::new(),
                    String::new(),
                ])
                .style(theme::muted().add_modifier(Modifier::BOLD)),
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
        } else {
            " "
        };
        let indent = "  ".repeat(item.depth.min(4));
        let text = if item.has_children {
            format!("{indent}{marker} {}", visible_text(&name(row)))
        } else {
            format!("{indent}  {}", visible_text(&name(row)))
        };
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
        let mut title = vec![Line::styled(
            roster_cell(&text, task_width),
            theme::foreground(theme::palette().text),
        )];
        if !supporting.is_empty() {
            title.push(Line::styled(
                roster_cell(&format!("{indent}  {supporting}"), task_width),
                theme::muted(),
            ));
        }
        let height = title.len() as u16;
        let mut cells = vec![Text::from(title)];
        if show_provider {
            cells.push(Text::styled(roster_cell(provider(row), 10), theme::muted()));
        }
        cells.push(Text::styled(
            status(row),
            if matches!(status(row), "Ready" | "Inactive" | "Unknown") {
                theme::muted()
            } else {
                theme::status(status(row))
            },
        ));
        if show_updated {
            cells.push(Text::styled(
                roster_cell(&age(row.updated_at_unix_ns), 8),
                theme::muted(),
            ));
        }
        rows.push(
            Row::new(cells)
                .height(height)
                .style(surface(theme::palette().surface)),
        );
    }
    if has_more {
        if agents.show_more_selected {
            selected_display = Some(rows.len());
        }
        rows.push(Row::new(["  Show more · Enter / ]", "", ""]).style(label_style()));
    }
    let mut selection = TableState::default()
        .with_selected(selected_display)
        .with_offset(agents.display_offset);
    let mut widths = vec![Constraint::Fill(1)];
    let mut headings = vec![" Task"];
    if show_provider {
        widths.push(Constraint::Length(10));
        headings.push("Agent");
    }
    widths.push(Constraint::Length(10));
    headings.push("Status");
    if show_updated {
        widths.push(Constraint::Length(8));
        headings.push("Updated↓");
    }
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .header(Row::new(headings).style(theme::muted()).bottom_margin(1))
            .column_spacing(1)
            .highlight_symbol("› ")
            .row_highlight_style(selection_style()),
        table_area,
        &mut selection,
    );
    agents.display_offset = selection.offset();
    let empty_state = agents.visible.is_empty().then(|| match sessions {
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
    });
    if let Some(empty) = empty_state {
        frame.render_widget(
            Paragraph::new(empty).wrap(Wrap { trim: false }),
            Rect::new(
                table_area.x + 1,
                table_area.y + 2,
                table_area.width.saturating_sub(2),
                table_area.height.saturating_sub(2),
            ),
        );
    }
    if let Some(detail_area) = detail_area {
        let detail_session = (!agents.show_more_selected)
            .then(|| agents.current())
            .flatten();
        let detail_empty = empty_state.map(|state| match sessions {
            None if pending => "Session history is loading.",
            None => "Session history is unavailable. Retry the history read.",
            Some([]) => "No sessions have been collected yet.",
            Some(_) if !agents.search.is_empty() => "No tasks match this search.",
            Some(_) => state,
        });
        draw_task_detail(
            frame,
            detail_area,
            detail_session,
            detail_empty,
            agents.show_more_selected,
        );
    }
    let loaded = agents
        .catalog
        .as_ref()
        .map_or(0, |catalog| catalog.sessions.len());
    let mut scope = if agents.paused {
        "Paused · p resume".to_owned()
    } else if pending {
        "Loading session evidence…".to_owned()
    } else {
        format!(
            "{} · {loaded} loaded{}",
            agents.time_scope.label(),
            if has_more { " · more available" } else { "" }
        )
    };
    if agents.session_scope.is_some() {
        scope.push_str(if agents.subtree_scope {
            " · agent + subtree"
        } else {
            " · selected agent"
        });
        if agents.subtree_unsupported > 0 || agents.subtree_outside_page > 0 {
            scope.push_str(" · partial relationships · ? details");
        }
    }
    if agents.errors_only {
        scope.push_str(" · errors only");
    }
    // The first notice line is a concise, typed collection summary. The full
    // notice and scope provenance remain available in the scrollable help.
    let status_line = message.map_or(scope, |message| {
        let brief = visible_text(message.lines().next().unwrap_or(message));
        let brief = if agents.paused {
            format!("Paused · p resume · {brief}")
        } else if pending {
            format!("Refreshing… · {brief}")
        } else {
            brief
        };
        format!(
            "{} · ? details",
            roster_cell(&brief, area.width.saturating_sub(13))
        )
    });
    let hints = if agents.editing {
        "Enter / Esc keep search   Backspace edit"
    } else if !agents.search.is_empty() {
        "Enter inspect   / edit search   a reset   ? help"
    } else if area.width >= 80 {
        "Enter inspect   / search   Tab filter   : commands   ? help"
    } else {
        "Enter inspect   / search   Tab filter   ? help"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(roster_cell(&status_line, area.width), theme::muted()),
            Line::from(hints),
        ]),
        footer,
    );
}

/// One contour per region. There is deliberately no outer application frame.
fn contour(title: &str) -> Block<'_> {
    panel(title)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title_style(theme::muted())
}

fn active_control_style() -> Style {
    theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD)
}

fn preview_lines(text: &str, width: u16, limit: usize) -> Vec<String> {
    let mut lines = theme::wrap_cells(text, usize::from(width));
    if lines.len() > limit {
        lines.truncate(limit);
        if let Some(last) = lines.last_mut() {
            *last = roster_cell(&format!("{last}…"), width);
        }
    }
    lines
}

/// Keep the full path as the grouping key; shorten only its presentation. Add
/// parent components until equal project basenames can be told apart.
fn group_label(key: &str, agents: &AgentList) -> String {
    if agents.grouping != Grouping::Project || key == "No recorded project" {
        return visible_text(key);
    }
    let paths = agents
        .catalog
        .as_ref()
        .map(|catalog| catalog.sessions.as_slice())
        .unwrap_or(&[]);
    let parts = key
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    for depth in 1..=parts.len() {
        let suffix = parts[parts.len() - depth..].join("/");
        let collision = paths.iter().any(|row| {
            let other = project(row);
            other != key && (other == suffix || other.ends_with(&format!("/{suffix}")))
        });
        if !collision {
            return visible_text(&suffix);
        }
    }
    visible_text(key)
}

fn short_path(path: &str, width: u16) -> String {
    let path = visible_text(path);
    if path.cell_width() <= width {
        return path;
    }
    let mut tail = String::new();
    for part in path.split('/').rev() {
        let next = if tail.is_empty() {
            part.to_owned()
        } else {
            format!("{part}/{tail}")
        };
        if next.cell_width() + 2 > width {
            break;
        }
        tail = next;
    }
    if tail.is_empty() {
        roster_cell(&path, width)
    } else {
        format!("…/{tail}")
    }
}

fn draw_task_detail(
    frame: &mut Frame,
    area: Rect,
    session: Option<&RecordedSession>,
    empty_state: Option<&str>,
    show_more: bool,
) {
    let Some(session) = session else {
        let text = if show_more {
            "Older sessions are available. Press Enter to load the next page."
        } else {
            empty_state.unwrap_or("Choose a task to see its details.")
        };
        frame.render_widget(
            Paragraph::new(text)
                .centered()
                .wrap(Wrap { trim: false })
                .block(panel(if show_more {
                    "Older tasks"
                } else {
                    "Task details"
                })),
            area,
        );
        return;
    };

    let block = contour(" Selected task ");
    let inner = block.inner(area).inner(ratatui::layout::Margin::new(1, 0));
    frame.render_widget(block, area);
    let [summary, actions] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(4)]).areas(inner);
    let title = visible_text(&name(session));
    let mut lines = preview_lines(&title, summary.width, 2)
        .into_iter()
        .map(|line| {
            Line::styled(
                line,
                theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(status(session).to_owned(), theme::status(status(session))),
        Span::styled(format!("  ·  {}", provider(session)), theme::muted()),
    ]));
    lines.push(Line::styled(
        format!("Updated {}", age(session.updated_at_unix_ns)),
        theme::muted(),
    ));
    let unseen = session
        .pending_requests
        .iter()
        .filter(|request| !request.seen)
        .count();
    if unseen > 0 {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("{} pending request(s)", unseen),
            theme::status("Needs you"),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("Project", theme::muted()));
    lines.push(Line::from(short_path(project(session), summary.width)));
    if let Some(activity) = session
        .native_context
        .as_deref()
        .and_then(|context| context.last_activity.as_deref())
        .filter(|activity| !activity.trim().is_empty())
        .filter(|_| usize::from(summary.height).saturating_sub(lines.len()) >= 3)
    {
        lines.push(Line::from(""));
        lines.push(Line::styled("Recent activity", theme::muted()));
        lines.extend(
            preview_lines(
                &visible_text(activity),
                summary.width,
                usize::from(summary.height)
                    .saturating_sub(lines.len())
                    .min(3),
            )
            .into_iter()
            .map(Line::from),
        );
    }
    frame.render_widget(Paragraph::new(lines), summary);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("Open task", theme::muted()),
            action_hint("Enter", "Overview & details"),
            action_hint("P", "Prompt history"),
            action_hint("s", "Session activity"),
        ]),
        actions,
    );
}

fn action_hint(key: &str, label: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<7}"),
            theme::foreground(theme::palette().text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label.to_owned(), theme::muted()),
    ])
}
