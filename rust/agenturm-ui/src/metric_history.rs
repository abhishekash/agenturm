//! Snapshot-pinned metric catalog, chronological points, and explicit exemplars.

use super::{
    Frame, KeyCode, Layout, LoadedView, MetricRow, Rect, label_style, panel, selection_style,
    theme, visible_text, wrap_display_width,
};
use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::metric::{ExponentialBuckets, MetricData, MetricPoint, MetricSeries};
use agenturm_kernel::retained_history::{
    MetricExemplarTrace, MetricHistoryPage, MetricHistoryQuery, MetricPointEntry,
    MetricPointPosition, MetricSeriesEntry, MetricSeriesKey, MetricSeriesPosition,
};
use ratatui::layout::Constraint;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use std::collections::VecDeque;
use std::fmt::Display;

pub const METRIC_PAGE_SIZE: usize = 32;
const BACK_PAGES: usize = 4;

pub enum MetricInput {
    Read(Box<MetricHistoryQuery>),
    Back,
    Handled,
    Unhandled,
}

struct Page<Row, Position> {
    rows: Vec<Row>,
    next: Option<Position>,
    selection: ListState,
}

impl<Row, Position> Page<Row, Position> {
    fn new(rows: Vec<Row>, next: Option<Position>) -> Self {
        let selection = ListState::default().with_selected((!rows.is_empty()).then_some(0));
        Self {
            rows,
            next,
            selection,
        }
    }

    fn selected(&self) -> Option<&Row> {
        self.rows.get(self.selection.selected()?)
    }

    fn move_selection(&mut self, delta: isize) {
        if let Some(index) = self.selection.selected() {
            self.selection.select(Some(
                index.saturating_add_signed(delta).min(self.rows.len() - 1),
            ));
        }
    }
}

struct Pages<Row, Position> {
    current: Page<Row, Position>,
    previous: VecDeque<Page<Row, Position>>,
}

impl<Row, Position> Pages<Row, Position> {
    fn new(rows: Vec<Row>, next: Option<Position>) -> Self {
        Self {
            current: Page::new(rows, next),
            previous: VecDeque::new(),
        }
    }

    fn advance(&mut self, rows: Vec<Row>, next: Option<Position>) {
        let old = std::mem::replace(&mut self.current, Page::new(rows, next));
        if self.previous.len() == BACK_PAGES {
            self.previous.pop_front();
        }
        self.previous.push_back(old);
    }

    fn back(&mut self) {
        if let Some(previous) = self.previous.pop_back() {
            self.current = previous;
        }
    }

    fn paging_hint(&self) -> &'static str {
        match (self.previous.is_empty(), self.current.next.is_some()) {
            (true, true) => "   ] next page",
            (false, true) => "   [ previous   ] next",
            (false, false) => "   [ previous page",
            (true, false) => "",
        }
    }
}

struct PointHistory {
    series: MetricSeriesKey,
    pages: Pages<MetricPointEntry, MetricPointPosition>,
}

#[derive(Default)]
struct PointDetail {
    exemplar: usize,
    scroll: Scroll,
}

#[derive(Default)]
struct Scroll {
    offset: u16,
    max_offset: u16,
}

impl Scroll {
    fn key(&mut self, key: KeyCode) -> bool {
        let delta = match key {
            KeyCode::Up | KeyCode::Char('k') => -1,
            KeyCode::Down | KeyCode::Char('j') => 1,
            KeyCode::PageUp => -8,
            KeyCode::PageDown => 8,
            _ => return false,
        };
        self.offset = self
            .offset
            .saturating_add_signed(delta)
            .min(self.max_offset);
        true
    }
}

struct TraceDetail {
    evidence: Option<Box<MetricExemplarTrace>>,
    scroll: Scroll,
}

enum PendingRead {
    SeriesPage,
    OpenSeries(MetricSeriesKey),
    PointPage(MetricSeriesKey),
    Exemplar(MetricPointPosition, usize),
}

pub struct MetricBrowser {
    snapshot_id: ReadSnapshot,
    source_id: Option<agenturm_kernel::ExtensionId>,
    catalog: Pages<MetricSeriesEntry, MetricSeriesPosition>,
    history: Option<PointHistory>,
    detail: Option<PointDetail>,
    trace: Option<TraceDetail>,
    pending: Option<PendingRead>,
}

