//! Copy/export interactions over a frozen, metadata-only display document.
use super::*;
use crate::loaded_view::{LoadedView, clipboard_request};
use crate::workbench::{ReviewDocumentRequest, ReviewFormat};

#[derive(Default)]
pub(super) struct ViewIo {
    pub(super) form: Option<ExportForm>,
    pub(super) review_form: Option<ReviewExportForm>,
    pub(super) clipboard: Option<Vec<u8>>,
}

pub(super) struct ExportForm {
    document: LoadedView,
    directory: String,
}

/// Destination form for the bounded Review receipt. It stays open on a
/// collision so the operator can choose a new directory without losing the
/// selected review scope.
pub(super) struct ReviewExportForm {
    pub(super) anchor: WorkAnchor,
    pub(super) format: ReviewFormat,
    pub(super) directory: String,
    pub(super) message: Option<String>,
}

impl ReviewExportForm {
    pub(super) fn new(anchor: WorkAnchor) -> Self {
        Self {
            anchor,
            format: ReviewFormat::Markdown,
            directory: String::new(),
            message: None,
        }
    }

    fn cycle_format(&mut self) {
        self.format = match self.format {
            ReviewFormat::Markdown => ReviewFormat::Html,
            ReviewFormat::Html => ReviewFormat::Json,
            ReviewFormat::Json => ReviewFormat::Text,
            ReviewFormat::Text => ReviewFormat::Markdown,
        };
    }
}

