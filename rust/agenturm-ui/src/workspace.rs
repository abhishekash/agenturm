//! Session navigation over immutable history pages supplied by the host.

use super::*;
use crate::keymap::{self, Destination};
use crate::workbench::investigation::{Investigation, InvestigationInput};
use crate::workbench::{PromptKind, PromptReceiver, WorkInput, Workbench, prompt_channel};
use agenturm_kernel::{ReadSnapshot, RelationshipKind};
use ratatui::buffer::CellWidth;
use ratatui::widgets::{Row, Table, TableState};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod command_center;
mod prompt;
mod view_io;

use prompt::PromptScreen;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedSession {
    pub harness_id: String,
    pub session_id: String,
    pub task_title: Option<Box<str>>,
    /// Explicit public project metadata; never inferred from prompts or local cwd.
    pub project_path: Option<Box<str>>,
    pub project_origin: Option<ProjectOrigin>,
    pub updated_at_unix_ns: u64,
    pub snapshot_id: ReadSnapshot,
    pub state: Option<String>,
    pub reason: Option<String>,
    pub turn_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub relationship_kind: Option<RelationshipKind>,
    pub review_version: ReviewVersion,
    pub reviewed: bool,
    pub history_identity: Option<String>,
    pub pending_requests: Vec<RequestEntry>,
}

