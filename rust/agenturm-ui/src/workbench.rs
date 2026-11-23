//! Keyboard-first lists and editors for actions owned by the application.

use super::*;
use crate::workspace::{RecordedSession, ReviewVersion};
use agenturm_kernel::ReadSnapshot;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestEntry {
    pub owner: WorkAnchor,
    pub event_id: String,
    pub revision: u32,
    pub timestamp: u64,
    pub reason: String,
    pub seen: bool,
    pub proof: RequestProof,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestProof {
    pub snapshot_id: agenturm_kernel::ReadSnapshot,
    pub history_identity: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PromptEntry {
    pub anchor: WorkAnchor,
    pub event_id: String,
    pub revision: u32,
    pub timestamp: u64,
    pub snapshot_id: Option<ReadSnapshot>,
    pub history_identity: Option<String>,
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
    pub history_identity: String,
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
    pub(super) kind: PromptKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PromptKind {
    User,
    Mcp,
    Edit,
    Telemetry,
    Evaluation,
    Investigation,
}

impl PromptKind {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::User => "Granted prompt",
            Self::Mcp => "Granted MCP payload",
            Self::Edit => "Granted file edit",
            Self::Telemetry => "Producer-captured text",
            Self::Evaluation => "Imported evaluation text",
            Self::Investigation => "Verified source evidence",
        }
    }
}

pub(super) fn prompt_channel(kind: PromptKind) -> (PromptDelivery, PromptReceiver) {
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
            kind,
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

pub(crate) mod evaluation;
mod host;
pub(crate) mod investigation;

pub enum WorkCommand {
    Investigate(Box<investigation::InvestigationRead>),
    RevealInvestigation {
        reference: String,
        history_identity: Option<String>,
        delivery: PromptDelivery,
    },
    ExportView {
        document: crate::LoadedView,
        directory: String,
    },
    Evaluations(Box<evaluation::EvaluationRead>),
    HostHistory(Option<agenturm_kernel::ObservationPosition>),
    HostNow,
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
    SessionAttention(RecordedSession),
    Seen(RequestEntry),
    Locate(RequestEntry),
    Prompts(WorkAnchor),
    RevealPrompt(Box<PromptEntry>, PromptDelivery),
    RevealOperation(OperationDetailEntry, PromptDelivery),
    RevealTelemetry(Box<TelemetrySpanEntry>, PromptDelivery),
    RevealEvalDetail(Box<evaluation::EvaluationDetailEntry>, PromptDelivery),
}

pub struct ReviewNoteEntry {
    pub version: ReviewVersion,
    pub read_incarnation: Option<agenturm_kernel::ReadIncarnation>,
    pub note: String,
}

/// A current same-user Host snapshot supplied by the application.
pub struct CurrentHostSample {
    pub captured_at_unix_ns: u64,
    pub sample: agenturm_kernel::HostSample,
}

pub enum WorkResult {
    Investigation(Box<investigation::InvestigationPage>),
    Evaluations(Box<evaluation::EvaluationPage>),
    HostHistory(agenturm_kernel::ObservationPage),
    HostNow(Option<Box<CurrentHostSample>>),
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
    AttentionRequests(Vec<RequestEntry>),
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
    RevealEvaluation(evaluation::EvaluationDetailEntry),
    None,
    Back,
    Close,
    Command(WorkCommand),
}

struct EvaluationReturn {
    page: Box<evaluation::EvaluationPage>,
    cursor: ListState,
    scroll: u16,
}

pub(super) struct Workbench {
    data: WorkResult,
    cursor: ListState,
    draft: Option<Draft>,
    target: Option<WorkAnchor>,
    help: bool,
    review_note_scroll: u16,
    host_level: host::HostLevel,
    host_filter: crate::pane_filter::Filter,
    host_inspection: Option<crate::evidence_view::Inspection>,
    host_inspection_bounds: (usize, usize),
    evaluation_return: Option<EvaluationReturn>,
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
            host_level: host::HostLevel::default(),
            host_filter: crate::pane_filter::Filter::default(),
            host_inspection: None,
            host_inspection_bounds: (1, 0),
            evaluation_return: None,
        };
        work.reset_cursor();
        work
    }

    pub(super) fn apply(&mut self, data: WorkResult) {
        let previous = self.selected_task().map(|item| item.id);
        let selected_process = self.current_process_identity();
        let selected_request = match &self.data {
            WorkResult::Workflow { items, .. } => self
                .cursor
                .selected()
                .and_then(|index| items.get(index))
                .map(|entry| (entry.owner.clone(), entry.event_id.clone(), entry.revision)),
            _ => None,
        };
        self.draft = None;
        let previous_data = std::mem::replace(&mut self.data, data);
        match (&self.data, previous_data) {
            (WorkResult::Evaluations(current), WorkResult::Evaluations(previous))
                if current.summary() =>
            {
                if !previous.summary() {
                    self.evaluation_return = Some(EvaluationReturn {
                        page: previous,
                        cursor: self.cursor,
                        scroll: self.review_note_scroll,
                    });
                }
            }
            _ => self.evaluation_return = None,
        }
        self.host_level = host::HostLevel::default();
        self.host_inspection = None;
        self.reset_cursor();
        if let (Some((boot_id, pid, start_ticks)), WorkResult::HostNow(Some(capture))) =
            (selected_process, &self.data)
            && capture.sample.boot_id == boot_id
            && let Some(index) = host::current_indices(&capture.sample, &self.host_filter)
                .iter()
                .position(|&index| {
                    let process = &capture.sample.processes[index];
                    process.pid == pid && process.start_ticks == start_ticks
                })
        {
            self.cursor.select(Some(index));
        }
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

    pub(super) fn prompt_owner(&self) -> Option<&WorkAnchor> {
        if matches!(self.data, WorkResult::Prompts(_)) && !self.help && !self.editing() {
            self.target.as_ref()
        } else {
            None
        }
    }

    pub(super) fn follows_requests(&self) -> bool {
        matches!(self.data, WorkResult::Workflow { .. }) && !self.help
    }

    pub(super) fn note_seen(&mut self, recorded: &RequestEntry) {
        if let WorkResult::Workflow { items, .. } | WorkResult::AttentionRequests(items) =
            &mut self.data
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
            WorkResult::Evaluations(page) => page.rows.len(),
            WorkResult::HostHistory(page) => self.host_level.len(page, &self.host_filter),
            WorkResult::HostNow(capture) => capture.as_ref().map_or(0, |capture| {
                host::current_indices(&capture.sample, &self.host_filter).len()
            }),
            WorkResult::ReviewNotes { items, .. } => items.len(),
            WorkResult::Tasks { items, .. } => items.len(),
            WorkResult::Handoffs { items, .. } => items.len(),
            WorkResult::TaskLinks { items, .. } => items.len(),
            WorkResult::TaskWorkspace { .. } => 0,
            WorkResult::Workflow { items, .. } | WorkResult::AttentionRequests(items) => {
                items.len()
            }
            WorkResult::Prompts(items) => items.len(),
            WorkResult::TelemetrySpans(spans) => spans.len(),
            WorkResult::Investigation(_) | WorkResult::Seen(_) | WorkResult::Status(_) => 0,
        }
    }

    fn selected_task(&self) -> Option<&TaskEntry> {
        match &self.data {
            WorkResult::Tasks { items, .. } => items.get(self.cursor.selected()?),
            _ => None,
        }
    }

    fn current_process_identity(&self) -> Option<(String, u32, u64)> {
        let WorkResult::HostNow(Some(capture)) = &self.data else {
            return None;
        };
        let visible = host::current_indices(&capture.sample, &self.host_filter);
        let index = *visible.get(self.cursor.selected()?)?;
        let process = &capture.sample.processes[index];
        Some((
            capture.sample.boot_id.clone(),
            process.pid,
            process.start_ticks,
        ))
    }

    pub(super) fn host_filter_key(&mut self, key: KeyCode) -> bool {
        use crate::pane_filter::{self, FilterKey};

        if self.help || self.editing() || self.host_inspection.is_some() {
            return false;
        }
        match &self.data {
            WorkResult::HostHistory(page) => {
                self.host_level
                    .filter_key(page, &mut self.host_filter, &mut self.cursor, key)
            }
            WorkResult::HostNow(Some(capture)) => {
                let previous = self.cursor.selected().and_then(|row| {
                    host::current_indices(&capture.sample, &self.host_filter)
                        .get(row)
                        .copied()
                });
                match self.host_filter.key(key) {
                    FilterKey::Ignored => false,
                    FilterKey::Handled => true,
                    FilterKey::Changed => {
                        let visible = host::current_indices(&capture.sample, &self.host_filter);
                        self.cursor = ListState::default()
                            .with_selected(pane_filter::selection(&visible, previous));
                        true
                    }
                }
            }
            _ => false,
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
        if let WorkResult::HostNow(capture) = &self.data {
            if let Some(inspection) = &mut self.host_inspection {
                if key.code == KeyCode::Esc {
                    self.host_inspection = None;
                } else {
                    let (page, limit) = self.host_inspection_bounds;
                    inspection.key(key.code, page, limit);
                }
                return WorkInput::None;
            }
            match key.code {
                KeyCode::Esc => return WorkInput::Close,
                KeyCode::Char('R') => return WorkInput::Command(WorkCommand::HostNow),
                KeyCode::Enter if capture.is_some() => {
                    self.host_inspection =
                        host::current_present(capture.as_deref(), &self.host_filter)
                            .table
                            .open_inspection(selected);
                    return WorkInput::None;
                }
                _ => {}
            }
        }
        if let WorkResult::HostHistory(page) = &self.data
            && !matches!(
                key.code,
                KeyCode::Down | KeyCode::Up | KeyCode::Char('j' | 'k')
            )
        {
            return self
                .host_level
                .action(page, &mut self.cursor, &self.host_filter, key.code);
        }
        match key.code {
            KeyCode::Esc if self.evaluation_return.is_some() => {
                let saved = self
                    .evaluation_return
                    .take()
                    .expect("evaluation return checked");
                self.data = WorkResult::Evaluations(saved.page);
                self.cursor = saved.cursor;
                self.review_note_scroll = saved.scroll;
                WorkInput::Back
            }
            KeyCode::Esc => WorkInput::Close,
            KeyCode::PageDown if self.scrollable_details() => {
                self.review_note_scroll = self.review_note_scroll.saturating_add(4);
                WorkInput::None
            }
            KeyCode::PageUp if self.scrollable_details() => {
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
            (WorkResult::Evaluations(page), key) => return page.action(key, selected),
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
                crate::keymap::OLDER,
            ) => return WorkInput::Command(WorkCommand::Tasks(Some(*before))),
            (
                WorkResult::Handoffs {
                    older: Some(before),
                    ..
                },
                crate::keymap::OLDER,
            ) => return WorkInput::Command(WorkCommand::Handoffs(Some(*before))),
            (WorkResult::Tasks { .. }, crate::keymap::NEWER | crate::keymap::LATEST) => {
                return WorkInput::Command(WorkCommand::Tasks(None));
            }
            (WorkResult::Handoffs { .. }, crate::keymap::NEWER | crate::keymap::LATEST) => {
                return WorkInput::Command(WorkCommand::Handoffs(None));
            }
            (WorkResult::AttentionRequests(items), KeyCode::Enter | KeyCode::Char('X'))
                if let Some(item) = items.get(selected).filter(|item| !item.seen) =>
            {
                return WorkInput::Command(WorkCommand::Seen(item.clone()));
            }
            (WorkResult::Workflow { .. }, KeyCode::Char('R')) => {
                return WorkInput::Command(WorkCommand::Workflow);
            }
            (WorkResult::Workflow { items, .. }, KeyCode::Char('r' | 's' | 'X'))
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

    fn scrollable_details(&self) -> bool {
        matches!(
            self.data,
            WorkResult::ReviewNotes { .. } | WorkResult::Evaluations(_)
        )
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
                Paragraph::new("Tasks · n new linked · N blank · e edit · c done\nl link selected session or turn · r links · Del delete\nG capture Git · g saved Git · h new handoff · o handoffs\nHandoffs · e edit · x export MD+HTML · Del delete · b tasks\nWorkflow · r seen · l locate · R refresh · p pause\nPrompts · Enter reveal · m activity details · Esc close\nHost history · Enter drill down · ] older · Esc back")
                    .wrap(Wrap { trim: false }).block(panel("")),
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
            frame.render_widget(paragraph.block(panel("")), body);
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
                    .block(panel("")),
                body,
            );
            draw_footer(frame, footer, "b tasks · Esc back", pending, status);
            return;
        }
        let selected = self.cursor.selected().unwrap_or_default();
        if let WorkResult::HostNow(capture) = &self.data {
            let view = host::current_present(capture.as_deref(), &self.host_filter);
            let body = if self.host_inspection.is_none()
                && let Some(label) = self.host_filter.label()
            {
                let [query, rest] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(body);
                frame.render_widget(Paragraph::new(label).style(label_style()), query);
                rest
            } else {
                body
            };
            frame.render_widget(
                Paragraph::new(format!(" Agenturm / {}", view.table.spec.title))
                    .style(heading_style()),
                header,
            );
            let keys = if let Some(inspection) = &mut self.host_inspection {
                let page = usize::from(body.height.saturating_sub(2)).max(1);
                let limit = view.table.inspection_limit(selected, body.width, page);
                self.host_inspection_bounds = (page, limit);
                inspection.clamp(limit);
                view.table.draw_expanded(frame, body, selected, inspection);
                "↑↓ scroll · PgUp/PgDn · Esc processes"
            } else {
                view.table.draw(frame, body, selected);
                view.keys
            };
            draw_footer(frame, footer, keys, pending, status);
            return;
        }
        if let WorkResult::HostHistory(page) = &self.data {
            let view = self.host_level.present(page, &self.host_filter);
            let body = if matches!(self.host_level, host::HostLevel::Processes(_))
                && let Some(label) = self.host_filter.label()
            {
                let [query, rest] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(body);
                frame.render_widget(Paragraph::new(label).style(label_style()), query);
                rest
            } else {
                body
            };
            frame.render_widget(
                Paragraph::new(format!(" Agenturm / {}", view.table.spec.title))
                    .style(heading_style()),
                header,
            );
            view.table.draw(frame, body, selected);
            draw_footer(frame, footer, view.keys, pending, status);
            return;
        }
        let (title, keys, options, details, empty) = match &self.data {
            WorkResult::Evaluations(page) => (
                page.title(),
                page.keys(selected),
                page.row_labels(),
                page.details(selected),
                page.empty(),
            ),
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
            WorkResult::AttentionRequests(items) => (
                "Mark one pending request seen",
                "Enter mark seen · ↑↓ select · Esc back",
                items
                    .iter()
                    .map(|item| {
                        format!(
                            "{} · worker {} · turn {}",
                            if item.seen {
                                "Pending · seen"
                            } else {
                                "Pending"
                            },
                            item.owner
                                .worker
                                .as_deref()
                                .map(visible_text)
                                .as_deref()
                                .unwrap_or("session"),
                            item.owner
                                .turn
                                .as_deref()
                                .map(visible_text)
                                .as_deref()
                                .unwrap_or("unscoped"),
                        )
                    })
                    .collect(),
                items.get(selected).map(request_details).unwrap_or_default(),
                "No eligible pending requests",
            ),
            WorkResult::Workflow { items, partial } => (
                if *partial {
                    "Workflow · recent history"
                } else {
                    "Workflow"
                },
                "r seen · l locate terminal · R refresh · Esc back",
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
            WorkResult::HostHistory(_)
            | WorkResult::HostNow(_)
            | WorkResult::Investigation(_)
            | WorkResult::Seen(_)
            | WorkResult::Status(_) => return,
        };
        frame.render_widget(
            Paragraph::new(format!(" Agenturm / {title}")).style(heading_style()),
            header,
        );
        let [list, detail] = if body.width >= 96 {
            Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(body)
        } else {
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body)
        };
        let options: Vec<String> = options;
        if options.is_empty() {
            let mut lines = vec![Line::from(empty)];
            if matches!(self.data, WorkResult::Evaluations(_)) {
                lines.push(Line::default());
                lines.extend(details);
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(panel("")),
                body,
            );
        } else {
            let items = options
                .into_iter()
                .enumerate()
                .map(|(index, text)| ListItem::new(visible_text(&text)).style(theme::row(index)));
            frame.render_stateful_widget(
                List::new(items)
                    .block(panel(title))
                    .highlight_symbol("› ")
                    .highlight_style(selection_style()),
                list,
                &mut self.cursor,
            );
            let paragraph = if self.scrollable_details() {
                let details = wrap_review_note(details, detail);
                self.review_note_scroll = self
                    .review_note_scroll
                    .min(note_scroll_limit(details.len(), detail));
                Paragraph::new(details).scroll((self.review_note_scroll, 0))
            } else {
                Paragraph::new(details).wrap(Wrap { trim: false })
            };
            frame.render_widget(paragraph.block(panel("Details")), detail);
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
        Paragraph::new(vec![Line::from(keys), Line::from(status)]).style(theme::footer()),
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
