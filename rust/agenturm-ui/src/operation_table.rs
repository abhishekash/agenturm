//! Selectable, host-supplied operation summaries and their recorded evidence.
use super::*;
use ratatui::widgets::{Cell, Row, Table, TableState};
use std::collections::BTreeMap;

pub type OperationTables = BTreeMap<ExtensionId, Vec<OperationRow>>;

pub struct OperationRow {
    pub time: String,
    pub server: Option<String>,
    pub tool: String,
    pub result: Option<String>,
    pub latency: Option<String>,
    pub evidence: Vec<MetricRow>,
    /// Source-scoped event and revision for the recorded MCP call, when present.
    pub mcp_event: Option<(String, String, u32)>,
}

pub(super) struct Browser {
    panes: BTreeMap<ExtensionId, OperationList>,
}

#[derive(Default)]
pub(super) struct OperationList {
    rows: Vec<OperationRow>,
    selection: TableState,
    servers: Vec<String>,
    server_filter: Option<usize>,
    visible: Vec<usize>,
    expanded: bool,
    offset: u16,
    max_offset: u16,
}

impl Browser {
    pub(super) fn new(tables: OperationTables) -> Self {
        Self {
            panes: tables
                .into_iter()
                .map(|(id, mut rows)| {
                    rows.reverse();
                    let selection =
                        TableState::default().with_selected((!rows.is_empty()).then_some(0));
                    (
                        id,
                        OperationList {
                            servers: rows
                                .iter()
                                .filter_map(|r| r.server.clone())
                                .collect::<std::collections::BTreeSet<_>>()
                                .into_iter()
                                .collect(),
                            visible: (0..rows.len()).collect(),
                            rows,
                            selection,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        }
    }

    pub(super) fn active(&mut self, ui: &UiModel) -> Option<&mut OperationList> {
        self.panes.get_mut(ui.panes.get(ui.selected)?.id.as_ref()?)
    }
}

impl OperationList {
    pub(super) fn selected_row(&self) -> Option<&OperationRow> {
        self.rows
            .get(*self.visible.get(self.selection.selected()?)?)
    }

    pub(super) fn key(&mut self, key: KeyCode) -> bool {
        let Some(i) = self.selection.selected() else {
            return false;
        };
        match key {
            KeyCode::Char('f') if !self.expanded && !self.servers.is_empty() => {
                self.server_filter = match self.server_filter {
                    None => Some(0),
                    Some(i) if i + 1 < self.servers.len() => Some(i + 1),
                    _ => None,
                };
                let filter = self.server_filter.map(|i| &self.servers[i]);
                self.visible = self
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| {
                        filter
                            .is_none_or(|s| row.server.as_ref() == Some(s))
                            .then_some(i)
                    })
                    .collect();
                self.selection.select(Some(0));
                self.offset = 0;
            }
            KeyCode::Enter => self.expanded = true,
            KeyCode::Esc if self.expanded => {
                self.expanded = false;
                self.offset = 0;
            }
            KeyCode::Down | KeyCode::Char('j') if self.expanded => {
                self.offset = self.offset.saturating_add(1).min(self.max_offset)
            }
            KeyCode::Up | KeyCode::Char('k') if self.expanded => {
                self.offset = self.offset.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => self
                .selection
                .select(Some((i + 1).min(self.visible.len() - 1))),
            KeyCode::Up | KeyCode::Char('k') => self.selection.select(Some(i.saturating_sub(1))),
            _ => return false,
        }
        true
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, area: Rect, title: &str) {
        let content = if !self.expanded && !self.servers.is_empty() && area.height >= 19 {
            let [inventory, content] = Layout::vertical([
                Constraint::Length((self.servers.len() as u16 + 3).min(7)),
                Constraint::Min(1),
            ])
            .areas(area);
            let rows = self.servers.iter().enumerate().map(|(index, server)| {
                let calls = self
                    .rows
                    .iter()
                    .filter(|row| row.server.as_ref() == Some(server));
                let count = calls.clone().count();
                let errors = calls
                    .filter(|row| matches!(row.result.as_deref(), Some("failed" | "error")))
                    .count();
                let style = if self.server_filter == Some(index) {
                    selection_style()
                } else {
                    theme::row(index)
                };
                Row::new([visible_text(server), count.to_string(), errors.to_string()]).style(style)
            });
            frame.render_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Fill(1),
                        Constraint::Length(9),
                        Constraint::Length(9),
                    ],
                )
                .header(Row::new(["Observed server", "Calls", "Errors"]).style(heading_style()))
                .block(panel(" MCP servers on this page · f filter ")),
                inventory,
            );
            content
        } else {
            area
        };
        let (list, detail) = if self.expanded {
            (Rect::default(), content)
        } else if content.width >= 96 {
            let [left, right] =
                Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
                    .areas(content);
            (left, right)
        } else {
            (content, Rect::default())
        };
        if !list.is_empty() {
            let filter = if self.servers.is_empty() {
                String::new()
            } else {
                let server = self
                    .server_filter
                    .map_or("All servers", |i| self.servers[i].as_str());
                format!(" · f {}", visible_text(server))
            };
            let block = panel(format!(
                " {title} · {} on page{filter} ",
                self.visible.len()
            ));
            if self.rows.is_empty() {
                frame.render_widget(
                    Paragraph::new("No recorded operations on this page").block(block),
                    list,
                );
            } else {
                let compact = list.width < 70;
                let headings: Vec<&str> = if compact {
                    vec!["Tool / method", "Result", "Latency"]
                } else {
                    vec!["UTC", "Server", "Tool / method", "Result", "Latency"]
                };
                let header = Row::new(headings).style(heading_style());
                let rows = self.visible.iter().enumerate().map(|(position, &index)| {
                    let op = &self.rows[index];
                    let mut cells = Vec::new();
                    if !compact {
                        let time = op
                            .time
                            .split_once(' ')
                            .map_or(op.time.as_str(), |(_, time)| time.trim_end_matches('Z'));
                        cells.push(Cell::from(visible_text(time)).style(theme::muted()));
                        cells.push(
                            Cell::from(visible_text(op.server.as_deref().unwrap_or("—")))
                                .style(label_style()),
                        );
                    }
                    cells.push(Cell::from(visible_text(&op.tool)));
                    cells.push(
                        Cell::from(visible_text(op.result.as_deref().unwrap_or("—")))
                            .style(theme::status(op.result.as_deref().unwrap_or(""))),
                    );
                    cells.push(Cell::from(visible_text(
                        op.latency.as_deref().unwrap_or("—"),
                    )));
                    Row::new(cells).style(theme::row(position))
                });
                let widths = if compact {
                    vec![
                        Constraint::Fill(1),
                        Constraint::Length(10),
                        Constraint::Length(10),
                    ]
                } else {
                    vec![
                        Constraint::Length(8),
                        Constraint::Ratio(1, 5),
                        Constraint::Fill(1),
                        Constraint::Length(10),
                        Constraint::Length(10),
                    ]
                };
                frame.render_stateful_widget(
                    Table::new(rows, widths)
                        .header(header)
                        .block(block)
                        .row_highlight_style(selection_style())
                        .highlight_symbol("› "),
                    list,
                    &mut self.selection,
                );
            }
        }
        if detail.is_empty() {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        let block = panel(" Tool inspector ");
        let inner = block.inner(detail);
        let mut lines = vec![Line::styled(visible_text(&row.tool), heading_style())];
        for (label, value) in [
            ("Server", row.server.as_deref()),
            ("Result", row.result.as_deref()),
            ("Latency", row.latency.as_deref()),
            ("Observed", Some(row.time.as_str())),
        ] {
            if let Some(value) = value {
                lines.push(Line::from(vec![
                    Span::styled(format!("{label:<10}"), label_style()),
                    Span::raw(visible_text(value)),
                ]));
            }
        }
        lines.push(Line::default());
        lines.push(Line::styled("Recorded evidence", heading_style()));
        for event in &row.evidence {
            lines.push(Line::styled(visible_text(&event.label), label_style()));
            lines.extend(
                wrap_display_width(&visible_text(&event.value), usize::from(inner.width.max(1)))
                    .into_iter()
                    .map(Line::from),
            );
        }
        self.max_offset = lines
            .len()
            .saturating_sub(inner.height.into())
            .min(u16::MAX.into()) as u16;
        self.offset = self.offset.min(self.max_offset);
        frame.render_widget(
            Paragraph::new(lines).block(block).scroll((self.offset, 0)),
            detail,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_expand_and_return_keep_the_selected_occurrence() {
        let id = ExtensionId::parse("test.operations").unwrap();
        let rows = ["a", "b", "a"]
            .into_iter()
            .enumerate()
            .map(|(i, server)| OperationRow {
                time: format!("2026-11-23 00:00:0{i}Z"),
                server: Some(server.into()),
                tool: format!("tool-{i}"),
                result: Some("succeeded".into()),
                latency: None,
                evidence: vec![],
                mcp_event: Some(("source".into(), format!("event-{i}"), 0)),
            })
            .collect();
        let mut browser = Browser::new([(id.clone(), rows)].into());
        let list = browser.panes.get_mut(&id).unwrap();
        assert!(list.key(KeyCode::Char('f')));
        assert_eq!(list.visible.len(), 2);
        assert!(list.key(KeyCode::Down));
        assert_eq!(list.selected_row().unwrap().tool, "tool-0");
        assert!(list.key(KeyCode::Enter));
        assert!(list.key(KeyCode::Down));
        assert!(list.key(KeyCode::Esc));
        assert_eq!(list.selected_row().unwrap().tool, "tool-0");
        assert!(list.key(KeyCode::Char('f')));
        assert_eq!(list.selected_row().unwrap().tool, "tool-1");
        assert!(list.key(KeyCode::Char('f')));
        assert_eq!(list.visible.len(), 3);
    }
}
