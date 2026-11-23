//! Copy/export interactions over a frozen, metadata-only display document.
use super::*;
use crate::loaded_view::{LoadedView, clipboard_request};

#[derive(Default)]
pub(super) struct ViewIo {
    pub(super) form: Option<ExportForm>,
    pub(super) clipboard: Option<Vec<u8>>,
}

pub(super) struct ExportForm {
    document: LoadedView,
    directory: String,
}

impl Workspace {
    fn loaded_view(&self) -> Option<LoadedView> {
        if self.prompt.is_some()
            || self.prompt_receiver.is_some()
            || self.work.is_some()
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
            let pane = page.ui.panes.get(page.ui.selected)?;
            let scope = match &view.selection {
                HistorySelection::All(_) => "Workspace · loaded metadata page".into(),
                HistorySelection::Session(session, _) => {
                    format!("{} / {}", session.harness_id, session.session_id)
                }
                HistorySelection::Turn(turn) => format!(
                    "{} / {} / turn {}",
                    turn.harness_id, turn.session_id, turn.turn_id
                ),
            };
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
                title: "Agents".into(),
                scope: "Loaded and filtered session page".into(),
                rows: self
                    .agents
                    .visible
                    .iter()
                    .filter_map(|item| catalog.sessions.get(item.index))
                    .map(|row| MetricRow {
                        label: format!("{} / {}", row.harness_id, row.session_id),
                        value: format!(
                            "state={} · updated_unix_ns={}",
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