impl Workspace {
    /// Review-scoped actions run before the loaded-view copy/export path so the
    /// frozen receipt, not the pane metadata, is the document under `c`/`x`.
    pub(super) fn review_io_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        key: event::KeyEvent,
    ) -> Option<bool> {
        if self.prompt.is_some() || self.work.is_some() || self.investigation.is_some() {
            return None;
        }
        if let Some(form) = &mut self.view_io.review_form {
            match key.code {
                KeyCode::Esc => self.view_io.review_form = None,
                KeyCode::Tab | KeyCode::BackTab => form.cycle_format(),
                KeyCode::Backspace => {
                    form.directory.pop();
                }
                KeyCode::Enter if self.pending.is_some() => {
                    form.message = Some("Wait for the current read before exporting".into());
                }
                KeyCode::Enter if form.directory.trim().is_empty() => {
                    form.message = Some("Enter a new export directory".into());
                }
                KeyCode::Enter => {
                    let request = ReviewDocumentRequest {
                        anchor: form.anchor.clone(),
                        format: form.format,
                        directory: Some(form.directory.clone()),
                    };
                    form.message = Some(
                        "Writing review.md, review.html and review.json; a collision leaves this form open"
                            .into(),
                    );
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::ReviewReceipt(Box::new(request))),
                        Change::Work(None),
                    );
                }
                KeyCode::Char('f') if key.modifiers == KeyModifiers::CONTROL => {
                    form.cycle_format();
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && form.directory.len() + c.len_utf8() <= 4096 =>
                {
                    form.directory.push(c);
                }
                _ => {}
            }
            return Some(false);
        }
        if !self.review_controls_focused() {
            return None;
        }
        let Some(anchor) = self.review_anchor() else {
            return None;
        };
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        match key.code {
            KeyCode::Char('c') => {
                let request = ReviewDocumentRequest {
                    anchor,
                    format: ReviewFormat::Text,
                    directory: None,
                };
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::ReviewReceipt(Box::new(request))),
                    Change::Work(None),
                );
                Some(false)
            }
            KeyCode::Char('x') => {
                self.view_io.review_form = Some(ReviewExportForm::new(anchor));
                self.message = None;
                Some(false)
            }
            KeyCode::Char('s') => {
                let title = format!(
                    "Review · {} / {}",
                    visible_text(&anchor.source),
                    visible_text(&anchor.session)
                );
                let mut note = format!(
                    "Saved from Review for {} / {}.",
                    anchor.source, anchor.session
                );
                if let Some(turn) = &anchor.turn {
                    note.push_str(&format!(" Turn {turn}."));
                }
                if let Some(worker) = &anchor.worker {
                    note.push_str(&format!(" Worker {worker}."));
                }
                note.push_str(" Use Copy or Export for the bounded review receipt.");
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::DraftReviewTask {
                        anchor,
                        title,
                        note,
                    }),
                    Change::Work(None),
                );
                Some(false)
            }
            KeyCode::Char('o') => {
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::ReviewExcerpts(anchor)),
                    Change::Work(None),
                );
                Some(false)
            }
            KeyCode::Char('d') => {
                let title = format!(
                    "Draft · {} / {}",
                    visible_text(&anchor.source),
                    visible_text(&anchor.session)
                );
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::DraftReviewTask {
                        anchor,
                        title,
                        note: String::new(),
                    }),
                    Change::Work(None),
                );
                Some(false)
            }
            KeyCode::Char('g' | 'm') => {
                self.message = Some(
                    "Review workspace mapping is unavailable · no verified workspace binding for this evidence"
                        .into(),
                );
                Some(false)
            }
            _ => None,
        }
    }

    pub(super) fn loaded_view(&self) -> Option<LoadedView> {
        if self.prompt.is_some()
            || self.prompt_receiver.is_some()
            || self.work.is_some()
            || self.view_io.review_form.is_some()
            || self.agents.editing
        {
            return None;
        }
        if let Some(view) = self.history.last() {
            let page = &view.page;
            if page.ui.help
                || page.turn_picker.is_some()
                || page.operation_picker.is_some()
                || page.evaluation_picker.is_some()
            {
                return None;
            }
            if let Some(analytics) = view.active_analytics() {
                return Some(analytics.loaded_view());
            }
            let scope = match &view.selection {
                HistorySelection::ReviewEdits(_) => return None,
                HistorySelection::All(_) => "Workspace · loaded metadata page".into(),
                HistorySelection::Session(session, _) => {
                    format!("{} / {}", session.harness_id, session.session_id)
                }
                HistorySelection::McpSearch {
                    scope, snapshot_id, ..
                } => {
                    let owner = scope.anchor().map_or_else(
                        || "All authorized sources".into(),
                        |row| {
                            let mut owner = format!("{} / {}", row.source, row.session);
                            if let Some(turn) = row.turn {
                                owner.push_str(" / turn ");
                                owner.push_str(&turn);
                            }
                            if let Some(worker) = row.worker {
                                owner.push_str(" / worker ");
                                owner.push_str(&worker);
                            }
                            owner
                        },
                    );
                    format!("{owner} / retained MCP search / snapshot {snapshot_id}")
                }
                HistorySelection::McpTraces {
                    source_id,
                    event_id,
                    revision,
                    snapshot_id,
                } => format!(
                    "{} / MCP call {} r{} / snapshot {}",
                    source_id.as_str(),
                    event_id,
                    revision,
                    snapshot_id
                ),
                HistorySelection::Turn(turn) => format!(
                    "{} / {} / turn {}",
                    turn.harness_id, turn.session_id, turn.turn_id
                ),
                HistorySelection::TurnReview(turn) => format!(
                    "{} / {} / turn {}",
                    turn.harness_id, turn.session_id, turn.turn_id
                ),
            };
            if let Some(table) = page.operation_table.active_ref(&page.ui)
                && let Some(row) = table.selected_row()
                && !row.evidence.is_empty()
            {
                let mut selected_scope = scope.clone();
                selected_scope.push_str(&format!(
                    " · selected operation {} · {} · loaded page only",
                    visible_text(&row.tool),
                    visible_text(row.server.as_deref().unwrap_or("server unreported"))
                ));
                return Some(LoadedView {
                    title: format!(
                        "{} · selected operation",
                        page.ui.selected_title().unwrap_or("Operations")
                    ),
                    scope: selected_scope,
                    rows: row.evidence.clone(),
                    truncated: false,
                });
            }
            if let Some((title, row, position, total)) = page.ui.selected_evidence_export() {
                return Some(LoadedView::selected_evidence(
                    format!("{title} · selected evidence"),
                    scope,
                    row,
                    position,
                    total,
                ));
            }
            let pane = page.ui.panes.get(page.ui.selected)?;
            Some(LoadedView {
                title: pane.title.clone(),
                scope,
                rows: pane.rows.clone(),
                truncated: page.ui.truncated,
            })
        } else {
            if self.agents.help {
                return None;
            }
            let catalog = self.agents.catalog.as_ref()?;
            Some(LoadedView {
                title: "Agenturm command center".into(),
                scope: "Loaded and filtered session page".into(),
                rows: self
                    .agents
                    .visible
                    .iter()
                    .filter_map(|item| catalog.sessions.get(item.index))
                    .map(|row| MetricRow {
                        label: format!("{} / {}", row.harness_id, row.session_id),
                        value: format!(
                            "task={} · project={} · project_origin={} · state={} · updated_unix_ns={}",
                            command_center::name(row),
                            command_center::project(row),
                            match row.project_origin { Some(ProjectOrigin::OperatorTaskWorkspace) => "operator_task_workspace", Some(ProjectOrigin::NativeRecordedWorkspace) => "native_recorded_workspace", None => "unavailable" },
                            recorded_state(row),
                            row.updated_at_unix_ns
                        ),
                    })
                    .collect(),
                truncated: catalog.older.is_some(),
            })
        }
    }

    /// Returns None when another focus owner should handle the key.
    pub(super) fn view_io_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        key: event::KeyEvent,
    ) -> Option<bool> {
        if self.prompt.is_some() || self.prompt_receiver.is_some() || self.work.is_some() {
            return None;
        }
        if let Some(form) = &mut self.view_io.form {
            match key.code {
                KeyCode::Esc => self.view_io.form = None,
                KeyCode::Backspace => {
                    form.directory.pop();
                }
                KeyCode::Enter if self.pending.is_some() => {
                    self.message = Some("Wait for the current read before exporting".into());
                }
                KeyCode::Enter if form.directory.trim().is_empty() => {
                    self.message = Some("Enter a new export directory".into());
                }
                KeyCode::Enter => {
                    let form = self.view_io.form.take().expect("export form is open");
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::ExportView {
                            document: form.document,
                            directory: form.directory,
                        }),
                        Change::Work(None),
                    );
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && form.directory.len() + c.len_utf8() <= 4096 =>
                {
                    form.directory.push(c);
                }
                _ => {}
            }
            return Some(false);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || !matches!(key.code, KeyCode::Char('c' | 'x'))
        {
            return None;
        }
        let document = self.loaded_view()?;
        let text = match document.plain_text() {
            Ok(text) => text,
            Err(error) => {
                self.message = Some(error);
                return Some(false);
            }
        };
        match key.code {
            KeyCode::Char('c') => match clipboard_request(&text) {
                Ok(request) => {
                    self.view_io.clipboard = Some(request);
                    self.message = Some(
                        "Clipboard request sent · terminal permission/support required".into(),
                    );
                }
                Err(error) => self.message = Some(error),
            },
            KeyCode::Char('x') => {
                self.view_io.form = Some(ExportForm {
                    document,
                    directory: String::new(),
                });
                self.message = None;
            }
            _ => unreachable!(),
        }
        Some(false)
    }
}

