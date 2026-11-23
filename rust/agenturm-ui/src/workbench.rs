//! Keyboard-first lists and editors for actions owned by the application.

use super::*;
use crate::workspace::{RecordedSession, ReviewVersion};
use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::workflow_alerts::WorkflowAlertPage;
use agenturm_kernel::workflow_notifications::{
    WorkflowDelivery, WorkflowDeliveryState, WorkflowNotificationAuthority,
    WorkflowNotificationPolicy, WorkflowPolicyChange,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

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

/// An edit selected from a retained, snapshot-pinned session Review.
#[derive(Clone, Debug)]
pub struct ReviewEditSelection {
    pub operation: OperationDetailEntry,
    pub snapshot_id: ReadSnapshot,
    pub history_identity: String,
}

/// Exact metadata for a producer-captured span text request.
#[derive(Clone, Debug, Eq, PartialEq)]
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

impl TelemetrySpanEntry {
    pub fn from_observation(
        row: &agenturm_kernel::Observation,
        snapshot_id: ReadSnapshot,
        history_identity: &str,
    ) -> Option<Self> {
        let agenturm_kernel::ObservationKind::TraceSpan(span) = &row.kind else {
            return None;
        };
        // The captured-text vault accepts only these canonical revision-zero
        // span receipts. A metadata correction cannot select the older body.
        if row.revision != 0
            || row.identity.operation_id.is_some()
            || row.event_id != format!("otlp:{}:{}", span.trace_id, span.span_id)
        {
            return None;
        }
        Some(Self {
            history_identity: history_identity.to_owned(),
            source_id: row.harness_id.clone(),
            native_source_id: span.target_source_id.clone(),
            trace_id: span.trace_id.clone(),
            span_id: span.span_id.clone(),
            session_id: row.identity.session_id.clone()?,
            turn_id: row.identity.turn_id.clone(),
            worker_id: row.identity.worker_id.clone(),
            snapshot_id,
        })
    }
}

/// Body-free identity of a single producer-captured log record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryLogEntry {
    pub history_identity: String,
    pub source_id: ExtensionId,
    pub native_source_id: Option<ExtensionId>,
    pub event_id: String,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
    pub identity: agenturm_kernel::ObservationIdentity,
    pub snapshot_id: ReadSnapshot,
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
    guarded_active: Arc<AtomicBool>,
}

pub(super) struct PromptReceiver {
    pub(super) frames: mpsc::Receiver<Vec<u8>>,
    pub(super) acknowledgement: mpsc::Sender<()>,
    pub(super) active: Arc<Mutex<bool>>,
    pub(super) guarded_active: Arc<AtomicBool>,
    pub(super) kind: PromptKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PromptKind {
    User,
    Mcp,
    Edit,
    Telemetry,
    TelemetryLog,
    Evaluation,
    Investigation,
    LegacyReview,
    LegacyFinding,
}

impl PromptKind {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::User => "Granted prompt",
            Self::Mcp => "Granted MCP payload",
            Self::Edit => "Granted file edit",
            Self::Telemetry => "Producer-captured text",
            Self::TelemetryLog => "Producer-captured log text",
            Self::Evaluation => "Imported evaluation text",
            Self::Investigation => "Verified source evidence",
            Self::LegacyReview => "Python receiver · verified Review",
            Self::LegacyFinding => "Python receiver · verified finding source",
        }
    }
}

pub(super) fn prompt_channel(kind: PromptKind) -> (PromptDelivery, PromptReceiver) {
    let (frames, received) = mpsc::channel();
    let (acknowledgement, ack) = mpsc::channel();
    let active = Arc::new(Mutex::new(true));
    let guarded_active = Arc::new(AtomicBool::new(true));
    (
        PromptDelivery {
            buffer: Vec::new(),
            frames,
            acknowledgement: ack,
            active: Arc::clone(&active),
            guarded_active: Arc::clone(&guarded_active),
        },
        PromptReceiver {
            frames: received,
            acknowledgement,
            active,
            guarded_active,
            kind,
        },
    )
}