impl MetricBrowser {
    pub fn new(page: MetricHistoryPage) -> Option<Self> {
        Self::scoped(page, None)
    }

    pub fn scoped(
        page: MetricHistoryPage,
        source_id: Option<agenturm_kernel::ExtensionId>,
    ) -> Option<Self> {
        let MetricHistoryPage::Series {
            snapshot_id,
            rows,
            next,
        } = page
        else {
            return None;
        };
        Some(Self {
            snapshot_id,
            source_id,
            catalog: Pages::new(rows, next),
            history: None,
            detail: None,
            trace: None,
            pending: None,
        })
    }

    pub fn cancel_read(&mut self) {
        self.pending = None;
    }

    pub fn apply(&mut self, page: MetricHistoryPage) -> bool {
        let Some(request) = self.pending.take() else {
            return false;
        };
        match (request, page) {
            (
                PendingRead::SeriesPage,
                MetricHistoryPage::Series {
                    snapshot_id,
                    rows,
                    next,
                },
            ) if snapshot_id == self.snapshot_id => self.catalog.advance(rows, next),
            (
                PendingRead::OpenSeries(series),
                MetricHistoryPage::Points {
                    snapshot_id,
                    rows,
                    next,
                },
            ) if self.matches_points(&series, snapshot_id, &rows, next.as_ref()) => {
                self.history = Some(PointHistory {
                    series,
                    pages: Pages::new(rows, next),
                });
            }
            (
                PendingRead::PointPage(series),
                MetricHistoryPage::Points {
                    snapshot_id,
                    rows,
                    next,
                },
            ) if self.matches_points(&series, snapshot_id, &rows, next.as_ref()) => {
                let Some(history) = &mut self.history else {
                    return false;
                };
                history.pages.advance(rows, next);
            }
            (PendingRead::Exemplar(point, index), MetricHistoryPage::Exemplar { trace }) => {
                let Some(current) = self.point() else {
                    return false;
                };
                let Some(exemplar) = current.point.exemplars.get(index) else {
                    return false;
                };
                if current.position != point
                    || self
                        .detail
                        .as_ref()
                        .is_none_or(|detail| detail.exemplar != index)
                    || trace.as_ref().is_some_and(|trace| {
                        trace.snapshot_id != self.snapshot_id
                            || trace.source_id != point.series.source_id
                            || exemplar.trace_id.as_deref() != Some(trace.span.trace_id.as_str())
                            || exemplar.span_id.as_deref() != Some(trace.span.span_id.as_str())
                    })
                {
                    return false;
                }
                self.trace = Some(TraceDetail {
                    evidence: trace,
                    scroll: Scroll::default(),
                });
            }
            _ => return false,
        }
        true
    }

    fn matches_points(
        &self,
        series: &MetricSeriesKey,
        snapshot: ReadSnapshot,
        rows: &[MetricPointEntry],
        next: Option<&MetricPointPosition>,
    ) -> bool {
        let matches = |point: &MetricPointPosition| {
            point.series == *series && point.snapshot_id == self.snapshot_id
        };
        snapshot == self.snapshot_id
            && rows.iter().all(|row| matches(&row.position))
            && next.is_none_or(matches)
    }

    fn point(&self) -> Option<&MetricPointEntry> {
        self.history.as_ref()?.pages.current.selected()
    }

