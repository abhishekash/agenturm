//! Keyboard-first lists and editors for actions owned by the application.

use super::*;
use crate::workspace::{RecordedSession, ReviewVersion};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkAnchor {
    pub source: String,
    pub session: String,
    pub worker: Option<String>,
    pub turn: Option<String>,
}

impl From<&RecordedSession> for WorkAnchor {
    fn from(session: &RecordedSession) -> Self {
        Self {
            source: session.harness_id.clone(),
            session: session.session_id.clone(),
            worker: None,
            turn: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskEntry {
    pub id: u64,
    pub revision: u64,
    pub anchor: Option<WorkAnchor>,
    pub title: String,
    pub note: String,
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub struct HandoffEntry {
    pub id: u64,
    pub from: WorkAnchor,
    pub to: WorkAnchor,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct RequestEntry {
    pub owner: WorkAnchor,
    pub event_id: String,
    pub revision: u32,
    pub timestamp: u64,
    pub reason: String,
    pub seen: bool,
}

#[derive(Clone, Debug)]
pub struct PromptEntry {
    pub anchor: WorkAnchor,
    pub event_id: String,
    pub revision: u32,
    pub timestamp: u64,
}

#[derive(Clone, Debug)]
pub struct OperationDetailEntry {
    pub kind: OperationDetailKind,
    pub source_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub event_id: String,
    pub revision: u32,
    pub operation_id: String,
    pub label: String,
}

/// Exact metadata for a producer-captured span text request.
#[derive(Clone, Debug)]
pub struct TelemetrySpanEntry {
    pub source_id: ExtensionId,
    pub native_source_id: Option<String>,
    pub trace_id: String,
    pub span_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub worker_id: Option<String>,
    pub snapshot_id: agenturm_kernel::ReadSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationDetailKind {
    Mcp,
    Edit,
}

pub struct PromptDelivery {
    buffer: Vec<u8>,
    frames: mpsc::Sender<Vec<u8>>,
    acknowledgement: mpsc::Receiver<()>,
    active: Arc<Mutex<bool>>,
}

pub(super) struct PromptReceiver {
    pub(super) frames: mpsc::Receiver<Vec<u8>>,
    pub(super) acknowledgement: mpsc::Sender<()>,
    pub(super) active: Arc<Mutex<bool>>,
    pub(super) title: &'static str,
}

pub(super) fn prompt_channel() -> (PromptDelivery, PromptReceiver) {
    let (frames, received) = mpsc::channel();
    let (acknowledgement, ack) = mpsc::channel();
    let active = Arc::new(Mutex::new(true));
    (
        PromptDelivery {
            buffer: Vec::new(),
            frames,
            acknowledgement: ack,
            active: Arc::clone(&active),
        },
        PromptReceiver {
            frames: received,
            acknowledgement,
            active,
            title: "Granted prompt",
        },
    )
}

impl io::Write for PromptDelivery {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > 16384usize.saturating_sub(self.buffer.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "prompt delivery exceeded its bound",
            ));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.frames
            .send(std::mem::take(&mut self.buffer))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "prompt window closed"))?;
        if self
            .acknowledgement
            .recv_timeout(Duration::from_secs(30))
            .is_ok()
        {
            return Ok(());
        }
        let mut active = self
            .active
            .lock()
            .map_err(|_| io::Error::other("prompt guard unavailable"))?;
        *active = false;
        self.acknowledgement.try_recv().map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "prompt display was not confirmed")
        })
    }
}

pub enum WorkCommand {
    ReviewNotes(RecordedSession),
    SaveReviewNote {
        session: RecordedSession,
        note: String,
    },
    Tasks(Option<u64>),
    Handoffs(Option<u64>),
    SaveTask {
        id: Option<u64>,
        revision: Option<u64>,
        anchor: Option<WorkAnchor>,
        title: String,
        note: String,
    },
    CompleteTask {
        id: u64,
        revision: u64,
        completed: bool,
    },
    TaskLinks(u64),
    LinkTask {
        id: u64,
        anchor: WorkAnchor,
    },
    UnlinkTask {
        id: u64,
        anchor: WorkAnchor,
    },
    TaskWorkspace(u64),
    CaptureWorkspace {
        id: u64,
        directory: String,
    },
    DeleteTask(u64),
    DeleteHandoff(u64),
    ExportHandoff(u64),
    SaveHandoff {
        id: Option<u64>,
        from: WorkAnchor,
        to: WorkAnchor,
        note: String,
    },
    Workflow,
    Seen(RequestEntry),
    Locate(RequestEntry),
    Prompts(WorkAnchor),
    RevealPrompt(PromptEntry, PromptDelivery),
    RevealOperation(OperationDetailEntry, PromptDelivery),
    RevealTelemetry(Box<TelemetrySpanEntry>, PromptDelivery),
}