/// Authority for a public project heading; never inferred from captured text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectOrigin {
    OperatorTaskWorkspace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewVersion {
    pub receipt_sequence: u64,
    pub retained_observations: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCursor {
    pub harness_id: String,
    pub session_id: String,
    pub updated_at_unix_ns: u64,
    pub snapshot_id: ReadSnapshot,
}

pub struct SessionCatalog {
    pub sessions: Vec<RecordedSession>,
    pub older: Option<SessionCursor>,
    pub notice: Option<String>,
}

#[derive(Clone, Debug)]
pub enum HistorySelection {
    All(WorkspaceView),
    Session(RecordedSession, SessionView),
    Turn(RecordedTurn),
}

#[derive(Clone, Copy, Debug)]
pub enum SessionView {
    Overview,
    Activity,
    Review,
    Usage,
    Traces,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceView {
    Overview,
    Activity,
    Mcp,
    Operations,
    Findings,
    Host,
    Metrics,
    Usage,
    Traces,
}

pub enum WorkspaceRequest {
    Sessions(Option<SessionCursor>),
    MarkReviewed(RecordedSession),
    Work(WorkCommand),
    History {
        selection: HistorySelection,
        before: Option<ObservationPosition>,
    },
}

pub enum WorkspaceResponse {
    Sessions(SessionCatalog),
    ReviewSaved,
    Work(WorkResult),
    History(HistoryPage),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Filter {
    #[default]
    All,
    Active,
    Attention,
    Finished,
    ToReview,
    Inactive,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Attention,
            Self::Attention => Self::Active,
            Self::Active => Self::Finished,
            Self::Finished => Self::Inactive,
            Self::Inactive | Self::ToReview => Self::All,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::All => Self::Inactive,
            Self::Attention => Self::All,
            Self::Active => Self::Attention,
            Self::Finished => Self::Active,
            Self::Inactive => Self::Finished,
            Self::ToReview => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Working",
            Self::Attention => "Needs you",
            Self::Finished => "Ready",
            Self::Inactive => "Inactive",
            Self::ToReview => "To review",
        }
    }

    fn accepts(self, session: &RecordedSession) -> bool {
        let state = session.state.as_deref();
        match self {
            Self::All => true,
            Self::Active => {
                !Self::Attention.accepts(session)
                    && matches!(state, Some("running" | "started" | "restored"))
            }
            Self::Attention => {
                session.pending_requests.iter().any(|request| !request.seen)
                    || matches!(
                        state,
                        Some("failed" | "interrupted" | "waiting" | "blocked")
                    )
            }
            Self::Finished => {
                !Self::Attention.accepts(session) && matches!(state, Some("completed" | "ended"))
            }
            Self::Inactive => {
                !Self::Attention.accepts(session)
                    && matches!(state, Some("idle" | "stopped" | "cancelled" | "closed"))
            }
            Self::ToReview => {
                !session.reviewed
                    && matches!(
                        state,
                        Some("completed" | "ended" | "failed" | "interrupted")
                    )
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AgentRow {
    index: usize,
    depth: usize,
    has_children: bool,
}

#[derive(Default)]
struct AgentList {
    catalog: Option<SessionCatalog>,
    visible: Vec<AgentRow>,
    selection: TableState,
    filter: Filter,
    grouping: command_center::Grouping,
    search: String,
    editing: bool,
    flat: bool,
    help: bool,
    sort_by_state: bool,
    show_more_selected: bool,
    display_offset: usize,
    paused: bool,
    collapsed: BTreeSet<(String, String)>,
}

impl AgentList {
    fn current(&self) -> Option<&RecordedSession> {
        if self.show_more_selected {
            return None;
        }
        let position = self.visible.get(self.selection.selected()?)?.index;
        self.catalog.as_ref()?.sessions.get(position)
    }

    fn update(&mut self, catalog: SessionCatalog) {
        let identity = self
            .current()
            .map(|row| (row.harness_id.clone(), row.session_id.clone()));
        self.catalog = Some(catalog);
        self.apply_filters(identity.as_ref());
    }

    fn apply_filters(&mut self, identity: Option<&(String, String)>) {
        let query = self.search.to_ascii_lowercase();
        self.visible = self.catalog.as_ref().map_or_else(Vec::new, |catalog| {
            let mut matching = catalog
                .sessions
                .iter()
                .enumerate()
                .filter_map(|(index, session)| {
                    let matches = self.filter.accepts(session)
                        && (query.is_empty()
                            || session.session_id.to_ascii_lowercase().contains(&query)
                            || session.harness_id.to_ascii_lowercase().contains(&query)
                            || session
                                .task_title
                                .as_deref()
                                .is_some_and(|title| title.to_ascii_lowercase().contains(&query))
                            || session
                                .state
                                .as_ref()
                                .is_some_and(|state| state.contains(&query))
                            || session
                                .reason
                                .as_ref()
                                .is_some_and(|reason| reason.contains(&query)));
                    matches.then_some(index)
                })
                .collect::<Vec<_>>();
            if self.sort_by_state {
                matching.sort_by_key(|&index| {
                    let row = &catalog.sessions[index];
                    (
                        !Filter::Attention.accepts(row),
                        !Filter::Active.accepts(row),
                    )
                });
            }
            agent_rows(&catalog.sessions, &matching, self.flat, &self.collapsed)
        });
        if let Some(catalog) = &self.catalog {
            self.visible.sort_by(|a, b| {
                command_center::group(&catalog.sessions[a.index], self.grouping).cmp(
                    command_center::group(&catalog.sessions[b.index], self.grouping),
                )
            });
        }
        self.show_more_selected = false;
        let selected = identity.and_then(|identity| {
            self.visible.iter().position(|item| {
                let row = &self
                    .catalog
                    .as_ref()
                    .expect("visible rows have a catalog")
                    .sessions[item.index];
                row.harness_id == identity.0 && row.session_id == identity.1
            })
        });
        self.selection
            .select((!self.visible.is_empty()).then_some(selected.unwrap_or(0)));
    }

    fn toggle_tree(&mut self) {
        let identity = self.current().map(session_key);
        self.flat = !self.flat;
        self.apply_filters(identity.as_ref());
    }

    fn expand(&mut self, expand: bool) {
        if let Some(row) = self
            .selection
            .selected()
            .and_then(|index| self.visible.get(index))
            && row.has_children
            && let Some(identity) = self.current().map(session_key)
        {
            if expand {
                self.collapsed.remove(&identity);
            } else {
                if self.collapsed.len() >= 200 {
                    self.collapsed.pop_first();
                }
                self.collapsed.insert(identity.clone());
            }
            self.apply_filters(Some(&identity));
        }
    }

    fn inspect_relative(&mut self, direction: KeyCode) {
        if direction == KeyCode::Down || direction == KeyCode::Right {
            self.expand(true);
        }
        let Some(index) = self.selection.selected() else {
            return;
        };
        let Some(current) = self.current() else {
            return;
        };
        let sessions = &self
            .catalog
            .as_ref()
            .expect("selected session has catalog")
            .sessions;
        let next = match direction {
            KeyCode::Up => current.parent_session_id.as_deref().and_then(|parent| {
                self.visible.iter().position(|row| {
                    let candidate = &sessions[row.index];
                    candidate.harness_id == current.harness_id && candidate.session_id == parent
                })
            }),
            KeyCode::Down => self.visible.iter().position(|row| {
                let candidate = &sessions[row.index];
                candidate.harness_id == current.harness_id
                    && candidate.parent_session_id.as_deref() == Some(&current.session_id)
            }),
            KeyCode::Right if !self.flat => {
                let depth = self.visible[index].depth;
                self.visible
                    .iter()
                    .enumerate()
                    .skip(index + 1)
                    .take_while(|(_, row)| row.depth > depth)
                    .find_map(|(position, row)| {
                        Filter::Attention
                            .accepts(&sessions[row.index])
                            .then_some(position)
                    })
            }
            _ => None,
        };
        if let Some(next) = next {
            self.selection.select(Some(next));
        }
    }

    fn move_selection(&mut self, down: bool) {
        if self.show_more_selected {
            if !down {
                self.show_more_selected = false;
            }
            return;
        }
        if down
            && self
                .selection
                .selected()
                .is_some_and(|i| i + 1 == self.visible.len())
            && self
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.older.is_some())
        {
            self.show_more_selected = true;
            return;
        }
        if let Some(index) = self.selection.selected() {
            let next = if down {
                (index + 1).min(self.visible.len().saturating_sub(1))
            } else {
                index.saturating_sub(1)
            };
            self.selection.select(Some(next));
        }
    }
}

fn session_key(row: &RecordedSession) -> (String, String) {
    (row.harness_id.clone(), row.session_id.clone())
}

fn source_family(source: &str) -> &str {
    // Generated per-file discovery IDs use a 32-character path digest.
    // Explicit source IDs retain their complete namespace.
    if let Some((family, suffix)) = source.rsplit_once('.')
        && family.starts_with("harness.")
        && suffix.len() == 32
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return family;
    }
    source
}

fn agent_rows(
    sessions: &[RecordedSession],
    matching: &[usize],
    flat: bool,
    collapsed: &BTreeSet<(String, String)>,
) -> Vec<AgentRow> {
    let mut rows = Vec::with_capacity(matching.len());
    if flat {
        rows.extend(matching.iter().map(|&index| AgentRow {
            index,
            depth: 0,
            has_children: false,
        }));
        return rows;
    }
    let mut identities = HashMap::new();
    for (index, row) in sessions.iter().enumerate() {
        let key = (source_family(&row.harness_id), row.session_id.as_str());
        identities
            .entry(key)
            .and_modify(|id| *id = None)
            .or_insert(Some(index));
    }
    let selected = matching.iter().copied().collect::<BTreeSet<_>>();
    let mut parents = vec![None; sessions.len()];
    let mut children = vec![Vec::new(); sessions.len()];
    for &index in matching {
        let row = &sessions[index];
        if row.relationship_kind == Some(RelationshipKind::Delegated)
            && let Some(parent) = row.parent_session_id.as_deref()
            && let Some(Some(parent)) = identities.get(&(source_family(&row.harness_id), parent))
            && *parent != index
            && selected.contains(parent)
        {
            parents[index] = Some(*parent);
            children[*parent].push(index);
        }
    }
    // Treat a cycle and its connected descendants as separate rows. There is
    // no verified root for that projection.
    let mut seen = vec![false; sessions.len()];
    let mut stack = Vec::new();
    for &root in matching.iter().filter(|&&id| parents[id].is_none()) {
        stack.push((root, 0usize, true));
        while let Some((index, depth, show)) = stack.pop() {
            if std::mem::replace(&mut seen[index], true) {
                continue;
            }
            let has_children = !children[index].is_empty();
            if show {
                rows.push(AgentRow {
                    index,
                    depth,
                    has_children,
                });
            }
            let show_children =
                show && (!has_children || !collapsed.contains(&session_key(&sessions[index])));
            stack.extend(
                children[index]
                    .iter()
                    .rev()
                    .map(|&child| (child, depth + 1, show_children)),
            );
        }
    }
    rows.extend(
        matching
            .iter()
            .filter(|&&id| !seen[id])
            .map(|&index| AgentRow {
                index,
                depth: 0,
                has_children: false,
            }),
    );
    rows
}

pub(super) fn move_selection(selection: &mut ListState, count: usize, down: bool) {
    if count != 0 {
        let current = selection.selected().unwrap_or_default();
        let next = if down {
            (current + 1).min(count - 1)
        } else {
            current.saturating_sub(1)
        };
        selection.select(Some(next));
    }
}

const CACHE_LIMIT: usize = 4;
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

struct HistoryView {
    selection: HistorySelection,
    page: PageView,
    newer: VecDeque<PageView>,
}

impl HistoryView {
    fn new(selection: HistorySelection, page: HistoryPage) -> Self {
        let in_turn = matches!(selection, HistorySelection::Turn(_));
        let mut page = PageView::new(page, in_turn);
        if matches!(selection, HistorySelection::All(WorkspaceView::Metrics)) {
            page.ui.select_metrics();
        }
        Self {
            selection,
            page,
            newer: VecDeque::new(),
        }
    }
}

enum Change {
    RefreshSessions,
    OlderSessions,
    Open(HistorySelection),
    Navigate(HistorySelection),
    SessionMode(HistorySelection),
    OlderHistory,
    LatestHistory,
    MarkReviewed(RecordedSession),
    Work(Option<WorkAnchor>),
    Investigation,
}

struct Workspace {
    view_io: view_io::ViewIo,
    agents: AgentList,
    newer_sessions: VecDeque<SessionCatalog>,
    browsing_older: bool,
    history: Vec<HistoryView>,
    work: Option<Workbench>,
    investigation: Option<Investigation>,
    discard_work_result: bool,
    prompt_receiver: Option<PromptReceiver>,
    queued_prompt: Option<PromptEntry>,
    prompt: Option<PromptScreen>,
    redact_then_acknowledge: Option<mpsc::Sender<()>>,
    pending: Option<Change>,
    deferred_request: Option<(WorkspaceRequest, Change)>,
    message: Option<String>,
    refresh_after: Instant,
}

impl Workspace {
    fn new() -> Self {
        Self {
            view_io: view_io::ViewIo::default(),
            agents: AgentList::default(),
            newer_sessions: VecDeque::new(),
            browsing_older: false,
            history: Vec::new(),
            work: None,
            investigation: None,
            discard_work_result: false,
            prompt_receiver: None,
            queued_prompt: None,
            prompt: None,
            redact_then_acknowledge: None,
            pending: None,
            deferred_request: None,
            message: None,
            refresh_after: Instant::now(),
        }
    }

    fn request(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        request: WorkspaceRequest,
        change: Change,
    ) {
        if self.pending.is_some() {
            if matches!(self.pending, Some(Change::RefreshSessions))
                && !matches!(change, Change::RefreshSessions)
            {
                self.deferred_request = Some((request, change));
            }
            return;
        }
        if sender.send(request).is_ok() {
            self.pending = Some(change);
            self.message = None;
        } else {
            self.message = Some("History reader stopped".into());
        }
    }

    fn finish(&mut self, result: Result<WorkspaceResponse, String>) {
        let Some(change) = self.pending.take() else {
            return;
        };
        self.refresh_after = Instant::now() + REFRESH_INTERVAL;
        if matches!(change, Change::Work(_)) && std::mem::take(&mut self.discard_work_result) {
            return;
        }
        if matches!(change, Change::Investigation) && self.investigation.is_none() {
            return;
        }
        match (change, result) {
            (_, Err(error)) => {
                self.prompt_receiver = None;
                self.message = Some(error);
            }
            (Change::RefreshSessions, Ok(WorkspaceResponse::Sessions(catalog))) => {
                self.agents.update(catalog);
                self.browsing_older = false;
                self.newer_sessions.clear();
            }
            (Change::OlderSessions, Ok(WorkspaceResponse::Sessions(catalog))) => {
                if let Some(previous) = self.agents.catalog.take() {
                    push_cached(&mut self.newer_sessions, previous);
                }
                self.agents.update(catalog);
                self.browsing_older = true;
            }
            (Change::Open(selection), Ok(WorkspaceResponse::History(page))) => {
                self.history.push(HistoryView::new(selection, page));
            }
            (Change::Navigate(selection), Ok(WorkspaceResponse::History(page))) => {
                self.history.clear();
                self.history.push(HistoryView::new(selection, page));
            }
            (Change::SessionMode(selection), Ok(WorkspaceResponse::History(page))) => {
                let next = HistoryView::new(selection, page);
                if let Some(view) = self.history.last_mut() {
                    *view = next;
                } else {
                    self.history.push(next);
                }
            }
            (
                change @ (Change::OlderHistory | Change::LatestHistory),
                Ok(WorkspaceResponse::History(page)),
            ) => {
                if let Some(view) = self.history.last_mut() {
                    let mut next =
                        PageView::new(page, matches!(view.selection, HistorySelection::Turn(_)));
                    if let Some(title) = view.page.ui.selected_title() {
                        if title == "Metrics"
                            && matches!(
                                view.selection,
                                HistorySelection::All(WorkspaceView::Metrics)
                            )
                        {
                            next.ui.select_metrics();
                        } else {
                            next.ui.select_title(title);
                        }
                    }
                    if matches!(change, Change::OlderHistory) {
                        push_cached(&mut view.newer, std::mem::replace(&mut view.page, next));
                    } else {
                        view.newer.clear();
                        view.page = next;
                    }
                }
            }
            (Change::MarkReviewed(session), Ok(WorkspaceResponse::ReviewSaved)) => {
                let identity = session_key(&session);
                if let Some(catalog) = &mut self.agents.catalog
                    && let Some(row) = catalog.sessions.iter_mut().find(|row| {
                        row.harness_id == session.harness_id
                            && row.session_id == session.session_id
                            && row.review_version == session.review_version
                    })
                {
                    row.reviewed = true;
                    self.agents.apply_filters(Some(&identity));
                }
                if let Some(view) = self.history.last_mut()
                    && let HistorySelection::Session(row, _) = &mut view.selection
                    && row.harness_id == session.harness_id
                    && row.session_id == session.session_id
                    && row.review_version == session.review_version
                {
                    row.reviewed = true;
                }
                self.message = Some("Review saved for this evidence".into());
            }
            (
                Change::Work(_) | Change::Investigation,
                Ok(WorkspaceResponse::Work(WorkResult::Status(status))),
            ) => {
                self.prompt_receiver = None;
                self.message = (!status.is_empty()).then_some(status);
            }
            (
                Change::Investigation,
                Ok(WorkspaceResponse::Work(WorkResult::Investigation(page))),
            ) => {
                if let Some(investigation) = &mut self.investigation {
                    investigation.apply(*page);
                }
            }
            (Change::Work(_), Ok(WorkspaceResponse::Work(WorkResult::Seen(entry)))) => {
                if let Some(work) = &mut self.work {
                    work.note_seen(&entry);
                }
                let identity = (entry.owner.source.clone(), entry.owner.session.clone());
                if let Some(row) = self.agents.catalog.as_mut().and_then(|catalog| {
                    catalog
                        .sessions
                        .iter_mut()
                        .find(|row| session_key(row) == identity)
                }) && let Some(request) = row.pending_requests.iter_mut().find(|request| {
                    request.owner == entry.owner
                        && request.event_id == entry.event_id
                        && request.revision == entry.revision
                        && request.proof.history_identity == entry.proof.history_identity
                        && request.proof.snapshot_id.incarnation()
                            == entry.proof.snapshot_id.incarnation()
                }) {
                    request.seen = true;
                    self.agents.apply_filters(Some(&identity));
                }
                self.message = Some(
                    "Marked seen. The request stays open until its source records resolution."
                        .into(),
                );
            }
            (Change::Work(target), Ok(WorkspaceResponse::Work(data))) => {
                if let Some(work) = &mut self.work {
                    work.apply(data);
                } else {
                    self.work = Some(Workbench::new(data, target));
                }
            }
            _ => self.message = Some("History reader returned an unexpected response".into()),
        }
    }

    fn open(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, selection: HistorySelection) {
        self.request(
            sender,
            WorkspaceRequest::History {
                selection: selection.clone(),
                before: None,
            },
            Change::Open(selection),
        );
    }

    fn navigate(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, destination: Destination) {
        let view = match destination {
            Destination::Agents => {
                if self.pending.is_none() {
                    self.history.clear();
                    self.message = None;
                }
                return;
            }
            Destination::Host => {
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::HostHistory(None)),
                    Change::Work(None),
                );
                return;
            }
            Destination::Activity => WorkspaceView::Activity,
            Destination::Mcp => WorkspaceView::Mcp,
            Destination::Traces => WorkspaceView::Traces,
            Destination::Metrics => WorkspaceView::Metrics,
            Destination::Findings => WorkspaceView::Findings,
            Destination::Operations => WorkspaceView::Operations,
        };
        let selection = HistorySelection::All(view);
        self.request(
            sender,
            WorkspaceRequest::History {
                selection: selection.clone(),
                before: None,
            },
            Change::Navigate(selection),
        );
    }

    fn select_session_mode(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        session: RecordedSession,
        mode: SessionView,
    ) {
        let selection = HistorySelection::Session(session, mode);
        self.request(
            sender,
            WorkspaceRequest::History {
                selection: selection.clone(),
                before: None,
            },
            Change::SessionMode(selection),
        );
    }

    fn toggle_pause(&mut self) {
        self.agents.paused = !self.agents.paused;
        self.message = Some(
            if self.agents.paused {
                "Auto-refresh paused"
            } else {
                "Auto-refresh resumed"
            }
            .into(),
        );
    }

    fn reveal(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, entry: PromptEntry) {
        let (delivery, receiver) = prompt_channel(PromptKind::User);
        self.request(
            sender,
            WorkspaceRequest::Work(WorkCommand::RevealPrompt(Box::new(entry), delivery)),
            Change::Work(None),
        );
        self.prompt_receiver = Some(receiver);
    }

    fn dismiss_prompt(&mut self) -> bool {
        let Some(mut prompt) = self.prompt.take() else {
            return false;
        };
        self.redact_then_acknowledge = prompt.acknowledge.take();
        true
    }

    fn expire_prompt(&mut self, now: Instant) -> bool {
        self.prompt
            .as_ref()
            .is_some_and(|prompt| prompt.expired(now))
            && self.dismiss_prompt()
    }

    fn acknowledge_redaction(&mut self) {
        if let Some(acknowledge) = self.redact_then_acknowledge.take() {
            let _ = acknowledge.send(());
        }
    }

    fn key(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, key: event::KeyEvent) -> bool {
        if keymap::interrupt(key) {
            return true;
        }
        if self.investigation.is_none()
            && let Some(quit) = self.view_io_key(sender, key)
        {
            return quit;
        }
        let code = keymap::plain(key).unwrap_or(KeyCode::Null);
        if let Some(prompt) = &mut self.prompt {
            match code {
                KeyCode::Esc => {
                    self.dismiss_prompt();
                    self.message = None;
                }
                KeyCode::Char('q') => return true,
                _ => prompt.key(code),
            }
            return false;
        }
        if let Some(investigation) = &mut self.investigation {
            match investigation.key(key, self.pending.is_some()) {
                InvestigationInput::Close => {
                    self.investigation = None;
                    self.prompt_receiver = None;
                    self.message = None;
                }
                InvestigationInput::Read(request) => self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::Investigate(request)),
                    Change::Investigation,
                ),
                InvestigationInput::Reveal(reference) => {
                    let (delivery, receiver) = prompt_channel(PromptKind::Investigation);
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::RevealInvestigation {
                            reference,
                            delivery,
                        }),
                        Change::Investigation,
                    );
                    self.prompt_receiver = Some(receiver);
                }
                InvestigationInput::Edited => self.message = None,
                InvestigationInput::Handled => {}
            }
            return false;
        }
        if let Some(work) = &mut self.work {
            if code == KeyCode::Char('q') && !work.editing() {
                return true;
            }
            if self.pending.is_some() && work.editing() {
                return false;
            }
            if code == KeyCode::Char('p') && work.follows_requests() && !work.editing() {
                self.toggle_pause();
                return false;
            }
            if code == KeyCode::Char('m')
                && self.pending.is_none()
                && self.prompt_receiver.is_none()
                && let Some(owner) = work.prompt_owner()
            {
                let session = match self.history.last().map(|view| &view.selection) {
                    Some(HistorySelection::Session(session, _)) => Some(session),
                    None => self.agents.current(),
                    _ => None,
                };
                if let Some(session) = session
                    .filter(|session| {
                        session.harness_id == owner.source && session.session_id == owner.session
                    })
                    .cloned()
                {
                    self.work = None;
                    self.select_session_mode(sender, session, SessionView::Activity);
                }
                return false;
            }
            let save_edit = work.editing()
                && key.code == KeyCode::Char('s')
                && key.modifiers == KeyModifiers::CONTROL;
            if code == KeyCode::Null && !save_edit {
                return false;
            }
            match work.key(key) {
                WorkInput::Reveal(entry) if self.pending.is_none() => self.reveal(sender, entry),
                WorkInput::Reveal(entry) if self.prompt_receiver.is_none() => {
                    self.queued_prompt = Some(entry)
                }
                WorkInput::Reveal(_) => {}
                WorkInput::RevealTelemetry(span) if self.pending.is_none() => {
                    let (delivery, receiver) = prompt_channel(PromptKind::Telemetry);
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::RevealTelemetry(
                            Box::new(span),
                            delivery,
                        )),
                        Change::Work(None),
                    );
                    self.prompt_receiver = Some(receiver);
                }
                WorkInput::RevealTelemetry(_) => {}
                WorkInput::RevealEvaluation(detail) if self.pending.is_none() => {
                    let (delivery, receiver) = prompt_channel(PromptKind::Evaluation);
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::RevealEvalDetail(
                            Box::new(detail),
                            delivery,
                        )),
                        Change::Work(None),
                    );
                    self.prompt_receiver = Some(receiver);
                }
                WorkInput::RevealEvaluation(_) => {}
                WorkInput::None => {}
                WorkInput::Back => {
                    self.discard_work_result = matches!(self.pending, Some(Change::Work(_)));
                    self.message = None;
                }
                WorkInput::Close => {
                    self.discard_work_result = matches!(self.pending, Some(Change::Work(_)));
                    self.prompt_receiver = None;
                    self.queued_prompt = None;
                    self.work = None;
                    self.message = None;
                }
                WorkInput::Command(command) => {
                    self.request(sender, WorkspaceRequest::Work(command), Change::Work(None))
                }
            }
            return false;
        }
        if key.code == KeyCode::Char('f') && key.modifiers == KeyModifiers::CONTROL {
            self.open_investigation();
            return false;
        }
        if self.history.is_empty() {
            return self.agent_key(sender, key);
        }
        let view = self.history.last_mut().expect("history exists");
        if view.page.ui.help {
            return view.page.ui.handle_key(code);
        }
        if let Some(picker) = &mut view.page.evaluation_picker {
            let entries = &view.page.evaluation_traces;
            match code {
                KeyCode::Char('q') => return true,
                KeyCode::Esc => {
                    view.page.evaluation_picker = None;
                    self.discard_work_result = matches!(self.pending, Some(Change::Work(_)));
                    self.message = None;
                }
                KeyCode::Down | KeyCode::Char('j') if self.pending.is_none() => {
                    move_selection(picker, entries.len(), true);
                    self.message = None;
                }
                KeyCode::Up | KeyCode::Char('k') if self.pending.is_none() => {
                    move_selection(picker, entries.len(), false);
                    self.message = None;
                }
                KeyCode::Enter if self.pending.is_none() => {
                    if let Some(trace) = picker
                        .selected()
                        .and_then(|index| entries.get(index))
                        .cloned()
                    {
                        let request = EvaluationRead {
                            trace,
                            view: EvaluationView::Revisions { before: None },
                        };
                        self.request(
                            sender,
                            WorkspaceRequest::Work(WorkCommand::Evaluations(Box::new(request))),
                            Change::Work(None),
                        );
                    }
                }
                _ => {}
            }
            return false;
        }
        if let Some((kind, picker)) = &mut view.page.operation_picker {
            let kind = *kind;
            let entries = view
                .page
                .operation_details
                .iter()
                .filter(|entry| entry.kind == kind);
            match code {
                KeyCode::Char('q') => return true,
                KeyCode::Esc => {
                    view.page.operation_picker = None;
                    self.discard_work_result = matches!(self.pending, Some(Change::Work(_)));
                    self.prompt_receiver = None;
                    self.message = None;
                }
                KeyCode::Down | KeyCode::Char('j') if self.pending.is_none() => {
                    move_selection(picker, entries.count(), true);
                    self.message = None;
                }
                KeyCode::Up | KeyCode::Char('k') if self.pending.is_none() => {
                    move_selection(picker, entries.count(), false);
                    self.message = None;
                }
                KeyCode::Enter if self.pending.is_none() => {
                    if let Some(operation) = picker
                        .selected()
                        .and_then(|i| entries.clone().nth(i))
                        .cloned()
                    {
                        let prompt_kind = match kind {
                            OperationDetailKind::Mcp => PromptKind::Mcp,
                            OperationDetailKind::Edit => PromptKind::Edit,
                        };
                        let (delivery, receiver) = prompt_channel(prompt_kind);
                        self.request(
                            sender,
                            WorkspaceRequest::Work(WorkCommand::RevealOperation(
                                operation, delivery,
                            )),
                            Change::Work(None),
                        );
                        self.prompt_receiver = Some(receiver);
                    }
                }
                _ => {}
            }
            return false;
        }
        if let Some(picker) = &mut view.page.turn_picker {
            match code {
                KeyCode::Char('q') => return true,
                KeyCode::Esc => view.page.turn_picker = None,
                KeyCode::Down | KeyCode::Char('j') => {
                    move_selection(picker, view.page.turns.len(), true)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    move_selection(picker, view.page.turns.len(), false)
                }
                KeyCode::Enter => {
                    if let Some(turn) = picker
                        .selected()
                        .and_then(|i| view.page.turns.get(i))
                        .cloned()
                    {
                        self.open(sender, HistorySelection::Turn(turn));
                    }
                }
                _ => {}
            }
            return false;
        }

        if keymap::back(key) {
            if self.pending.is_none() {
                self.history.pop();
                self.message = None;
            }
            return false;
        }
        if keymap::review(key) {
            if let HistorySelection::Session(session, _) = &view.selection {
                let selection = HistorySelection::Session(session.clone(), SessionView::Review);
                self.open(sender, selection);
            }
            return false;
        }
        if key.modifiers == KeyModifiers::CONTROL
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            return view.page.ui.handle_key(key.code);
        }
        if let Some(destination) = keymap::destination(code) {
            self.navigate(sender, destination);
            return false;
        }

        if view.page.ui.evidence_key(code) {
            return false;
        }
        if let Some(table) = view.page.operation_table.active(&view.page.ui)
            && table.key(code)
        {
            return false;
        }
        if code == KeyCode::Char('E') && self.pending.is_none() && evaluation_details(view) {
            view.page.evaluation_picker = Some(ListState::default().with_selected(Some(0)));
            self.message = None;
            return false;
        }
        if code == KeyCode::Char('d') && self.pending.is_none() && telemetry_details(view) {
            self.work = Some(Workbench::new(
                WorkResult::TelemetrySpans(view.page.telemetry_spans.clone()),
                None,
            ));
            self.message = None;
            return false;
        }
        if matches!(code, KeyCode::Char('d' | 'D')) {
            self.navigate(sender, Destination::Findings);
            return false;
        }
        let mcp_pane = view.page.ui.selected_title() == Some("MCP");
        if code == KeyCode::Char('m')
            && !mcp_pane
            && let HistorySelection::Session(session, mode) = &view.selection
        {
            if matches!(mode, SessionView::Overview) {
                let anchor = WorkAnchor::from(session);
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::Prompts(anchor.clone())),
                    Change::Work(Some(anchor)),
                );
            } else {
                let session = session.clone();
                self.select_session_mode(sender, session, SessionView::Overview);
            }
            return false;
        }
        let detail = match code {
            KeyCode::Char('M') => Some(OperationDetailKind::Mcp),
            KeyCode::Char('m') if mcp_pane => Some(OperationDetailKind::Mcp),
            KeyCode::Char('e') if view.page.reviewing() || view.page.ui.in_turn => {
                Some(OperationDetailKind::Edit)
            }
            _ => None,
        };
        if !view.page.ui.help
            && let Some(kind) = detail
            && view
                .page
                .operation_details
                .iter()
                .any(|entry| entry.kind == kind)
        {
            let selected = if let Some(table) = view.page.operation_table.active(&view.page.ui) {
                let event = table.selected_row().and_then(|row| row.mcp_event.as_ref());
                let matched = view
                    .page
                    .operation_details
                    .iter()
                    .filter(|entry| entry.kind == kind)
                    .position(|entry| {
                        event.is_some_and(|(source, id, revision)| {
                            entry.source_id == *source
                                && entry.event_id == *id
                                && entry.revision == *revision
                        })
                    });
                let Some(index) = matched else {
                    self.message = Some("No payload reference for the selected operation".into());
                    return false;
                };
                index
            } else {
                0
            };
            view.page.operation_picker =
                Some((kind, ListState::default().with_selected(Some(selected))));
            self.message = None;
            return false;
        }
        match code {
            KeyCode::Esc if self.pending.is_none() => {
                self.history.pop();
                self.message = None;
            }
            KeyCode::Esc => {}
            KeyCode::Char('p') => self.toggle_pause(),
            KeyCode::Char('b' | 'P') => {
                let target = match &view.selection {
                    HistorySelection::Session(session, _) => Some(WorkAnchor::from(session)),
                    HistorySelection::Turn(turn) => Some(WorkAnchor {
                        source: turn.harness_id.clone(),
                        session: turn.session_id.clone(),
                        worker: turn.worker_id.clone(),
                        turn: Some(turn.turn_id.clone()),
                    }),
                    HistorySelection::All(_) => None,
                };
                let command = if code == KeyCode::Char('b') {
                    Some(WorkCommand::Tasks(None))
                } else {
                    target.clone().map(|mut anchor| {
                        anchor.worker = None;
                        anchor.turn = None;
                        WorkCommand::Prompts(anchor)
                    })
                };
                if let Some(command) = command {
                    self.request(
                        sender,
                        WorkspaceRequest::Work(command),
                        Change::Work(target),
                    );
                }
            }
            KeyCode::Char('y') if view.page.reviewing() => {
                if let HistorySelection::Session(session, _) = &view.selection
                    && !session.reviewed
                {
                    let session = session.clone();
                    self.request(
                        sender,
                        WorkspaceRequest::MarkReviewed(session.clone()),
                        Change::MarkReviewed(session),
                    );
                }
            }
            KeyCode::Char('n') if view.page.reviewing() => {
                if let HistorySelection::Session(session, _) = &view.selection {
                    let session = session.clone();
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::ReviewNotes(session)),
                        Change::Work(None),
                    );
                }
            }
            KeyCode::Char('t' | '8') if !view.page.ui.help => {
                if let HistorySelection::Session(session, kind) = &view.selection
                    && !matches!(kind, SessionView::Traces)
                {
                    let selection = HistorySelection::Session(session.clone(), SessionView::Traces);
                    self.open(sender, selection);
                }
            }
            keymap::OLDER if !view.page.ui.help => {
                if let Some(before) = view.page.ui.older_cursor().cloned() {
                    let selection = view.selection.clone();
                    self.request(
                        sender,
                        WorkspaceRequest::History {
                            selection,
                            before: Some(before),
                        },
                        Change::OlderHistory,
                    );
                }
            }
            keymap::NEWER if self.pending.is_none() && !view.page.ui.help => {
                if let Some(newer) = view.newer.pop_back() {
                    view.page = newer;
                }
            }
            keymap::LATEST | KeyCode::Char('R') => {
                let selection = view.selection.clone();
                self.request(
                    sender,
                    WorkspaceRequest::History {
                        selection,
                        before: None,
                    },
                    Change::LatestHistory,
                );
            }
            KeyCode::Char('T') if view.page.ui.turn_available && !view.page.ui.help => {
                view.page.turn_picker = Some(ListState::default().with_selected(Some(0)));
            }
            key => return view.page.ui.handle_key(key),
        }
        false
    }

    fn open_investigation(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let (owner, snapshot) = match self.history.last() {
            Some(view) => {
                if view.page.ui.help
                    || view.page.turn_picker.is_some()
                    || view.page.operation_picker.is_some()
                    || view.page.evaluation_picker.is_some()
                {
                    return;
                }
                let owner = match &view.selection {
                    HistorySelection::All(_) => None,
                    HistorySelection::Session(row, _) => Some(WorkAnchor::from(row)),
                    HistorySelection::Turn(row) => Some(WorkAnchor {
                        source: row.harness_id.clone(),
                        session: row.session_id.clone(),
                        worker: row.worker_id.clone(),
                        turn: Some(row.turn_id.clone()),
                    }),
                };
                (owner, Some(view.page.snapshot_id))
            }
            None => {
                if self.agents.help || self.agents.editing {
                    return;
                }
                let row = self.agents.current();
                (row.map(WorkAnchor::from), row.map(|row| row.snapshot_id))
            }
        };
        self.investigation = Some(Investigation::new(owner, snapshot));
        self.message = None;
    }

    fn agent_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        event: event::KeyEvent,
    ) -> bool {
        let key = keymap::plain(event).unwrap_or(KeyCode::Null);
        if key == KeyCode::F(1)
            || (self.agents.help && matches!(key, KeyCode::Char('?') | KeyCode::Esc))
        {
            self.agents.help = !self.agents.help;
            return false;
        }
        if self.agents.help {
            return key == KeyCode::Char('q') && !self.agents.editing;
        }
        if self.agents.editing {
            let identity = self.agents.current().map(session_key);
            match key {
                KeyCode::Enter | KeyCode::Esc => self.agents.editing = false,
                KeyCode::Char(c) if self.agents.search.len() < 128 => self.agents.search.push(c),
                KeyCode::Backspace => {
                    self.agents.search.pop();
                }
                _ => return false,
            }
            self.agents.apply_filters(identity.as_ref());
            return false;
        }
        if event.modifiers == KeyModifiers::ALT
            && matches!(event.code, KeyCode::Up | KeyCode::Down | KeyCode::Right)
        {
            self.agents.inspect_relative(event.code);
            return false;
        }
        if keymap::review(event) {
            if let Some(session) = self.agents.current().cloned() {
                self.open(
                    sender,
                    HistorySelection::Session(session, SessionView::Review),
                );
            }
            return false;
        }
        if let Some(destination) = keymap::destination(key) {
            self.navigate(sender, destination);
            return false;
        }
        match key {
            KeyCode::Char('q') => return true,
            KeyCode::Down | KeyCode::Char('j') => self.agents.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.agents.move_selection(false),
            KeyCode::Char('/') => self.agents.editing = true,
            KeyCode::Char('f' | 'h') => self.agents.toggle_tree(),
            KeyCode::Char('d' | 'D') => self.navigate(sender, Destination::Findings),
            KeyCode::Char('?') => self.agents.help = true,
            KeyCode::Char('G') => {
                let identity = self.agents.current().map(session_key);
                self.agents.grouping = self.agents.grouping.next();
                self.agents.apply_filters(identity.as_ref());
            }
            KeyCode::Char('o') => {
                let identity = self.agents.current().map(session_key);
                self.agents.sort_by_state = !self.agents.sort_by_state;
                self.agents.apply_filters(identity.as_ref());
            }
            KeyCode::Left => self.agents.expand(false),
            KeyCode::Right => self.agents.expand(true),
            KeyCode::Char('a' | '!') => {
                let identity = self.agents.current().map(session_key);
                self.agents.search.clear();
                self.agents.filter = if key == KeyCode::Char('!') {
                    Filter::Attention
                } else {
                    Filter::All
                };
                self.agents.apply_filters(identity.as_ref());
            }
            KeyCode::Char('Y') => {
                self.agents.filter = Filter::ToReview;
                self.agents.apply_filters(None);
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('F') => {
                self.agents.filter = if key == KeyCode::BackTab {
                    self.agents.filter.previous()
                } else {
                    self.agents.filter.next()
                };
                self.agents.apply_filters(None);
            }
            KeyCode::Char('R') | keymap::LATEST => self.request(
                sender,
                WorkspaceRequest::Sessions(None),
                Change::RefreshSessions,
            ),
            KeyCode::Char('b' | 'w') => {
                let command = if key == KeyCode::Char('b') {
                    WorkCommand::Tasks(None)
                } else {
                    WorkCommand::Workflow
                };
                self.request(
                    sender,
                    WorkspaceRequest::Work(command),
                    Change::Work(self.agents.current().map(WorkAnchor::from)),
                );
            }
            KeyCode::Char('v') => self.message = Some("Internal approval reviews are outside Rust capture. Available in Python receiver mode.".into()),
            KeyCode::Char('X') => {
                if let Some(session) = self.agents.current().cloned() {
                    self.request(sender, WorkspaceRequest::Work(WorkCommand::SessionAttention(session)), Change::Work(None));
                }
            }
            KeyCode::Char('p') => self.toggle_pause(),
            KeyCode::Char('P') => {
                if let Some(session) = self.agents.current() {
                    let anchor = WorkAnchor::from(session);
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::Prompts(anchor.clone())),
                        Change::Work(Some(anchor)),
                    );
                }
            }
            KeyCode::Char('g' | 'H' | 'U' | '8') => {
                let view = match key {
                    KeyCode::Char('H') => WorkspaceView::Host,
                    KeyCode::Char('U') => WorkspaceView::Usage,
                    KeyCode::Char('8') => WorkspaceView::Traces,
                    _ => WorkspaceView::Overview,
                };
                self.open(sender, HistorySelection::All(view));
            }
            KeyCode::Enter if self.agents.show_more_selected => {
                if let Some(cursor) = self.agents.catalog.as_ref().and_then(|c| c.older.clone()) {
                    self.request(
                        sender,
                        WorkspaceRequest::Sessions(Some(cursor)),
                        Change::OlderSessions,
                    );
                }
            }
            KeyCode::Enter | KeyCode::Char('e' | 'm' | 'A' | 's' | 'V' | 'u' | 't') => {
                if let Some(session) = self.agents.current().cloned() {
                    let view = match key {
                        KeyCode::Char('A' | 's') => SessionView::Activity,
                        KeyCode::Char('V') => SessionView::Review,
                        KeyCode::Char('u') => SessionView::Usage,
                        KeyCode::Char('t') => SessionView::Traces,
                        _ => SessionView::Overview,
                    };
                    self.open(sender, HistorySelection::Session(session, view));
                }
            }
            keymap::OLDER => {
                if let Some(cursor) = self
                    .agents
                    .catalog
                    .as_ref()
                    .and_then(|catalog| catalog.older.clone())
                {
                    self.request(
                        sender,
                        WorkspaceRequest::Sessions(Some(cursor)),
                        Change::OlderSessions,
                    );
                }
            }
            keymap::NEWER if self.pending.is_none() => {
                if let Some(newer) = self.newer_sessions.pop_back() {
                    self.agents.update(newer);
                    // If the oldest cached page was dropped, a refresh still
                    // offers an explicit return to the live catalog.
                    self.browsing_older = !self.newer_sessions.is_empty();
                }
            }
            _ => {}
        }
        false
    }
}

