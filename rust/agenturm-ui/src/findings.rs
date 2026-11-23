//! Page-local findings and exact retained occurrence browsing.

use super::pane_filter::{self, Filter, FilterKey};
use super::{
    MIN_HEIGHT, MIN_WIDTH, heading_style, label_style, panel, selection_style, theme, visible_text,
    wrap_display_width,
};
use agenturm_kernel::{
    ExtensionId, MetricRow, ObservationIdentity, ObservationPosition, ObservationScope,
    ReadSnapshot,
};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, TableState, Wrap},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindingsPage {
    pub groups: Vec<FindingGroup>,
    pub snapshot_id: ReadSnapshot,
    pub scanned: usize,
    pub has_older: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindingGroup {
    pub source_id: ExtensionId,
    pub kind: &'static str,
    pub severity: String,
    pub signal: String,
    pub occurrences: Vec<FindingOccurrence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindingOccurrence {
    pub position: ObservationPosition,
    pub identity: ObservationIdentity,
    pub details: Vec<MetricRow>,
}

#[derive(Clone, Copy)]
enum Level {
    Groups,
    Occurrences,
    Detail,
}

pub(super) struct Browser {
    page: FindingsPage,
    level: Level,
    group: TableState,
    visible: Vec<usize>,
    filter: Filter,
    occurrence: TableState,
    scroll: usize,
    scroll_limit: usize,
    page_height: usize,
}

impl Browser {
    pub(super) fn new(page: FindingsPage) -> Self {
        let selection = (!page.groups.is_empty()).then_some(0);
        Self {
            visible: (0..page.groups.len()).collect(),
            filter: Filter::default(),
            page,
            level: Level::Groups,
            group: TableState::default().with_selected(selection),
            occurrence: TableState::default(),
            scroll: 0,
            scroll_limit: 0,
            page_height: 1,
        }
    }

    fn group(&self) -> Option<&FindingGroup> {
        self.page
            .groups
            .get(*self.visible.get(self.group.selected()?)?)
    }

    pub(super) fn editing_filter(&self) -> bool {
        self.filter.editing()
    }

    pub(super) fn filter(&self) -> Filter {
        self.filter.committed()
    }

    pub(super) fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let previous = self
            .group
            .selected()
            .and_then(|index| self.visible.get(index).copied());
        self.visible = self
            .page
            .groups
            .iter()
            .enumerate()
            .filter_map(|(index, group)| {
                let fields = [
                    group.kind,
                    group.signal.as_str(),
                    group.severity.as_str(),
                    group.source_id.as_str(),
                ];
                let matches = self.filter.matches(fields)
                    || group.occurrences.iter().any(|row| {
                        self.filter.matches(
                            fields
                                .into_iter()
                                .chain(row.identity.session_id.as_deref())
                                .chain(row.identity.turn_id.as_deref())
                                .chain(row.identity.worker_id.as_deref()),
                        )
                    });
                matches.then_some(index)
            })
            .collect();
        self.group =
            TableState::default().with_selected(pane_filter::selection(&self.visible, previous));
    }

    pub(super) fn key(&mut self, code: KeyCode) -> bool {
        if matches!(self.level, Level::Groups) {
            match self.filter.key(code) {
                FilterKey::Changed => {
                    self.apply_filter();
                    return true;
                }
                FilterKey::Handled => return true,
                FilterKey::Ignored => {}
            }
        }
        if code == KeyCode::Enter {
            match self.level {
                Level::Groups
                    if self
                        .group()
                        .is_some_and(|group| !group.occurrences.is_empty()) =>
                {
                    self.occurrence = TableState::default().with_selected(Some(0));
                    self.level = Level::Occurrences;
                }
                Level::Occurrences if self.occurrence.selected().is_some() => {
                    self.scroll = 0;
                    self.level = Level::Detail;
                }
                _ => {}
            }
            return true;
        }
        if code == KeyCode::Esc {
            self.level = match self.level {
                Level::Groups => return false,
                Level::Occurrences => Level::Groups,
                Level::Detail => Level::Occurrences,
            };
            return true;
        }
        let page = self.page_height;
        match self.level {
            Level::Groups => move_cursor(&mut self.group, code, self.visible.len(), page),
            Level::Occurrences => {
                let count = self.group().map_or(0, |group| group.occurrences.len());
                move_cursor(&mut self.occurrence, code, count, page)
            }
            Level::Detail => {
                let Some(scroll) = moved(code, self.scroll, self.scroll_limit, page) else {
                    return false;
                };
                self.scroll = scroll;
                true
            }
        }
    }

    pub(super) fn hints(&self) -> &'static str {
        if let Some(keys) = self.filter.keys() {
            return keys;
        }
        match self.level {
            Level::Groups => "/ search · ↑↓ select · Enter occurrences · [ ] pages",
            Level::Occurrences => "↑↓ select · Enter exact evidence · Esc findings",
            Level::Detail => "↑↓ scroll · PgUp/PgDn · Esc occurrences",
        }
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT.saturating_sub(4) {
            frame.render_widget(
                Paragraph::new("Terminal too small · resize · Esc back"),
                area,
            );
            return;
        }
        let [summary, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let older = if self.page.has_older {
            " · older history available"
        } else {
            ""
        };
        let count = self.page.groups.len();
        let label = if count == 1 { "group" } else { "groups" };
        frame.render_widget(
            Paragraph::new(format!(
                "{count} {label} · {} examined on this page{older}",
                self.page.scanned
            ))
            .style(label_style()),
            summary,
        );
        let body = if matches!(self.level, Level::Groups)
            && let Some(label) = self.filter.label()
        {
            let [query, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(body);
            frame.render_widget(Paragraph::new(label).style(label_style()), query);
            rest
        } else {
            body
        };
        self.page_height = usize::from(body.height.saturating_sub(3)).max(1);
        match self.level {
            Level::Groups => self.draw_groups(frame, body),
            Level::Occurrences => self.draw_occurrences(frame, body),
            Level::Detail => self.draw_detail(frame, body),
        }
    }

    fn draw_groups(&mut self, frame: &mut Frame, area: Rect) {
        if self.visible.is_empty() {
            let empty = if self.page.groups.is_empty() {
                "No findings on this page."
            } else {
                "No matching findings on this page."
            };
            frame.render_widget(Paragraph::new(empty).block(panel(" Findings ")), area);
            return;
        }
        let [list, info] = if area.width >= 96 {
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area)
        } else {
            Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(area)
        };
        let rows = self.visible.iter().enumerate().map(|(index, group)| {
            let group = &self.page.groups[*group];
            Row::new(vec![
                Cell::from(visible_text(&group.severity)).style(theme::status(&group.severity)),
                Cell::from(group.kind),
                Cell::from(visible_text(&group.signal)),
                Cell::from(group.occurrences.len().to_string()),
            ])
            .style(theme::row(index))
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Fill(1),
                Constraint::Length(5),
            ],
        )
        .header(Row::new(["Severity", "Kind", "Signal", "Count"]).style(heading_style()))
        .block(panel(" Findings "))
        .row_highlight_style(selection_style())
        .highlight_symbol("› ");
        frame.render_stateful_widget(table, list, &mut self.group);
        if let Some(group) = self.group() {
            let caption = format!(
                "{} · {}",
                visible_text(group.source_id.as_str()),
                visible_text(&group.signal)
            );
            frame.render_widget(
                Paragraph::new(caption)
                    .wrap(Wrap { trim: true })
                    .block(panel(" Selected finding ")),
                info,
            );
        }
    }

    fn draw_occurrences(&mut self, frame: &mut Frame, area: Rect) {
        let Some(group) = self
            .group
            .selected()
            .and_then(|index| self.visible.get(index))
            .and_then(|index| self.page.groups.get(*index))
        else {
            return;
        };
        let [list, identity] =
            Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(area);
        let rows = group.occurrences.iter().enumerate().map(|(index, row)| {
            let scope = row.identity.session_id.as_deref().unwrap_or("Source scope");
            let turn = row.identity.turn_id.as_deref().unwrap_or("—");
            let worker = row.identity.worker_id.as_deref().unwrap_or("—");
            Row::new(vec![
                visible_text(scope),
                visible_text(turn),
                visible_text(worker),
                row.position.revision.to_string(),
            ])
            .style(theme::row(index))
        });
        let title = format!(
            " {} occurrences · {} ",
            group.occurrences.len(),
            visible_text(&group.signal)
        );
        let table = Table::new(
            rows,
            [
                Constraint::Fill(2),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Length(8),
            ],
        )
        .header(Row::new(["Session", "Turn", "Worker", "Revision"]).style(heading_style()))
        .block(panel(title))
        .row_highlight_style(selection_style())
        .highlight_symbol("› ");
        frame.render_stateful_widget(table, list, &mut self.occurrence);
        if let Some(row) = self
            .occurrence
            .selected()
            .and_then(|index| group.occurrences.get(index))
        {
            frame.render_widget(
                Paragraph::new(visible_text(&row.position.event_id))
                    .block(panel(" Selected event ")),
                identity,
            );
        }
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect) {
        let Some(group) = self.group() else { return };
        let Some(row) = self
            .occurrence
            .selected()
            .and_then(|index| group.occurrences.get(index))
        else {
            return;
        };
        let time = if row.position.observed_at_unix_ns == 0 {
            "Unavailable".into()
        } else {
            row.position.observed_at_unix_ns.to_string()
        };
        let revision = row.position.revision.to_string();
        let snapshot = row.position.snapshot_id.to_string();
        let scope = match row.identity.scope {
            ObservationScope::Source => "Source",
            ObservationScope::Session => "Session",
        };
        let mut fields = vec![
            ("Signal", group.signal.as_str()),
            ("Source", row.position.harness_id.as_str()),
            ("Scope", scope),
            ("Event", row.position.event_id.as_str()),
            ("Revision", revision.as_str()),
            ("Snapshot", snapshot.as_str()),
            ("Observed Unix ns", time.as_str()),
        ];
        for (label, value) in [
            ("Session", row.identity.session_id.as_deref()),
            ("Turn", row.identity.turn_id.as_deref()),
            ("Worker", row.identity.worker_id.as_deref()),
            ("Operation", row.identity.operation_id.as_deref()),
        ] {
            if let Some(value) = value {
                fields.push((label, value));
            }
        }
        fields.extend(
            row.details
                .iter()
                .map(|field| (field.label.as_str(), field.value.as_str())),
        );
        let width = area.width.saturating_sub(2).max(1);
        let height = usize::from(area.height.saturating_sub(2)).max(1);
        let lines = fields
            .into_iter()
            .flat_map(|(label, value)| {
                let value = format!("{label}  {}", visible_text(value));
                wrap_display_width(&value, usize::from(width))
                    .into_iter()
                    .map(move |text| {
                        if let Some(rest) = text.strip_prefix(label) {
                            Line::from(vec![
                                Span::styled(label.to_owned(), label_style()),
                                Span::raw(rest.to_owned()),
                            ])
                        } else {
                            Line::from(text)
                        }
                    })
            })
            .collect::<Vec<_>>();
        self.page_height = height;
        self.scroll_limit = lines.len().saturating_sub(height);
        self.scroll = self.scroll.min(self.scroll_limit);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(self.scroll)
                    .take(height)
                    .collect::<Vec<_>>(),
            )
            .block(panel(" Exact retained occurrence ")),
            area,
        );
    }
}