pub struct ReviewNoteEntry {
    pub version: ReviewVersion,
    pub read_incarnation: Option<agenturm_kernel::ReadIncarnation>,
    pub note: String,
}

pub enum WorkResult {
    ReviewNotes {
        session: RecordedSession,
        items: Vec<ReviewNoteEntry>,
    },
    Tasks {
        items: Vec<TaskEntry>,
        older: Option<u64>,
    },
    TaskLinks {
        id: u64,
        items: Vec<WorkAnchor>,
    },
    TaskWorkspace {
        root: String,
        head: String,
        summary: String,
    },
    Handoffs {
        items: Vec<HandoffEntry>,
        older: Option<u64>,
    },
    Workflow {
        items: Vec<RequestEntry>,
        partial: bool,
    },
    Prompts(Vec<PromptEntry>),
    TelemetrySpans(Vec<TelemetrySpanEntry>),
    Seen(RequestEntry),
    Status(String),
}

enum Draft {
    ReviewNote {
        session: RecordedSession,
        note: String,
    },
    Task {
        id: Option<u64>,
        revision: Option<u64>,
        anchor: Option<WorkAnchor>,
        title: String,
        note: String,
        editing_note: bool,
    },
    Capture {
        id: u64,
        directory: String,
    },
    Delete {
        id: u64,
        handoff: bool,
    },
    Handoff {
        id: Option<u64>,
        from: WorkAnchor,
        to: WorkAnchor,
        note: String,
    },
}

pub(super) enum WorkInput {
    Reveal(PromptEntry),
    RevealTelemetry(TelemetrySpanEntry),
    None,
    Close,
    Command(WorkCommand),
}

pub(super) struct Workbench {
    data: WorkResult,
    cursor: ListState,
    draft: Option<Draft>,
    target: Option<WorkAnchor>,
    help: bool,
    review_note_scroll: u16,
}

impl Workbench {
    pub(super) fn new(data: WorkResult, target: Option<WorkAnchor>) -> Self {
        let mut work = Self {
            data,
            cursor: ListState::default(),
            draft: None,
            target,
            help: false,
            review_note_scroll: 0,
        };
        work.reset_cursor();
        work
    }

    pub(super) fn apply(&mut self, data: WorkResult) {
        let previous = self.selected_task().map(|item| item.id);
        let selected_request = match &self.data {
            WorkResult::Workflow { items, .. } => self
                .cursor
                .selected()
                .and_then(|index| items.get(index))
                .map(|entry| (entry.owner.clone(), entry.event_id.clone(), entry.revision)),
            _ => None,
        };
        self.draft = None;
        self.data = data;
        self.reset_cursor();
        if let (Some(id), WorkResult::Tasks { items, .. }) = (previous, &self.data)
            && let Some(index) = items.iter().position(|item| item.id == id)
        {
            self.cursor.select(Some(index));
        }
        if let (Some((owner, event, revision)), WorkResult::Workflow { items, .. }) =
            (selected_request, &self.data)
            && let Some(index) = items.iter().position(|item| {
                item.owner == owner && item.event_id == event && item.revision == revision
            })
        {
            self.cursor.select(Some(index));
        }
    }

    pub(super) fn follows_requests(&self) -> bool {
        matches!(self.data, WorkResult::Workflow { .. }) && !self.help
    }

    pub(super) fn note_seen(&mut self, recorded: &RequestEntry) {
        if let WorkResult::Workflow { items, .. } = &mut self.data
            && let Some(entry) = items.iter_mut().find(|entry| {
                entry.owner == recorded.owner
                    && entry.event_id == recorded.event_id
                    && entry.revision == recorded.revision
            })
        {
            entry.seen = true;
        }
    }

    fn reset_cursor(&mut self) {
        self.cursor = ListState::default().with_selected((self.len() != 0).then_some(0));
        self.review_note_scroll = 0;
    }

