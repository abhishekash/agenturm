//! Full-width task navigation; inspectors open through the normal history stack.
use super::*;

const FILTERS: [Filter; 5] = [
    Filter::All,
    Filter::Attention,
    Filter::Active,
    Filter::Finished,
    Filter::Inactive,
];

pub(super) fn project(row: &RecordedSession) -> &str {
    row.project_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or("Project unavailable")
}

fn status(row: &RecordedSession) -> &str {
    match row.state.as_deref() {
        Some("failed") => "Error",
        Some("interrupted" | "waiting" | "blocked") => "Needs you",
        Some("running" | "started" | "restored") => "Working",
        Some("completed" | "ended") => "Ready",
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
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window"),
            area,
        );
        return;
    }
    let narrow = area.width < 76;
    let [brand, filters, navigation, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if narrow { 2 } else { 1 }),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);
    let [title, group] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(18)]).areas(brand);
    frame.render_widget(
        Paragraph::new(" Agenturm command center").style(heading_style()),
        title,
    );
    frame.render_widget(
        Paragraph::new("Group: Project ")
            .right_aligned()
            .style(label_style()),
        group,
    );
    let filter_spans = |items: &[Filter]| {
        let mut spans = vec![Span::raw(" ")];
        for filter in items {
            let count = agents.catalog.as_ref().map_or("—".into(), |catalog| {
                catalog
                    .sessions
                    .iter()
                    .filter(|row| filter.accepts(row))
                    .count()
                    .to_string()
            });
            spans.push(Span::styled(
                format!(" {} {count} ", filter.label()),
                if *filter == agents.filter {
                    selection_style()
                } else {
                    label_style()
                },
            ));
            spans.push(Span::raw(" "));
        }
        Line::from(spans)
    };
    let lines = if narrow {
        vec![filter_spans(&FILTERS[..3]), filter_spans(&FILTERS[3..])]
    } else {
        vec![filter_spans(&FILTERS)]
    };
    frame.render_widget(Paragraph::new(lines).style(surface(SURFACE)), filters);
    let navigation_text = if agents.editing || !agents.search.is_empty() {
        format!(
            " / {}{}",
            visible_text(&agents.search),
            if agents.editing { "▏" } else { "" }
        )
    } else if area.width >= 110 {
        format!(" {}", keymap::navigation_hints(&keymap::NAVIGATION))
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
            "{}\n{} · H live Host · U Usage\nTab / Shift+Tab Status filter · / Search · a Reset\nEnter Inspect · Esc Back · Ctrl+R Review · P Prompts\nf Tree/flat · ←/→ Collapse/expand · o Sort · p Pause\nAlt+↑/↓ Parent/child · Alt+→ Needs attention\n[ Newer · ] Show more · l Latest · R Refresh\nb Tasks · w Workflow · c Copy · x Export · q Quit",
            keymap::navigation_hints(&keymap::NAVIGATION[..5]),
            keymap::navigation_hints(&keymap::NAVIGATION[5..])
        );
        frame.render_widget(
            Paragraph::new(help)
                .wrap(Wrap { trim: false })
                .block(panel(HELP_CLOSE_HINT)),
            body,
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
        let group = project(row);
        if previous_project != Some(group) {
            let count = agents
                .visible
                .iter()
                .filter(|item| {
                    sessions.is_some_and(|rows| {
                        rows.get(item.index)
                            .is_some_and(|row| project(row) == group)
                    })
                })
                .count();
            rows.push(
                Row::new([
                    format!(
                        " {}  {count}",
                        roster_cell(&visible_text(group), task_width)
                    ),
                    String::new(),
                    String::new(),
                ])
                .style(label_style()),
            );
            previous_project = Some(group);
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
        let text = format!(
            "{indent}{marker} {}",
            visible_text(row.task_title.as_deref().unwrap_or("Untitled session"))
        );
        rows.push(Row::new([
            roster_cell(&text, task_width),
            status(row).into(),
            age(row.updated_at_unix_ns),
        ]));
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
                "{} / {} · {}",
                visible_text(&row.harness_id),
                visible_text(&row.session_id),
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
    let detail = message
        .map(visible_text)
        .unwrap_or_else(|| connection.into());
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