fn move_cursor(state: &mut TableState, code: KeyCode, count: usize, page: usize) -> bool {
    let Some(selected) = moved(
        code,
        state.selected().unwrap_or(0),
        count.saturating_sub(1),
        page,
    ) else {
        return false;
    };
    state.select((count != 0).then_some(selected));
    true
}

fn moved(code: KeyCode, current: usize, end: usize, page: usize) -> Option<usize> {
    match code {
        KeyCode::Down | KeyCode::Char('j') => Some(current.saturating_add(1).min(end)),
        KeyCode::Up | KeyCode::Char('k') => Some(current.saturating_sub(1)),
        KeyCode::PageDown => Some(current.saturating_add(page).min(end)),
        KeyCode::PageUp => Some(current.saturating_sub(page)),
        KeyCode::Home => Some(0),
        KeyCode::End => Some(end),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn finding_drilldown_keeps_group_occurrence_and_exact_scope_at_three_widths() {
        let source = ExtensionId::parse("harness.codex.fixture").unwrap();
        let one = |event: &str, worker: &str, revision| {
            let mut identity = ObservationIdentity::session("fixture-session");
            identity.turn_id = Some("fixture-turn".into());
            identity.worker_id = Some(worker.into());
            FindingOccurrence {
                position: ObservationPosition {
                    observed_at_unix_ns: 12,
                    harness_id: source.as_str().to_owned(),
                    event_id: event.into(),
                    revision,
                    snapshot_id: 88.into(),
                },
                identity,
                details: vec![MetricRow {
                    label: "Reason".into(),
                    value: "confirmed-fixture".into(),
                }],
            }
        };
        let page = FindingsPage {
            groups: vec![FindingGroup {
                source_id: source.clone(),
                kind: "Lifecycle",
                severity: "error".into(),
                signal: "Recorded fixture failure".into(),
                occurrences: vec![
                    one("fixture-event-a", "worker-a", 1),
                    one("fixture-event-b", "worker-b", 3),
                ],
            }],
            snapshot_id: 88.into(),
            scanned: 7,
            has_older: true,
        };
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut browser = Browser::new(page.clone());
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut render = |browser: &mut Browser| {
                terminal
                    .draw(|frame| browser.draw(frame, frame.area()))
                    .unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .chunks(usize::from(width))
                    .map(|line| line.iter().map(|cell| cell.symbol()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let grouped = render(&mut browser);
            for marker in [
                "1 group · 7 examined on this page",
                "Count",
                "Recorded fixture",
                source.as_str(),
            ] {
                assert!(
                    grouped.contains(marker),
                    "{width}×{height}: {marker}\n{grouped}"
                );
            }
            assert!(
                grouped
                    .lines()
                    .flat_map(|line| line.split('│'))
                    .any(|cells| {
                        cells.contains("Lifecycle") && cells.split_whitespace().last() == Some("2")
                    }),
                "{grouped}"
            );
            assert!(browser.hints().contains("Enter occurrences"));
            assert!(browser.key(KeyCode::Enter));
            assert!(browser.key(KeyCode::Down));
            let listing = render(&mut browser);
            assert!(
                listing.contains("2 occurrences") && listing.contains("worker-b"),
                "{listing}"
            );
            assert!(listing.contains("fixture-event-b"));
            assert!(browser.key(KeyCode::Enter));
            let details = render(&mut browser);
            for marker in [
                "Exact retained occurrence",
                "Event  fixture-event-b",
                "Revision  3",
                "Snapshot  88",
                "fixture-session",
                "fixture-turn",
                "worker-b",
            ] {
                assert!(
                    details.contains(marker),
                    "{width}×{height}: {marker}\n{details}"
                );
            }
            assert!(!details.contains("fixture-event-a"));
            assert!(browser.key(KeyCode::Esc));
            assert!(render(&mut browser).contains("fixture-event-b"));
            assert!(browser.key(KeyCode::Esc));
            assert_eq!(browser.group.selected(), Some(0));
            for key in std::iter::once(KeyCode::Char('/'))
                .chain("worker-a worker-b".chars().map(KeyCode::Char))
                .chain([KeyCode::Enter])
            {
                assert!(browser.key(key));
            }
            assert!(render(&mut browser).contains("No matching findings on this page."));
            for key in [KeyCode::Esc, KeyCode::Char('/')]
                .into_iter()
                .chain("ERROR worker-b".chars().map(KeyCode::Char))
                .chain([KeyCode::Enter])
            {
                assert!(browser.key(key));
            }
            assert!(render(&mut browser).contains("Recorded fixture"));
            assert!(browser.key(KeyCode::Esc));
            assert!(!browser.key(KeyCode::Esc));
        }
    }

    #[test]
    fn empty_findings_keep_enter_and_back_safe() {
        let mut browser = Browser::new(FindingsPage {
            groups: Vec::new(),
            snapshot_id: 0.into(),
            scanned: 0,
            has_older: false,
        });
        assert!(browser.key(KeyCode::Enter));
        assert!(!browser.key(KeyCode::Esc));
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| browser.draw(frame, frame.area()))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("No findings on this page."));
    }
}