    fn len(&self) -> usize {
        match &self.data {
            WorkResult::ReviewNotes { items, .. } => items.len(),
            WorkResult::Tasks { items, .. } => items.len(),
            WorkResult::Handoffs { items, .. } => items.len(),
            WorkResult::TaskLinks { items, .. } => items.len(),
            WorkResult::TaskWorkspace { .. } => 0,
            WorkResult::Workflow { items, .. } => items.len(),
            WorkResult::Prompts(items) => items.len(),
            WorkResult::TelemetrySpans(spans) => spans.len(),
            WorkResult::Seen(_) | WorkResult::Status(_) => 0,
        }
    }

    fn selected_task(&self) -> Option<&TaskEntry> {
        match &self.data {
            WorkResult::Tasks { items, .. } => items.get(self.cursor.selected()?),
            _ => None,
        }
    }

    pub(super) fn key(&mut self, key: event::KeyEvent) -> WorkInput {
        if key.code == KeyCode::F(1) || (self.help && key.code == KeyCode::Esc) {
            self.help = !self.help;
            return WorkInput::None;
        }
        if self.help {
            return WorkInput::None;
        }
        if self.draft.is_some() {
            return self.edit_key(key);
        }
        if key.code == KeyCode::Char('?') {
            self.help = true;
            return WorkInput::None;
        }
        let selected = self.cursor.selected().unwrap_or_default();
        match key.code {
            KeyCode::Esc => WorkInput::Close,
            KeyCode::PageDown if matches!(self.data, WorkResult::ReviewNotes { .. }) => {
                self.review_note_scroll = self.review_note_scroll.saturating_add(4);
                WorkInput::None
            }
            KeyCode::PageUp if matches!(self.data, WorkResult::ReviewNotes { .. }) => {
                self.review_note_scroll = self.review_note_scroll.saturating_sub(4);
                WorkInput::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.len() != 0 => {
                self.cursor.select(Some((selected + 1).min(self.len() - 1)));
                self.review_note_scroll = 0;
                WorkInput::None
            }
            KeyCode::Up | KeyCode::Char('k') if self.len() != 0 => {
                self.cursor.select(Some(selected.saturating_sub(1)));
                self.review_note_scroll = 0;
                WorkInput::None
            }
            _ => self.action(key.code, selected),
        }
    }

    fn action(&mut self, key: KeyCode, selected: usize) -> WorkInput {
        match (&self.data, key) {
            (WorkResult::ReviewNotes { session, .. }, KeyCode::Char('e')) => {
                self.draft = Some(Draft::ReviewNote {
                    session: session.clone(),
                    note: String::new(),
                });
            }
            (WorkResult::Tasks { .. }, KeyCode::Char('n' | 'N')) => {
                self.draft = Some(Draft::Task {
                    id: None,
                    revision: None,
                    anchor: (key == KeyCode::Char('n'))
                        .then(|| self.target.clone())
                        .flatten(),
                    title: String::new(),
                    note: String::new(),
                    editing_note: false,
                });
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('e'))
                if let Some(task) = items.get(selected) =>
            {
                self.draft = Some(Draft::Task {
                    id: Some(task.id),
                    revision: Some(task.revision),
                    anchor: task.anchor.clone(),
                    title: task.title.clone(),
                    note: task.note.clone(),
                    editing_note: false,
                });
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('c'))
                if let Some(task) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::CompleteTask {
                    id: task.id,
                    revision: task.revision,
                    completed: !task.completed,
                });
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('h'))
                if let Some(from) = items.get(selected).and_then(|task| task.anchor.clone())
                    && let Some(to) = self.target.clone()
                    && (from.source != to.source || from.session != to.session) =>
            {
                self.draft = Some(Draft::Handoff {
                    id: None,
                    from,
                    to,
                    note: String::new(),
                });
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('l'))
                if let Some(item) = items.get(selected)
                    && let Some(anchor) = self.target.clone() =>
            {
                return WorkInput::Command(WorkCommand::LinkTask {
                    id: item.id,
                    anchor,
                });
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('r'))
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::TaskLinks(item.id));
            }
            (WorkResult::TaskLinks { id, items }, KeyCode::Delete)
                if let Some(anchor) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::UnlinkTask {
                    id: *id,
                    anchor: anchor.clone(),
                });
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('g'))
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::TaskWorkspace(item.id));
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Char('G'))
                if let Some(item) = items.get(selected) =>
            {
                self.draft = Some(Draft::Capture {
                    id: item.id,
                    directory: String::new(),
                });
            }
            (WorkResult::Tasks { .. }, KeyCode::Char('o')) => {
                return WorkInput::Command(WorkCommand::Handoffs(None));
            }
            (
                WorkResult::TaskLinks { .. } | WorkResult::TaskWorkspace { .. },
                KeyCode::Char('b'),
            ) => {
                return WorkInput::Command(WorkCommand::Tasks(None));
            }
            (WorkResult::Tasks { items, .. }, KeyCode::Delete)
                if let Some(item) = items.get(selected) =>
            {
                self.draft = Some(Draft::Delete {
                    id: item.id,
                    handoff: false,
                });
            }
            (WorkResult::Handoffs { items, .. }, KeyCode::Delete)
                if let Some(item) = items.get(selected) =>
            {
                self.draft = Some(Draft::Delete {
                    id: item.id,
                    handoff: true,
                });
            }
            (WorkResult::Handoffs { items, .. }, KeyCode::Char('e'))
                if let Some(item) = items.get(selected) =>
            {
                self.draft = Some(Draft::Handoff {
                    id: Some(item.id),
                    from: item.from.clone(),
                    to: item.to.clone(),
                    note: item.note.clone(),
                });
            }
            (WorkResult::Handoffs { items, .. }, KeyCode::Char('x'))
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::ExportHandoff(item.id));
            }
            (WorkResult::Handoffs { .. }, KeyCode::Char('b')) => {
                return WorkInput::Command(WorkCommand::Tasks(None));
            }
            (
                WorkResult::Tasks {
                    older: Some(before),
                    ..
                },
                KeyCode::Char('['),
            ) => return WorkInput::Command(WorkCommand::Tasks(Some(*before))),
            (
                WorkResult::Handoffs {
                    older: Some(before),
                    ..
                },
                KeyCode::Char('['),
            ) => return WorkInput::Command(WorkCommand::Handoffs(Some(*before))),
            (WorkResult::Tasks { .. }, KeyCode::Char(']')) => {
                return WorkInput::Command(WorkCommand::Tasks(None));
            }
            (WorkResult::Handoffs { .. }, KeyCode::Char(']')) => {
                return WorkInput::Command(WorkCommand::Handoffs(None));
            }
            (WorkResult::Workflow { .. }, KeyCode::Char('r')) => {
                return WorkInput::Command(WorkCommand::Workflow);
            }
            (WorkResult::Workflow { items, .. }, KeyCode::Char('s'))
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::Seen(item.clone()));
            }
            (WorkResult::Workflow { items, .. }, KeyCode::Char('l'))
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::Locate(item.clone()));
            }
            (WorkResult::Prompts(items), KeyCode::Enter)
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Reveal(item.clone());
            }
            (WorkResult::TelemetrySpans(spans), KeyCode::Enter)
                if let Some(span) = spans.get(selected) =>
            {
                return WorkInput::RevealTelemetry(span.clone());
            }
            _ => {}
        }
        WorkInput::None
    }

    fn edit_key(&mut self, key: event::KeyEvent) -> WorkInput {
        if key.code == KeyCode::Esc {
            self.draft = None;
            return WorkInput::None;
        }
        let draft = self.draft.as_mut().expect("draft is active");
        if let Draft::Delete { id, handoff } = draft {
            return if key.code == KeyCode::Char('y') {
                WorkInput::Command(if *handoff {
                    WorkCommand::DeleteHandoff(*id)
                } else {
                    WorkCommand::DeleteTask(*id)
                })
            } else {
                WorkInput::None
            };
        }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return match draft {
                Draft::ReviewNote { session, note } if !note.trim().is_empty() => {
                    WorkInput::Command(WorkCommand::SaveReviewNote {
                        session: session.clone(),
                        note: note.clone(),
                    })
                }
                Draft::Task {
                    id,
                    revision,
                    anchor,
                    title,
                    note,
                    ..
                } if !title.trim().is_empty() => WorkInput::Command(WorkCommand::SaveTask {
                    id: *id,
                    revision: *revision,
                    anchor: anchor.clone(),
                    title: title.trim().to_owned(),
                    note: note.clone(),
                }),
                Draft::Handoff { id, from, to, note } if !note.trim().is_empty() => {
                    WorkInput::Command(WorkCommand::SaveHandoff {
                        id: *id,
                        from: from.clone(),
                        to: to.clone(),
                        note: note.clone(),
                    })
                }
                Draft::Capture { id, directory } if !directory.trim().is_empty() => {
                    WorkInput::Command(WorkCommand::CaptureWorkspace {
                        id: *id,
                        directory: directory.trim().into(),
                    })
                }
                _ => WorkInput::None,
            };
        }
        let (value, limit) = match draft {
            Draft::ReviewNote { note, .. } => (note, 4096),
            Draft::Task {
                title,
                note,
                editing_note,
                ..
            } => {
                if key.code == KeyCode::Tab {
                    *editing_note = !*editing_note;
                    return WorkInput::None;
                }
                if *editing_note {
                    (note, 4096)
                } else {
                    (title, 120)
                }
            }
            Draft::Handoff { note, .. } => (note, 4096),
            Draft::Capture { directory, .. } => (directory, 1024),
            Draft::Delete { .. } => return WorkInput::None,
        };
        match key.code {
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Enter if limit == 4096 && value.len() < limit => value.push('\n'),
            KeyCode::Char(c)
                if !c.is_control()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && value.len() + c.len_utf8() <= limit =>
            {
                value.push(c)
            }
            _ => {}
        }
        WorkInput::None
    }

    pub(super) fn editing(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, pending: bool, status: Option<&str>) {
        let area = frame.area();
        if area.height < MIN_HEIGHT || area.width < MIN_WIDTH {
            frame.render_widget(
                Paragraph::new("Terminal too small · widen the window"),
                area,
            );
            return;
        }
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(area);
        if self.help {
            frame.render_widget(Paragraph::new(" Workbench keys"), header);
            frame.render_widget(
                Paragraph::new("Tasks · n new linked · N blank · e edit · c done\nl link selected session or turn · r links · Del delete\nG capture Git · g saved Git · h new handoff · o handoffs\nHandoffs · e edit · x export MD+HTML · Del delete · b tasks\nWorkflow · s seen · l locate · r refresh\nPrompts · Enter reveal · ↑↓ scroll · Esc close")
                    .wrap(Wrap { trim: false }).block(Block::bordered()),
                body,
            );
            draw_footer(frame, footer, "F1 or Esc close help", pending, status);
            return;
        }
        if let Some(draft) = &self.draft {
            let (title, lines) = draft_text(draft);
            frame.render_widget(Paragraph::new(title), header);
            let paragraph = if matches!(draft, Draft::ReviewNote { .. }) {
                let lines = wrap_review_note(lines, body);
                let scroll = note_scroll_limit(lines.len(), body);
                Paragraph::new(lines).scroll((scroll, 0))
            } else {
                Paragraph::new(lines).wrap(Wrap { trim: false })
            };
            frame.render_widget(paragraph.block(Block::bordered()), body);
            draw_footer(
                frame,
                footer,
                if matches!(draft, Draft::Delete { .. }) {
                    "y delete · Esc cancel"
                } else if matches!(draft, Draft::ReviewNote { .. }) {
                    "Enter new line · Ctrl+S save · Esc cancel"
                } else {
                    "Tab fields · Ctrl+S save · Esc cancel"
                },
                pending,
                status,
            );
            return;
        }
        if let WorkResult::TaskWorkspace {
            root,
            head,
            summary,
        } = &self.data
        {
            frame.render_widget(Paragraph::new(" Agenturm / Saved Git workspace"), header);
            let mut lines = vec![
                Line::from(visible_text(root)),
                Line::from(format!("HEAD  {}", visible_text(head))),
            ];
            lines.extend(summary.lines().map(|line| Line::from(visible_text(line))));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(Block::bordered()),
                body,
            );
            draw_footer(frame, footer, "b tasks · Esc back", pending, status);
            return;
        }
        let selected = self.cursor.selected().unwrap_or_default();
        let (title, keys, options, details, empty) = match &self.data {
            WorkResult::ReviewNotes { session, items } => (
                "Review decisions",
                "e new · ↑↓ select · PgUp/PgDn read · Esc review",
                items
                    .iter()
                    .map(|item| {
                        let scope = if item.read_incarnation != session.snapshot_id.incarnation() {
                            "Prior history evidence"
                        } else if item.version == session.review_version {
                            "Current evidence"
                        } else {
                            "Other evidence version"
                        };
                        format!("{scope} · {}", item.note.lines().next().unwrap_or_default())
                    })
                    .collect(),
                items
                    .get(selected)
                    .map(|item| {
                        let mut details = vec![Line::from(format!(
                            "Evidence receipt {} · {} observations",
                            item.version.receipt_sequence, item.version.retained_observations,
                        ))];
                        details
                            .extend(item.note.lines().map(|line| Line::from(visible_text(line))));
                        details
                    })
                    .unwrap_or_default(),
                "No decision notes for this session",
            ),
            WorkResult::Tasks { items, .. } => (
                "Saved tasks",
                "n new · N blank · e edit · l link · r runs · ? help",
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{}  {}",
                            if item.completed { "Done" } else { "Open" },
                            item.title
                        )
                    })
                    .collect(),
                items.get(selected).map(task_details).unwrap_or_default(),
                "No saved tasks",
            ),
            WorkResult::Handoffs { items, .. } => (
                "Handoffs",
                "b tasks · e edit · x export · Del delete · [ ] pages",
                items
                    .iter()
                    .map(|item| {
                        format!("#{}  {} → {}", item.id, item.from.session, item.to.session)
                    })
                    .collect(),
                items.get(selected).map(handoff_details).unwrap_or_default(),
                "No saved handoffs",
            ),
            WorkResult::TaskLinks { items, .. } => (
                "Linked sessions and runs",
                "b tasks · Del unlink · ↑↓ select · Esc back",
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{} · {}",
                            item.session,
                            item.turn.as_deref().unwrap_or("Session")
                        )
                    })
                    .collect(),
                items.get(selected).map(anchor_details).unwrap_or_default(),
                "No additional links",
            ),
            WorkResult::TaskWorkspace { .. } => return,
            WorkResult::Workflow { items, partial } => (
                if *partial {
                    "Workflow · recent history"
                } else {
                    "Workflow"
                },
                "s seen · l locate terminal · r refresh · Esc back",
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{}  {}",
                            if item.seen {
                                "Pending · seen"
                            } else {
                                "Pending"
                            },
                            item.owner.session
                        )
                    })
                    .collect(),
                items.get(selected).map(request_details).unwrap_or_default(),
                "No pending requests in loaded history",
            ),
            WorkResult::Prompts(items) => (
                "Prompts",
                "Enter reveal · ↑↓ select · Esc back",
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{} · {}",
                            item.anchor.turn.as_deref().unwrap_or("User message"),
                            workspace::age(item.timestamp)
                        )
                    })
                    .collect(),
                items
                    .get(selected)
                    .map(|item| anchor_details(&item.anchor))
                    .unwrap_or_default(),
                "No eligible prompts",
            ),
            WorkResult::TelemetrySpans(spans) => (
                "Captured span text",
                "Enter reveal · ↑↓ select · Esc traces",
                spans
                    .iter()
                    .map(|span| format!("{} · span {}", span.source_id.as_str(), span.span_id))
                    .collect(),
                spans
                    .get(selected)
                    .map(|span| {
                        let mut detail = vec![
                            Line::from(format!("Trace  {}", span.trace_id)),
                            Line::from(format!("Session  {}", span.session_id)),
                        ];
                        if let Some(source) = &span.native_source_id {
                            detail.push(Line::from(format!("Native source  {source}")));
                        }
                        if let Some(turn) = &span.turn_id {
                            detail.push(Line::from(format!("Turn  {turn}")));
                        }
                        if let Some(worker) = &span.worker_id {
                            detail.push(Line::from(format!("Worker  {worker}")));
                        }
                        detail
                    })
                    .unwrap_or_default(),
                "No spans in the loaded trace page",
            ),
            WorkResult::Seen(_) | WorkResult::Status(_) => return,
        };
        frame.render_widget(Paragraph::new(format!(" Agenturm / {title}")), header);
        let [list, detail] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);
        let options: Vec<String> = options;
        if options.is_empty() {
            frame.render_widget(Paragraph::new(empty).block(Block::bordered()), body);
        } else {
            let items = options
                .into_iter()
                .map(|text| ListItem::new(visible_text(&text)));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::bordered())
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                list,
                &mut self.cursor,
            );
            let paragraph = if matches!(self.data, WorkResult::ReviewNotes { .. }) {
                let details = wrap_review_note(details, detail);
                self.review_note_scroll = self
                    .review_note_scroll
                    .min(note_scroll_limit(details.len(), detail));
                Paragraph::new(details).scroll((self.review_note_scroll, 0))
            } else {
                Paragraph::new(details).wrap(Wrap { trim: false })
            };
            frame.render_widget(paragraph.block(Block::bordered().title("Details")), detail);
        }
        draw_footer(frame, footer, keys, pending, status);
    }
}

