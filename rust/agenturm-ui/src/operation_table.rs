//! Selectable, host-supplied operation summaries and their recorded evidence.
use super::pane_filter::{self, Filter, FilterKey};
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
    server_filter: Option<String>,
    visible: Vec<usize>,
    search: Filter,
    retained_search: bool,
    expanded: bool,
    offset: u16,
    max_offset: u16,
    inventory: Option<ServerInventory>,
    inventory_open: bool,
}

pub(super) enum SearchInput {
    Unhandled,
    Handled,
    Submit(String),
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

    pub(super) fn inherit_filters(&mut self, previous: &Self) {
        for (id, pane) in &mut self.panes {
            let Some(prior) = previous.panes.get(id) else {
                continue;
            };
            if pane.is_mcp() {
                pane.retained_search = prior.retained_search;
                if pane.retained_search {
                    pane.inventory_open = false;
                }
                pane.server_filter.clone_from(&prior.server_filter);
                pane.set_filter(prior.filter());
            } else {
                // A page-local filter stays applied when an older or newer
                // Operations/Activity page replaces the loaded rows.
                pane.set_filter(prior.filter());
            }
        }
    }

    pub(super) fn show_retained_search(&mut self, query: &str) {
        for pane in self.panes.values_mut().filter(|pane| pane.is_mcp()) {
            pane.retained_search = true;
            pane.inventory_open = false;
            pane.set_filter(Filter::with_query(query.to_owned()));
        }
    }