fn push_cached<T>(cache: &mut VecDeque<T>, page: T) {
    if cache.len() == CACHE_LIMIT {
        cache.pop_front();
    }
    cache.push_back(page);
}

/// Run the interactive session workspace. All database and extension work
/// runs on the loader thread while the terminal handles input and resize.
pub fn run_workspace(
    mut load: impl FnMut(WorkspaceRequest) -> Result<WorkspaceResponse, String> + Send,
) -> io::Result<()> {
    std::thread::scope(|scope| {
        let (request, requests) = mpsc::channel();
        let (send, results) = mpsc::channel();
        scope.spawn(move || {
            while let Ok(request) = requests.recv() {
                if send.send(load(request)).is_err() {
                    break;
                }
            }
        });
        let terminal_result = ratatui::run(|terminal| {
            let mut workspace = Workspace::new();
            let mut dirty = true;
            loop {
                if let Some(receiver) = &workspace.prompt_receiver
                    && let Ok(bytes) = receiver.frames.try_recv()
                {
                    let receiver = workspace
                        .prompt_receiver
                        .take()
                        .expect("prompt delivery arrived");
                    if let Ok(text) = serde_json::from_slice::<String>(&bytes) {
                        workspace.prompt = Some(PromptScreen::new(text, receiver, Instant::now()));
                    } else {
                        workspace.message = Some("Invalid private content delivery".into());
                    }
                    dirty = true;
                }
                if let Ok(result) = results.try_recv() {
                    workspace.finish(result);
                    if let Some((next, change)) = workspace.deferred_request.take() {
                        workspace.request(&request, next, change);
                    }
                    if workspace.work.is_some()
                        && let Some(queued) = workspace.queued_prompt.take()
                    {
                        workspace.reveal(&request, queued);
                    }
                    dirty = true;
                }
                if workspace
                    .work
                    .as_ref()
                    .is_some_and(Workbench::follows_requests)
                    && workspace.pending.is_none()
                    && !workspace.agents.paused
                    && Instant::now() >= workspace.refresh_after
                {
                    workspace.refresh_after = Instant::now() + REFRESH_INTERVAL;
                    workspace.request(
                        &request,
                        WorkspaceRequest::Work(WorkCommand::Workflow),
                        Change::Work(None),
                    );
                    dirty = true;
                }
                if workspace.history.is_empty()
                    && workspace.work.is_none()
                    && workspace.investigation.is_none()
                    && workspace.pending.is_none()
                    && !workspace.browsing_older
                    && !workspace.agents.paused
                    && Instant::now() >= workspace.refresh_after
                {
                    workspace.refresh_after = Instant::now() + REFRESH_INTERVAL;
                    workspace.request(
                        &request,
                        WorkspaceRequest::Sessions(None),
                        Change::RefreshSessions,
                    );
                    dirty = true;
                }
                dirty |= workspace.expire_prompt(Instant::now());
                if dirty {
                    let active = workspace
                        .prompt
                        .as_ref()
                        .filter(|prompt| prompt.acknowledge.is_some())
                        .map(|prompt| Arc::clone(&prompt.active));
                    let gate = active.as_ref().and_then(|active| active.lock().ok());
                    if active.is_some() && !gate.as_deref().copied().unwrap_or(false) {
                        workspace.prompt = None;
                        workspace.message =
                            Some("Content authorization expired before display".into());
                    }
                    terminal.draw(|frame| draw_workspace(frame, &mut workspace))?;
                    workspace.acknowledge_redaction();
                    drop(gate);
                    dirty = false;
                }
                if event::poll(Duration::from_millis(80))? {
                    match event::read()? {
                        Event::Key(key)
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            if workspace.key(&request, key) {
                                // Keep the proof fence until a flushed non-private
                                // frame replaces the modal, including q and Ctrl-C.
                                if workspace.dismiss_prompt()
                                    || workspace.redact_then_acknowledge.is_some()
                                {
                                    terminal.draw(|frame| draw_workspace(frame, &mut workspace))?;
                                    workspace.acknowledge_redaction();
                                }
                                break Ok(());
                            }
                            if let Some(bytes) = workspace.view_io.clipboard.take() {
                                use std::io::Write;
                                let mut output = io::stdout().lock();
                                output.write_all(&bytes)?;
                                output.flush()?;
                            }
                            dirty = true;
                        }
                        Event::Resize(_, _) => dirty = true,
                        _ => {}
                    }
                }
            }
        });
        drop(request);
        terminal_result
    })
}