fn wrap_review_note(lines: Vec<Line<'_>>, area: Rect) -> Vec<Line<'static>> {
    let width = usize::from(area.width.saturating_sub(2).max(1));
    lines
        .into_iter()
        .flat_map(|line| wrap_display_width(&line.to_string(), width))
        .map(Line::from)
        .collect()
}

fn note_scroll_limit(rows: usize, area: Rect) -> u16 {
    let visible = usize::from(area.height.saturating_sub(2).max(1));
    rows.saturating_sub(visible).try_into().unwrap_or(u16::MAX)
}

fn draw_footer(frame: &mut Frame, area: Rect, keys: &str, pending: bool, status: Option<&str>) {
    let status = status.map(visible_text).unwrap_or_else(|| {
        if pending {
            "Loading…".into()
        } else {
            "↑↓ select · q quit".into()
        }
    });
    frame.render_widget(
        Paragraph::new(vec![Line::from(keys), Line::from(status)]),
        area,
    );
}

fn task_details(task: &TaskEntry) -> Vec<Line<'static>> {
    let mut details = task
        .anchor
        .as_ref()
        .map_or_else(|| vec![Line::from("Unlinked task")], anchor_details);
    details.extend(task.note.lines().map(|line| Line::from(visible_text(line))));
    details
}