pub(super) fn draw_review_export(
    frame: &mut Frame,
    form: &ReviewExportForm,
    message: Option<&str>,
) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(frame.area());
    let lines = vec![
        Line::styled("Export review receipt", heading_style()),
        Line::from(format!(
            "Scope  {} / {}",
            visible_text(&form.anchor.source),
            visible_text(&form.anchor.session)
        )),
        Line::from(format!(
            "Format {} · Tab / Ctrl+F cycle",
            form.format.as_str()
        )),
        Line::from(""),
        Line::from("New directory:"),
        Line::from(visible_text(&form.directory)),
        Line::from(""),
        Line::from("Saves review.md, review.html and review.json."),
        Line::from("Bounded to this exact history scope · partial pages stay labelled."),
        Line::from(
            form.message
                .as_deref()
                .or(message)
                .map(visible_text)
                .unwrap_or_default(),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(" Export review "))
            .wrap(Wrap { trim: false }),
        body,
    );
    frame.render_widget(
        Paragraph::new("Type a new directory · Tab format · Enter save · Esc cancel"),
        footer,
    );
}

pub(super) fn draw(frame: &mut Frame, form: &ExportForm, message: Option<&str>) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(frame.area());
    let lines = vec![
        Line::styled("Export loaded view", heading_style()),
        Line::from(visible_text(&form.document.title)),
        Line::from(""),
        Line::from("New directory:"),
        Line::from(visible_text(&form.directory)),
        Line::from(""),
        Line::from("Saves view.md, view.html and view.json."),
        Line::from("Only loaded metadata · not a verified evidence receipt."),
        Line::from(message.map(visible_text).unwrap_or_default()),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(" Export "))
            .wrap(Wrap { trim: false }),
        body,
    );
    frame.render_widget(
        Paragraph::new("Type a new directory · Enter save · Esc cancel"),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_form_owns_printable_keys_and_cancel_keeps_selection() {
        let mut workspace = Workspace::new();
        workspace.agents.catalog = Some(SessionCatalog {
            sessions: Vec::new(),
            older: None,
            notice: None,
            day_start_unix_ns: 0,
        });
        let (sender, receiver) = mpsc::channel();
        let key = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(
            workspace.view_io_key(&sender, key(KeyCode::Char('x'))),
            Some(false)
        );
        workspace.view_io_key(&sender, key(KeyCode::Char('q')));
        assert_eq!(workspace.view_io.form.as_ref().unwrap().directory, "q");
        workspace.view_io_key(&sender, key(KeyCode::Esc));
        assert!(workspace.view_io.form.is_none());
        assert!(receiver.try_recv().is_err());
        workspace.agents.editing = true;
        assert_eq!(
            workspace.view_io_key(&sender, key(KeyCode::Char('c'))),
            None
        );
        assert!(workspace.view_io.clipboard.is_none());
    }
}