impl PromptDelivery {
    /// Deliver one frame and check a revocable receiver authority until the
    /// terminal has redrawn its metadata view and acknowledged redaction.
    /// Each verification callback must honor the supplied absolute deadline.
    pub fn flush_guarded(
        &mut self,
        mut verify: impl FnMut(Instant) -> Result<(), String>,
    ) -> Result<(), String> {
        const INTERVAL: Duration = Duration::from_secs(1);
        const CHECK_LIMIT: Duration = Duration::from_millis(750);
        const DISPLAY_LIMIT: Duration = Duration::from_secs(30);
        const UNCONFIRMED: &str = "Private Review redaction was not acknowledged";
        if let Err(error) = verify(Instant::now() + CHECK_LIMIT) {
            self.buffer.clear();
            self.close_guard();
            return Err(error);
        }
        self.frames
            .send(std::mem::take(&mut self.buffer))
            .map_err(|_| "Private Review window closed")?;
        let deadline = Instant::now() + DISPLAY_LIMIT;
        loop {
            let now = Instant::now();
            match self
                .acknowledgement
                .recv_timeout(deadline.saturating_duration_since(now).min(INTERVAL))
            {
                Ok(()) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.close_guard();
                    return Err(UNCONFIRMED.into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            let verified = if now < deadline {
                verify((now + CHECK_LIMIT).min(deadline))
            } else {
                Err(UNCONFIRMED.into())
            };
            if let Err(error) = verified {
                self.close_guard();
                self.acknowledgement
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| UNCONFIRMED.to_owned())?;
                return Err(error);
            }
        }
    }

    fn close_guard(&self) {
        self.guarded_active.store(false, Ordering::Release);
    }
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
mod workflow_alerts;
pub(crate) mod workflow_resume;
mod workflow_settings;
use workflow_resume::WorkflowResume;

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
    WorkflowAlerts,
    WorkflowSettings,
    ChangeWorkflowSettings {
        authority: WorkflowNotificationAuthority,
        expected_revision: u64,
        change: WorkflowPolicyChange,
    },
    RetryWorkflow {
        authority: WorkflowNotificationAuthority,
        key: [u8; 32],
        expected_revision: u64,
        expected_attempt: u64,
    },
    WorkflowResume(RequestEntry),
    SessionAttention(RecordedSession),
    Seen(RequestEntry),
    Locate(RequestEntry),
    Prompts(WorkAnchor),
    RevealPrompt(Box<PromptEntry>, PromptDelivery),
    RevealOperation(OperationDetailEntry, PromptDelivery),
    RevealReviewEdit(Box<ReviewEditSelection>, PromptDelivery),
    RevealTelemetry(Box<TelemetrySpanEntry>, PromptDelivery),
    RevealTelemetryLog(Box<TelemetryLogEntry>, PromptDelivery),
    RevealEvalDetail(Box<evaluation::EvaluationDetailEntry>, PromptDelivery),
    RevealFailedEvalFinding {
        detail: Box<evaluation::EvaluationDetailEntry>,
        history_identity: String,
        evidence_snapshot: ReadSnapshot,
        delivery: PromptDelivery,
    },
}

pub struct ReviewNoteEntry {
    pub version: ReviewVersion,
    pub read_incarnation: Option<agenturm_kernel::ReadIncarnation>,
    pub note: String,
}

/// A same-user host capture or a retained sample from an authorized reader.
pub struct CurrentHostSample {
    pub captured_at_unix_ns: u64,
    /// The retained source, when this sample was read from a receiver.
    pub recorded_source: Option<agenturm_kernel::ExtensionId>,
    pub sample: agenturm_kernel::HostSample,
}

fn delivery_label(state: WorkflowDeliveryState) -> &'static str {
    match state {
        WorkflowDeliveryState::Claimed => "Claimed",
        WorkflowDeliveryState::Delivered => "Delivered",
        WorkflowDeliveryState::Failed => "Failed",
        WorkflowDeliveryState::Unavailable => "Unavailable",
        WorkflowDeliveryState::Uncertain => "Uncertain",
    }
}

fn retry_command(delivery: &WorkflowDelivery) -> Option<WorkCommand> {
    delivery
        .state
        .retryable()
        .then(|| WorkCommand::RetryWorkflow {
            authority: delivery.authority.clone(),
            key: delivery.key,
            expected_revision: delivery.policy_revision,
            expected_attempt: delivery.attempt,
        })
}

