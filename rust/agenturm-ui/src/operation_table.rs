//! Selectable, host-supplied operation summaries and their recorded evidence.
use super::*;
use ratatui::widgets::{Cell, Row, Table, TableState};
use std::collections::{BTreeMap, BTreeSet};

pub type OperationTables = BTreeMap<ExtensionId, Vec<OperationRow>>;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConfiguredMcpServer {
    pub name: String,
    pub client: String,
    pub scope: String,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpInventory {
    pub servers: Vec<ConfiguredMcpServer>,
    pub partial: bool,
}

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
    inventory: Option<ServerInventory>,
    inventory_open: bool,
}

impl Browser {
    pub(super) fn new(tables: OperationTables, inventory: Option<McpInventory>) -> Self {
        Self {
            panes: tables
                .into_iter()
                .map(|(id, mut rows)| {
                    rows.reverse();
                    let selection =
                        TableState::default().with_selected((!rows.is_empty()).then_some(0));
                    let inventory = matches!(
                        id.as_str(),
                        "app.workspace-mcp" | "app.session-mcp" | "app.turn-mcp"
                    )
                    .then(|| ServerInventory::new(&rows, inventory.as_ref()));
                    let inventory_open = rows.is_empty()
                        && inventory
                            .as_ref()
                            .is_some_and(|inventory| !inventory.rows.is_empty());
                    (
                        id,
                        OperationList {
                            servers: rows
                                .iter()
                                .filter_map(|r| r.server.clone())
                                .collect::<BTreeSet<_>>()
                                .into_iter()
                                .collect(),
                            visible: (0..rows.len()).collect(),
                            rows,
                            selection,
                            inventory,
                            inventory_open,
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
    pub(super) fn is_mcp(&self) -> bool {
        self.inventory.is_some()
    }

    pub(super) fn selected_row(&self) -> Option<&OperationRow> {
        if self.inventory_open {
            return None;
        }
        self.rows
            .get(*self.visible.get(self.selection.selected()?)?)
    }

    pub(super) fn key_hint(&self) -> &'static str {
        if self.inventory_open {
            "↑↓ servers · i calls · Esc back · ←→ views"
        } else if self.expanded
            && self.inventory.is_some()
            && self
                .selected_row()
                .is_some_and(|row| row.mcp_event.is_some())
        {
            "↑↓ scroll · t traces · m payload · Esc back"
        } else if self.inventory.is_some() {
            "↑↓ calls · i servers · f filter · Enter inspect · t traces"
        } else {
            "↑↓ select / scroll · Enter inspect · Esc back · ←→ views"
        }
    }

    pub(super) fn key(&mut self, key: KeyCode) -> bool {
        if let Some(inventory) = &mut self.inventory {
            if !self.expanded
                && (key == KeyCode::Char('i') || self.inventory_open && key == KeyCode::Esc)
            {
                self.inventory_open = !self.inventory_open;
                return true;
            }
            if self.inventory_open {
                return inventory.key(key);
            }
        }
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
        if self.inventory_open
            && let Some(inventory) = &mut self.inventory
        {
            inventory.draw(frame, area);
            return;
        }
        let content = if !self.expanded
            && self.inventory.is_some()
            && area.height >= 19
            && area.width >= 96
            && let Some(inventory) = self
                .inventory
                .as_mut()
                .filter(|inventory| !inventory.rows.is_empty())
        {
            let [preview, calls] = Layout::vertical([
                Constraint::Length((inventory.rows.len() as u16 + 3).min(6)),
                Constraint::Min(1),
            ])
            .areas(area);
            inventory.draw_table(frame, preview, false);
            calls
        } else if !self.expanded
            && self.inventory.is_none()
            && !self.servers.is_empty()
            && area.height >= 19
        {
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
                " {title} · {} on page{}{} ",
                self.visible.len(),
                if self.inventory.is_some() {
                    " · i servers"
                } else {
                    ""
                },
                filter,
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

struct InventoryRow {
    name: String,
    declaration: Option<ConfiguredMcpServer>,
    counts: Option<(usize, usize)>,
}

impl InventoryRow {
    fn provenance(&self) -> String {
        self.declaration.as_ref().map_or_else(
            || "Recorded calls on this page".to_owned(),
            |server| format!("{} · {}", server.client, server.scope),
        )
    }

    fn state(&self) -> &'static str {
        match self.declaration.as_ref().map(|server| server.enabled) {
            Some(Some(false)) => "Disabled config",
            Some(_) => "Configured",
            None => "Observed",
        }
    }
}

struct ServerInventory {
    rows: Vec<InventoryRow>,
    selection: TableState,
    partial: bool,
}

impl ServerInventory {
    fn new(calls: &[OperationRow], configured: Option<&McpInventory>) -> Self {
        let mut observed: BTreeMap<Option<&str>, (usize, usize)> = BTreeMap::new();
        for call in calls {
            let (count, errors) = observed.entry(call.server.as_deref()).or_default();
            *count += 1;
            *errors += usize::from(matches!(call.result.as_deref(), Some("failed" | "error")));
        }
        let mut rows: Vec<_> = configured
            .into_iter()
            .flat_map(|inventory| &inventory.servers)
            .map(|declaration| InventoryRow {
                name: declaration.name.clone(),
                declaration: Some(declaration.clone()),
                counts: None,
            })
            .chain(observed.into_iter().map(|(name, counts)| InventoryRow {
                name: name.unwrap_or("Server unreported").to_owned(),
                declaration: None,
                counts: Some(counts),
            }))
            .collect();
        rows.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.declaration.cmp(&right.declaration))
        });
        Self {
            selection: TableState::default().with_selected((!rows.is_empty()).then_some(0)),
            partial: configured.is_some_and(|inventory| inventory.partial),
            rows,
        }
    }

    fn key(&mut self, key: KeyCode) -> bool {
        let index = self.selection.selected().unwrap_or(0);
        let next = match key {
            KeyCode::Down | KeyCode::Char('j') => index.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => index.saturating_sub(1),
            KeyCode::PageDown => index.saturating_add(5),
            KeyCode::PageUp => index.saturating_sub(5),
            KeyCode::Enter => return true,
            _ => return false,
        };
        if !self.rows.is_empty() {
            self.selection.select(Some(next.min(self.rows.len() - 1)));
        }
        true
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let [table, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(area);
        self.draw_table(frame, table, true);
        let origin = self
            .selection
            .selected()
            .and_then(|index| self.rows.get(index))
            .map(|row| {
                format!(
                    "{} · {}",
                    visible_text(&row.name),
                    visible_text(&row.provenance())
                )
            })
            .unwrap_or_default();
        let coverage = if self.partial {
            "Configuration coverage is partial · counts cover loaded calls"
        } else {
            "Counts cover loaded calls · configuration has no health data"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(origin, label_style()),
                Line::styled(coverage, theme::muted()),
            ]),
            footer,
        );
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let title = panel(" MCP servers · i calls / inventory ");
        if self.rows.is_empty() {
            frame.render_widget(
                Paragraph::new("No server declarations or recorded calls on this page")
                    .block(title),
                area,
            );
            return;
        }
        let wide = area.width >= 80;
        let mut widths = vec![Constraint::Fill(1), Constraint::Length(16)];
        let mut headings = vec!["Server", "Evidence"];
        if wide {
            widths.push(Constraint::Length(24));
            headings.push("Origin");
        }
        widths.extend([Constraint::Length(10), Constraint::Length(7)]);
        headings.extend(["Page calls", "Errors"]);
        let rows = self.rows.iter().enumerate().map(|(index, row)| {
            let mut cells = vec![Cell::from(visible_text(&row.name)), Cell::from(row.state())];
            if wide {
                cells.push(Cell::from(visible_text(&row.provenance())));
            }
            let counts = row.counts.map_or_else(
                || ("—".to_owned(), "—".to_owned()),
                |(calls, errors)| (calls.to_string(), errors.to_string()),
            );
            cells.push(Cell::from(Line::from(counts.0).right_aligned()));
            cells.push(Cell::from(Line::from(counts.1).right_aligned()));
            Row::new(cells).style(theme::row(index))
        });
        let table = Table::new(rows, widths)
            .header(Row::new(headings).style(heading_style()))
            .block(title)
            .row_highlight_style(selection_style())
            .highlight_symbol("› ");
        if focused {
            frame.render_stateful_widget(table, area, &mut self.selection);
        } else {
            frame.render_widget(table, area);
        }
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
        let mut browser = Browser::new([(id.clone(), rows)].into(), None);
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

    #[test]
    fn inventory_shows_idle_declarations_at_all_widths_without_changing_call_selection() {
        let pane = ExtensionId::parse("app.workspace-mcp").unwrap();
        let calls = vec![OperationRow {
            time: "12:00:00".into(),
            server: Some("shared".into()),
            tool: "chosen-call".into(),
            result: Some("failed".into()),
            latency: None,
            evidence: vec![],
            mcp_event: Some(("source".into(), "call-event".into(), 2)),
        }];
        let configured = McpInventory {
            servers: vec![
                ConfiguredMcpServer {
                    name: "idle-server".into(),
                    client: "Codex".into(),
                    scope: "User".into(),
                    enabled: Some(false),
                },
                ConfiguredMcpServer {
                    name: "shared".into(),
                    client: "Claude Code".into(),
                    scope: "Project".into(),
                    enabled: None,
                },
            ],
            partial: true,
        };
        let mut browser = Browser::new([(pane.clone(), calls)].into(), Some(configured));
        let list = browser.panes.get_mut(&pane).unwrap();
        let exact = list.selected_row().unwrap().mcp_event.clone();
        assert!(list.key(KeyCode::Char('i')));
        assert!(list.selected_row().is_none());
        let rows = &list.inventory.as_ref().unwrap().rows;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().filter(|row| row.name == "shared").count(), 2);
        assert_eq!(rows[0].name, "idle-server");
        assert_eq!(rows[0].counts, None);

        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| list.draw(frame, frame.area(), "MCP"))
                .unwrap();
            let display = terminal.backend().to_string();
            for text in [
                "idle-server",
                "Disabled config",
                "Codex · User",
                "Configuration coverage is partial",
            ] {
                assert!(display.contains(text), "missing {text} at width {width}");
            }
        }
        assert!(list.key(KeyCode::Esc));
        assert_eq!(list.selected_row().unwrap().mcp_event, exact);
    }
}