fn pending_evidence_titles(selection: &HistorySelection) -> Option<(&'static str, &'static str)> {
    match selection {
        HistorySelection::All(WorkspaceView::Activity)
        | HistorySelection::Session(_, SessionView::Activity) => Some(("Activity", "Event detail")),
        HistorySelection::All(WorkspaceView::Findings) => Some(("Findings", "Evidence inspector")),
        HistorySelection::All(WorkspaceView::Host) => Some(("Host now", "Collection status")),
        HistorySelection::All(WorkspaceView::Usage)
        | HistorySelection::Session(_, SessionView::Usage) => Some(("Usage", "Metric inspector")),
        HistorySelection::All(WorkspaceView::Traces)
        | HistorySelection::Session(_, SessionView::Traces) => Some(("Traces", "Execution tree")),
        _ => None,
    }
}

fn draw_workspace(frame: &mut Frame, workspace: &mut Workspace) {
    frame.render_widget(Block::default().style(surface(CANVAS)), frame.area());
    if let Some(prompt) = &mut workspace.prompt {
        prompt.draw(frame);
        return;
    }
    if let Some(form) = &workspace.view_io.form {
        view_io::draw(frame, form, workspace.message.as_deref());
        return;
    }
    if let Some(investigation) = &mut workspace.investigation {
        investigation.draw(
            frame,
            workspace.pending.is_some(),
            workspace.message.as_deref(),
        );
        return;
    }
    if let Some(work) = &mut workspace.work {
        work.draw(
            frame,
            workspace.pending.is_some(),
            workspace.message.as_deref(),
        );
        return;
    }
    let area = frame.area();
    if workspace.history.is_empty()
        && frame.area().width >= MIN_WIDTH
        && frame.area().height >= MIN_HEIGHT
        && let Some(Change::Open(selection)) = &workspace.pending
        && let Some((title, inspector)) = pending_evidence_titles(selection)
    {
        evidence_view::loading(frame, title, inspector);
        return;
    }
    if let Some(view) = workspace.history.last_mut() {
        if area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && let Some(selection) = &mut view.page.evaluation_picker
        {
            let [body, footer] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(4)]).areas(area);
            let entries = &view.page.evaluation_traces;
            let items = entries.iter().map(|entry| {
                ListItem::new(format!(
                    "{}  {}",
                    entry.trace_id,
                    visible_text(entry.source_id.as_str())
                ))
            });
            let mut hints = Vec::new();
            if let Some(entry) = selection.selected().and_then(|index| entries.get(index)) {
                hints.push(Line::from(format!(
                    "Source {}",
                    visible_text(entry.source_id.as_str())
                )));
            }
            if let Some(message) = workspace.message.as_deref() {
                hints.push(Line::from(visible_text(message)));
            } else if workspace.pending.is_some() {
                hints.push(Line::from("Loading evaluation metadata…"));
            }
            hints.push(Line::from("↑↓ select · Enter evaluations · Esc back"));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::bordered().title("Trace evaluations · loaded traces"))
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                body,
                selection,
            );
            frame.render_widget(Paragraph::new(hints).wrap(Wrap { trim: true }), footer);
            return;
        }
        if area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && let Some((kind, selection)) = &mut view.page.operation_picker
        {
            let [body, foot] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
            let items = view
                .page
                .operation_details
                .iter()
                .filter(|entry| entry.kind == *kind)
                .map(|entry| ListItem::new(visible_text(&entry.label)));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::bordered().title(match kind {
                        OperationDetailKind::Mcp => "MCP operations",
                        OperationDetailKind::Edit => "File edits",
                    }))
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                body,
                selection,
            );
            let footer = workspace
                .message
                .as_deref()
                .unwrap_or(if workspace.pending.is_some() {
                    "Checking content approval…"
                } else {
                    "↑↓ select · Enter reveal · Esc back"
                });
            frame.render_widget(Paragraph::new(visible_text(footer)), foot);
            return;
        }
        if view.page.ui.help && area.width >= MIN_WIDTH && area.height >= MIN_HEIGHT {
            let primary = keymap::navigation_hints(&keymap::NAVIGATION[..5]);
            let secondary = keymap::navigation_hints(&keymap::NAVIGATION[5..]);
            let mut help = format!(
                "{primary}\n{secondary} · d Recorded diagnostics\n←/→ or Ctrl+←/→ Pane · ↑/↓ Scroll\n[ Newer · ] Older · l Latest · R Refresh · p Pause\nCtrl+F Investigate · Esc or Alt+← Back · q Quit"
            );
            if matches!(view.selection, HistorySelection::Session(_, _)) {
                help.push_str("\nCtrl+R Review · t Traces · m Inspector mode · P Prompts");
            }
            if view.page.ui.turn_available {
                help.push_str("\nT Select a recorded turn");
            }
            let has_kind = |kind| {
                view.page
                    .operation_details
                    .iter()
                    .any(|entry| entry.kind == kind)
            };
            if has_kind(OperationDetailKind::Mcp) {
                help.push_str("\nM MCP payloads · m on MCP");
            }
            if view.page.reviewing() {
                help.push_str("\ny Mark reviewed · n Review notes");
            }
            if telemetry_details(view) || evaluation_details(view) {
                help.push('\n');
                if telemetry_details(view) {
                    help.push_str("d Captured text");
                }
                if telemetry_details(view) && evaluation_details(view) {
                    help.push_str(" · ");
                }
                if evaluation_details(view) {
                    help.push_str("E Evaluations");
                }
            }
            frame.render_widget(
                Paragraph::new(help)
                    .wrap(Wrap { trim: false })
                    .block(panel(HELP_CLOSE_HINT)),
                area,
            );
            return;
        }
        view.page.ui.set_viewport(area.width, area.height);
        if !view.page.ui.help
            && view.page.turn_picker.is_none()
            && area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && let Some(table) = view.page.operation_table.active(&view.page.ui)
        {
            let [nav, body, keys, _] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
            let [brand, tabs] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(nav);
            frame.render_widget(
                Paragraph::new(format!(
                    " Agenturm   ·   {} observations",
                    view.page.ui.observations
                ))
                .style(heading_style()),
                brand,
            );
            draw_view_bar(frame, &view.page.ui, tabs);
            table.draw(
                frame,
                body,
                view.page.ui.selected_title().unwrap_or("Operations"),
            );
            frame.render_widget(
                Paragraph::new("↑↓ select / scroll · Enter inspect · Esc back · ←→ views"),
                keys,
            );
        } else {
            draw(
                frame,
                &view.page.ui,
                !view.newer.is_empty(),
                view.page
                    .turn_picker
                    .as_mut()
                    .map(|state| (view.page.turns.as_slice(), state)),
            );
        }
        if area.height != 0 {
            let has_details = |kind| {
                view.page
                    .operation_details
                    .iter()
                    .any(|entry| entry.kind == kind)
            };
            let status = if view.page.ui.help {
                HELP_CLOSE_HINT
            } else if let Some(error) = workspace.message.as_deref() {
                error
            } else if workspace.pending.is_some() {
                "Loading history…"
            } else if view.page.ui.evidence_inspection.is_some() {
                ""
            } else if evaluation_details(view) && telemetry_details(view) {
                "E evaluations · d captured text · Esc back · [ ] pages"
            } else if evaluation_details(view) {
                "E evaluations · Esc back · [ ] pages"
            } else if telemetry_details(view) {
                "d captured text · Esc back · [ ] pages"
            } else if let HistorySelection::Session(session, _) = &view.selection
                && view.page.reviewing()
            {
                if has_details(OperationDetailKind::Edit) {
                    if session.reviewed {
                        "Reviewed · e edits · n decisions · t traces · Esc back"
                    } else {
                        "y review · e edits · n decisions · t traces · Esc back"
                    }
                } else if session.reviewed {
                    "Reviewed · n decisions · t traces · Esc back"
                } else {
                    "y mark reviewed · n decisions · t traces · Esc back"
                }
            } else if view.page.ui.in_turn && has_details(OperationDetailKind::Edit) {
                if has_details(OperationDetailKind::Mcp) {
                    "e edits · M MCP payloads · Esc back · [ ] pages"
                } else {
                    "e edits · Esc back · [ ] history pages"
                }
            } else if has_details(OperationDetailKind::Mcp)
                && matches!(view.selection, HistorySelection::Session(_, kind) if !matches!(kind, SessionView::Traces))
            {
                "M payloads · t traces · Esc back · [ ] pages"
            } else if has_details(OperationDetailKind::Mcp) {
                "M MCP payloads · Esc back · [ ] history pages"
            } else if matches!(view.selection, HistorySelection::Session(_, kind) if !matches!(kind, SessionView::Traces))
            {
                "t session traces · Esc back · [ ] history pages"
            } else {
                "Esc back · [ ] history pages"
            };
            let footer = Rect::new(0, area.height.saturating_sub(1), area.width, 1);
            frame.render_widget(Paragraph::new(visible_text(status)), footer);
        }
        return;
    }
    let notice = workspace
        .agents
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.notice.as_deref());
    let message = workspace.message.as_deref().or(notice).map(str::to_owned);
    draw_agents(
        frame,
        &mut workspace.agents,
        workspace.pending.is_some(),
        message.as_deref(),
    );
}

