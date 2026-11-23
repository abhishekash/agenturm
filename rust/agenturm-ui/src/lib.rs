//! Generic terminal rendering for kernel snapshots.
//!
//! The UI consumes data-only panes and never switches on extension IDs. It
//! owns terminal lifecycle and keyboard state, while the app supplies an
//! immutable snapshot prepared outside the render loop.

#![forbid(unsafe_code)]

mod evidence_view;
mod findings;
mod keymap;
mod live_receiver;
mod loaded_view;
pub mod metric_history;
mod operation_table;
mod pane_filter;
mod theme;
pub mod usage_chart;
mod workbench;
mod workspace;
pub use evidence_view::{EvidenceRow, EvidenceSpec, EvidenceTable, EvidenceTables};
pub use findings::{FindingGroup, FindingOccurrence, FindingSource, FindingsPage};
pub use operation_table::{ConfiguredMcpServer, McpInventory, OperationRow, OperationTables};

pub use live_receiver::{
    LiveOwner, LivePage, LiveRequest, LiveResponse, LiveReviewMode, LiveReviewSelection,
    LiveReviewTarget, LiveRow, run_live_receiver, run_live_receiver_with_review,
};
pub use loaded_view::LoadedView;

pub use workbench::evaluation::{
    EvaluationDetailEntry, EvaluationPage, EvaluationRead, EvaluationRow, EvaluationView,
    TraceEvaluationEntry,
};
pub use workbench::investigation::{
    InvestigationKind, InvestigationMatch, InvestigationPage, InvestigationRead,
};
pub use workbench::workflow_resume::{ResumeRuntime, WorkflowResume};

pub use workbench::{
    CurrentHostSample, HandoffEntry, OperationDetailEntry, OperationDetailKind, PromptDelivery,
    PromptEntry, RequestEntry, RequestProof, ReviewNoteEntry, TaskEntry, TelemetrySpanEntry,
    WorkAnchor, WorkCommand, WorkResult,
};

pub use workspace::{
    HistorySelection, McpSearchScope, NativeSessionContext, ProjectOrigin, RecordedSession,
    ReviewVersion, SessionCatalog, SessionCursor, SessionView, WorkspaceRequest, WorkspaceResponse,
    WorkspaceView, run_workspace,
};

use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::{ExtensionId, MetricRow, ObservationPosition, Snapshot};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::io;
use unicode_width::UnicodeWidthChar;

const MIN_WIDTH: u16 = 38;
const MIN_HEIGHT: u16 = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pane {
    id: Option<ExtensionId>,
    title: String,
    rows: Vec<MetricRow>,
}

/// Keyboard and selection state for a rendered kernel snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UiModel {
    panes: Vec<Pane>,
    evidence: EvidenceTables,
    evidence_inspection: Option<evidence_view::Inspection>,
    selected: usize,
    scroll: usize,
    scroll_limit: usize,
    viewport_width: u16,
    viewport_height: u16,
    observations: usize,
    truncated: bool,
    older_cursor: Option<ObservationPosition>,
    help: bool,
    turn_available: bool,
    in_turn: bool,
}