    pub fn key(&mut self, key: KeyCode, loading: bool) -> MetricInput {
        if key == KeyCode::Esc {
            if self.trace.take().is_some()
                || self.detail.take().is_some()
                || self.history.take().is_some()
            {
                self.cancel_read();
                return MetricInput::Handled;
            }
            self.cancel_read();
            return MetricInput::Back;
        }
        if let Some(trace) = &mut self.trace {
            return if trace.scroll.key(key) {
                MetricInput::Handled
            } else {
                MetricInput::Unhandled
            };
        }
        if loading || self.pending.is_some() {
            return match key {
                KeyCode::Enter
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Char('j' | 'k' | 'e' | '[' | ']') => MetricInput::Handled,
                _ => MetricInput::Unhandled,
            };
        }
        if self.detail.is_some() {
            return self.detail_key(key);
        }
        let delta = match key {
            KeyCode::Up | KeyCode::Char('k') => -1,
            KeyCode::Down | KeyCode::Char('j') => 1,
            KeyCode::PageUp => -8,
            KeyCode::PageDown => 8,
            _ => 0,
        };
        if delta != 0 {
            if let Some(history) = &mut self.history {
                history.pages.current.move_selection(delta);
            } else {
                self.catalog.current.move_selection(delta);
            }
            return MetricInput::Handled;
        }
        let request = if let Some(history) = &mut self.history {
            match key {
                KeyCode::Enter => {
                    if history.pages.current.selected().is_some() {
                        self.detail = Some(PointDetail::default());
                    }
                    return MetricInput::Handled;
                }
                KeyCode::Char('[') => {
                    history.pages.back();
                    return MetricInput::Handled;
                }
                KeyCode::Char(']') => history.pages.current.next.clone().map(|before| {
                    (
                        PendingRead::PointPage(history.series.clone()),
                        MetricHistoryQuery::Points {
                            series: history.series.clone(),
                            snapshot_id: self.snapshot_id,
                            before: Some(before),
                            limit: METRIC_PAGE_SIZE,
                        },
                    )
                }),
                _ => return MetricInput::Unhandled,
            }
        } else {
            match key {
                KeyCode::Enter => self.catalog.current.selected().map(|row| {
                    (
                        PendingRead::OpenSeries(row.key.clone()),
                        MetricHistoryQuery::Points {
                            series: row.key.clone(),
                            snapshot_id: self.snapshot_id,
                            before: None,
                            limit: METRIC_PAGE_SIZE,
                        },
                    )
                }),
                KeyCode::Char('[') => {
                    self.catalog.back();
                    return MetricInput::Handled;
                }
                KeyCode::Char(']') => self.catalog.current.next.clone().map(|after| {
                    (
                        PendingRead::SeriesPage,
                        MetricHistoryQuery::Series {
                            source_id: self.source_id.clone(),
                            snapshot_id: Some(self.snapshot_id),
                            after: Some(after),
                            limit: METRIC_PAGE_SIZE,
                        },
                    )
                }),
                _ => return MetricInput::Unhandled,
            }
        };
        match request {
            Some((pending, query)) => self.read(pending, query),
            None => MetricInput::Handled,
        }
    }

    fn detail_key(&mut self, key: KeyCode) -> MetricInput {
        let Some(point) = self.point() else {
            return MetricInput::Unhandled;
        };
        let count = point.point.exemplars.len();
        let detail = self.detail.as_mut().expect("open metric point");
        if detail.scroll.key(key) {
            return MetricInput::Handled;
        }
        match key {
            KeyCode::Char('e') if count > 0 => {
                detail.exemplar = (detail.exemplar + 1) % count;
                detail.scroll.offset = 0;
            }
            KeyCode::Enter if count > 0 => {
                let index = detail.exemplar;
                let point = self.point().expect("open metric point");
                let exemplar = &point.point.exemplars[index];
                if exemplar.trace_id.is_none() || exemplar.span_id.is_none() {
                    self.trace = Some(TraceDetail {
                        evidence: None,
                        scroll: Scroll::default(),
                    });
                    return MetricInput::Handled;
                }
                let position = point.position.clone();
                return self.read(
                    PendingRead::Exemplar(position.clone(), index),
                    MetricHistoryQuery::Exemplar {
                        point: position,
                        exemplar_index: index,
                    },
                );
            }
            KeyCode::Char('e') | KeyCode::Enter => {}
            _ => return MetricInput::Unhandled,
        }
        MetricInput::Handled
    }

    fn read(&mut self, pending: PendingRead, query: MetricHistoryQuery) -> MetricInput {
        self.pending = Some(pending);
        MetricInput::Read(Box::new(query))
    }