fn replace_delivery_state(stored: &mut WorkflowDelivery, updated: &WorkflowDelivery) -> bool {
    if stored.authority != updated.authority
        || stored.key != updated.key
        || stored.policy_revision != updated.policy_revision
        || stored.attempt.checked_add(1) != Some(updated.attempt)
        || !stored.state.retryable()
    {
        return false;
    }
    stored.attempt = updated.attempt;
    stored.state = updated.state;
    true
}

pub struct WorkflowAlertInbox {
    pub page: WorkflowAlertPage,
    pub deliveries: Vec<(String, WorkflowDelivery)>,
}

pub enum WorkResult {
    Investigation(Box<investigation::InvestigationPage>),
    Evaluations(Box<evaluation::EvaluationPage>),
    HostHistory(agenturm_kernel::ObservationPage),
    HostInventory(agenturm_kernel::ObservationPage),
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
        deliveries: Vec<(RequestEntry, WorkflowDelivery)>,
    },
    WorkflowAlerts(Box<WorkflowAlertInbox>),
    WorkflowSettings {
        policy: Box<WorkflowNotificationPolicy>,
        authority: WorkflowNotificationAuthority,
    },
    WorkflowResume(Box<WorkflowResume>),
    WorkflowDeliveryUpdated(WorkflowDelivery),
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
    resume_preview: Option<(Box<WorkflowResume>, String)>,
    workflow_settings: Option<workflow_settings::Settings>,
    workflow_alerts: Option<workflow_alerts::Alerts>,
    show_workflow_alerts: bool,
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
            resume_preview: None,
            workflow_settings: None,
            workflow_alerts: None,
            show_workflow_alerts: false,
        };
        work.reset_cursor();
        work
    }

    pub(super) fn apply(&mut self, data: WorkResult) {
        let data = match data {
            WorkResult::WorkflowSettings { policy, authority } => {
                if let Some(settings) = &mut self.workflow_settings {
                    settings.apply(policy, authority);
                } else {
                    self.workflow_settings =
                        Some(workflow_settings::Settings::new(policy, authority));
                }
                return;
            }
            WorkResult::WorkflowAlerts(inbox) => {
                if let Some(alerts) = &mut self.workflow_alerts {
                    alerts.apply(inbox);
                } else {
                    self.workflow_alerts = Some(workflow_alerts::Alerts::new(inbox));
                }
                self.show_workflow_alerts = true;
                return;
            }
            data => data,
        };
        let previous = self.selected_task().map(|item| item.id);
        let selected_process = self.current_process_identity();
        let selected_host_stream = match &self.data {
            WorkResult::HostInventory(page) => self.cursor.selected().and_then(|selected| {
                page.observations
                    .iter()
                    .rev()
                    .nth(selected)
                    .map(|row| row.harness_id.clone())
            }),
            _ => None,
        };
        let selected_request = match &self.data {
            WorkResult::Workflow { items, .. } => self
                .cursor
                .selected()
                .and_then(|index| items.get(index))
                .map(|entry| (entry.owner.clone(), entry.event_id.clone(), entry.revision)),
            _ => None,
        };
        self.draft = None;
        self.resume_preview = None;
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
        if let (Some((source, boot_id, pid, start_ticks)), WorkResult::HostNow(Some(capture))) =
            (selected_process, &self.data)
            && capture.recorded_source == source
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
        if let (Some(source), WorkResult::HostInventory(page)) = (selected_host_stream, &self.data)
            && let Some(index) = page
                .observations
                .iter()
                .rev()
                .position(|row| row.harness_id == source)
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

    pub(super) fn workflow_refresh(&self) -> Option<WorkCommand> {
        if matches!(self.data, WorkResult::Workflow { .. })
            && !self.help
            && self.resume_preview.is_none()
            && self.workflow_settings.is_none()
        {
            Some(if self.show_workflow_alerts {
                WorkCommand::WorkflowAlerts
            } else {
                WorkCommand::Workflow
            })
        } else {
            None
        }
    }

    pub(super) fn accept_resume(
        &mut self,
        receipt: Box<WorkflowResume>,
    ) -> Result<String, &'static str> {
        let WorkResult::Workflow { items, .. } = &self.data else {
            return Err("The selected workflow request changed");
        };
        if self.help
            || self.cursor.selected().and_then(|index| items.get(index)) != Some(&receipt.request)
        {
            return Err("The selected workflow request changed");
        }
        let command = receipt
            .command()
            .ok_or("Native resume is unavailable for this request")?;
        self.resume_preview = Some((receipt, command.clone()));
        Ok(command)
    }

    pub(super) fn accept_delivery(
        &mut self,
        updated: &WorkflowDelivery,
    ) -> Result<&'static str, &'static str> {
        let applied = if self.show_workflow_alerts {
            self.workflow_alerts
                .as_mut()
                .is_some_and(|alerts| alerts.accept_delivery(updated))
        } else if let WorkResult::Workflow {
            items, deliveries, ..
        } = &mut self.data
        {
            deliveries.iter_mut().any(|(request, stored)| {
                request.proof.history_identity.as_deref()
                    == Some(updated.authority.history_identity.as_str())
                    && request.proof.snapshot_id == updated.authority.snapshot_id
                    && items.contains(request)
                    && replace_delivery_state(stored, updated)
            })
        } else {
            false
        };
        if applied {
            Ok(delivery_label(updated.state))
        } else {
            Err("The workflow delivery receipt changed")
        }
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
            WorkResult::HostHistory(page) | WorkResult::HostInventory(page) => {
                self.host_level.len(page, &self.host_filter)
            }
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
            WorkResult::Investigation(_)
            | WorkResult::WorkflowAlerts(_)
            | WorkResult::WorkflowSettings { .. }
            | WorkResult::WorkflowResume(_)
            | WorkResult::WorkflowDeliveryUpdated(_)
            | WorkResult::Seen(_)
            | WorkResult::Status(_) => 0,
        }
    }

    fn selected_task(&self) -> Option<&TaskEntry> {
        match &self.data {
            WorkResult::Tasks { items, .. } => items.get(self.cursor.selected()?),
            _ => None,
        }
    }

    fn workflow_delivery(&self, index: usize) -> Option<&WorkflowDelivery> {
        let WorkResult::Workflow {
            items, deliveries, ..
        } = &self.data
        else {
            return None;
        };
        let selected = items.get(index)?;
        deliveries
            .iter()
            .find(|(request, delivery)| {
                request == selected
                    && selected.proof.history_identity.as_deref()
                        == Some(delivery.authority.history_identity.as_str())
                    && selected.proof.snapshot_id == delivery.authority.snapshot_id
            })
            .map(|(_, delivery)| delivery)
    }

    fn current_process_identity(
        &self,
    ) -> Option<(Option<agenturm_kernel::ExtensionId>, String, u32, u64)> {
        let WorkResult::HostNow(Some(capture)) = &self.data else {
            return None;
        };
        let visible = host::current_indices(&capture.sample, &self.host_filter);
        let index = *visible.get(self.cursor.selected()?)?;
        let process = &capture.sample.processes[index];
        Some((
            capture.recorded_source.clone(),
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
            WorkResult::HostHistory(page) | WorkResult::HostInventory(page) => self
                .host_level
                .filter_key(page, &mut self.host_filter, &mut self.cursor, key),
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
        if self.resume_preview.is_some() {
            return if key.code == KeyCode::Esc {
                self.resume_preview = None;
                WorkInput::Back
            } else {
                WorkInput::None
            };
        }
        if let Some(settings) = &mut self.workflow_settings {
            let input = settings.key(key);
            if matches!(input, WorkInput::Back) {
                self.workflow_settings = None;
            }
            return input;
        }
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
        if matches!(self.data, WorkResult::Workflow { .. }) {
            if key.code == KeyCode::Char('S') {
                return WorkInput::Command(WorkCommand::WorkflowSettings);
            }
            if self.show_workflow_alerts
                && let Some(alerts) = &mut self.workflow_alerts
            {
                if alerts.inspecting() {
                    return alerts.key(key.code);
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Tab => {
                        self.show_workflow_alerts = false;
                        return WorkInput::Back;
                    }
                    KeyCode::Char('R') => return WorkInput::Command(WorkCommand::WorkflowAlerts),
                    KeyCode::Char('T') => {
                        return alerts.retry().map_or(WorkInput::None, WorkInput::Command);
                    }
                    _ => return alerts.key(key.code),
                }
            }
            if key.code == KeyCode::Tab {
                return WorkInput::Command(WorkCommand::WorkflowAlerts);
            }
            if key.code == KeyCode::Char('T') {
                return self
                    .workflow_delivery(selected)
                    .and_then(retry_command)
                    .map_or(WorkInput::None, WorkInput::Command);
            }
        }
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
        if let WorkResult::HostHistory(page) | WorkResult::HostInventory(page) = &self.data
            && !matches!(
                key.code,
                KeyCode::Down | KeyCode::Up | KeyCode::Char('j' | 'k')
            )
        {
            return self.host_level.action(
                page,
                &mut self.cursor,
                &self.host_filter,
                key.code,
                matches!(self.data, WorkResult::HostInventory(_)),
            );
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
            (WorkResult::Workflow { items, .. }, KeyCode::Char('c'))
                if let Some(item) = items.get(selected) =>
            {
                return WorkInput::Command(WorkCommand::WorkflowResume(item.clone()));
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
        self.draft.is_some() || self.workflow_settings.is_some()
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
        if let Some(settings) = &self.workflow_settings {
            settings.draw(frame, pending, status);
            return;
        }
        if self.show_workflow_alerts
            && !self.help
            && let Some(alerts) = &mut self.workflow_alerts
        {
            alerts.draw(frame, pending, status);
            return;
        }
        let footer_height = if matches!(self.data, WorkResult::Workflow { .. }) {
            3
        } else {
            2
        };
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(footer_height),
        ])
        .areas(area);
        if let Some((receipt, command)) = &self.resume_preview {
            frame.render_widget(
                Paragraph::new(" Agenturm / Native resume command").style(heading_style()),
                header,
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(format!(
                        "Source  {}",
                        visible_text(&receipt.request.owner.source)
                    )),
                    Line::from(""),
                    Line::styled(command.clone(), label_style()),
                    Line::from(""),
                    Line::from("Check for an active session before running the command."),
                ])
                .wrap(Wrap { trim: false })
                .block(panel(" Selected request ")),
                body,
            );
            draw_footer(frame, footer, "Esc Workflow", pending, status);
            return;
        }
        if self.help {
            frame.render_widget(Paragraph::new(" Workbench keys"), header);
            frame.render_widget(
                Paragraph::new("Tasks · n new linked · N blank · e edit · c done\nl link selected session or turn · r links · Del delete\nG capture Git · g saved Git · h new handoff · o handoffs\nHandoffs · e edit · x export MD+HTML · Del delete · b tasks\nWorkflow · r seen · l locate · c copy resume · R refresh\nTab incidents · S settings · T retry eligible delivery\nPrompts · Enter reveal · m activity details · Esc close\nHost history · Enter drill down · ] older · Esc back")
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
        if let WorkResult::HostHistory(page) | WorkResult::HostInventory(page) = &self.data {
            let view = self.host_level.present(
                page,
                &self.host_filter,
                matches!(self.data, WorkResult::HostInventory(_)),
            );
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
            WorkResult::Workflow { items, partial, .. } => (
                if *partial {
                    "Workflow · recent history"
                } else {
                    "Workflow"
                },
                if self
                    .workflow_delivery(selected)
                    .is_some_and(|delivery| delivery.state.retryable())
                {
                    "r seen · l locate · c copy resume · R refresh · Esc back\nTab incidents · S settings · T retry delivery"
                } else {
                    "r seen · l locate · c copy resume · R refresh · Esc back\nTab incidents · S settings"
                },
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
            | WorkResult::HostInventory(_)
            | WorkResult::HostNow(_)
            | WorkResult::Investigation(_)
            | WorkResult::WorkflowAlerts(_)
            | WorkResult::WorkflowSettings { .. }
            | WorkResult::WorkflowResume(_)
            | WorkResult::WorkflowDeliveryUpdated(_)
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
            let title = self
                .workflow_delivery(selected)
                .map(|delivery| format!("Details · Delivery {}", delivery_label(delivery.state)));
            frame.render_widget(
                paragraph.block(panel(title.as_deref().unwrap_or("Details"))),
                detail,
            );
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
    let lines = keys
        .lines()
        .map(Line::from)
        .chain([Line::from(status)])
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(theme::footer()), area);
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