impl UiModel {
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        let older_cursor = if snapshot.observations_truncated {
            snapshot
                .observations
                .first()
                .map(|row| ObservationPosition {
                    observed_at_unix_ns: row.observed_at_unix_ns,
                    harness_id: row.harness_id.as_str().to_owned(),
                    event_id: row.event_id.clone(),
                    revision: row.revision,
                    snapshot_id: snapshot.history_snapshot_id,
                })
        } else {
            None
        };
        let mut panes: Vec<_> = snapshot
            .panes
            .into_iter()
            .map(|(id, pane)| Pane {
                id: Some(id),
                title: visible_text(&pane.title),
                rows: pane.rows,
            })
            .collect();
        if !snapshot.metrics.is_empty() {
            panes.push(Pane {
                id: None,
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
                id: None,
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
            evidence: EvidenceTables::new(),
            evidence_inspection: None,
            selected: 0,
            scroll: 0,
            scroll_limit: 0,
            viewport_width: 0,
            viewport_height: 0,
            observations: snapshot.observations.len(),
            truncated: snapshot.observations_truncated,
            older_cursor,
            help: false,
            turn_available: false,
            in_turn: false,
        }
    }

    pub fn older_cursor(&self) -> Option<&ObservationPosition> {
        self.older_cursor.as_ref()
    }

    pub fn selected_title(&self) -> Option<&str> {
        self.panes
            .get(self.selected)
            .map(|pane| pane.title.as_str())
    }

    fn select_title(&mut self, title: &str) {
        if let Some(index) = self.panes.iter().position(|pane| pane.title == title) {
            self.selected = index;
            self.evidence_inspection = None;
            self.scroll = 0;
            self.update_scroll_limit();
        }
    }

    pub fn select_next(&mut self) {
        if !self.panes.is_empty() {
            self.selected = (self.selected + 1) % self.panes.len();
            self.evidence_inspection = None;
            self.scroll = 0;
            self.update_scroll_limit();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.panes.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.panes.len() - 1);
            self.evidence_inspection = None;
            self.scroll = 0;
            self.update_scroll_limit();
        }
    }

    pub fn set_viewport(&mut self, width: u16, height: u16) {
        let resized = (width, height) != (self.viewport_width, self.viewport_height);
        self.viewport_width = width;
        self.viewport_height = height;
        self.update_scroll_limit();
        if resized && self.evidence_inspection.is_some() {
            let (_, limit) = self.evidence_inspection_bounds();
            if let Some(inspection) = &mut self.evidence_inspection {
                inspection.clamp(limit);
            }
        }
    }

    fn selected_evidence(&self) -> Option<&EvidenceTable> {
        let id = self.panes.get(self.selected)?.id.as_ref()?;
        self.evidence.get(id)
    }

    /// Routes input to the active metadata inspector before regular history keys.
    fn evidence_key(&mut self, key: KeyCode) -> bool {
        if self.help {
            return false;
        }
        if self.evidence_inspection.is_none() {
            if key == KeyCode::Enter {
                self.evidence_inspection = self
                    .selected_evidence()
                    .and_then(|table| table.open_inspection(self.scroll));
            }
            return self.evidence_inspection.is_some();
        }
        match key {
            KeyCode::Esc => self.evidence_inspection = None,
            KeyCode::Char('q' | '?') | KeyCode::F(1) => return false,
            _ => {
                let (page, limit) = self.evidence_inspection_bounds();
                if let Some(inspection) = &mut self.evidence_inspection {
                    inspection.key(key, page, limit);
                }
            }
        }
        true
    }

    fn evidence_inspection_bounds(&self) -> (usize, usize) {
        let page = usize::from(self.viewport_height.saturating_sub(6).max(1));
        let limit = self.selected_evidence().map_or(0, |table| {
            table.inspection_limit(self.scroll, self.viewport_width, page)
        });
        (page, limit)
    }

    fn update_scroll_limit(&mut self) {
        if let Some(table) = self.selected_evidence() {
            self.scroll_limit = table.rows.len().saturating_sub(1);
            self.scroll = self.scroll.min(self.scroll_limit);
            return;
        }
        let Some(pane) = self.panes.get(self.selected) else {
            self.scroll = 0;
            self.scroll_limit = 0;
            return;
        };
        let visible_lines = usize::from(self.viewport_height.saturating_sub(8).max(1));
        self.scroll_limit = pane_lines(pane, self.viewport_width.saturating_sub(2))
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

    fn select_metrics(&mut self) {
        self.selected = self
            .panes
            .iter()
            .position(|pane| pane.id.is_none() && pane.title == "Metrics")
            .unwrap_or_else(|| {
                self.panes.push(Pane {
                    id: None,
                    title: "Metrics".into(),
                    rows: Vec::new(),
                });
                self.panes.len() - 1
            });
        self.scroll = 0;
        self.update_scroll_limit();
    }

    /// Applies one key press. Returns true when the user requests exit.
    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if keymap::is_help(key) || (self.help && key == KeyCode::Esc) {
            self.help = !self.help;
            return false;
        }
        if self.help {
            return key == KeyCode::Char('q');
        }
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
            _ => false,
        }
    }
}