fn recorded_state(row: &RecordedSession) -> &str {
    row.state.as_deref().unwrap_or("Unknown")
}

fn roster_cell(text: &str, width: u16) -> String {
    if width == 0 {
        return String::new();
    }
    if text.cell_width() <= width {
        return text.into();
    }
    let mut output = String::new();
    let mut remaining = width - 1;
    for grapheme in Span::raw(text).styled_graphemes(Style::default()) {
        let width = grapheme.symbol.cell_width();
        if width > remaining {
            break;
        }
        output.push_str(grapheme.symbol);
        remaining -= width;
    }
    output.push('…');
    output
}

pub(super) fn age(nanos: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let elapsed = now.saturating_sub(Duration::from_nanos(nanos)).as_secs();
    match elapsed {
        0..=59 => "<1m ago".into(),
        60..=3599 => format!("{}m ago", elapsed / 60),
        3600..=86399 => format!("{}h ago", elapsed / 3600),
        _ => format!("{}d ago", elapsed / 86400),
    }
}

fn telemetry_details(view: &HistoryView) -> bool {
    !view.page.telemetry_spans.is_empty() && is_trace_view(view)
}

fn evaluation_details(view: &HistoryView) -> bool {
    !view.page.evaluation_traces.is_empty() && is_trace_view(view)
}