fn handoff_details(handoff: &HandoffEntry) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!("From  {}", visible_text(&handoff.from.session))),
        Line::from(format!("To  {}", visible_text(&handoff.to.session))),
    ];
    lines.extend(
        handoff
            .note
            .lines()
            .map(|line| Line::from(visible_text(line))),
    );
    lines
}

fn request_details(request: &RequestEntry) -> Vec<Line<'static>> {
    let mut lines = anchor_details(&request.owner);
    lines.push(Line::from(visible_text(&format!(
        "{} · {}",
        request.reason,
        workspace::age(request.timestamp)
    ))));
    lines.push(Line::from("Respond in the native agent terminal."));
    lines
}

fn anchor_details(anchor: &WorkAnchor) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(visible_text(&anchor.source)),
        Line::from(visible_text(&anchor.session)),
    ];
    if let Some(worker) = &anchor.worker {
        lines.push(Line::from(format!("Worker  {}", visible_text(worker))));
    }
    if let Some(turn) = &anchor.turn {
        lines.push(Line::from(format!("Turn  {}", visible_text(turn))));
    }
    lines
}

fn draft_text(draft: &Draft) -> (&'static str, Vec<Line<'static>>) {
    match draft {
        Draft::ReviewNote { session, note } => {
            let mut lines = vec![Line::from(format!(
                "{} · evidence receipt {}",
                visible_text(&session.session_id),
                session.review_version.receipt_sequence,
            ))];
            lines.extend(note.lines().map(|line| Line::from(visible_text(line))));
            (" New review decision", lines)
        }
        Draft::Task {
            title,
            note,
            editing_note,
            ..
        } => {
            let mut lines = vec![
                Line::from(format!(
                    "{} Title  {}",
                    if *editing_note { " " } else { "›" },
                    visible_text(title)
                )),
                Line::from(format!("{} Notes", if *editing_note { "›" } else { " " })),
            ];
            lines.extend(note.lines().map(|line| Line::from(visible_text(line))));
            (" Saved task", lines)
        }
        Draft::Capture { directory, .. } => (
            " Capture Git workspace",
            vec![
                Line::from(format!("› Directory  {}", visible_text(directory))),
                Line::from("Capture reads the selected Git repository."),
            ],
        ),
        Draft::Delete { id, handoff } => {
            let kind = if *handoff { "handoff" } else { "task" };
            (
                " Confirm deletion",
                vec![Line::from(format!("Delete saved {kind} #{id}?"))],
            )
        }
        Draft::Handoff { from, to, note, .. } => {
            let mut lines = vec![
                Line::from(format!("From  {}", visible_text(&from.session))),
                Line::from(format!("To    {}", visible_text(&to.session))),
                Line::from("› Handoff notes"),
            ];
            lines.extend(note.lines().map(|line| Line::from(visible_text(line))));
            (" New handoff", lines)
        }
    }
}