/// Runs the terminal UI and restores the terminal on every return path.
pub fn run(mut snapshot: Snapshot) -> io::Result<()> {
    snapshot.observations_truncated = false;
    run_with_history(snapshot, |_| Ok(None))
}

/// Host-provided identity and lifecycle for a selectable recorded turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedTurn {
    pub harness_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub worker_id: Option<String>,
    pub snapshot_id: ReadSnapshot,
    pub lifecycle: Option<(String, Option<String>)>,
}

#[derive(Default)]
pub struct HistoryFeatures {
    /// Ordered continuation of the unfiltered history page.
    pub continuation: Option<Box<ObservationPosition>>,
    pub metric_history: Option<Box<agenturm_kernel::retained_history::MetricHistoryPage>>,
    pub usage_history: Option<Box<usage_chart::UsageChartData>>,
    pub findings: Option<(ExtensionId, FindingsPage)>,
    pub mcp_inventory: Option<McpInventory>,
}

pub struct HistoryPage {
    pub features: HistoryFeatures,
    pub snapshot: Snapshot,
    pub history_identity: Option<String>,
    pub turns: Vec<RecordedTurn>,
    pub initial_pane: Option<ExtensionId>,
    pub review_pane: Option<ExtensionId>,
    pub operation_details: Vec<OperationDetailEntry>,
    pub private_telemetry_history: Option<String>,
    pub operation_tables: OperationTables,
    pub evidence_tables: EvidenceTables,
}

struct PageView {
    snapshot_id: ReadSnapshot,
    history_identity: Option<String>,
    ui: UiModel,
    turns: Vec<RecordedTurn>,
    turn_picker: Option<ListState>,
    review_pane: Option<ExtensionId>,
    operation_details: Vec<OperationDetailEntry>,
    operation_table: operation_table::Browser,
    findings: Option<(ExtensionId, findings::Browser)>,
    operation_picker: Option<(OperationDetailKind, ListState)>,
    telemetry_spans: Vec<TelemetrySpanEntry>,
    evaluation_traces: Vec<TraceEvaluationEntry>,
    evaluation_picker: Option<ListState>,
}