    pub(super) fn loaded_view(&self) -> LoadedView {
        let snapshot = format!("Retained metric metadata · Snapshot {}", self.snapshot_id);
        if let Some(trace) = &self.trace {
            let scope = self.point().map_or(snapshot.clone(), |row| {
                format!(
                    "{} / {} / {}",
                    row.position.series.source_id.as_str(),
                    row.position.event_id,
                    snapshot
                )
            });
            return LoadedView {
                title: "Metric exemplar trace".into(),
                scope,
                rows: detail_rows(trace_lines(trace.evidence.as_deref())),
                truncated: false,
            };
        }
        if let Some(detail) = &self.detail
            && let Some(point) = self.point()
        {
            return LoadedView {
                title: "Metric point".into(),
                scope: format!(
                    "{} / {} / {snapshot}",
                    point.position.series.source_id.as_str(),
                    point.position.event_id
                ),
                rows: detail_rows(point_lines(point, detail.exemplar, true)),
                truncated: point.point.exemplars.len() > 1,
            };
        }
        if let Some(history) = &self.history {
            let rows = history
                .pages
                .current
                .rows
                .iter()
                .map(|row| MetricRow {
                    label: timestamp(row.position.observed_at_unix_ns),
                    value: point_summary(&row.point),
                })
                .collect();
            return LoadedView {
                title: "Metric points".into(),
                scope: format!(
                    "{} / {} / {snapshot}",
                    history.series.source_id.as_str(),
                    history.series.series_id
                ),
                rows,
                truncated: !history.pages.previous.is_empty()
                    || history.pages.current.next.is_some(),
            };
        }
        LoadedView {
            title: "Metrics · OTLP series".into(),
            scope: snapshot,
            rows: self
                .catalog
                .current
                .rows
                .iter()
                .map(|row| MetricRow {
                    label: row.series.name.clone(),
                    value: format!(
                        "{} · {} · {} · series {}",
                        row.key.source_id.as_str(),
                        row.series.kind.label(),
                        if row.series.unit.is_empty() {
                            "unit unreported"
                        } else {
                            &row.series.unit
                        },
                        row.series.id
                    ),
                })
                .collect(),
            truncated: !self.catalog.previous.is_empty() || self.catalog.current.next.is_some(),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, pending: bool, status: Option<&str>) {
        let area = frame.area();
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);
        let hint = self.hint();
        let status = status.map(visible_text);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(hint).style(label_style()),
                Line::from(status.unwrap_or_else(|| {
                    if pending || self.pending.is_some() {
                        "Reading pinned metric history…".into()
                    } else {
                        format!("Snapshot {}", self.snapshot_id)
                    }
                })),
                Line::from("B evidence · c copy · x export · q quit").style(label_style()),
            ]),
            footer,
        );
        if let Some(trace) = &mut self.trace {
            let lines = trace_lines(trace.evidence.as_deref());
            render_lines(
                frame,
                body,
                " Exemplar trace ",
                lines,
                Some(&mut trace.scroll),
            );
        } else if let (Some(detail), Some(point)) = (&self.detail, self.point()) {
            let lines = point_lines(point, detail.exemplar, true);
            render_lines(
                frame,
                body,
                " Metric point ",
                lines,
                self.detail.as_mut().map(|detail| &mut detail.scroll),
            );
        } else if let Some(history) = &mut self.history {
            let (list, preview) = split(body);
            let page = &mut history.pages.current;
            let items = page.rows.iter().map(|row| {
                ListItem::new(format!(
                    "{}   {}",
                    timestamp(row.position.observed_at_unix_ns),
                    point_summary(&row.point),
                ))
            });
            if page.rows.is_empty() {
                render_lines(
                    frame,
                    list,
                    " Metric points ",
                    vec!["No retained points.".into()],
                    None,
                );
            } else {
                frame.render_stateful_widget(
                    List::new(items)
                        .block(panel(" Metric points  ·  newest first "))
                        .highlight_symbol("›  ")
                        .highlight_style(selection_style()),
                    list,
                    &mut page.selection,
                );
            }
            if let Some(area) = preview {
                let rows = page
                    .selected()
                    .map_or_else(Vec::new, |row| point_lines(row, 0, false));
                render_lines(frame, area, " Selected point ", rows, None);
            }
        } else {
            let (list, preview) = split(body);
            let page = &mut self.catalog.current;
            let items = page.rows.iter().map(|row| {
                ListItem::new(Line::from(vec![
                    Span::raw(visible_text(&row.series.name)),
                    Span::styled(
                        format!(
                            "  {}  {}",
                            row.series.kind.label(),
                            row.key.source_id.as_str()
                        ),
                        label_style(),
                    ),
                ]))
            });
            if page.rows.is_empty() {
                render_lines(
                    frame,
                    list,
                    " Metrics  ·  OTLP series ",
                    vec!["No retained OTLP metric series.".into()],
                    None,
                );
            } else {
                frame.render_stateful_widget(
                    List::new(items)
                        .block(panel(" Metrics  ·  OTLP series "))
                        .highlight_symbol("›  ")
                        .highlight_style(selection_style()),
                    list,
                    &mut page.selection,
                );
            }
            if let Some(area) = preview {
                let rows = page.selected().map_or_else(Vec::new, series_lines);
                render_lines(frame, area, " Selected series ", rows, None);
            }
        }
    }

    fn hint(&self) -> String {
        if self.trace.is_some() {
            "Esc point   ↑↓ scroll".into()
        } else if self.detail.is_some() {
            let extras = if self
                .point()
                .is_some_and(|row| !row.point.exemplars.is_empty())
            {
                "   e next exemplar   Enter trace"
            } else {
                ""
            };
            format!("Esc points   ↑↓ scroll{extras}")
        } else if let Some(history) = &self.history {
            format!(
                "Esc series   ↑↓ select   Enter details{}",
                history.pages.paging_hint()
            )
        } else {
            format!(
                "Esc back   ↑↓ select   Enter points{}",
                self.catalog.paging_hint()
            )
        }
    }
}

