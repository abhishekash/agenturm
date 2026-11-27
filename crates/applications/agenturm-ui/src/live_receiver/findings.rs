//! Bounded, body-free metadata for Python receiver Findings.

use super::*;
use ratatui::widgets::{Cell, Row, Table, TableState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveFindingTarget {
    pub receiver_id: String,
    pub finding_id: String,
    pub producer: Option<String>,
    pub actor: Option<String>,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub turn_id: Option<String>,
    pub members: Vec<LiveFindingMember>,
    pub references_total: usize,
    pub policy_witness: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveFindingMember {
    pub kind: LiveFindingMemberKind,
    pub id: String,
    pub span_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveFindingMemberKind {
    Span,
    Log,
    Outcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveFindingKind {
    SpanError,
    ErrorLog,
    BlockedActivity,
    McpToolError,
    RepeatedToolError,
    IncompleteRun,
    FailedOutcome,
    CredentialExposure,
    RepeatedCompaction,
    RepeatedEquivalentCall,
    CompletionUnresolvedError,
    Other,
}

impl LiveFindingKind {
    fn label(self) -> &'static str {
        match self {
            Self::SpanError => "Span error",
            Self::ErrorLog => "Error log",
            Self::BlockedActivity => "Blocked activity",
            Self::McpToolError => "MCP tool error",
            Self::RepeatedToolError => "Repeated tool error",
            Self::IncompleteRun => "Incomplete run",
            Self::FailedOutcome => "Failed outcome",
            Self::CredentialExposure => "Possible credential exposure",
            Self::RepeatedCompaction => "Repeated compaction",
            Self::RepeatedEquivalentCall => "Repeated equivalent call",
            Self::CompletionUnresolvedError => "Completed with unresolved error",
            Self::Other => "Recorded finding",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveFindingSeverity {
    Info,
    Medium,
    High,
    Other,
}

impl LiveFindingSeverity {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Other => "Unknown",
        }
    }
}

#[derive(Debug)]
pub struct LiveFindingRow {
    pub target: LiveFindingTarget,
    pub kind: LiveFindingKind,
    pub severity: LiveFindingSeverity,
    pub time_unix_ns: u64,
    pub occurrence_count: u64,
    pub trace_count: u64,
    pub session_count: u64,
    pub source_available: bool,
}

#[derive(Debug)]
pub struct LiveFindingPage {
    pub rows: Vec<LiveFindingRow>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum PageNav {
    Current,
    Older,
    Newer,
    Latest,
}

#[derive(Default)]
pub(super) struct Findings {
    page: Option<LiveFindingPage>,
    selection: TableState,
    expanded: bool,
    scroll: usize,
    scroll_limit: usize,
    viewport: usize,
    before: Option<String>,
    previous_pages: Vec<Option<String>>,
    pending: Option<(PageNav, Option<String>)>,
    error: Option<String>,
    selection_changed: bool,
}

impl Findings {
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn auto_refresh(&self) -> bool {
        !self.expanded && self.before.is_none()
    }

    pub(super) fn connection(&self) -> &'static str {
        if self.error.is_some() {
            "Unavailable"
        } else if self.page.is_none() {
            "Loading"
        } else {
            "Connected"
        }
    }

    pub(super) fn begin(&mut self, nav: PageNav) -> Option<LiveRequest> {
        if self.pending() {
            return None;
        }
        let before = match nav {
            PageNav::Current => self.before.clone(),
            PageNav::Older => {
                let next = self.page.as_ref()?.next_cursor.clone()?;
                if self.expanded || self.before.as_ref() == Some(&next) {
                    return None;
                }
                Some(next)
            }
            PageNav::Newer if !self.expanded => self.previous_pages.last()?.clone(),
            PageNav::Newer => return None,
            PageNav::Latest => None,
        };
        self.pending = Some((nav, before.clone()));
        Some(LiveRequest::Findings { before })
    }

    pub(super) fn finish(&mut self, result: Result<LiveFindingPage, String>) {
        let Some((nav, before)) = self.pending.take() else {
            return;
        };
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let prior = self.row().filter(|_| matches!(nav, PageNav::Current));
        let selected = if let Some(prior) = prior {
            page.rows.iter().position(|row| row.target == prior.target)
        } else {
            None
        };
        let selection_changed = prior.is_some() && selected.is_none()
            || matches!(nav, PageNav::Current) && self.selection_changed;
        if selected.is_none() {
            self.expanded = false;
            self.scroll = 0;
        }
        match nav {
            PageNav::Current => {}
            PageNav::Older => {
                const MAX_RETURN_PAGES: usize = 32;
                if self.previous_pages.len() == MAX_RETURN_PAGES {
                    self.previous_pages.remove(0);
                }
                self.previous_pages.push(self.before.take());
            }
            PageNav::Newer => {
                self.previous_pages.pop();
            }
            PageNav::Latest => self.previous_pages.clear(),
        }
        self.before = before;
        self.selection.select(
            selected.or_else(|| (!selection_changed && !page.rows.is_empty()).then_some(0)),
        );
        self.selection_changed = selection_changed;
        self.page = Some(page);
        self.error = None;
    }

    fn row(&self) -> Option<&LiveFindingRow> {
        self.page.as_ref()?.rows.get(self.selection.selected()?)
    }

    pub(super) fn source(&self) -> Option<&LiveFindingTarget> {
        let row = self.row()?;
        (!self.pending() && self.error.is_none() && row.source_available).then_some(&row.target)
    }

    pub(super) fn key(&mut self, key: KeyCode) -> bool {
        if self.expanded {
            match key {
                KeyCode::Esc => self.expanded = false,
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.viewport),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(self.viewport),
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = self.scroll_limit,
                _ => return false,
            }
            self.scroll = self.scroll.min(self.scroll_limit);
            return true;
        }
        let Some(count) = self
            .page
            .as_ref()
            .map(|page| page.rows.len())
            .filter(|&count| count != 0)
        else {
            return false;
        };
        let selected = self.selection.selected();
        let next = match key {
            KeyCode::Enter => {
                if selected.is_none() {
                    self.selection.select(Some(0));
                    self.selection_changed = false;
                }
                self.expanded = true;
                self.scroll = 0;
                return true;
            }
            KeyCode::Up | KeyCode::Char('k') => selected.unwrap_or(0).saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => selected.map_or(0, |row| row.saturating_add(1)),
            KeyCode::PageUp => selected.unwrap_or(0).saturating_sub(self.viewport),
            KeyCode::PageDown => selected.map_or(0, |row| row.saturating_add(self.viewport)),
            KeyCode::Home => 0,
            KeyCode::End => count - 1,
            _ => return false,
        };
        self.selection.select(Some(next.min(count - 1)));
        self.selection_changed = false;
        true
    }

    pub(super) fn draw(
        &mut self,
        frame: &mut Frame,
        body: Rect,
        footer: Rect,
        guarded: bool,
        status: Option<&str>,
    ) {
        if self.expanded
            && let Some(row) = self.row()
        {
            let width = usize::from(body.width.saturating_sub(2).max(1));
            let lines = details(row)
                .iter()
                .flat_map(|line| wrap_display_width(line, width))
                .collect::<Vec<_>>();
            self.viewport = usize::from(body.height.saturating_sub(2).max(1));
            self.scroll_limit = lines.len().saturating_sub(self.viewport);
            self.scroll = self.scroll.min(self.scroll_limit);
            frame.render_widget(
                Paragraph::new(
                    lines
                        .into_iter()
                        .skip(self.scroll)
                        .take(self.viewport)
                        .map(Line::from)
                        .collect::<Vec<_>>(),
                )
                .block(panel(" Selected finding ")),
                body,
            );
        } else if let Some(page) = &self.page
            && !page.rows.is_empty()
        {
            let rows = page.rows.iter().enumerate().map(|(index, row)| {
                Row::new([
                    Cell::from(row.severity.label()),
                    Cell::from(row.kind.label()),
                    Cell::from(row.occurrence_count.to_string()),
                ])
                .style(theme::row(index))
            });
            self.viewport = usize::from(body.height.saturating_sub(3).max(1));
            frame.render_stateful_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Length(8),
                        Constraint::Fill(1),
                        Constraint::Length(11),
                    ],
                )
                .header(Row::new(["Severity", "Kind", "Occurrences"]).style(heading_style()))
                .block(panel(" Findings "))
                .highlight_symbol("› ")
                .row_highlight_style(selection_style()),
                body,
                &mut self.selection,
            );
        } else {
            let empty = if self.pending() || self.page.is_none() && self.error.is_none() {
                "Loading Python receiver Findings…"
            } else if let Some(error) = &self.error {
                error
            } else {
                "No recorded findings on this page."
            };
            frame.render_widget(
                Paragraph::new(visible_text(empty))
                    .wrap(Wrap { trim: true })
                    .block(panel(" Findings ")),
                body,
            );
        }
        let coverage = if self.pending() {
            "Loading Python receiver Findings…"
        } else if let Some(error) = &self.error {
            error
        } else if self.selection_changed {
            "Selected finding changed · choose a current row"
        } else if self
            .page
            .as_ref()
            .is_some_and(|page| page.next_cursor.is_some())
        {
            "Loaded findings · older page available"
        } else {
            "Loaded findings"
        };
        let mut keys = if self.expanded {
            "↑↓ scroll · PgUp/PgDn · Esc list".to_owned()
        } else {
            "↑↓ select · Enter inspect · Esc Agents".to_owned()
        };
        if guarded && self.source().is_some() {
            keys.push_str(" · s source");
        }
        let paging = if self.expanded {
            "R refresh · q quit"
        } else {
            "] older · [ newer · l latest · R refresh · q quit"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(visible_text(status.unwrap_or(coverage))),
                Line::from(keys),
                Line::from(paging),
            ]),
            footer,
        );
    }
}