    pub(super) fn inherit_server_filters(&mut self, previous: &Self) {
        for (id, pane) in &mut self.panes {
            if let Some(prior) = previous.panes.get(id)
                && pane.is_mcp()
            {
                pane.server_filter.clone_from(&prior.server_filter);
                pane.apply_filters();
            }
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
        if let Some(keys) = self.search.keys() {
            keys
        } else if self.inventory_open {
            "↑↓ servers · i calls · Esc back · ←→ views"
        } else if self.expanded
            && self.inventory.is_some()
            && self
                .selected_row()
                .is_some_and(|row| row.mcp_event.is_some())
        {
            "↑↓ scroll · t traces · m payload · Esc back"
        } else if self.inventory.is_some() {
            "/ search · f server · i inventory · Enter inspect · t trace"
        } else if self.search.active() {
            "↑↓ select · / edit filter · Esc clear · Enter inspect"
        } else {
            "/ filter · ↑↓ select / scroll · Enter inspect · Esc back · ←→ views"
        }
    }

    pub(super) fn search_key(&mut self, key: KeyCode) -> SearchInput {
        if self.expanded {
            return SearchInput::Unhandled;
        }
        if !self.is_mcp() {
            // Activity and Operations panes filter their loaded page locally.
            // Retained MCP searches stay server-side through Submit below.
            if key != KeyCode::Char('/') && !self.search.editing() {
                return SearchInput::Unhandled;
            }
            return match self.search.key(key) {
                FilterKey::Ignored => SearchInput::Unhandled,
                FilterKey::Handled => SearchInput::Handled,
                FilterKey::Changed => {
                    self.apply_filters();
                    SearchInput::Handled
                }
            };
        }
        if self.inventory_open {
            return SearchInput::Unhandled;
        }
        let previous = self.search.committed();
        match self.search.key(key) {
            FilterKey::Ignored => SearchInput::Unhandled,
            FilterKey::Handled => SearchInput::Handled,
            FilterKey::Changed => {
                let query = self.search.query().to_owned();
                self.search = previous;
                SearchInput::Submit(query)
            }
        }
    }

    pub(super) fn editing_filter(&self) -> bool {
        self.search.editing()
    }

    pub(super) fn filter(&self) -> Filter {
        self.search.committed()
    }

    pub(super) fn set_filter(&mut self, filter: Filter) {
        self.search = filter;
        self.apply_filters();
    }

    fn apply_filters(&mut self) {
        let previous = self
            .selection
            .selected()
            .and_then(|row| self.visible.get(row).copied());
        let server = self.server_filter.as_ref();
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let fields = std::iter::once(row.tool.as_str())
                    .chain(row.server.as_deref())
                    .chain(row.result.as_deref())
                    .chain(row.latency.as_deref())
                    .chain(row.evidence.iter().map(|field| field.value.as_str()));
                (server.is_none_or(|server| row.server.as_ref() == Some(server))
                    && (self.retained_search || self.search.matches(fields)))
                .then_some(index)
            })
            .collect();
        self.selection =
            TableState::default().with_selected(pane_filter::selection(&self.visible, previous));
        self.offset = 0;
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
        if !self.expanded && !self.is_mcp() && self.search.active() && key == KeyCode::Esc {
            self.search = Filter::default();
            self.apply_filters();
            return true;
        }
        if !self.expanded && self.is_mcp() && (!self.retained_search || self.search.editing()) {
            match self.search.key(key) {
                FilterKey::Changed => {
                    self.apply_filters();
                    return true;
                }
                FilterKey::Handled => return true,
                FilterKey::Ignored => {}
            }
        }
        if key == KeyCode::Char('f')
            && !self.expanded
            && (!self.servers.is_empty() || self.server_filter.is_some())
        {
            let next = self.server_filter.as_ref().map_or(0, |current| {
                self.servers.partition_point(|server| server <= current)
            });
            self.server_filter = self.servers.get(next).cloned();
            self.apply_filters();
            return true;
        }
        let Some(i) = self.selection.selected() else {
            return false;
        };
        match key {
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
                let style = if self.server_filter.as_ref() == Some(server) {
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
        let content = if !self.expanded
            && let Some(label) = self.search.label_with(if self.is_mcp() {
                "Search retained MCP"
            } else {
                "Filter page"
            }) {
            let [query, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(content);
            frame.render_widget(Paragraph::new(label).style(label_style()), query);
            rest
        } else {
            content
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
            let filter = if self.servers.is_empty() && self.server_filter.is_none() {
                String::new()
            } else {
                let server = self.server_filter.as_deref().unwrap_or("All servers");
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
            if self.visible.is_empty() {
                let empty = if self.retained_search {
                    "No matching retained calls on this page"
                } else if self.rows.is_empty() {
                    "No recorded operations on this page"
                } else {
                    "No matching operations on this page"
                };
                frame.render_widget(Paragraph::new(empty).block(block), list);
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
        let block = panel(" Operation detail ");
        let inner = block.inner(detail);
        let mut lines = vec![Line::styled(visible_text(&row.tool), heading_style())];
        for (label, value) in [
            ("Server", row.server.as_deref().unwrap_or("Unreported")),
            ("Result", row.result.as_deref().unwrap_or("Unreported")),
            ("Latency", row.latency.as_deref().unwrap_or("Unreported")),
            ("Observed", row.time.as_str()),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<10}"), label_style()),
                Span::raw(visible_text(value)),
            ]));
        }
        let worker = row
            .evidence
            .iter()
            .find(|field| field.label == "Worker")
            .map(|field| field.value.as_str())
            .unwrap_or("Unreported (main session or not recorded)");
        lines.push(Line::from(vec![
            Span::styled(format!("{:<10}", "Worker"), label_style()),
            Span::raw(visible_text(worker)),
        ]));
        match &row.mcp_event {
            Some((source, event, revision)) => lines.push(Line::from(vec![
                Span::styled(format!("{:<10}", "Trace"), label_style()),
                Span::raw(visible_text(&format!(
                    "{source} / {event} r{revision}{}",
                    if self.is_mcp() {
                        " · t opens the linked retained trace"
                    } else {
                        " · follow it from MCP or Traces"
                    }
                ))),
            ])),
            None => lines.push(Line::from(vec![
                Span::styled(format!("{:<10}", "Trace"), label_style()),
                Span::raw("No linked MCP call for this operation"),
            ])),
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
    declarations: Vec<ConfiguredMcpServer>,
    counts: Option<(usize, usize)>,
}

impl InventoryRow {
    fn provenance(&self) -> String {
        if self.declarations.is_empty() {
            return "Recorded calls on this page".to_owned();
        }
        self.declarations
            .iter()
            .map(|server| format!("{} · {}", server.client, server.scope))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" · ")
    }

    fn state(&self) -> &'static str {
        let disabled = self
            .declarations
            .iter()
            .any(|server| server.enabled == Some(false));
        match (
            self.declarations.is_empty(),
            self.counts.is_some(),
            disabled,
        ) {
            (true, _, _) => "Observed",
            (false, true, false) => "Observed & config",
            (false, true, true) => "Observed, disabled",
            (false, false, true) => "Disabled config",
            (false, false, false) => "Configured · idle",
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
        let mut declared: BTreeMap<&str, Vec<ConfiguredMcpServer>> = BTreeMap::new();
        for declaration in configured
            .into_iter()
            .flat_map(|inventory| &inventory.servers)
        {
            declared
                .entry(declaration.name.as_str())
                .or_default()
                .push(declaration.clone());
        }
        let names = declared
            .keys()
            .copied()
            .chain(
                observed
                    .keys()
                    .map(|name| name.unwrap_or("Server unreported")),
            )
            .collect::<BTreeSet<_>>();
        // One row per server, as in the reference: configuration and the calls
        // loaded on this page are joined by the exact recorded server name.
        let mut rows: Vec<_> = names
            .into_iter()
            .map(|name| InventoryRow {
                name: name.to_owned(),
                declarations: declared.get(name).cloned().unwrap_or_default(),
                counts: observed.get(&Some(name)).copied(),
            })
            .collect();
        rows.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.declarations.cmp(&right.declarations))
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
        let mut widths = vec![Constraint::Fill(1), Constraint::Length(18)];
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
    fn operations_filter_the_loaded_page_and_clear_with_escape() {
        let id = ExtensionId::parse("test.operations").unwrap();
        let rows = [
            ("tool-shell", Some("main"), Some("failed")),
            ("tool-read", Some("worker-a"), Some("succeeded")),
            ("tool-check", None, None),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (tool, server, result))| OperationRow {
            time: format!("2026-11-23 00:00:0{i}Z"),
            server: server.map(str::to_owned),
            tool: tool.into(),
            result: result.map(str::to_owned),
            latency: None,
            evidence: vec![MetricRow {
                label: "Worker".into(),
                value: if i == 1 { "worker-a" } else { "" }.into(),
            }],
            mcp_event: None,
        })
        .collect();
        let mut browser = Browser::new([(id.clone(), rows)].into(), None);
        let list = browser.panes.get_mut(&id).unwrap();
        assert!(!list.is_mcp());
        let search = |list: &mut OperationList, text: &str| {
            assert!(matches!(
                list.search_key(KeyCode::Char('/')),
                SearchInput::Handled
            ));
            for ch in text.chars() {
                assert!(matches!(
                    list.search_key(KeyCode::Char(ch)),
                    SearchInput::Handled
                ));
            }
        };
        search(list, "worker-a failed");
        // Every word must match somewhere in the row's recorded fields.
        assert!(matches!(
            list.search_key(KeyCode::Enter),
            SearchInput::Handled
        ));
        assert!(list.visible.is_empty());
        assert!(list.key(KeyCode::Esc));
        assert_eq!(list.visible.len(), 3);
        search(list, "worker-a");
        assert!(matches!(
            list.search_key(KeyCode::Enter),
            SearchInput::Handled
        ));
        assert_eq!(
            list.selected_row().map(|row| row.tool.as_str()),
            Some("tool-read")
        );
        let row = list.selected_row().unwrap();
        assert_eq!(
            row.evidence
                .iter()
                .find(|field| field.label == "Worker")
                .map(|field| field.value.as_str()),
            Some("worker-a")
        );
        assert!(list.key(KeyCode::Esc));
        search(list, "tool-check");
        assert!(matches!(
            list.search_key(KeyCode::Enter),
            SearchInput::Handled
        ));
        assert!(list.key_hint().contains("Esc clear"));
        // Inspector attribution keeps an absent result explicit.
        assert_eq!(
            list.selected_row().map(|row| row.tool.as_str()),
            Some("tool-check")
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
        terminal
            .draw(|frame| list.draw(frame, frame.area(), "Operations"))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("Result"));
        assert!(screen.contains("Unreported"));
        assert!(screen.contains("Worker"));
        assert!(screen.contains("Filter page"));
        assert!(screen.contains("No linked MCP call"));
        let next_rows = ["tool-check", "tool-other"]
            .into_iter()
            .map(|tool| OperationRow {
                time: "2026-11-24 00:00:00Z".into(),
                server: None,
                tool: tool.into(),
                result: Some("succeeded".into()),
                latency: None,
                evidence: vec![],
                mcp_event: None,
            })
            .collect();
        let mut older = Browser::new([(id, next_rows)].into(), None);
        older.inherit_filters(&browser);
        let prior = older.panes.values().next().unwrap();
        assert_eq!(prior.search.query(), "tool-check");
        assert_eq!(prior.visible.len(), 1);
        assert_eq!(
            prior.selected_row().map(|row| row.tool.as_str()),
            Some("tool-check")
        );
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
                    enabled: None,
                },
                ConfiguredMcpServer {
                    name: "disabled-server".into(),
                    client: "Claude Code".into(),
                    scope: "Project".into(),
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
        let mut browser = Browser::new([(pane.clone(), calls)].into(), Some(configured.clone()));
        let list = browser.panes.get_mut(&pane).unwrap();
        let exact = list.selected_row().unwrap().mcp_event.clone();
        assert!(list.key(KeyCode::Char('i')));
        assert!(list.selected_row().is_none());
        let rows = &list.inventory.as_ref().unwrap().rows;
        assert_eq!(rows.len(), 3);
        // Configured and observed evidence for the same server is one row.
        assert_eq!(rows.iter().filter(|row| row.name == "shared").count(), 1);
        assert_eq!(rows[0].name, "disabled-server");
        assert_eq!(rows[0].state(), "Disabled config");
        let idle = rows.iter().find(|row| row.name == "idle-server").unwrap();
        assert_eq!(idle.state(), "Configured · idle");
        assert_eq!(idle.counts, None);
        let observed = rows.iter().find(|row| row.name == "shared").unwrap();
        assert_eq!(observed.state(), "Observed & config");
        assert_eq!(observed.counts, Some((1, 1)));

        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| list.draw(frame, frame.area(), "MCP"))
                .unwrap();
            let display = terminal.backend().to_string();
            let mut expected = vec![
                "idle-server",
                "Configured · idle",
                "Disabled config",
                "Observed & config",
                "Configuration coverage is partial",
            ];
            // At compact widths the selected server keeps its origin in the footer.
            expected.push(if width >= 80 {
                "Codex · User"
            } else {
                "Claude Code · Project"
            });
            for text in expected {
                assert!(display.contains(text), "missing {text} at width {width}");
            }
        }
        assert!(list.key(KeyCode::Esc));
        assert_eq!(list.selected_row().unwrap().mcp_event, exact);
        assert!(list.key(KeyCode::Char('f')));

        let mut search = Browser::new([(pane.clone(), Vec::new())].into(), Some(configured));
        search.show_retained_search("missing tool");
        search.inherit_server_filters(&browser);
        let mut next = Browser::new([(pane.clone(), Vec::new())].into(), None);
        next.inherit_filters(&search);
        for browser in [&mut search, &mut next] {
            let list = browser.panes.get_mut(&pane).unwrap();
            assert_eq!(list.server_filter.as_deref(), Some("shared"));
            assert!(!list.inventory_open);
            assert!(list.key(KeyCode::Char('f')));
            assert!(list.server_filter.is_none());
            let backend = ratatui::backend::TestBackend::new(60, 18);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| list.draw(frame, frame.area(), "MCP"))
                .unwrap();
            let display = terminal.backend().to_string();
            assert!(display.contains("No matching retained calls on this page"));
            assert!(display.contains("Search retained MCP"));
        }
    }
}
