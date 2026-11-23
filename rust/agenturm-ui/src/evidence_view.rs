//! Tables for the app's already-scoped, metadata-only evidence projections.

use super::{draw_view_bar, heading_style, label_style, panel, selection_style};
use super::{surface, visible_text, wrap_display_width};
use crate::theme;
use agenturm_kernel::{ExtensionId, MetricRow};
use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, HighlightSpacing, Paragraph, Row, Table, TableState};
use std::collections::BTreeMap;

pub type EvidenceTables = BTreeMap<ExtensionId, EvidenceTable>;

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct EvidenceTable {
    pub spec: EvidenceSpec,
    pub rows: Vec<EvidenceRow>,
    pub context: Vec<MetricRow>,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub struct EvidenceSpec {
    pub title: &'static str,
    pub inspector: &'static str,
    pub columns: &'static [(&'static str, u16)],
    pub empty: &'static str,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct EvidenceRow {
    pub cells: Vec<String>,
    pub details: Vec<MetricRow>,
}

impl EvidenceSpec {
    pub fn table(self, rows: Vec<EvidenceRow>) -> EvidenceTable {
        EvidenceTable {
            spec: self,
            rows,
            context: Vec::new(),
        }
    }
}

pub(super) fn header(frame: &mut Frame, model: &super::UiModel, area: Rect) {
    let [brand, tabs] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let coverage = if model.truncated {
        " · partial page"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(format!(
            " Agenturm   ·   {} observations{coverage}",
            model.observations
        ))
        .style(heading_style()),
        brand,
    );
    draw_view_bar(frame, model, tabs);
}

pub(super) fn loading(frame: &mut Frame, title: &str, inspector: &str) {
    let [brand, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(frame.area());
    frame.render_widget(
        Paragraph::new(format!(" Agenturm / {title}")).style(heading_style()),
        brand,
    );
    let (list, detail) = panes(body);
    frame.render_widget(Paragraph::new("Loading history…").block(panel(title)), list);
    frame.render_widget(
        Paragraph::new("Waiting for recorded evidence").block(panel(inspector)),
        detail,
    );
}

fn panes(area: Rect) -> (Rect, Rect) {
    let [list, inspector] = if area.width >= 96 {
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area)
    } else {
        Layout::vertical([
            Constraint::Min(3),
            Constraint::Length((area.height / 2).clamp(3, 10)),
        ])
        .areas(area)
    };
    (list, inspector)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Inspection {
    offset: usize,
}

impl Inspection {
    pub(super) fn clamp(&mut self, limit: usize) {
        self.offset = self.offset.min(limit);
    }

    pub(super) fn key(&mut self, key: KeyCode, page: usize, limit: usize) {
        self.offset = match key {
            KeyCode::PageDown => self.offset.saturating_add(page),
            KeyCode::PageUp => self.offset.saturating_sub(page),
            KeyCode::Down | KeyCode::Char('j') => self.offset.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.offset.saturating_sub(1),
            _ => self.offset,
        }
        .min(limit);
    }
}

impl EvidenceTable {
    fn details(&self, selected: usize) -> impl Iterator<Item = &MetricRow> {
        self.context.iter().chain(
            self.rows
                .get(selected)
                .into_iter()
                .flat_map(|row| row.details.iter()),
        )
    }

    fn has_detail(&self, selected: usize) -> bool {
        self.details(selected).next().is_some()
    }

    pub(super) fn open_inspection(&self, selected: usize) -> Option<Inspection> {
        self.has_detail(selected).then(Inspection::default)
    }

    pub(super) fn inspection_limit(&self, selected: usize, width: u16, page: usize) -> usize {
        detail_lines(self.details(selected), width.saturating_sub(2).max(1))
            .count()
            .saturating_sub(page)
    }

    pub(super) fn draw_expanded(
        &self,
        frame: &mut Frame,
        area: Rect,
        selected: usize,
        inspection: &Inspection,
    ) {
        let width = area.width.saturating_sub(2).max(1);
        let height = usize::from(area.height.saturating_sub(2));
        let lines = detail_lines(self.details(selected), width).collect::<Vec<_>>();
        let offset = inspection.offset.min(lines.len().saturating_sub(height));
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(offset)
                    .take(height)
                    .collect::<Vec<_>>(),
            )
            .block(panel(self.spec.inspector)),
            area,
        );
    }

    pub(super) fn draw(&self, frame: &mut Frame, area: Rect, selected: usize) {
        self.draw_with_inspector(frame, area, selected, false);
    }

    pub(super) fn draw_with_inspector(
        &self,
        frame: &mut Frame,
        area: Rect,
        selected: usize,
        expandable: bool,
    ) {
        let (list, inspector) = panes(area);
        self.draw_list(frame, list, selected);
        let width = inspector.width.saturating_sub(2).max(1);
        let height = usize::from(inspector.height.saturating_sub(2));
        let mut lines = if self.has_detail(selected) {
            detail_lines(self.details(selected), width)
                .take(height.saturating_add(1))
                .collect::<Vec<_>>()
        } else {
            vec![Line::from("No selected evidence")]
        };
        if lines.len() > height {
            lines.truncate(height);
            if let Some(last) = lines.last_mut() {
                let hint = if expandable {
                    "… Enter to read more"
                } else {
                    "… more detail at a larger size"
                };
                *last = Line::from(hint)
                    .style(surface(theme::palette().canvas).add_modifier(Modifier::DIM));
            }
        }
        frame.render_widget(
            Paragraph::new(lines).block(panel(self.spec.inspector)),
            inspector,
        );
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect, selected: usize) {
        if self.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(self.spec.empty)
                    .wrap(ratatui::widgets::Wrap { trim: true })
                    .block(panel(self.spec.title)),
                area,
            );
            return;
        }

        let columns = self
            .spec
            .columns
            .iter()
            .map(|&(_, weight)| Constraint::Fill(weight));
        let header =
            Row::new(self.spec.columns.iter().map(|&(title, _)| title)).style(heading_style());
        let rows =
            self.rows.iter().enumerate().map(|(index, row)| {
                Row::new(row.cells.iter().enumerate().map(|(column, value)| {
                    let cell = Cell::from(visible_text(value));
                    if self.spec.columns.get(column).is_some_and(|(name, _)| {
                        matches!(*name, "Result" | "Severity" | "Root result")
                    }) {
                        cell.style(theme::status(value))
                    } else {
                        cell
                    }
                }))
                .style(theme::row(index))
            });
        let mut cursor = TableState::default().with_selected(Some(selected));
        let table = Table::new(rows, columns)
            .block(panel(self.spec.title))
            .header(header)
            .row_highlight_style(selection_style())
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol("› ");
        frame.render_stateful_widget(table, area, &mut cursor);
    }
}

fn detail_lines<'a>(
    details: impl Iterator<Item = &'a MetricRow>,
    width: u16,
) -> impl Iterator<Item = Line<'static>> {
    details.flat_map(move |detail| {
        let label = visible_text(&detail.label);
        let formatted = format!("{label}  {}", visible_text(&detail.value));
        wrap_display_width(&formatted, usize::from(width))
            .into_iter()
            .enumerate()
            .map(move |(index, text)| {
                if index == 0
                    && let Some(value) = text.strip_prefix(&label)
                {
                    Line::from(vec![
                        Span::styled(label.clone(), label_style()),
                        Span::raw(value.to_owned()),
                    ])
                } else {
                    Line::from(text)
                }
            })
    })
}