fn details(row: &LiveFindingRow) -> Vec<String> {
    let target = &row.target;
    let mut details = vec![
        format!("{} · {}", row.severity.label(), row.kind.label()),
        format!("Finding  {}", visible_text(&target.finding_id)),
        format!(
            "Occurrences  {} · Traces  {} · Sessions  {}",
            row.occurrence_count, row.trace_count, row.session_count
        ),
        format!("Source references  {}", target.references_total),
    ];
    for (label, value) in [
        ("Producer", target.producer.as_deref()),
        ("Actor", target.actor.as_deref()),
        ("Session", target.session_id.as_deref()),
        ("Trace", target.trace_id.as_deref()),
        ("Turn", target.turn_id.as_deref()),
    ] {
        if let Some(value) = value {
            details.push(format!("{label}  {}", visible_text(value)));
        }
    }
    if row.time_unix_ns != 0 {
        details.push(format!("Recorded Unix ns  {}", row.time_unix_ns));
    }
    if !row.source_available {
        details.push("Protected source unavailable".into());
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn page(ids: &[&str], next: Option<&str>) -> LiveFindingPage {
        LiveFindingPage {
            rows: ids
                .iter()
                .map(|id| LiveFindingRow {
                    target: LiveFindingTarget {
                        receiver_id: "receiver".into(),
                        finding_id: (*id).into(),
                        producer: Some("opencode".into()),
                        actor: Some("actor-fixture".into()),
                        session_id: Some("session-fixture".into()),
                        trace_id: None,
                        turn_id: None,
                        members: Vec::new(),
                        references_total: 1,
                        policy_witness: Some("proof-witness-not-for-display".into()),
                    },
                    kind: LiveFindingKind::ErrorLog,
                    severity: LiveFindingSeverity::High,
                    time_unix_ns: 0,
                    occurrence_count: 16,
                    trace_count: 2,
                    session_count: 1,
                    source_available: true,
                })
                .collect(),
            next_cursor: next.map(str::to_owned),
        }
    }

    #[test]
    fn same_finding_selection_survives_refresh_and_stale_evidence_stays_closed() {
        let mut findings = Findings::default();
        assert!(matches!(
            findings.begin(PageNav::Current),
            Some(LiveRequest::Findings { before: None })
        ));
        findings.finish(Ok(page(&["first", "selected"], Some("older-page"))));
        assert!(findings.key(KeyCode::Down));
        assert!(findings.key(KeyCode::Enter));
        assert_eq!(findings.source().unwrap().finding_id, "selected");
        assert!(!findings.auto_refresh());
        findings.begin(PageNav::Current).unwrap();
        assert!(findings.source().is_none());
        findings.finish(Ok(page(&["selected", "first"], Some("older-page"))));
        assert_eq!(findings.source().unwrap().finding_id, "selected");
        assert!(findings.expanded);
        findings.begin(PageNav::Current).unwrap();
        findings.finish(Err("Receiver identity changed".into()));
        assert!(findings.source().is_none());
        findings.begin(PageNav::Current).unwrap();
        findings.finish(Ok(page(&["first"], Some("older-page"))));
        assert!(findings.source().is_none());
        assert!(!findings.expanded);
        assert!(findings.key(KeyCode::Down));
        assert_eq!(findings.source().unwrap().finding_id, "first");

        assert!(
            matches!(findings.begin(PageNav::Older), Some(LiveRequest::Findings {before: Some(cursor)}) if cursor == "older-page")
        );
        findings.finish(Ok(page(&["older"], None)));
        assert_eq!(findings.source().unwrap().finding_id, "older");
        assert!(matches!(
            findings.begin(PageNav::Newer),
            Some(LiveRequest::Findings { before: None })
        ));
        findings.finish(Ok(page(&["first"], Some("older-page"))));
        assert_eq!(findings.source().unwrap().finding_id, "first");
    }

    #[test]
    fn compact_findings_distinguish_loading_empty_error_and_exact_inspector() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut draw = |findings: &mut Findings| {
                terminal
                    .draw(|frame| {
                        let [body, footer] =
                            Layout::vertical([Constraint::Min(1), Constraint::Length(3)])
                                .areas(frame.area());
                        findings.draw(frame, body, footer, true, None);
                    })
                    .unwrap();
                terminal.backend().to_string()
            };
            let mut findings = Findings::default();
            findings.begin(PageNav::Current).unwrap();
            assert!(draw(&mut findings).contains("Loading Python receiver Findings"));
            findings.finish(Ok(page(&[], None)));
            assert!(draw(&mut findings).contains("No recorded findings"));
            findings.begin(PageNav::Current).unwrap();
            findings.finish(Err("Receiver Findings unavailable".into()));
            assert!(draw(&mut findings).contains("R refresh"));
            assert!(draw(&mut findings).contains("Receiver Findings unavailable"));
            findings.begin(PageNav::Current).unwrap();
            findings.finish(Ok(page(&["exact-record"], None)));
            assert!(draw(&mut findings).contains("Error log"));
            assert!(findings.key(KeyCode::Enter));
            let detail = draw(&mut findings);
            assert!(
                detail.contains("Selected finding") && detail.contains("Finding  exact-record")
            );
            assert!(detail.contains("Occurrences  16") && detail.contains("s source"));
            assert!(!detail.contains("proof-witness-not-for-display"));
            assert!(findings.key(KeyCode::Esc));
            assert_eq!(findings.source().unwrap().finding_id, "exact-record");
        }
    }
}
