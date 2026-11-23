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
        self.detail_view(selected, width.saturating_sub(2).max(1))
            .len()
            .saturating_sub(page)
    }

    fn detail_view(&self, selected: usize, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if !self.context.is_empty() {
            lines.extend(detail_lines(self.context.iter(), width));
        }
        if let Some(row) = self.rows.get(selected) {
            lines.extend(detail_lines(row.details.iter(), width));
        }
        lines
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
        let lines = self.detail_view(selected, width);
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
            self.detail_view(selected, width)
                .into_iter()
                .take(height.saturating_add(1))
                .collect::<Vec<_>>()
        } else {
            vec![
                Line::from("No recorded detail for this selected row."),
                Line::styled(
                    "Select another row or press Enter for the recorded evidence.",
                    theme::muted(),
                ),
            ]
        };
        if lines.len() > height {
            lines.truncate(height);
            if let Some(last) = lines.last_mut() {
                let hint = if expandable {
                    "… Enter to read the full detail"
                } else {
                    "… widen the terminal for more detail"
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
        let rows = self.rows.iter().enumerate().map(|(index, row)| {
            Row::new(row.cells.iter().enumerate().map(|(column, value)| {
                let cell = Cell::from(visible_text(value));
                if self.spec.columns.get(column).is_some_and(|(name, _)| {
                    matches!(
                        *name,
                        "Result" | "Severity" | "Root result" | "State" | "Status" | "Coverage"
                    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    const SPEC: EvidenceSpec = EvidenceSpec {
        title: "Activity",
        inspector: "Event detail",
        columns: &[("Time", 3), ("Type", 2), ("Result", 2), ("Action", 4)],
        empty: "No recorded activity.",
    };

    fn table(details: Vec<MetricRow>) -> EvidenceTable {
        let mut table = SPEC.table(vec![EvidenceRow {
            cells: vec![
                "12:00:00".into(),
                "Tool".into(),
                "failed".into(),
                "shell".into(),
            ],
            details,
        }]);
        table.context = vec![
            MetricRow {
                label: "Source".into(),
                value: "harness.fixture".into(),
            },
            MetricRow {
                label: "Session".into(),
                value: "session-1".into(),
            },
        ];
        table
    }

    fn render(table: &EvidenceTable, width: u16, height: u16, selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| table.draw_with_inspector(frame, frame.area(), selected, true))
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
    fn selected_evidence_separates_page_context_from_row_attribution() {
        let table = table(vec![MetricRow {
            label: "Result".into(),
            value: "failed · recorded at the exact event".into(),
        }]);
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let text = render(&table, width, height, 0);
            for marker in [
                "Source",
                "harness.fixture",
                "Result",
                "recorded at the exact event",
            ] {
                assert!(text.contains(marker), "{width}×{height}: {marker}\n{text}");
            }
            // Page context precedes the selected row's own attribution.
            let context = text.find("harness.fixture");
            let selected = text.find("recorded at the exact event");
            assert!(
                context
                    .zip(selected)
                    .is_some_and(|(context, selected)| context < selected),
                "{width}×{height}: {text}"
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| table.draw_expanded(frame, frame.area(), 0, &Inspection::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Source") && text.contains("Result"));
        assert!(table.open_inspection(0).is_some());
        assert!(table.inspection_limit(0, 80, 1) > 0);
    }

    #[test]
    fn rows_without_detail_say_so_and_do_not_open_an_inspector() {
        let mut table = table(Vec::new());
        table.context.clear();
        assert!(table.open_inspection(0).is_none());
        let text = render(&table, 80, 24, 0);
        assert!(
            text.contains("No recorded detail for this selected row"),
            "{text}"
        );
    }

    #[test]
    fn a_long_selected_detail_points_to_the_expanded_reader() {
        let table = table(
            (0..12)
                .map(|index| MetricRow {
                    label: format!("Field {index}"),
                    value: "recorded value".into(),
                })
                .collect(),
        );
        let text = render(&table, 80, 24, 0);
        assert!(text.contains("Enter to read the full detail"), "{text}");
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| table.draw_with_inspector(frame, frame.area(), 0, false))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            text.contains("widen the terminal for more detail"),
            "{text}"
        );
    }
}