fn detail_rows(lines: Vec<String>) -> Vec<MetricRow> {
    lines
        .into_iter()
        .map(|line| {
            let (label, value) = line
                .trim()
                .split_once(' ')
                .map_or(("Detail", line.trim()), |(label, value)| (label, value));
            MetricRow {
                label: label.into(),
                value: value.into(),
            }
        })
        .collect()
}

fn trace_lines(trace: Option<&MetricExemplarTrace>) -> Vec<String> {
    match trace {
        Some(trace) => vec![
            format!("Trace {}", trace.span.trace_id),
            format!("Span {}", trace.span.span_id),
            format!("Source {}", trace.source_id.as_str()),
            format!("Session {}", trace.session_id),
            format!("Turn {}", present(trace.turn_id.as_deref())),
            format!("Worker {}", present(trace.worker_id.as_deref())),
            format!("Started {}", timestamp(trace.span.started_at_unix_ns)),
            format!("Status {:?}", trace.span.status),
        ],
        None => vec![
            "No exact retained trace span for this exemplar.".into(),
            "Both IDs must match a span in the same source and snapshot.".into(),
        ],
    }
}

fn split(area: Rect) -> (Rect, Option<Rect>) {
    if area.width >= 100 {
        let [list, preview] =
            Layout::horizontal([Constraint::Percentage(54), Constraint::Min(1)]).areas(area);
        (list, Some(preview))
    } else {
        (area, None)
    }
}

fn render_lines(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<String>,
    detail: Option<&mut Scroll>,
) {
    let block = panel(title);
    let inner = block.inner(area);
    let rows = lines
        .iter()
        .flat_map(|line| wrap_display_width(line, usize::from(inner.width.max(1))))
        .enumerate()
        .map(|(index, line)| Line::from(line).style(theme::row(index)))
        .collect::<Vec<_>>();
    let scroll = detail.map_or(0, |detail| {
        detail.max_offset = rows.len().saturating_sub(usize::from(inner.height)) as u16;
        detail.offset = detail.offset.min(detail.max_offset);
        detail.offset
    });
    frame.render_widget(Paragraph::new(rows).block(block).scroll((scroll, 0)), area);
}

fn timestamp(nanos: u64) -> String {
    format!(
        "Unix {}.{:09}",
        nanos / 1_000_000_000,
        nanos % 1_000_000_000
    )
}

fn present<T: Display>(value: Option<T>) -> String {
    value.map_or_else(|| "Unreported".into(), |value| value.to_string())
}

fn series_descriptor(series: &MetricSeries, source: &agenturm_kernel::ExtensionId) -> Vec<String> {
    vec![
        visible_text(&series.name),
        format!("Kind {}", series.kind.label()),
        format!(
            "Unit {}",
            if series.unit.is_empty() {
                "Unreported".into()
            } else {
                visible_text(&series.unit)
            }
        ),
        format!("Source {}", source.as_str()),
        format!(
            "Service {}",
            present(series.service.as_deref().map(visible_text))
        ),
        format!(
            "Scope {}",
            present(series.scope.as_deref().map(visible_text))
        ),
        format!("Temporality {}", series.temporality.label()),
        format!("Monotonic {}", present(series.monotonic)),
        format!("Series {}", series.id),
    ]
}