impl PageView {
    fn new(page: HistoryPage, in_turn: bool) -> Self {
        let selected = page
            .initial_pane
            .as_ref()
            .and_then(|pane| page.snapshot.panes.keys().position(|id| id == pane));
        let telemetry_spans = if let Some(history_identity) = &page.private_telemetry_history {
            page.snapshot
                .observations
                .iter()
                .rev()
                .filter_map(|observation| {
                    TelemetrySpanEntry::from_observation(
                        observation,
                        page.snapshot.history_snapshot_id,
                        history_identity,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let evaluation_traces = workbench::evaluation::trace_entries(&page.snapshot);
        let snapshot_id = page.snapshot.history_snapshot_id;
        let mut ui = UiModel::from_snapshot(page.snapshot);
        if ui.truncated
            && let Some(continuation) = page.features.continuation
            && continuation.snapshot_id == snapshot_id
        {
            ui.older_cursor = Some(*continuation);
        }
        ui.evidence = page.evidence_tables;
        if let Some(selected) = selected {
            ui.selected = selected;
        }
        ui.in_turn = in_turn;
        ui.turn_available = !in_turn && !page.turns.is_empty();
        Self {
            snapshot_id,
            history_identity: page.history_identity,
            ui,
            turns: page.turns,
            turn_picker: None,
            review_pane: page.review_pane,
            operation_details: page.operation_details,
            operation_table: operation_table::Browser::new(
                page.operation_tables,
                page.features.mcp_inventory,
            ),
            findings: page
                .features
                .findings
                .map(|(id, data)| (id, findings::Browser::new(data))),
            operation_picker: None,
            telemetry_spans,
            evaluation_traces,
            evaluation_picker: None,
        }
    }
    fn inherit_filters(&mut self, previous: &Self) {
        self.operation_table
            .inherit_filters(&previous.operation_table);
        if let (Some((id, next)), Some((previous_id, prior))) =
            (&mut self.findings, &previous.findings)
            && id == previous_id
        {
            next.set_filter(prior.filter());
        }
    }

    fn active_findings(&mut self) -> Option<&mut findings::Browser> {
        let selected = self.ui.panes.get(self.ui.selected)?.id.as_ref()?;
        self.findings
            .as_mut()
            .filter(|(id, _)| id == selected)
            .map(|(_, browser)| browser)
    }

    fn reviewing(&self) -> bool {
        self.review_pane.as_ref().is_some_and(|review| {
            self.ui
                .panes
                .get(self.ui.selected)
                .and_then(|pane| pane.id.as_ref())
                == Some(review)
        })
    }
}

/// Runs the inspector with bounded older-page navigation.
pub fn run_with_history(
    snapshot: Snapshot,
    mut load_older: impl FnMut(&ObservationPosition) -> io::Result<Option<Snapshot>>,
) -> io::Result<()> {
    let initial = HistoryPage {
        features: Default::default(),
        history_identity: None,
        snapshot,
        turns: Vec::new(),
        initial_pane: None,
        review_pane: None,
        operation_details: Vec::new(),
        private_telemetry_history: None,
        operation_tables: Default::default(),
        evidence_tables: Default::default(),
    };
    run_with_turn_activity(initial, |_, before| {
        let Some(cursor) = before else {
            return Ok(None);
        };
        Ok(load_older(cursor)?.map(|snapshot| HistoryPage {
            features: Default::default(),
            history_identity: None,
            snapshot,
            turns: Vec::new(),
            initial_pane: None,
            review_pane: None,
            operation_details: Vec::new(),
            private_telemetry_history: None,
            operation_tables: Default::default(),
            evidence_tables: Default::default(),
        }))
    })
}

/// Navigates immutable, data-only pages supplied by the application.
pub fn run_with_turn_activity(
    initial: HistoryPage,
    mut load_page: impl FnMut(
        Option<&RecordedTurn>,
        Option<&ObservationPosition>,
    ) -> io::Result<Option<HistoryPage>>,
) -> io::Result<()> {
    let mut appearance = theme::Picker::new();
    let mut page = PageView::new(initial, false);
    let mut newer_pages = Vec::<PageView>::new();
    let mut overview = None;
    let mut active_turn = None;
    const MAX_CACHED_NEWER_PAGES: usize = 4;
    ratatui::run(|terminal| {
        loop {
            let area = terminal.size()?;
            page.ui.set_viewport(area.width, area.height);
            terminal.draw(|frame| {
                draw(
                    frame,
                    &page.ui,
                    !newer_pages.is_empty(),
                    page.turn_picker
                        .as_mut()
                        .map(|state| (page.turns.as_slice(), state)),
                );
                appearance.draw(frame);
            })?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if appearance.key(key) {
                    continue;
                }
                if keymap::interrupt(key) {
                    break Ok(());
                }
                let code = keymap::plain(key).unwrap_or_else(|| {
                    if keymap::back(key) {
                        KeyCode::Esc
                    } else {
                        KeyCode::Null
                    }
                });
                if let Some(selection) = &mut page.turn_picker {
                    match code {
                        KeyCode::Char('q') => break Ok(()),
                        KeyCode::Esc => page.turn_picker = None,
                        KeyCode::Down | KeyCode::Char('j') => {
                            let current = selection.selected().unwrap_or_default();
                            selection.select(Some((current + 1).min(page.turns.len() - 1)));
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let current = selection.selected().unwrap_or_default();
                            selection.select(Some(current.saturating_sub(1)));
                        }
                        KeyCode::Enter => {
                            if let Some(turn) = selection
                                .selected()
                                .and_then(|index| page.turns.get(index))
                                .cloned()
                                && let Some(detail) = load_page(Some(&turn), None)?
                            {
                                overview = Some((
                                    std::mem::replace(&mut page, PageView::new(detail, true)),
                                    std::mem::take(&mut newer_pages),
                                ));
                                active_turn = Some(turn);
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if page.ui.evidence_key(code) {
                    continue;
                }
                match code {
                    keymap::OLDER if !page.ui.help => {
                        if let Some(cursor) = page.ui.older_cursor()
                            && let Some(next) = load_page(active_turn.as_ref(), Some(cursor))?
                        {
                            let previous = std::mem::replace(
                                &mut page,
                                PageView::new(next, active_turn.is_some()),
                            );
                            if newer_pages.len() == MAX_CACHED_NEWER_PAGES {
                                newer_pages.remove(0);
                            }
                            newer_pages.push(previous);
                        }
                    }
                    keymap::NEWER if !page.ui.help => {
                        if let Some(newer) = newer_pages.pop() {
                            page = newer;
                        }
                    }
                    keymap::LATEST if !page.ui.help => {
                        if let Some(latest) = load_page(active_turn.as_ref(), None)? {
                            let mut next = PageView::new(latest, active_turn.is_some());
                            if let Some(title) = page.ui.selected_title() {
                                next.ui.select_title(title);
                            }
                            page = next;
                            newer_pages.clear();
                        }
                    }
                    KeyCode::Char('t' | 'T') if page.ui.turn_available && !page.ui.help => {
                        page.turn_picker = Some(ListState::default().with_selected(Some(0)));
                    }
                    KeyCode::Esc if active_turn.is_some() && !page.ui.help => {
                        if let Some((previous, cached)) = overview.take() {
                            page = previous;
                            newer_pages = cached;
                        }
                        active_turn = None;
                    }
                    key if page.ui.handle_key(key) => break Ok(()),
                    _ => {}
                }
            }
        }
    })
}

const HELP_CLOSE_HINT: &str = "Esc, F1 or ? close help · q quit";

fn draw(
    frame: &mut Frame,
    model: &UiModel,
    has_newer_page: bool,
    turn_picker: Option<(&[RecordedTurn], &mut ListState)>,
) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(surface(theme::palette().canvas)),
        area,
    );
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window to inspect Agenturm")
                .centered()
                .style(theme::foreground(theme::palette().primary)),
            area,
        );
        return;
    }

    if !model.help
        && turn_picker.is_none()
        && let Some(table) = model.selected_evidence()
    {
        let [head, body, foot] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(area);
        frame.render_widget(
            Block::default().style(surface(theme::palette().canvas)),
            area,
        );
        evidence_view::header(frame, model, head);
        let keys = if let Some(inspection) = &model.evidence_inspection {
            table.draw_expanded(frame, body, model.scroll, inspection);
            "↑↓ scroll · PgUp/PgDn page · Esc back"
        } else {
            table.draw_with_inspector(frame, body, model.scroll, true);
            "↑↓ select · Enter inspect · ←/→ panes · ? help"
        };
        frame.render_widget(Paragraph::new(keys), foot);
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
        label_style().add_modifier(Modifier::BOLD),
    )];
    header.push(Span::raw(format!(" · {} observations", model.observations)));
    if turn_picker.is_some() {
        header.push(Span::raw(" · Select a turn"));
    } else if model.in_turn {
        header.push(Span::raw(" · Esc back"));
    } else if model.turn_available {
        header.push(Span::raw(" · T Turns"));
    }
    if model.truncated {
        header.push(Span::styled(
            " · older history available",
            theme::foreground(theme::palette().primary),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(header)).block(Block::default().borders(Borders::BOTTOM)),
        rows[0],
    );

    let picking_turn = turn_picker.is_some();
    if let Some((turns, state)) = turn_picker {
        render_turn_picker(frame, turns, state, rows[1]);
    } else if model.panes.is_empty() {
        frame.render_widget(
            Paragraph::new("No panes are registered yet.\nAdd a UI contribution to inspect data.")
                .block(panel("Overview"))
                .wrap(Wrap { trim: true }),
            rows[1],
        );
    } else {
        render_panes(frame, model, rows[1]);
    }

    let footer = if picking_turn {
        "↑/↓ select · Enter open · Esc back"
    } else {
        history_keys(model, has_newer_page)
    };
    frame.render_widget(
        Paragraph::new(footer).style(theme::muted()).centered(),
        rows[2],
    );
}

fn history_keys(model: &UiModel, has_newer_page: bool) -> &'static str {
    if model.help {
        HELP_CLOSE_HINT
    } else if model.older_cursor.is_some() && has_newer_page {
        " ←/→ panes · ↑/↓ scroll · ] older · [ newer · F2 themes · ? help · q quit "
    } else if model.older_cursor.is_some() {
        " ←/→ panes · ↑/↓ scroll · ] older · F2 themes · ? help · q quit "
    } else if has_newer_page {
        " ←/→ panes · ↑/↓ scroll · [ newer · F2 themes · ? help · q quit "
    } else {
        " ←/→ or Tab switch panes · ↑/↓ scroll · F2 themes · ? help · q quit "
    }
}

fn render_turn_picker(
    frame: &mut Frame,
    turns: &[RecordedTurn],
    state: &mut ListState,
    area: Rect,
) {
    let items = turns.iter().map(|turn| {
        let (status, reason) = turn
            .lifecycle
            .as_ref()
            .map_or(("No lifecycle recorded", None), |(status, reason)| {
                (status.as_str(), reason.as_deref())
            });
        let reason = reason.map_or_else(String::new, |reason| format!(" · {reason}"));
        let summary = visible_text(&format!("{status} · {}{reason}", turn.turn_id));
        if area.height < 6 {
            return ListItem::new(summary);
        }
        let worker = turn
            .worker_id
            .as_deref()
            .map_or_else(String::new, |worker| format!(" · worker {worker}"));
        let source = visible_text(&format!(
            "{} · {}{worker}",
            turn.harness_id, turn.session_id
        ));
        ListItem::new(Text::from(vec![Line::from(summary), Line::from(source)]))
    });
    let list = List::new(items)
        .block(panel("Turns on this page"))
        .highlight_style(selection_style())
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, state);
}

fn render_panes(frame: &mut Frame, model: &UiModel, area: Rect) {
    let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    draw_view_bar(frame, model, bar);
    render_selected_pane(frame, model, body);
}

fn draw_view_bar(frame: &mut Frame, model: &UiModel, area: Rect) {
    // Scroll the strip just enough to keep the active pane visible on a split terminal.
    let mut tabs = Vec::new();
    let mut used = 0;
    for (index, pane) in model.panes.iter().enumerate() {
        let style = if index == model.selected {
            selection_style()
        } else {
            Style::default()
        };
        let tab = Span::styled(format!(" {} ", pane.title), style);
        if index > model.selected && used + tab.width() > usize::from(area.width) {
            break;
        }
        used += tab.width();
        tabs.push(tab);
        while index <= model.selected && used > usize::from(area.width) && tabs.len() > 1 {
            used -= tabs.remove(0).width();
        }
    }
    frame.render_widget(Paragraph::new(Line::from(tabs)), area);
}

fn render_selected_pane(frame: &mut Frame, model: &UiModel, area: Rect) {
    let pane = &model.panes[model.selected];
    let content_width = area.width.saturating_sub(2);
    frame.render_widget(
        Paragraph::new(pane_lines(pane, content_width))
            .block(panel(pane.title.as_str()))
            .scroll((model.scroll as u16, 0)),
        area,
    );
}

fn pane_lines(pane: &Pane, width: u16) -> Vec<Line<'static>> {
    if pane.rows.is_empty() {
        return vec![Line::from(Span::styled(
            "No data in this view yet",
            theme::muted(),
        ))];
    }
    let width = usize::from(width.max(1));
    let mut output = Vec::new();
    for row in &pane.rows {
        let label = visible_text(&row.label);
        let text = format!("{label}  {}", visible_text(&row.value));
        for (index, chunk) in wrap_display_width(&text, width).into_iter().enumerate() {
            if index == 0
                && let Some(value) = chunk.strip_prefix(&label)
            {
                output.push(Line::from(vec![
                    Span::styled(label.clone(), label_style()),
                    Span::raw(value.to_owned()),
                ]));
            } else {
                output.push(Line::from(chunk));
            }
        }
    }
    output
}