fn is_trace_view(view: &HistoryView) -> bool {
    matches!(
        view.selection,
        HistorySelection::All(WorkspaceView::Traces)
            | HistorySelection::Session(_, SessionView::Traces)
    )
}

fn draw_agents(frame: &mut Frame, agents: &mut AgentList, pending: bool, message: Option<&str>) {
    command_center::draw(frame, agents, pending, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn session(source: &str, id: &str, state: &str, parent: Option<&str>) -> RecordedSession {
        RecordedSession {
            harness_id: source.into(),
            session_id: id.into(),
            task_title: None,
            project_path: None,
            project_origin: None,
            updated_at_unix_ns: 1,
            snapshot_id: 9.into(),
            state: Some(state.into()),
            reason: None,
            turn_id: None,
            parent_session_id: parent.map(str::to_owned),
            relationship_kind: parent.map(|_| RelationshipKind::Delegated),
            review_version: ReviewVersion {
                receipt_sequence: 7,
                retained_observations: 2,
            },
            reviewed: false,
            history_identity: None,
            pending_requests: Vec::new(),
        }
    }

    fn catalog(sessions: Vec<RecordedSession>) -> SessionCatalog {
        SessionCatalog {
            sessions,
            older: None,
            notice: None,
        }
    }

    fn empty_history_page() -> HistoryPage {
        HistoryPage {
            snapshot: Snapshot::default(),
            turns: Vec::new(),
            initial_pane: None,
            review_pane: None,
            operation_details: Vec::new(),
            private_telemetry_history: None,
            operation_tables: Default::default(),
            evidence_tables: Default::default(),
        }
    }

    fn press(
        workspace: &mut Workspace,
        sender: &mpsc::Sender<WorkspaceRequest>,
        code: KeyCode,
    ) -> bool {
        workspace.key(sender, event::KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn attention_shortcut_pins_the_selected_source_and_seen_does_not_resolve() {
        let mut workspace = Workspace::new();
        let mut selected = session("harness.pi", "same", "paused", None);
        selected.history_identity = Some("history".into());
        let request = RequestEntry {
            owner: WorkAnchor::from(&selected),
            event_id: "approval".into(),
            revision: 2,
            timestamp: 1,
            reason: "codex_approval_request".into(),
            seen: false,
            proof: RequestProof {
                snapshot_id: selected.snapshot_id,
                history_identity: selected.history_identity.clone(),
            },
        };
        selected.pending_requests.push(request.clone());
        workspace.agents.update(catalog(vec![
            session("harness.codex", "same", "paused", None),
            selected,
        ]));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Down));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('v')));
        assert!(requests.try_recv().is_err());
        assert!(
            workspace
                .message
                .as_deref()
                .is_some_and(|line| line.contains("outside Rust capture"))
        );
        assert!(!press(&mut workspace, &sender, KeyCode::Char('X')));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(WorkCommand::SessionAttention(row))) if row.harness_id == "harness.pi" && row.session_id == "same")
        );
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Seen(request))));
        let row = workspace.agents.current().unwrap();
        assert_eq!(row.state.as_deref(), Some("paused"));
        assert_eq!(row.pending_requests.len(), 1);
        assert!(row.pending_requests[0].seen);
    }

    #[test]
    fn investigation_keeps_exact_turn_scope_and_returns_to_the_same_inspector() {
        let mut workspace = Workspace::new();
        let session = session("harness.pi", "retained-session", "completed", None);
        let turn = RecordedTurn {
            harness_id: session.harness_id.clone(),
            session_id: session.session_id.clone(),
            turn_id: "chosen-turn".into(),
            worker_id: Some("worker-1".into()),
            snapshot_id: 17.into(),
            lifecycle: None,
        };
        workspace.agents.update(catalog(vec![session.clone()]));
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(session.clone(), SessionView::Overview),
            empty_history_page(),
        ));
        let mut page = empty_history_page();
        page.snapshot.history_snapshot_id = turn.snapshot_id;
        workspace
            .history
            .push(HistoryView::new(HistorySelection::Turn(turn.clone()), page));
        let (sender, requests) = mpsc::channel();
        let ctrl_f = event::KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(!workspace.key(&sender, ctrl_f));
        assert!(!press(&mut workspace, &sender, KeyCode::Tab));
        for ch in ['q', 'x', '1'] {
            assert!(!press(&mut workspace, &sender, KeyCode::Char(ch)));
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let WorkspaceRequest::Work(WorkCommand::Investigate(read)) = requests.try_recv().unwrap()
        else {
            panic!("expected an investigation request");
        };
        assert_eq!(read.kind, InvestigationKind::Instruction);
        assert_eq!(read.query, "qx1");
        assert_eq!(read.snapshot_id, Some(turn.snapshot_id));
        assert_eq!(
            read.owner,
            Some(WorkAnchor {
                source: turn.harness_id.clone(),
                session: turn.session_id.clone(),
                worker: turn.worker_id.clone(),
                turn: Some(turn.turn_id.clone()),
            })
        );
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Investigation(
            Box::new(InvestigationPage {
                rows: vec![InvestigationMatch {
                    position: ObservationPosition {
                        observed_at_unix_ns: 10,
                        harness_id: turn.harness_id.clone(),
                        event_id: "exact-event".into(),
                        revision: 3,
                        snapshot_id: turn.snapshot_id,
                    },
                    session: Some(turn.session_id.clone()),
                    turn: Some(turn.turn_id.clone()),
                    worker: turn.worker_id.clone(),
                    label: "Verified instruction match",
                    source_reference: Some("opaque-reference".into()),
                }],
                snapshot_id: turn.snapshot_id,
                inspected: 3,
                verified: Some(2),
                unavailable: 1,
                next_cursor: Some("older-page".into()),
            }),
        ))));
        for (width, height) in [(120, 32), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_workspace(frame, &mut workspace))
                .unwrap();
            let screen = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            for label in [
                "Investigate retained history",
                "qx1",
                "exact-event",
                "2 verified / 1 unavailable",
                "PgDn more",
                "Enter open source",
            ] {
                assert!(
                    screen.contains(label),
                    "missing {label} at {width}×{height}"
                );
            }
            assert!(!screen.contains("opaque-reference"));
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let WorkspaceRequest::Work(WorkCommand::RevealInvestigation { reference, .. }) =
            requests.try_recv().unwrap()
        else {
            panic!("expected an exact protected reveal request");
        };
        assert_eq!(reference, "opaque-reference");
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Status(
            String::new(),
        ))));
        assert!(!press(&mut workspace, &sender, KeyCode::PageDown));
        let WorkspaceRequest::Work(WorkCommand::Investigate(read)) = requests.try_recv().unwrap()
        else {
            panic!("expected a continuation request");
        };
        assert_eq!(read.cursor.as_deref(), Some("older-page"));
        assert_eq!(read.snapshot_id, Some(turn.snapshot_id));
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        workspace.finish(Err("late reply after back".into()));
        assert!(workspace.message.is_none());
        assert_eq!(workspace.history.len(), 2);
        assert!(
            matches!(&workspace.history.last().unwrap().selection, HistorySelection::Turn(row) if row == &turn)
        );
        assert_eq!(workspace.agents.current(), Some(&session));

        workspace.history.clear();
        assert!(!workspace.key(&sender, ctrl_f));
        assert!(!press(&mut workspace, &sender, KeyCode::Tab));
        for ch in ['c', 'x', 'q'] {
            assert!(!press(&mut workspace, &sender, KeyCode::Char(ch)));
        }
        assert!(workspace.view_io.clipboard.is_none());
        assert!(workspace.view_io.form.is_none());
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let WorkspaceRequest::Work(WorkCommand::Investigate(read)) = requests.try_recv().unwrap()
        else {
            panic!("expected an investigation request from the roster");
        };
        assert_eq!(read.query, "cxq");
    }

    #[test]
    fn numeric_shortcuts_route_recorded_views_and_return_to_the_same_agent() {
        let mut workspace = Workspace::new();
        let selected = session("harness.pi", "selected", "completed", None);
        workspace.agents.update(catalog(vec![
            session("harness.codex", "other", "running", None),
            selected.clone(),
        ]));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Down));
        for (key, expected) in [
            ('2', WorkspaceView::Activity),
            ('3', WorkspaceView::Mcp),
            ('4', WorkspaceView::Traces),
            ('6', WorkspaceView::Metrics),
            ('7', WorkspaceView::Findings),
            ('9', WorkspaceView::Operations),
        ] {
            assert!(!press(&mut workspace, &sender, KeyCode::Char(key)));
            let WorkspaceRequest::History {
                selection: HistorySelection::All(view),
                before: None,
            } = requests.try_recv().unwrap()
            else {
                panic!("missing workspace destination")
            };
            assert_eq!(view, expected);
            workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
            assert_eq!(
                workspace.history.len(),
                1,
                "tab changes replace the current global view"
            );
            if key == '6' {
                assert_eq!(
                    workspace.history[0].page.ui.selected_title(),
                    Some("Metrics")
                );
            }
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Char('5')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::Work(WorkCommand::HostHistory(None)))
        ));
        workspace.pending = None;
        assert!(!press(&mut workspace, &sender, KeyCode::Char('1')));
        assert!(workspace.history.is_empty());
        assert_eq!(workspace.agents.current(), Some(&selected));
    }

    #[test]
    fn text_focus_and_private_modal_precede_global_shortcuts() {
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![session(
            "harness.codex",
            "selected",
            "completed",
            None,
        )]));
        let (sender, requests) = mpsc::channel();
        let review = event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(!press(&mut workspace, &sender, KeyCode::Char('/')));
        for c in ['4', 'p', 'm', 'q'] {
            assert!(!press(&mut workspace, &sender, KeyCode::Char(c)));
        }
        assert!(!workspace.key(&sender, review));
        assert!(!press(&mut workspace, &sender, KeyCode::F(1)));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('9')));
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(workspace.agents.editing);
        assert_eq!(workspace.agents.search, "4pmq");
        assert!(!workspace.agents.paused);
        assert!(requests.try_recv().is_err());
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('a')));

        let (_delivery, receiver) = prompt_channel(PromptKind::User);
        workspace.prompt = Some(PromptScreen::new(
            "private".into(),
            receiver,
            Instant::now(),
        ));
        for code in [KeyCode::Char('4'), KeyCode::Char('p'), KeyCode::F(1)] {
            assert!(!press(&mut workspace, &sender, code));
        }
        assert!(!workspace.key(&sender, review));
        assert!(workspace.prompt.is_some());
        assert!(!workspace.agents.help && !workspace.agents.paused);
        assert!(requests.try_recv().is_err());
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(workspace.prompt.is_none() && workspace.redact_then_acknowledge.is_some());
        assert!(!workspace.key(&sender, review));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::History { selection: HistorySelection::Session(row, SessionView::Review), before: None }) if row.session_id == "selected")
        );
    }

    #[test]
    fn prompt_layout_keeps_the_original_deadline_and_redaction_acknowledgement() {
        let now = Instant::now();
        for exit in [
            Some(event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(event::KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(event::KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )),
            None,
        ] {
            let mut workspace = Workspace::new();
            let (_producer, frames) = mpsc::channel();
            let (acknowledgement, original_ack) = mpsc::channel();
            let active = Arc::new(Mutex::new(true));
            let receiver = PromptReceiver {
                frames,
                acknowledgement,
                active: Arc::clone(&active),
                kind: PromptKind::User,
            };
            let (sender, requests) = mpsc::channel();
            workspace.prompt = Some(PromptScreen::new(
                "SCOPED_PROMPT_TEXT".into(),
                receiver,
                now,
            ));

            let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
            let mut draw = |workspace: &mut Workspace| {
                terminal
                    .draw(|frame| draw_workspace(frame, workspace))
                    .unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            };
            assert!(draw(&mut workspace).contains("SCOPED_PROMPT_TEXT"));
            for _ in 0..3 {
                assert!(!press(&mut workspace, &sender, KeyCode::Char('z')));
                assert!(!workspace.expire_prompt(now + Duration::from_millis(24_999)));
                assert!(draw(&mut workspace).contains("SCOPED_PROMPT_TEXT"));
            }
            for code in [KeyCode::Char('v'), KeyCode::Char('X')] {
                assert!(!press(&mut workspace, &sender, code));
            }
            assert!(requests.try_recv().is_err());
            assert!(workspace.view_io.clipboard.is_none());
            assert!(workspace.view_io.form.is_none());
            let prompt = workspace.prompt.as_ref().unwrap();
            assert!(Arc::ptr_eq(&prompt.active, &active));
            assert!(prompt.acknowledge.is_some());
            assert_eq!(original_ack.try_recv(), Err(mpsc::TryRecvError::Empty));

            if let Some(key) = exit {
                let quits = workspace.key(&sender, key);
                assert_eq!(quits, key.code != KeyCode::Esc);
                if quits {
                    assert!(workspace.dismiss_prompt());
                }
            } else {
                assert!(workspace.expire_prompt(now + Duration::from_secs(25)));
            }
            assert!(workspace.prompt.is_none());
            assert_eq!(original_ack.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert!(!draw(&mut workspace).contains("SCOPED_PROMPT_TEXT"));
            assert_eq!(original_ack.try_recv(), Err(mpsc::TryRecvError::Empty));
            workspace.acknowledge_redaction();
            assert_eq!(original_ack.try_recv(), Ok(()));
            assert!(workspace.redact_then_acknowledge.is_none());
        }
    }

    #[test]
    fn pause_refresh_and_exact_session_inspector_modes() {
        let mut workspace = Workspace::new();
        let selected = session("harness.pi", "selected", "completed", None);
        workspace.agents.update(catalog(vec![selected]));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Char('p')));
        assert!(workspace.agents.paused && requests.try_recv().is_err());
        assert!(!press(&mut workspace, &sender, KeyCode::Char('R')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::Sessions(None))
        ));
        workspace.pending = None;
        assert!(!press(&mut workspace, &sender, KeyCode::Char('p')));
        assert!(!workspace.agents.paused);
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::Session(_, SessionView::Overview),
                ..
            })
        ));
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(WorkCommand::Prompts(owner))) if owner.source == "harness.pi" && owner.session == "selected")
        );
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Prompts(Vec::new()))));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::History { selection: HistorySelection::Session(row, SessionView::Activity), .. }) if row.session_id == "selected")
        );
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        assert_eq!(workspace.history.len(), 1);
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::Session(_, SessionView::Overview),
                ..
            })
        ));
    }

    #[test]
    fn roster_titles_fit_terminal_cells_and_keep_secondary_identity() {
        assert_eq!(roster_cell("調査中", 4), "調…");
        assert_eq!(roster_cell("e\u{301}clair", 4), "e\u{301}cl…");
        let mut titled = session("harness.codex", "source-session", "running", None);
        titled.task_title =
            Some("調査 tracing and verify the very long agent session release title".into());
        let mut agents = AgentList::default();
        agents.update(catalog(vec![
            titled,
            session("harness.pi", "only-structural", "completed", None),
        ]));
        for (width, height) in [(120, 32), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_agents(frame, &mut agents, false, None))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("Tasks"), "{text}");
            assert!(text.contains("tracing and verify"), "{text}");
            if width == 60 {
                assert!(text.contains('…'), "{text}");
            }
            assert!(text.contains("source-session"), "{text}");
            assert!(text.contains("Untitled session"), "{text}");
        }
    }

    #[test]
    fn scoped_evidence_views_keep_the_selected_inspector_on_pending_and_error() {
        for kind in [
            WorkspaceView::Activity,
            WorkspaceView::Findings,
            WorkspaceView::Host,
            WorkspaceView::Usage,
            WorkspaceView::Traces,
        ] {
            let selection = HistorySelection::All(kind);
            let (title, inspector) = pending_evidence_titles(&selection).unwrap();
            let spec = EvidenceSpec {
                title,
                inspector,
                columns: &[("Type", 1), ("Recorded value", 2)],
                empty: "No recorded evidence on this page.",
            };
            let id = ExtensionId::parse("fixture.evidence").unwrap();
            for (width, height) in [(120, 32), (80, 24), (60, 18)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let mut workspace = Workspace::new();
                let mut draw = |workspace: &mut Workspace| {
                    terminal
                        .draw(|frame| draw_workspace(frame, workspace))
                        .unwrap();
                    terminal
                        .backend()
                        .buffer()
                        .content()
                        .chunks(usize::from(width))
                        .map(|line| line.iter().map(|cell| cell.symbol()).collect::<String>())
                        .collect::<Vec<_>>()
                };
                let build_page = |table| HistoryPage {
                    snapshot: Snapshot {
                        observations_truncated: true,
                        panes: [(
                            id.clone(),
                            agenturm_kernel::PaneModel {
                                title: title.into(),
                                rows: Vec::new(),
                            },
                        )]
                        .into(),
                        ..Snapshot::default()
                    },
                    turns: Vec::new(),
                    initial_pane: Some(id.clone()),
                    review_pane: None,
                    operation_details: Vec::new(),
                    private_telemetry_history: None,
                    operation_tables: Default::default(),
                    evidence_tables: [(id.clone(), table)].into(),
                };
                workspace.pending = Some(Change::Open(selection.clone()));
                let loading = draw(&mut workspace).join("\n");
                assert!(loading.contains("Loading history…"));
                assert!(loading.contains(inspector));
                assert!(!loading.contains("No recorded evidence"));

                workspace.finish(Ok(WorkspaceResponse::History(build_page(
                    spec.table(Vec::new()),
                ))));
                let empty = draw(&mut workspace).join("\n");
                assert!(empty.contains("No recorded evidence"));
                assert!(empty.contains(inspector));

                workspace.history.clear();
                workspace.pending = Some(Change::Open(selection.clone()));
                let rows = (0..24)
                    .map(|index| EvidenceRow {
                        cells: vec!["Fixture".into(), format!("Observation {index}")],
                        details: std::iter::once(MetricRow {
                            label: "Selected identity".into(),
                            value: format!("Scoped event {index}"),
                        })
                        .chain((0..39).map(|detail| MetricRow {
                            label: "Detail".into(),
                            value: format!("Field {detail} on event {index}"),
                        }))
                        .collect(),
                    })
                    .collect();
                workspace.finish(Ok(WorkspaceResponse::History(build_page(spec.table(rows)))));
                let initial = draw(&mut workspace);
                let detail_top = initial
                    .iter()
                    .position(|line| line.contains(inspector))
                    .unwrap();
                assert_eq!(detail_top == 2, width >= 96);
                assert!(initial[0].contains("partial page"));
                let model = &mut workspace.history.last_mut().unwrap().page.ui;
                for _ in 0..28 {
                    model.scroll_down();
                }
                let populated = draw(&mut workspace).join("\n");
                assert!(populated.contains("› Fixture"));
                assert!(populated.contains("Observation 23"));
                assert!(populated.contains("Scoped event 23"));
                assert!(!populated.contains("Scoped event 0"));

                let (sender, _) = mpsc::channel();
                let key = |workspace: &mut Workspace, code| {
                    assert!(
                        !workspace.key(&sender, event::KeyEvent::new(code, KeyModifiers::NONE))
                    );
                };
                key(&mut workspace, KeyCode::Enter);
                let expanded = draw(&mut workspace).join("\n");
                assert!(expanded.contains("Scoped event 23"));
                assert!(expanded.contains("PgUp/PgDn"));
                assert!(!expanded.contains("Observation 23"));
                for _ in 0..5 {
                    key(&mut workspace, KeyCode::PageDown);
                }
                let last = draw(&mut workspace).join("\n");
                assert!(last.contains("Field 38 on event 23"));
                assert!(!last.contains("Scoped event 23"));
                for _ in 0..5 {
                    key(&mut workspace, KeyCode::PageUp);
                }
                assert!(draw(&mut workspace).join("\n").contains("Scoped event 23"));
                key(&mut workspace, KeyCode::Esc);
                assert_eq!(draw(&mut workspace).join("\n"), populated);

                workspace.pending = Some(Change::OlderHistory);
                let pending = draw(&mut workspace).join("\n");
                assert!(pending.contains("Loading history…"));
                assert!(pending.contains("Scoped event 23"));
                workspace.finish(Err("History unavailable · Fixture read failed".into()));
                let error = draw(&mut workspace).join("\n");
                assert!(error.contains("Fixture read failed"));
                assert!(error.contains("Scoped event 23"));
                assert!(error.contains(inspector));
            }
        }
    }

    #[test]
    fn escape_closes_help_without_losing_agent_or_inspector_focus() {
        for (width, height) in [(120, 32), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut workspace = Workspace::new();
            let (sender, requests) = mpsc::channel();
            let press = |workspace: &mut Workspace, code| {
                workspace.key(&sender, event::KeyEvent::new(code, KeyModifiers::NONE))
            };
            let mut frame = |workspace: &mut Workspace| {
                terminal
                    .draw(|frame| draw_workspace(frame, workspace))
                    .unwrap();
                terminal.backend().buffer().clone()
            };
            let visible = |buffer: &ratatui::buffer::Buffer| {
                buffer
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            };
            let selected = session("harness.codex", "selected", "completed", None);
            workspace.agents.update(catalog(vec![
                session("harness.codex", "first", "running", None),
                selected.clone(),
            ]));
            assert!(!press(&mut workspace, KeyCode::Down));
            assert_eq!(workspace.agents.current(), Some(&selected));
            let agents = frame(&mut workspace);

            assert!(!press(&mut workspace, KeyCode::F(1)));
            assert!(visible(&frame(&mut workspace)).contains(HELP_CLOSE_HINT));
            assert!(!press(&mut workspace, KeyCode::Up));
            assert!(!press(&mut workspace, KeyCode::Esc));
            assert_eq!(workspace.agents.current(), Some(&selected));
            assert_eq!(frame(&mut workspace), agents);

            assert!(!press(&mut workspace, KeyCode::Enter));
            let WorkspaceRequest::History { selection, before } = requests.try_recv().unwrap()
            else {
                panic!("expected selected session history");
            };
            assert!(before.is_none());
            assert!(
                matches!(selection, HistorySelection::Session(ref session, _) if session == &selected)
            );
            let panes = ["Activity", "Usage"]
                .into_iter()
                .map(|title| {
                    let id =
                        agenturm_kernel::ExtensionId::parse(title.to_ascii_lowercase()).unwrap();
                    let pane = agenturm_kernel::PaneModel {
                        title: title.into(),
                        rows: (0..40)
                            .map(|index| MetricRow {
                                label: format!("Evidence {index}"),
                                value: index.to_string(),
                            })
                            .collect(),
                    };
                    (id, pane)
                })
                .collect();
            workspace.finish(Ok(WorkspaceResponse::History(HistoryPage {
                snapshot: Snapshot {
                    panes,
                    ..Snapshot::default()
                },
                turns: Vec::new(),
                initial_pane: None,
                review_pane: None,
                operation_details: Vec::new(),
                private_telemetry_history: None,
                operation_tables: Default::default(),
                evidence_tables: Default::default(),
            })));
            let _ = frame(&mut workspace);
            assert!(!press(&mut workspace, KeyCode::Right));
            assert!(!press(&mut workspace, KeyCode::Down));
            assert_eq!(workspace.history[0].page.ui.selected_title(), Some("Usage"));
            assert_eq!(workspace.history[0].page.ui.scroll, 1);
            let inspector = frame(&mut workspace);

            assert!(!press(&mut workspace, KeyCode::F(1)));
            assert!(visible(&frame(&mut workspace)).contains(HELP_CLOSE_HINT));
            assert!(!press(&mut workspace, KeyCode::Left));
            assert!(!press(&mut workspace, KeyCode::Esc));
            assert_eq!(workspace.history.len(), 1);
            assert_eq!(frame(&mut workspace), inspector);

            assert!(!press(&mut workspace, KeyCode::Esc));
            assert!(workspace.history.is_empty());
            assert_eq!(frame(&mut workspace), agents);
            assert!(!press(&mut workspace, KeyCode::F(1)));
            assert!(press(&mut workspace, KeyCode::Char('q')));
        }
    }

    #[test]
    fn agents_search_keeps_the_selected_source_when_session_ids_match() {
        let mut workspace = Workspace::new();
        let (sender, requests) = mpsc::channel();
        let selected = session("harness.codex", "d3-main", "completed", None);
        workspace.agents.update(catalog(vec![
            session("otlp.d3", "d3-main", "unknown", None),
            selected.clone(),
            session("harness.pi", "other", "running", None),
        ]));
        let press = |workspace: &mut Workspace, code| {
            workspace.key(&sender, event::KeyEvent::new(code, KeyModifiers::NONE))
        };
        assert!(!press(&mut workspace, KeyCode::Down));
        assert_eq!(workspace.agents.current(), Some(&selected));
        assert!(!press(&mut workspace, KeyCode::Char('/')));
        for ch in "d3-main".chars() {
            assert!(!press(&mut workspace, KeyCode::Char(ch)));
            assert_eq!(workspace.agents.current(), Some(&selected));
        }
        assert!(!press(&mut workspace, KeyCode::Esc));
        assert_eq!(workspace.agents.visible.len(), 2);
        assert_eq!(workspace.agents.current(), Some(&selected));
        assert!(!press(&mut workspace, KeyCode::Char('a')));
        assert_eq!(workspace.agents.visible.len(), 3);
        assert_eq!(workspace.agents.current(), Some(&selected));
        assert!(!press(&mut workspace, KeyCode::Enter));
        assert!(
            matches!(requests.try_recv().unwrap(), WorkspaceRequest::History {
            selection: HistorySelection::Session(ref session, _), before: None,
        } if session == &selected)
        );
    }

    #[test]
    fn tree_collapses_only_explicit_unambiguous_children() {
        let child_source = format!("harness.codex.{}", "a".repeat(32));
        let parent_source = format!("harness.codex.{}", "b".repeat(32));
        let mut agents = AgentList::default();
        agents.update(catalog(vec![
            session(&child_source, "child", "running", Some("parent")),
            session(&parent_source, "parent", "running", None),
            session("harness.pi", "parent", "failed", None),
        ]));
        assert_eq!(
            agents
                .visible
                .iter()
                .map(|row| (row.index, row.depth))
                .collect::<Vec<_>>(),
            [(1, 0), (0, 1), (2, 0)]
        );
        agents.expand(false);
        assert_eq!(
            agents
                .visible
                .iter()
                .map(|row| row.index)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        agents.expand(true);
        assert_eq!(agents.visible.len(), 3);
        agents.toggle_tree();
        assert_eq!(agents.visible[0].index, 0);
        assert_eq!(agents.current().unwrap().harness_id, parent_source);
    }

    #[test]
    fn selection_and_review_filter_use_source_and_session() {
        let row = session("harness.codex", "same", "completed", None);
        let other = session("harness.pi", "same", "completed", None);
        let mut agents = AgentList::default();
        agents.update(catalog(vec![row.clone(), other.clone()]));
        agents.move_selection(true);
        let mut reviewed = other;
        reviewed.reviewed = true;
        agents.update(catalog(vec![reviewed, row]));
        assert_eq!(agents.current().unwrap().harness_id, "harness.pi");
        agents.filter = Filter::ToReview;
        agents.apply_filters(None);
        assert_eq!(agents.current().unwrap().harness_id, "harness.codex");
        assert_eq!(agents.visible.len(), 1);
    }

    #[test]
    fn dashboard_shows_scoped_session_at_both_viewport_sizes() {
        for (width, height) in [(60, 18), (100, 24)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut agents = AgentList::default();
            agents.update(catalog(vec![session(
                "harness.codex",
                "sample-session",
                "interrupted",
                None,
            )]));
            terminal
                .draw(|frame| draw_agents(frame, &mut agents, false, None))
                .unwrap();
            let screen = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            for text in [
                "Agenturm command center",
                "sample-session",
                "interrupted",
                "Enter inspect",
            ] {
                assert!(screen.contains(text), "{text} missing at {width} columns");
            }
            assert!(!screen.contains("Session inspector"));
        }
    }
    #[test]
    fn command_center_groups_projects_and_counts_only_evidenced_states() {
        let mut first = session("harness.codex", "a-session", "running", None);
        first.task_title = Some("Implement checkpoint ordering".into());
        first.project_path = Some("/projects/alpha".into());
        first.project_origin = Some(ProjectOrigin::OperatorTaskWorkspace);
        let mut second = session("harness.pi", "b-session", "completed", None);
        second.project_path = Some("/projects/beta".into());
        second.project_origin = Some(ProjectOrigin::OperatorTaskWorkspace);
        let rows = vec![
            second,
            first,
            session("harness.codex", "needs", "waiting", None),
            session("harness.codex", "inactive", "idle", None),
            session("harness.codex", "unknown", "unrecognized", None),
        ];
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut agents = AgentList::default();
            agents.update(catalog(rows.clone()));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_agents(frame, &mut agents, false, None))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            for expected in [
                "Agenturm command center",
                "Group: Project",
                "All 5",
                "Needs you 1",
                "Working 1",
                "Ready 1",
                "Inactive 1",
                "/projects/alpha",
                "/projects/beta",
                "Project unavailable",
                "Unknown",
            ] {
                assert!(
                    text.contains(expected),
                    "{expected} absent at {width}: {text}"
                );
            }
            assert!(!text.contains("Session inspector"));
            assert_eq!(agents.current().unwrap().session_id, "a-session");
        }
    }

    #[test]
    fn command_center_tab_filter_and_inspector_back_keep_exact_selection() {
        let (sender, receiver) = mpsc::channel();
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![
            session("harness.codex", "same", "waiting", None),
            session("harness.pi", "same", "waiting", None),
            session("harness.pi", "other", "completed", None),
        ]));
        press(&mut workspace, &sender, KeyCode::Tab);
        assert_eq!(workspace.agents.filter, Filter::Attention);
        press(&mut workspace, &sender, KeyCode::Down);
        let selected = workspace.agents.current().unwrap().clone();
        press(&mut workspace, &sender, KeyCode::Char('G'));
        assert_eq!(workspace.agents.grouping, command_center::Grouping::Source);
        assert_eq!(workspace.agents.current(), Some(&selected));
        press(&mut workspace, &sender, KeyCode::Enter);
        let WorkspaceRequest::History {
            selection: HistorySelection::Session(requested, _),
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("exact inspector request")
        };
        assert_eq!(selected, requested);
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        press(&mut workspace, &sender, KeyCode::Esc);
        assert_eq!(workspace.agents.current(), Some(&selected));
        assert_eq!(workspace.agents.filter, Filter::Attention);
        assert!(workspace.history.is_empty());
        assert_eq!(workspace.agents.grouping, command_center::Grouping::Source);
        press(&mut workspace, &sender, KeyCode::BackTab);
        assert_eq!(workspace.agents.filter, Filter::All);
    }

    #[test]
    fn command_center_show_more_is_an_exact_selectable_page_request() {
        let (sender, receiver) = mpsc::channel();
        let mut workspace = Workspace::new();
        let mut page = catalog(vec![session("harness.codex", "selected", "running", None)]);
        let cursor = SessionCursor {
            harness_id: "harness.codex".into(),
            session_id: "selected".into(),
            updated_at_unix_ns: 1,
            snapshot_id: 9.into(),
        };
        page.older = Some(cursor.clone());
        workspace.agents.update(page);
        press(&mut workspace, &sender, KeyCode::Down);
        assert!(workspace.agents.show_more_selected);
        press(&mut workspace, &sender, KeyCode::Enter);
        assert!(
            matches!(receiver.try_recv().unwrap(), WorkspaceRequest::Sessions(Some(actual)) if actual == cursor)
        );
    }

    #[test]
    fn command_center_empty_loading_unavailable_and_cached_error_are_distinct() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for (loaded, pending, message, expected) in [
                (false, true, None, "Loading session evidence"),
                (
                    false,
                    false,
                    Some("Receiver disconnected"),
                    "Session history unavailable",
                ),
                (true, false, None, "No sessions collected yet"),
            ] {
                let mut agents = AgentList::default();
                if loaded {
                    agents.update(catalog(vec![]));
                }
                terminal
                    .draw(|frame| draw_agents(frame, &mut agents, pending, message))
                    .unwrap();
                let text = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>();
                assert!(text.contains(expected), "{width}: {text}");
                assert!(
                    !text.contains("All 0") || loaded,
                    "unknown counts must not become zero"
                );
            }
            let mut agents = AgentList::default();
            agents.update(catalog(vec![session(
                "harness.codex",
                "retained-selection",
                "running",
                None,
            )]));
            terminal
                .draw(|frame| draw_agents(frame, &mut agents, false, Some("Receiver disconnected")))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(text.contains("retained-selection"));
            assert!(text.contains("Receiver disconnected"));
        }
    }
}