fn series_lines(row: &MetricSeriesEntry) -> Vec<String> {
    let mut lines = series_descriptor(&row.series, &row.key.source_id);
    lines.push(format!("Latest {}", timestamp(row.latest_unix_ns)));
    lines
}

fn point_summary(point: &MetricPoint) -> String {
    let value = match &point.data {
        MetricData::Gauge { value } | MetricData::Sum { value } => present(*value),
        MetricData::Histogram { count, sum, .. }
        | MetricData::ExponentialHistogram { count, sum, .. }
        | MetricData::Summary { count, sum, .. } => {
            format!("count {}   sum {}", present(*count), present(*sum))
        }
    };
    if !point.no_recorded_value() {
        value
    } else {
        format!("No recorded value  {value}")
    }
}

fn point_lines(row: &MetricPointEntry, exemplar: usize, full: bool) -> Vec<String> {
    let point = &row.point;
    let mut lines = vec![
        format!("End {}", timestamp(row.position.observed_at_unix_ns)),
        format!("Start {}", present(point.start_unix_ns.map(timestamp))),
        format!(
            "Flags {}{}",
            point.flags,
            if point.no_recorded_value() {
                "  No recorded value"
            } else {
                ""
            }
        ),
    ];
    if let Some(sample) = point.exemplars.get(exemplar) {
        lines.extend([
            format!(
                "Exemplar {}/{}  Value {}",
                exemplar + 1,
                point.exemplars.len(),
                present(sample.value)
            ),
            format!("  Time {}", timestamp(sample.time_unix_ns)),
            format!("  Trace {}", present(sample.trace_id.as_deref())),
            format!("  Span {}", present(sample.span_id.as_deref())),
        ]);
    }
    match &point.data {
        MetricData::Gauge { value } | MetricData::Sum { value } => {
            lines.push(format!("Value {}", present(*value)));
        }
        MetricData::Histogram {
            count,
            sum,
            min,
            max,
            explicit_bounds,
            bucket_counts,
        } => {
            lines.extend(summary(*count, *sum));
            lines.extend([
                format!("Min {}", present(*min)),
                format!("Max {}", present(*max)),
            ]);
            if full {
                for (index, count) in bucket_counts.iter().enumerate() {
                    let lower = explicit_bounds
                        .get(index.wrapping_sub(1))
                        .map_or_else(|| "−∞".into(), ToString::to_string);
                    let upper = explicit_bounds
                        .get(index)
                        .map_or_else(|| "+∞".into(), ToString::to_string);
                    lines.push(format!("Bucket ({lower}, {upper}]  {count}"));
                }
            }
        }
        MetricData::ExponentialHistogram {
            count,
            sum,
            min,
            max,
            scale,
            zero_count,
            zero_threshold,
            positive,
            negative,
        } => {
            lines.extend(summary(*count, *sum));
            lines.extend([
                format!("Min {}", present(*min)),
                format!("Max {}", present(*max)),
                format!("Scale {}", present(*scale)),
                format!("Zero count {}", present(*zero_count)),
                format!("Zero threshold {}", present(*zero_threshold)),
            ]);
            if full {
                exponential_rows(&mut lines, "Positive", positive.as_ref());
                exponential_rows(&mut lines, "Negative", negative.as_ref());
            }
        }
        MetricData::Summary {
            count,
            sum,
            quantiles,
        } => {
            lines.extend(summary(*count, *sum));
            if full {
                lines.extend(
                    quantiles
                        .iter()
                        .map(|sample| format!("Quantile {}  {}", sample.quantile, sample.value)),
                );
            }
        }
    }
    lines.extend(series_descriptor(
        &point.series,
        &row.position.series.source_id,
    ));
    lines
}

fn summary(count: Option<u64>, sum: Option<agenturm_kernel::metric::MetricFloat>) -> [String; 2] {
    [
        format!("Count {}", present(count)),
        format!("Sum {}", present(sum)),
    ]
}

