//! Session navigation over immutable history pages supplied by the host.

use super::*;
use crate::workbench::{PromptReceiver, WorkInput, Workbench, prompt_channel};
use agenturm_kernel::{ReadSnapshot, RelationshipKind};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedSession {
    pub harness_id: String,
    pub session_id: String,
    pub updated_at_unix_ns: u64,
    pub snapshot_id: ReadSnapshot,
    pub state: Option<String>,
    pub reason: Option<String>,
    pub turn_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub relationship_kind: Option<RelationshipKind>,
    pub review_version: ReviewVersion,
    pub reviewed: bool,
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

#[derive(Clone, Copy, Debug)]
pub enum WorkspaceView {
    Overview,
    Activity,
    Mcp,
    Operations,
    Findings,
    Host,
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
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Active,
            Self::Active => Self::Attention,
            Self::Attention => Self::Finished,
            Self::Finished => Self::ToReview,
            Self::ToReview => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Attention => "Attention",
            Self::Finished => "Finished",
            Self::ToReview => "To review",
        }
    }

    fn accepts(self, session: &RecordedSession) -> bool {
        let state = session.state.as_deref();
        match self {
            Self::All => true,
            Self::Active => matches!(state, Some("running" | "started" | "restored")),
            Self::Attention => matches!(
                state,
                Some("failed" | "interrupted" | "waiting" | "blocked")
            ),
            Self::Finished => matches!(state, Some("completed" | "ended")),
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
    selection: ListState,
    filter: Filter,
    search: String,
    editing: bool,
    flat: bool,
    help: bool,
    sort_by_state: bool,
    collapsed: BTreeSet<(String, String)>,
}

impl AgentList {
    fn current(&self) -> Option<&RecordedSession> {
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

    fn move_selection(&mut self, down: bool) {
        move_selection(&mut self.selection, self.visible.len(), down);
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

fn move_selection(selection: &mut ListState, count: usize, down: bool) {
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
        Self {
            selection,
            page: PageView::new(page, in_turn),
            newer: VecDeque::new(),
        }
    }
}

enum Change {
    RefreshSessions,
    OlderSessions,
    Open(HistorySelection),
    OlderHistory,
    MarkReviewed(RecordedSession),
    Work(Option<WorkAnchor>),
}

struct Workspace {
    agents: AgentList,
    newer_sessions: VecDeque<SessionCatalog>,
    browsing_older: bool,
    history: Vec<HistoryView>,
    work: Option<Workbench>,
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

struct PromptScreen {
    text: String,
    title: &'static str,
    scroll: u16,
    acknowledge: Option<mpsc::Sender<()>>,
    active: Arc<Mutex<bool>>,
    expires: Instant,
}

impl PromptScreen {
    fn draw(&self, frame: &mut Frame) {
        let [main, foot] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
        let text = self
            .text
            .lines()
            .map(|line| Line::from(visible_text(line)))
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .block(Block::bordered().title(self.title)),
            main,
        );
        frame.render_widget(
            Paragraph::new("↑↓ scroll · Esc close · closes after 25s"),
            foot,
        );
    }
}

impl Workspace {
    fn new() -> Self {
        Self {
            agents: AgentList::default(),
            newer_sessions: VecDeque::new(),
            browsing_older: false,
            history: Vec::new(),
            work: None,
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
            (Change::OlderHistory, Ok(WorkspaceResponse::History(page))) => {
                if let Some(view) = self.history.last_mut() {
                    let mut next =
                        PageView::new(page, matches!(view.selection, HistorySelection::Turn(_)));
                    if let Some(title) = view.page.ui.selected_title() {
                        next.ui.select_title(title);
                    }
                    push_cached(&mut view.newer, std::mem::replace(&mut view.page, next));
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
            (Change::Work(_), Ok(WorkspaceResponse::Work(WorkResult::Status(status)))) => {
                self.prompt_receiver = None;
                self.message = (!status.is_empty()).then_some(status);
            }
            (Change::Work(_), Ok(WorkspaceResponse::Work(WorkResult::Seen(entry)))) => {
                if let Some(work) = &mut self.work {
                    work.note_seen(&entry);
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

    fn reveal(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, entry: PromptEntry) {
        let (delivery, receiver) = prompt_channel();
        self.request(
            sender,
            WorkspaceRequest::Work(WorkCommand::RevealPrompt(entry, delivery)),
            Change::Work(None),
        );
        self.prompt_receiver = Some(receiver);
    }

    fn key(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, key: event::KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if let Some(prompt) = &mut self.prompt {
            match key.code {
                KeyCode::Esc => {
                    self.redact_then_acknowledge = prompt.acknowledge.take();
                    self.prompt = None;
                    self.message = None;
                }
                KeyCode::Char('q') => return true,
                KeyCode::Down | KeyCode::Char('j') => {
                    prompt.scroll = prompt.scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => prompt.scroll = prompt.scroll.saturating_sub(1),
                KeyCode::Home => prompt.scroll = 0,
                _ => {}
            }
            return false;
        }
        if let Some(work) = &mut self.work {
            if key.code == KeyCode::Char('q') && !work.editing() {
                return true;
            }
            if self.pending.is_some() && work.editing() {
                return false;
            }
            match work.key(key) {
                WorkInput::Reveal(entry) if self.pending.is_none() => self.reveal(sender, entry),
                WorkInput::Reveal(entry) if self.prompt_receiver.is_none() => {
                    self.queued_prompt = Some(entry)
                }
                WorkInput::Reveal(_) => {}
                WorkInput::RevealTelemetry(span) if self.pending.is_none() => {
                    let (delivery, mut receiver) = prompt_channel();
                    receiver.title = "Producer-captured text";
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
                WorkInput::None => {}
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
        if self.history.is_empty() {
            return self.agent_key(sender, key.code);
        }
        let view = self.history.last_mut().expect("history exists");
        if view.page.ui.help {
            return view.page.ui.handle_key(key.code);
        }
        if let Some((kind, picker)) = &mut view.page.operation_picker {
            let kind = *kind;
            let entries = view
                .page
                .operation_details
                .iter()
                .filter(|entry| entry.kind == kind);
            match key.code {
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
                        let (delivery, mut receiver) = prompt_channel();
                        receiver.title = match kind {
                            OperationDetailKind::Mcp => "Granted MCP payload",
                            OperationDetailKind::Edit => "Granted file edit",
                        };
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
            match key.code {
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

        if key.code == KeyCode::Char('d') && self.pending.is_none() && telemetry_details(view) {
            self.work = Some(Workbench::new(
                WorkResult::TelemetrySpans(view.page.telemetry_spans.clone()),
                None,
            ));
            self.message = None;
            return false;
        }
        let detail = match key.code {
            KeyCode::Char('m') => Some(OperationDetailKind::Mcp),
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
            view.page.operation_picker = Some((kind, ListState::default().with_selected(Some(0))));
            self.message = None;
            return false;
        }
        match key.code {
            KeyCode::Esc if self.pending.is_none() => {
                self.history.pop();
                self.message = None;
            }
            KeyCode::Esc => {}
            KeyCode::Char('b' | 'p') => {
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
                let command = if key.code == KeyCode::Char('b') {
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
            KeyCode::Char('8') if !view.page.ui.help => {
                if let HistorySelection::Session(session, kind) = &view.selection
                    && !matches!(kind, SessionView::Traces)
                {
                    let selection = HistorySelection::Session(session.clone(), SessionView::Traces);
                    self.open(sender, selection);
                }
            }
            KeyCode::Char('[') if !view.page.ui.help => {
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
            KeyCode::Char(']') if self.pending.is_none() && !view.page.ui.help => {
                if let Some(newer) = view.newer.pop_back() {
                    view.page = newer;
                }
            }
            KeyCode::Char('t') if view.page.ui.turn_available && !view.page.ui.help => {
                view.page.turn_picker = Some(ListState::default().with_selected(Some(0)));
            }
            key => return view.page.ui.handle_key(key),
        }
        false
    }

    fn agent_key(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, key: KeyCode) -> bool {
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
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => self.agents.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.agents.move_selection(false),
            KeyCode::Char('/') => self.agents.editing = true,
            KeyCode::Char('h') => self.agents.toggle_tree(),
            KeyCode::Char('?') => self.agents.help = true,
            KeyCode::Char('s') => {
                let identity = self.agents.current().map(session_key);
                self.agents.sort_by_state = !self.agents.sort_by_state;
                self.agents.apply_filters(identity.as_ref());
            }
            KeyCode::Left => self.agents.expand(false),
            KeyCode::Right => self.agents.expand(true),
            KeyCode::Char('x') => {
                let identity = self.agents.current().map(session_key);
                self.agents.search.clear();
                self.agents.filter = Filter::All;
                self.agents.apply_filters(identity.as_ref());
            }
            KeyCode::Char('f') => {
                self.agents.filter = self.agents.filter.next();
                self.agents.apply_filters(None);
            }
            KeyCode::Char('r') => self.request(
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
            KeyCode::Char('p') => {
                if let Some(session) = self.agents.current() {
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::Prompts(WorkAnchor::from(session))),
                        Change::Work(None),
                    );
                }
            }
            KeyCode::Char('6') => self.request(
                sender,
                WorkspaceRequest::Work(WorkCommand::HostHistory(None)),
                Change::Work(None),
            ),
            KeyCode::Char('g' | 'H' | '2'..='5' | '7' | '8') => {
                let view = match key {
                    KeyCode::Char('2') => WorkspaceView::Activity,
                    KeyCode::Char('3') => WorkspaceView::Mcp,
                    KeyCode::Char('4') => WorkspaceView::Operations,
                    KeyCode::Char('5') => WorkspaceView::Findings,
                    KeyCode::Char('H') => WorkspaceView::Host,
                    KeyCode::Char('7') => WorkspaceView::Usage,
                    KeyCode::Char('8') => WorkspaceView::Traces,
                    _ => WorkspaceView::Overview,
                };
                self.open(sender, HistorySelection::All(view));
            }
            KeyCode::Enter | KeyCode::Char('a' | 'v' | 'u') => {
                if let Some(session) = self.agents.current().cloned() {
                    let view = match key {
                        KeyCode::Char('a') => SessionView::Activity,
                        KeyCode::Char('v') => SessionView::Review,
                        KeyCode::Char('u') => SessionView::Usage,
                        _ => SessionView::Overview,
                    };
                    self.open(sender, HistorySelection::Session(session, view));
                }
            }
            KeyCode::Char('[') => {
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
            KeyCode::Char(']') if self.pending.is_none() => {
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
                        workspace.prompt = Some(PromptScreen {
                            text,
                            title: receiver.title,
                            scroll: 0,
                            acknowledge: Some(receiver.acknowledgement),
                            active: receiver.active,
                            expires: Instant::now() + Duration::from_secs(25),
                        });
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
                    && workspace.pending.is_none()
                    && !workspace.browsing_older
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
                if workspace
                    .prompt
                    .as_ref()
                    .is_some_and(|prompt| Instant::now() >= prompt.expires)
                    && let Some(mut prompt) = workspace.prompt.take()
                {
                    workspace.redact_then_acknowledge = prompt.acknowledge.take();
                    dirty = true;
                }
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
                    if let Some(acknowledge) = workspace.redact_then_acknowledge.take() {
                        let _ = acknowledge.send(());
                    }
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
                                if let Some(mut prompt) = workspace.prompt.take() {
                                    terminal.draw(|frame| draw_workspace(frame, &mut workspace))?;
                                    if let Some(acknowledge) = prompt.acknowledge.take() {
                                        let _ = acknowledge.send(());
                                    }
                                }
                                break Ok(());
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

fn draw_workspace(frame: &mut Frame, workspace: &mut Workspace) {
    if let Some(prompt) = &workspace.prompt {
        prompt.draw(frame);
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
    if let Some(view) = workspace.history.last_mut() {
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
        view.page.ui.set_viewport(area.width, area.height);
        draw(
            frame,
            &view.page.ui,
            !view.newer.is_empty(),
            view.page
                .turn_picker
                .as_mut()
                .map(|state| (view.page.turns.as_slice(), state)),
        );
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
            } else if telemetry_details(view) {
                "d captured text · Esc back · [ ] pages"
            } else if let HistorySelection::Session(session, _) = &view.selection
                && view.page.reviewing()
            {
                if has_details(OperationDetailKind::Edit) {
                    if session.reviewed {
                        "Reviewed · e edits · n decisions · 8 traces · Esc back"
                    } else {
                        "y review · e edits · n decisions · 8 traces · Esc back"
                    }
                } else if session.reviewed {
                    "Reviewed · n decisions · 8 traces · Esc back"
                } else {
                    "y mark reviewed · n decisions · 8 traces · Esc back"
                }
            } else if view.page.ui.in_turn && has_details(OperationDetailKind::Edit) {
                if has_details(OperationDetailKind::Mcp) {
                    "e edits · m MCP payloads · Esc back · [ ] pages"
                } else {
                    "e edits · Esc back · [ ] history pages"
                }
            } else if has_details(OperationDetailKind::Mcp)
                && matches!(view.selection, HistorySelection::Session(_, kind) if !matches!(kind, SessionView::Traces))
            {
                "m payloads · 8 traces · Esc back · [ ] pages"
            } else if has_details(OperationDetailKind::Mcp) {
                "m MCP payloads · Esc back · [ ] history pages"
            } else if matches!(view.selection, HistorySelection::Session(_, kind) if !matches!(kind, SessionView::Traces))
            {
                "8 session traces · Esc back · [ ] history pages"
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

fn source_label(source: &str) -> &str {
    source
        .strip_prefix("harness.")
        .unwrap_or(source)
        .split('.')
        .next()
        .unwrap_or(source)
}

fn recorded_state(row: &RecordedSession) -> &str {
    row.state.as_deref().unwrap_or("Unknown")
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
    !view.page.telemetry_spans.is_empty()
        && matches!(
            view.selection,
            HistorySelection::All(WorkspaceView::Traces)
                | HistorySelection::Session(_, SessionView::Traces)
        )
}

fn draw_agents(frame: &mut Frame, agents: &mut AgentList, pending: bool, message: Option<&str>) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new("Terminal too small · widen the window"),
            area,
        );
        return;
    }
    let [header, content, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);
    let total = agents
        .catalog
        .as_ref()
        .map_or(0, |catalog| catalog.sessions.len());
    let count = if pending && agents.catalog.is_none() {
        "Loading…".into()
    } else {
        format!("{} of {} on page", agents.visible.len(), total)
    };
    let filter = agents.filter.label();
    let mut lines = vec![Line::from(vec![
        Span::styled(
            " Agenturm  /  Agents",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("   {filter} · {count}")),
    ])];
    if agents.editing || !agents.search.is_empty() {
        let cursor = if agents.editing { "▏" } else { "" };
        lines.push(Line::from(format!(
            " / {}{cursor}",
            visible_text(&agents.search)
        )));
    }
    if !agents.editing && agents.search.is_empty() {
        let order = if agents.sort_by_state {
            "Attention first"
        } else {
            "Newest first"
        };
        lines.push(Line::from(format!("  {order}")));
    }
    frame.render_widget(Paragraph::new(lines), header);
    let wide = area.width >= 84;
    let [left, right] = if wide {
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(content)
    } else {
        [content, Rect::default()]
    };
    if agents.help {
        frame.render_widget(
            Paragraph::new("Enter Overview · a Activity · v Review · u Usage\n2 Activity · 3 MCP · 4 Operations · 5 Findings\n6 Host history · H Live host · 7 Usage · 8 Traces\ng History · b Tasks · w Workflow · p Prompts\n/ Search · x Clear · f Filter · s Sort · r Refresh\n←/→ Collapse/expand · h Flat/tree · [ ] Page · q Quit")
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(HELP_CLOSE_HINT)),
            content,
        );
        return;
    }
    if let Some(catalog) = &agents.catalog {
        let items = agents.visible.iter().map(|item| {
            let row = &catalog.sessions[item.index];
            let indent = "  ".repeat(item.depth.min(6));
            let marker = if item.has_children {
                if agents.collapsed.contains(&session_key(row)) {
                    "+ "
                } else {
                    "− "
                }
            } else {
                ""
            };
            ListItem::new(visible_text(&format!(
                "{indent}{marker}{} · {} · {}",
                recorded_state(row),
                source_label(&row.harness_id),
                row.session_id
            )))
        });
        if agents.visible.is_empty() {
            let empty = if catalog.sessions.is_empty() {
                "No sessions collected yet"
            } else {
                "No matches on this page"
            };
            frame.render_widget(
                Paragraph::new(empty).block(Block::bordered().title("Sessions")),
                left,
            );
        } else {
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::bordered().title("Sessions"))
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                left,
                &mut agents.selection,
            );
        }
    }
    if wide && let Some(row) = agents.current() {
        let mut lines = vec![
            Line::from(visible_text(&row.session_id)),
            Line::from(visible_text(&row.harness_id)),
            Line::from(format!(
                "Recorded state   {}",
                visible_text(recorded_state(row))
            )),
            Line::from(format!(
                "Last observation   {}",
                age(row.updated_at_unix_ns)
            )),
        ];
        if row.reviewed {
            lines.push(Line::from("Review   Reviewed"));
        }
        if let Some(reason) = &row.reason {
            lines.push(Line::from(format!("Reason   {}", visible_text(reason))));
        }
        if let Some(parent) = &row.parent_session_id {
            lines.push(Line::from(format!(
                "Recorded parent   {}",
                visible_text(parent)
            )));
        }
        if let Some(turn) = &row.turn_id {
            lines.push(Line::from(format!("Turn   {}", visible_text(turn))));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title("Selected session")),
            right,
        );
    }
    let keys = if agents.editing {
        "Enter / Esc done · Backspace edit"
    } else {
        "Enter overview · a activity · v review · / search"
    };
    let status = message.map(visible_text).unwrap_or_else(|| {
        if pending {
            "Loading…".into()
        } else {
            "b tasks · w workflow · p prompts · F1 help · q quit".into()
        }
    });
    frame.render_widget(
        Paragraph::new(vec![Line::from(keys), Line::from(status)]),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn session(source: &str, id: &str, state: &str, parent: Option<&str>) -> RecordedSession {
        RecordedSession {
            harness_id: source.into(),
            session_id: id.into(),
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
        }
    }

    fn catalog(sessions: Vec<RecordedSession>) -> SessionCatalog {
        SessionCatalog {
            sessions,
            older: None,
            notice: None,
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
                private_telemetry: false,
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
        assert!(!press(&mut workspace, KeyCode::Char('x')));
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
            for text in ["Agents", "sample-session", "interrupted", "Enter overview"] {
                assert!(screen.contains(text), "{text} missing at {width} columns");
            }
            assert_eq!(screen.contains("Selected session"), width >= 84);
        }
    }
}