fn colors() -> bool {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("NO_COLOR").is_none());
    *ENABLED
}

fn surface(background: Color) -> Style {
    if colors() {
        Style::default().fg(theme::palette().text).bg(background)
    } else {
        Style::default()
    }
}

fn heading_style() -> Style {
    surface(theme::palette().panel).add_modifier(Modifier::BOLD)
}

fn label_style() -> Style {
    theme::foreground(theme::palette().secondary)
}

fn selection_style() -> Style {
    if colors() {
        surface(theme::palette().cursor)
            .fg(theme::palette().on_cursor)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .title(title)
        .style(surface(theme::palette().surface))
        .title_style(label_style().add_modifier(Modifier::BOLD))
        .border_style(theme::foreground(theme::palette().border_subtle))
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
    use agenturm_kernel::{
        ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationScope,
        PaneModel, UiContribution,
    };
    use agenturm_lifecycle_extension_example::LifecyclePane;
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
    fn older_history_cursor_is_exact_and_keeps_the_query_snapshot() {
        let mut snapshot = snapshot_with_panes();
        snapshot.observations_truncated = true;
        snapshot.history_snapshot_id = 77.into();
        snapshot.observations.push(agenturm_kernel::Observation {
            harness_id: ExtensionId::parse("harness.codex").unwrap(),
            event_id: "event-1".into(),
            revision: 2,
            identity: agenturm_kernel::ObservationIdentity {
                scope: agenturm_kernel::ObservationScope::Session,
                session_id: Some("session-1".into()),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: 9_007_199_254_740_993,
            kind: agenturm_kernel::ObservationKind::SessionStarted,
        });

        let model = UiModel::from_snapshot(snapshot);
        assert_eq!(
            model.older_cursor(),
            Some(&ObservationPosition {
                observed_at_unix_ns: 9_007_199_254_740_993,
                harness_id: "harness.codex".into(),
                event_id: "event-1".into(),
                revision: 2,
                snapshot_id: 77.into(),
            })
        );
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
        for (width, height) in [(100, 30), (80, 24), (60, 18), (50, 16), (20, 6)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut model = UiModel::from_snapshot(snapshot_with_panes());
            model.set_viewport(width, height);
            terminal
                .draw(|frame| draw(frame, &model, false, None))
                .unwrap();
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

    #[test]
    fn diagnostic_evidence_remains_readable_at_narrow_terminal_width() {
        let mut snapshot = snapshot_with_panes();
        snapshot
            .panes
            .get_mut(&ExtensionId::parse("pane.activity").unwrap())
            .unwrap()
            .rows = vec![MetricRow {
            label: "Latest diagnostic".into(),
            value: "unsupported_claude_stop_reason · error · harness.claude · session session-123456789"
                .into(),
        }];
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut model = UiModel::from_snapshot(snapshot);
        model.set_viewport(60, 18);
        terminal
            .draw(|frame| draw(frame, &model, false, None))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Latest diagnostic"));
        assert!(text.contains("unsupported_claude_stop_reason"));
        assert!(text.contains("session-123456789"));
    }

    #[test]
    fn independently_packaged_pane_renders_generically_at_standard_and_narrow_sizes() {
        let mut snapshot = snapshot_with_panes();
        snapshot.observations.push(Observation {
            harness_id: ExtensionId::parse("harness.fixture").unwrap(),
            event_id: "lifecycle-1".into(),
            revision: 0,
            identity: ObservationIdentity {
                scope: ObservationScope::Session,
                session_id: Some("session-1".into()),
                worker_id: None,
                turn_id: None,
                operation_id: None,
            },
            observed_at_unix_ns: 1,
            kind: ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: Some("adapter_stopped".into()),
            },
        });
        let pane = LifecyclePane.build_pane(&snapshot).unwrap();
        snapshot
            .panes
            .insert(ExtensionId::parse("pane.example.lifecycle").unwrap(), pane);

        for (width, height) in [(80, 24), (60, 18)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut model = UiModel::from_snapshot(snapshot.clone());
            model.select_next();
            assert_eq!(model.selected_title(), Some("Lifecycle by source"));
            model.set_viewport(width, height);
            terminal
                .draw(|frame| draw(frame, &model, false, None))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("Lifecycle by source"));
            assert!(text.contains("harness.fixture"));
            assert!(text.contains("1 total"));
        }
    }
}