fn exponential_rows(lines: &mut Vec<String>, title: &str, buckets: Option<&ExponentialBuckets>) {
    if let Some(buckets) = buckets {
        for (index, count) in buckets.bucket_counts.iter().enumerate() {
            let bucket = i64::from(buckets.offset) + index as i64;
            lines.push(format!("{title} bucket {bucket}  {count}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::ExtensionId;
    use agenturm_kernel::metric::{MetricExemplar, MetricKind, MetricNumber, MetricTemporality};
    use ratatui::{Terminal, backend::TestBackend};

    fn render(terminal: &mut Terminal<TestBackend>, browser: &mut MetricBrowser) -> String {
        terminal
            .draw(|frame| browser.draw(frame, false, None))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn metric_browser_keeps_exact_selection_through_pinned_points_and_exemplar() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let snapshot = 10.into();
            let source = ExtensionId::parse("otlp.fixture").unwrap();
            let series = MetricSeries {
                id: "1".repeat(64),
                name: "agent.latency".into(),
                unit: "ms".into(),
                service: Some("fixture".into()),
                scope: None,
                kind: MetricKind::Gauge,
                temporality: MetricTemporality::Unspecified,
                monotonic: None,
            };
            let key = MetricSeriesKey {
                source_id: source,
                series_id: series.id.clone(),
            };
            let mut other = series.clone();
            other.id = "2".repeat(64);
            other.name = "agent.sibling".into();
            let mut browser = MetricBrowser::new(MetricHistoryPage::Series {
                snapshot_id: snapshot,
                rows: vec![
                    MetricSeriesEntry {
                        key: MetricSeriesKey {
                            series_id: other.id.clone(),
                            ..key.clone()
                        },
                        series: other,
                        latest_unix_ns: 15,
                    },
                    MetricSeriesEntry {
                        key: key.clone(),
                        series: series.clone(),
                        latest_unix_ns: 16,
                    },
                ],
                next: None,
            })
            .unwrap();
            assert!(render(&mut terminal, &mut browser).contains("OTLP series"));
            assert!(matches!(
                browser.key(KeyCode::Down, false),
                MetricInput::Handled
            ));
            let MetricInput::Read(query) = browser.key(KeyCode::Enter, false) else {
                panic!("open a selected metric series");
            };
            assert!(matches!(
                *query, MetricHistoryQuery::Points { series, snapshot_id, .. }
                if series == key && snapshot_id == snapshot,
            ));
            let point = MetricPoint {
                series,
                start_unix_ns: None,
                flags: 0,
                data: MetricData::Gauge {
                    value: Some(MetricNumber::Int(0)),
                },
                exemplars: vec![MetricExemplar {
                    time_unix_ns: 15,
                    value: Some(MetricNumber::Int(0)),
                    trace_id: Some("3".repeat(32)),
                    span_id: Some("4".repeat(16)),
                }],
            };
            let position = MetricPointPosition {
                series: key,
                snapshot_id: snapshot,
                observed_at_unix_ns: 16,
                event_id: point.event_id(16),
            };
            let page = MetricHistoryPage::Points {
                snapshot_id: snapshot,
                rows: vec![MetricPointEntry {
                    position: position.clone(),
                    point,
                }],
                next: None,
            };
            assert!(browser.apply(page));
            assert!(render(&mut terminal, &mut browser).contains("Metric points"));
            assert!(matches!(
                browser.key(KeyCode::Enter, false),
                MetricInput::Handled
            ));
            let screen = render(&mut terminal, &mut browser);
            assert!(screen.contains("Value 0"));
            assert!(screen.contains("Exemplar 1/1"));
            assert!(screen.contains("Unit ms"));
            let MetricInput::Read(request) = browser.key(KeyCode::Enter, false) else {
                panic!("resolve exact metric exemplar");
            };
            assert!(matches!(
                *request, MetricHistoryQuery::Exemplar { point, exemplar_index: 0 } if point == position,
            ));
            assert!(browser.apply(MetricHistoryPage::Exemplar { trace: None }));
            assert!(render(&mut terminal, &mut browser).contains("No exact retained trace span"));
            for _ in 0..3 {
                assert!(matches!(
                    browser.key(KeyCode::Esc, false),
                    MetricInput::Handled
                ));
            }
            let screen = render(&mut terminal, &mut browser);
            assert!(screen.contains("›  agent.latency"));
            assert!(matches!(
                browser.key(KeyCode::Esc, false),
                MetricInput::Back
            ));
        }
    }
}
