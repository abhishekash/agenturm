//! Session navigation over immutable history pages supplied by the host.

use super::*;
use crate::keymap::{self, Destination};
use crate::workbench::investigation::{Investigation, InvestigationInput};
use crate::workbench::{PromptKind, PromptReceiver, WorkInput, Workbench, prompt_channel};
use agenturm_kernel::workflow_notifications::WorkflowNotificationAuthority;
use agenturm_kernel::{ReadSnapshot, RelationshipKind};
use ratatui::buffer::CellWidth;
use ratatui::widgets::{Row, Table, TableState};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod analytics;
mod command_center;
mod inspector;
pub(crate) mod prompt;
mod review_visibility;
mod view_io;

use prompt::PromptScreen;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedSession {
    pub harness_id: String,
    pub session_id: String,
    /// Exact worker identity when the recorded lifecycle names one.
    pub worker_id: Option<String>,
    pub task_title: Option<Box<str>>,
    pub native_context: Option<Box<NativeSessionContext>>,
    /// Operator-linked project or the exact native session’s recorded workspace.
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

/// Explicit display metadata, separate from stable source/session identity.
impl RecordedSession {
    /// Human-readable primary label; stable identity stays in details and links.
    pub fn display_name(&self) -> std::borrow::Cow<'_, str> {
        command_center::name(self)
    }

    /// Human provider label derived from the recorded source family. This is a
    /// presentation fallback, never a routing decision.
    pub fn provider_label(&self) -> &str {
        command_center::provider(self)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeSessionContext {
    pub agent_name: Option<Box<str>>,
    pub agent_role: Option<Box<str>>,
    pub agent_path: Option<Box<str>>,
    pub model_provider: Option<Box<str>>,
    pub last_activity: Option<Box<str>>,
}

/// Authority for a public project heading; never inferred from captured text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectOrigin {
    OperatorTaskWorkspace,
    NativeRecordedWorkspace,
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
    /// Local calendar-day start in Unix nanoseconds, resolved by the host for
    /// the instant this page was read. It keeps the Today boundary off the UI
    /// thread and out of provider code.
    pub day_start_unix_ns: u64,
}

#[derive(Clone, Debug)]
pub enum HistorySelection {
    All(WorkspaceView),
    Session(RecordedSession, SessionView),
    ReviewEdits(Box<RecordedSession>),
    Turn(RecordedTurn),
    /// A Review route pinned to one exact recorded turn and worker.
    TurnReview(Box<RecordedTurn>),
    McpSearch {
        scope: McpSearchScope,
        query: String,
        snapshot_id: ReadSnapshot,
    },
    McpTraces {
        source_id: ExtensionId,
        event_id: String,
        revision: u32,
        snapshot_id: ReadSnapshot,
    },
}

#[derive(Clone, Debug)]
pub enum McpSearchScope {
    All,
    Session(Box<RecordedSession>),
    Turn(Box<RecordedTurn>),
}

impl McpSearchScope {
    fn anchor(&self) -> Option<WorkAnchor> {
        match self {
            Self::All => None,
            Self::Session(row) => Some(WorkAnchor::from(row.as_ref())),
            Self::Turn(row) => Some(WorkAnchor {
                source: row.harness_id.clone(),
                session: row.session_id.clone(),
                worker: row.worker_id.clone(),
                turn: Some(row.turn_id.clone()),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionView {
    Overview,
    Details,
    Activity,
    Review,
    Metrics,
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

/// Roster time scope. `All` is the approved landing behavior; `Today` is the
/// discoverable narrowed scope and `History` keeps the complete retained page.
/// UI-03 owns the final Today boundary; this shell preserves the scope across
/// refresh, resize and session changes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum TimeScope {
    Today,
    #[default]
    All,
}

impl TimeScope {
    fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::All => "History",
        }
    }

    fn kernel(self) -> agenturm_kernel::session_scope::SessionTimeScope {
        match self {
            Self::Today => agenturm_kernel::session_scope::SessionTimeScope::Today,
            Self::All => agenturm_kernel::session_scope::SessionTimeScope::History,
        }
    }

    /// The neutral kernel rule consumes the host-resolved local day start.
    /// `History` (the approved landing scope) always accepts.
    fn accepts(self, row: &RecordedSession, day_start_unix_ns: u64) -> bool {
        agenturm_kernel::session_scope::in_session_time_scope(
            self.kernel(),
            agenturm_kernel::session_scope::SessionScopeFacts {
                state: row.state.as_deref(),
                updated_at_unix_ns: row.updated_at_unix_ns,
                pending_input: row.pending_requests.iter().any(|request| !request.seen),
                reviewed: row.reviewed,
            },
            day_start_unix_ns,
        )
    }
}

pub enum WorkspaceRequest {
    Sessions(Option<SessionCursor>),
    MarkReviewed(RecordedSession),
    Work(WorkCommand),
    MetricHistory(Box<agenturm_kernel::retained_history::MetricHistoryQuery>),
    ReviewVisibility(bool),
    History {
        selection: HistorySelection,
        before: Option<ObservationPosition>,
        include_reviews: bool,
    },
}

pub enum WorkspaceResponse {
    Sessions(SessionCatalog),
    ReviewSaved,
    Work(WorkResult),
    MetricHistory(agenturm_kernel::retained_history::MetricHistoryPage),
    ReviewVisibility(bool),
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
                    && matches!(
                        state,
                        Some("paused" | "idle" | "stopped" | "cancelled" | "closed")
                    )
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
    show_more_selected: bool,
    display_offset: usize,
    paused: bool,
    collapsed: BTreeSet<(String, String)>,
    time_scope: TimeScope,
    session_scope: Option<(String, String)>,
    errors_only: bool,
    /// Include confirmed delegated descendants of `session_scope`.
    subtree_scope: bool,
    scope_members: Option<BTreeSet<(String, String)>>,
    subtree_unsupported: usize,
    subtree_outside_page: usize,
    lost_selection: bool,
}

impl AgentList {
    fn current(&self) -> Option<&RecordedSession> {
        if self.show_more_selected {
            return None;
        }
        let position = self.visible.get(self.selection.selected()?)?.index;
        self.catalog.as_ref()?.sessions.get(position)
    }

    /// Returns true when a prior identity disappeared from the new page.
    fn update(&mut self, catalog: SessionCatalog) -> bool {
        let identity = self
            .current()
            .map(|row| (row.harness_id.clone(), row.session_id.clone()));
        self.catalog = Some(catalog);
        self.apply_filters(identity.as_ref());
        self.lost_selection
    }

    /// Exact-agent or evidence-backed subtree membership, without time scope.
    fn in_identity_scope(&self, session: &RecordedSession) -> bool {
        match &self.session_scope {
            None => true,
            Some(_) => self
                .scope_members
                .as_ref()
                .is_some_and(|members| members.contains(&session_key(session))),
        }
    }

    /// Membership and time scope, without the status, error or search facets.
    fn in_session_scope(&self, session: &RecordedSession) -> bool {
        self.in_identity_scope(session) && self.time_scope.accepts(session, self.day_start())
    }

    fn matches_search(&self, session: &RecordedSession) -> bool {
        let query = self.search.to_ascii_lowercase();
        query.is_empty()
            || session.session_id.to_ascii_lowercase().contains(&query)
            || session.harness_id.to_ascii_lowercase().contains(&query)
            || session
                .worker_id
                .as_deref()
                .is_some_and(|worker| worker.to_ascii_lowercase().contains(&query))
            || command_center::provider(session)
                .to_ascii_lowercase()
                .contains(&query)
            || session
                .task_title
                .as_deref()
                .is_some_and(|title| title.to_ascii_lowercase().contains(&query))
            || session
                .project_path
                .as_deref()
                .is_some_and(|project| project.to_ascii_lowercase().contains(&query))
            || session.native_context.as_deref().is_some_and(|context| {
                [
                    context.agent_name.as_deref(),
                    context.agent_role.as_deref(),
                    context.agent_path.as_deref(),
                    context.model_provider.as_deref(),
                ]
                .iter()
                .flatten()
                .any(|name| name.to_ascii_lowercase().contains(&query))
            })
            || session
                .state
                .as_ref()
                .is_some_and(|state| state.contains(&query))
            || session
                .reason
                .as_ref()
                .is_some_and(|reason| reason.contains(&query))
    }

    fn accepts_scope(&self, session: &RecordedSession) -> bool {
        self.in_session_scope(session)
            && (!self.errors_only || errors_only(session))
            && self.matches_search(session)
    }

    fn day_start(&self) -> u64 {
        self.catalog
            .as_ref()
            .map_or(0, |catalog| catalog.day_start_unix_ns)
    }

    /// Recomputes the exact-agent or evidence-backed subtree membership once
    /// per filter pass. Unsupported and off-page relationships stay counted so
    /// the footer can disclose them instead of silently widening the scope.
    fn refresh_scope_members(&mut self) {
        let Some(root) = self.session_scope.clone() else {
            self.scope_members = None;
            self.subtree_unsupported = 0;
            self.subtree_outside_page = 0;
            return;
        };
        let Some(catalog) = &self.catalog else {
            self.scope_members = Some(BTreeSet::from([root]));
            self.subtree_unsupported = 0;
            self.subtree_outside_page = 0;
            return;
        };
        let (members, unsupported, outside) =
            scope_members(&catalog.sessions, &root, self.subtree_scope);
        self.scope_members = Some(members);
        self.subtree_unsupported = unsupported;
        self.subtree_outside_page = outside;
    }

    fn apply_filters(&mut self, identity: Option<&(String, String)>) {
        self.refresh_scope_members();
        let previous = self.selection.selected().unwrap_or_default();
        self.visible = self.catalog.as_ref().map_or_else(Vec::new, |catalog| {
            let matching = catalog
                .sessions
                .iter()
                .enumerate()
                .filter_map(|(index, session)| {
                    (self.accepts_scope(session) && self.filter.accepts(session)).then_some(index)
                })
                .collect::<Vec<_>>();
            agent_rows(&catalog.sessions, &matching, self.flat, &self.collapsed)
        });
        if let Some(catalog) = &self.catalog {
            let mut newest = std::collections::BTreeMap::<&str, u64>::new();
            for item in &self.visible {
                let row = &catalog.sessions[item.index];
                newest
                    .entry(command_center::group(row, self.grouping))
                    .and_modify(|time| *time = (*time).max(row.updated_at_unix_ns))
                    .or_insert(row.updated_at_unix_ns);
            }
            self.visible.sort_by(|a, b| {
                let left = &catalog.sessions[a.index];
                let right = &catalog.sessions[b.index];
                let left_group = command_center::group(left, self.grouping);
                let right_group = command_center::group(right, self.grouping);
                newest[right_group]
                    .cmp(&newest[left_group])
                    .then_with(|| left_group.cmp(right_group))
                    .then_with(|| right.updated_at_unix_ns.cmp(&left.updated_at_unix_ns))
                    .then_with(|| a.depth.cmp(&b.depth))
                    .then_with(|| left.harness_id.cmp(&right.harness_id))
                    .then_with(|| left.session_id.cmp(&right.session_id))
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
        self.lost_selection = identity.is_some() && selected.is_none();
        let selected =
            selected.unwrap_or_else(|| previous.min(self.visible.len().saturating_sub(1)));
        self.selection
            .select((!self.visible.is_empty()).then_some(selected));
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

/// Recorded failure filter. This is a page filter over recorded lifecycle, not
/// a claim about unseen errors or current process health; the footer and help
/// say so.
fn errors_only(row: &RecordedSession) -> bool {
    matches!(row.state.as_deref(), Some("failed" | "interrupted"))
}

/// Exact-agent membership or an evidence-backed delegated subtree.
///
/// Only a `delegated` relationship with a parent retained on this page joins
/// the subtree. `forked`, cross-family and off-page parents are counted, not
/// guessed. This keeps the scope honest: an unsupported relationship never
/// silently becomes a descendant, and a recorded lifecycle never proves a live
/// process.
fn scope_members(
    sessions: &[RecordedSession],
    root: &(String, String),
    subtree: bool,
) -> (BTreeSet<(String, String)>, usize, usize) {
    let mut members = BTreeSet::from([root.clone()]);
    if !subtree {
        return (members, 0, 0);
    }
    let identities = sessions
        .iter()
        .map(|row| {
            (
                source_family(&row.harness_id).to_owned(),
                row.session_id.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let mut unsupported = 0;
    let mut outside = 0;
    let mut frontier = vec![root.1.clone()];
    while let Some(parent) = frontier.pop() {
        for row in sessions {
            if row.parent_session_id.as_deref() != Some(parent.as_str()) {
                continue;
            }
            let same_family = source_family(&row.harness_id) == source_family(&root.0);
            if !same_family || row.relationship_kind != Some(RelationshipKind::Delegated) {
                unsupported += 1;
                continue;
            }
            if members.insert(session_key(row)) {
                frontier.push(row.session_id.clone());
            }
        }
    }
    // A delegated row whose parent is not retained on this page cannot be
    // attributed to any root on this page. Report the coverage gap rather than
    // attaching it to the nearest relationship.
    for row in sessions {
        if row.relationship_kind == Some(RelationshipKind::Delegated)
            && source_family(&row.harness_id) == source_family(&root.0)
            && let Some(parent) = row.parent_session_id.as_deref()
            && !identities.contains(&(source_family(&row.harness_id).to_owned(), parent.to_owned()))
        {
            outside += 1;
        }
    }
    (members, unsupported, outside)
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

fn plain_key(ch: char) -> event::KeyEvent {
    event::KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
}

fn plain_code(code: KeyCode) -> event::KeyEvent {
    event::KeyEvent::new(code, KeyModifiers::NONE)
}

const CACHE_LIMIT: usize = 4;
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// Deterministic Review control order for Tab / Shift+Tab and Enter/Space.
const REVIEW_CONTROLS: &[(&str, char)] = &[
    ("Earlier", ']'),
    ("Refresh", 'R'),
    ("Copy", 'c'),
    ("Export", 'x'),
    ("Save task", 's'),
    ("Excerpt", 'o'),
    ("Draft", 'd'),
    ("Back", '\u{0}'),
];

struct HistoryView {
    selection: HistorySelection,
    page: PageView,
    newer: VecDeque<PageView>,
    analytics: Option<analytics::HistoryAnalytics>,
    show_evidence: bool,
}

impl HistoryView {
    fn new(selection: HistorySelection, mut page: HistoryPage) -> Self {
        let analytics = analytics::HistoryAnalytics::take(&selection, &mut page);
        let mut page = Self::make_page(&selection, page);
        if matches!(
            selection,
            HistorySelection::All(WorkspaceView::Metrics)
                | HistorySelection::Session(_, SessionView::Metrics)
        ) {
            page.ui.select_metrics();
        }
        if let HistorySelection::McpSearch { query, .. } = &selection {
            page.operation_table.show_retained_search(query);
        }
        Self {
            selection,
            page,
            newer: VecDeque::new(),
            analytics,
            show_evidence: false,
        }
    }

    fn make_page(selection: &HistorySelection, page: HistoryPage) -> PageView {
        let mut page = PageView::new(page, matches!(selection, HistorySelection::Turn(_)));
        if matches!(selection, HistorySelection::ReviewEdits(_)) {
            let selected = (!page.operation_details.is_empty()).then_some(0);
            page.operation_picker = Some((
                OperationDetailKind::Edit,
                ListState::default().with_selected(selected),
            ));
        }
        page
    }

    fn active_analytics(&self) -> Option<&analytics::HistoryAnalytics> {
        self.analytics.as_ref().filter(|_| !self.show_evidence)
    }

    fn active_analytics_mut(&mut self) -> Option<&mut analytics::HistoryAnalytics> {
        self.analytics.as_mut().filter(|_| !self.show_evidence)
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
    MetricHistory {
        cancelled: bool,
    },
    ReviewVisibility,
    MarkReviewed(RecordedSession),
    Work(Option<WorkAnchor>),
    ReviewCopy,
    ReviewExport(String),
    PromptPage(u64),
    WorkflowResume(Box<RequestEntry>),
    WorkflowRetry {
        authority: WorkflowNotificationAuthority,
        key: [u8; 32],
        revision: u64,
        attempt: u64,
    },
    FindingSource {
        cancelled: bool,
    },
    Investigation,
}

impl Change {
    fn is_work(&self) -> bool {
        matches!(
            self,
            Self::Work(_)
                | Self::ReviewCopy
                | Self::ReviewExport(_)
                | Self::PromptPage(_)
                | Self::WorkflowResume(_)
                | Self::WorkflowRetry { .. }
        )
    }
}

struct Workspace {
    view_io: view_io::ViewIo,
    agents: AgentList,
    newer_sessions: VecDeque<SessionCatalog>,
    browsing_older: bool,
    include_reviews: bool,
    history: Vec<HistoryView>,
    work: Option<Workbench>,
    investigation: Option<Investigation>,
    discard_work_result: bool,
    prompt_receiver: Option<PromptReceiver>,
    queued_prompt: Option<PromptEntry>,
    prompt: Option<PromptScreen>,
    prompt_nav: Option<prompt::PromptNavigator>,
    prompt_cursor: Option<crate::workbench::PromptHistoryCursor>,
    prompt_retry_cursor: Option<crate::workbench::PromptHistoryCursor>,
    prompt_read_epoch: Option<u64>,
    prompt_read_key: Option<(String, u32)>,
    deferred_prompt_read: Option<(PromptEntry, u64)>,
    redact_then_acknowledge: Option<mpsc::Sender<()>>,
    pending: Option<Change>,
    deferred_request: Option<(WorkspaceRequest, Change)>,
    message: Option<String>,
    refresh_after: Instant,
    palette: palette::Palette,
    help: help::Help,
    inspector: inspector::State,
    theme_request: bool,
    pending_task_search: bool,
    generation: u64,
    pending_generation: Option<u64>,
}

impl Workspace {
    fn new() -> Self {
        Self {
            view_io: view_io::ViewIo::default(),
            agents: AgentList::default(),
            newer_sessions: VecDeque::new(),
            browsing_older: false,
            include_reviews: false,
            history: Vec::new(),
            work: None,
            investigation: None,
            discard_work_result: false,
            prompt_receiver: None,
            queued_prompt: None,
            prompt: None,
            prompt_nav: None,
            prompt_cursor: None,
            prompt_retry_cursor: None,
            prompt_read_epoch: None,
            prompt_read_key: None,
            deferred_prompt_read: None,
            redact_then_acknowledge: None,
            pending: None,
            deferred_request: None,
            message: None,
            refresh_after: Instant::now(),
            palette: palette::Palette::default(),
            help: help::Help::default(),
            inspector: inspector::State::default(),
            theme_request: false,
            pending_task_search: false,
            generation: 0,
            pending_generation: None,
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
            self.pending_generation = Some(self.generation);
            self.message = None;
        } else {
            self.message = Some("History reader stopped".into());
        }
    }

    fn finish(&mut self, result: Result<WorkspaceResponse, String>) {
        let Some(change) = self.pending.take() else {
            return;
        };
        let generation = self.pending_generation.take();
        self.refresh_after = Instant::now() + REFRESH_INTERVAL;
        if generation.is_some_and(|generation| generation != self.generation) {
            self.discard_work_result = false;
            self.message = Some("Discarded a late response for a closed or superseded view".into());
            return;
        }
        if change.is_work() && std::mem::take(&mut self.discard_work_result) {
            return;
        }
        if let Change::MetricHistory { cancelled } = change {
            if !cancelled {
                self.finish_metric_read(result);
            }
            return;
        }
        if matches!(change, Change::FindingSource { cancelled: true }) {
            return;
        }
        if matches!(change, Change::Investigation) && self.investigation.is_none() {
            return;
        }
        match (change, result) {
            (_, Err(error)) => {
                self.prompt_receiver = None;
                if let Some(form) = &mut self.view_io.review_form {
                    form.message = Some(error.clone());
                }
                self.message = Some(error);
            }
            (
                Change::ReviewVisibility,
                Ok(WorkspaceResponse::ReviewVisibility(include_reviews)),
            ) => {
                self.include_reviews = include_reviews;
                if let Some(current) = self.history.last().map(|view| view.selection.clone()) {
                    self.history.clear();
                    self.deferred_request = Some((
                        self.history_request(current.clone(), None),
                        Change::Open(current),
                    ));
                }
                self.message = Some(if include_reviews {
                    "Showing captured internal review metadata in history".into()
                } else {
                    "Internal review metadata hidden".into()
                });
            }
            (Change::RefreshSessions, Ok(WorkspaceResponse::Sessions(catalog))) => {
                if self.agents.update(catalog) {
                    self.message = Some(
                        "The selected session left this page · kept the nearest recorded row"
                            .into(),
                    );
                }
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
                let mut next = HistoryView::new(selection, page);
                if matches!(next.selection, HistorySelection::McpSearch { .. })
                    && let Some(previous) = self.history.last()
                {
                    next.page
                        .operation_table
                        .inherit_server_filters(&previous.page.operation_table);
                }
                self.history.push(next);
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
                Ok(WorkspaceResponse::History(mut page)),
            ) => {
                let mut missing_pane = None;
                if let Some(view) = self.history.last_mut() {
                    if matches!(change, Change::LatestHistory) {
                        view.analytics =
                            analytics::HistoryAnalytics::take(&view.selection, &mut page);
                        view.show_evidence &= view.analytics.is_some();
                    }
                    let previous_title = view.page.ui.selected_title().map(str::to_owned);
                    let previous_scroll = view.page.ui.scroll;
                    let previous_index = view.page.ui.selected;
                    let mut next = HistoryView::make_page(&view.selection, page);
                    if matches!(change, Change::OlderHistory) {
                        next.review_before = view.page.ui.older_cursor().cloned();
                    }
                    let metrics = previous_title.as_deref() == Some("Metrics")
                        && matches!(
                            view.selection,
                            HistorySelection::All(WorkspaceView::Metrics)
                                | HistorySelection::Session(_, SessionView::Metrics)
                        );
                    let restored = if metrics {
                        next.ui.select_metrics();
                        true
                    } else {
                        next.ui
                            .restore_view(previous_title.as_deref(), previous_scroll)
                    };
                    if !restored && previous_title.is_some() {
                        // Deterministic nearby selection: keep the prior row
                        // position when the exact pane identity disappeared.
                        next.ui.selected =
                            previous_index.min(next.ui.panes.len().saturating_sub(1));
                        next.ui.restore_view(None, 0);
                        missing_pane = previous_title;
                    }
                    next.inherit_filters(&view.page);
                    if matches!(change, Change::OlderHistory) {
                        push_cached(&mut view.newer, std::mem::replace(&mut view.page, next));
                    } else {
                        view.newer.clear();
                        view.page = next;
                    }
                }
                if let Some(title) = missing_pane {
                    self.message = Some(format!(
                        "{title} is no longer in this page · kept the nearest view"
                    ));
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
                Change::Work(_)
                | Change::WorkflowResume(_)
                | Change::FindingSource { .. }
                | Change::Investigation,
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
            (
                Change::WorkflowResume(expected),
                Ok(WorkspaceResponse::Work(WorkResult::WorkflowResume(receipt))),
            ) => {
                let result = self
                    .work
                    .as_mut()
                    .filter(|_| *expected == receipt.request)
                    .ok_or("The selected workflow request changed")
                    .and_then(|work| work.accept_resume(receipt));
                match result
                    .map_err(str::to_owned)
                    .and_then(|command| crate::loaded_view::clipboard_request(&command))
                {
                    Ok(request) => {
                        self.view_io.clipboard = Some(request);
                        self.message = Some(
                            "Clipboard request sent · terminal permission/support required".into(),
                        );
                    }
                    Err(error) => self.message = Some(error),
                }
            }
            (
                Change::WorkflowRetry {
                    authority,
                    key,
                    revision,
                    attempt,
                },
                Ok(WorkspaceResponse::Work(WorkResult::WorkflowDeliveryUpdated(updated))),
            ) => {
                let result = self
                    .work
                    .as_mut()
                    .filter(|_| {
                        updated.authority == authority
                            && updated.key == key
                            && updated.policy_revision == revision
                            && attempt.checked_add(1) == Some(updated.attempt)
                    })
                    .ok_or("The workflow delivery receipt changed")
                    .and_then(|work| work.accept_delivery(&updated));
                self.message = Some(match result {
                    Ok(state) => format!("Notification retry · {state}"),
                    Err(error) => error.into(),
                });
            }
            (_, Ok(WorkspaceResponse::Work(WorkResult::WorkflowDeliveryUpdated(_)))) => {
                self.message = Some("No matching workflow retry was requested".into());
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
            (
                Change::ReviewCopy,
                Ok(WorkspaceResponse::Work(WorkResult::ReviewDocument { text, scope, .. })),
            ) => match crate::loaded_view::clipboard_request(&text) {
                Ok(request) => {
                    self.view_io.clipboard = Some(request);
                    self.message = Some(format!(
                        "Review receipt copied · {scope} · terminal permission/support required"
                    ));
                }
                Err(error) => self.message = Some(error),
            },
            (
                Change::ReviewExport(directory),
                Ok(WorkspaceResponse::Work(WorkResult::ReviewDocument { scope, .. })),
            ) => {
                self.view_io.review_form = None;
                self.message = Some(format!(
                    "Saved review.md, review.html and review.json to {directory} · {scope}"
                ));
            }
            (
                Change::Work(Some(anchor)),
                Ok(WorkspaceResponse::Work(WorkResult::Prompts(entries))),
            ) => {
                // Compatibility metadata-only result: no availability or page
                // proof was supplied, so do not offer a captured-text read.
                let scope =
                    prompt::PromptScope::session(&anchor, prompt::PromptCapture::MetadataOnly);
                let mut nav = prompt::PromptNavigator::new(scope);
                let token = nav.begin_load();
                let requests = entries
                    .iter()
                    .map(prompt::PromptRequest::from)
                    .collect::<Vec<_>>();
                nav.accept_page(token, requests, prompt::PromptPageMeta::default());
                nav.unavailable(
                    "This reader did not supply prompt capability or retained-page facts",
                );
                self.prompt_nav = Some(nav);
                self.prompt_read_epoch = None;
                self.work = None;
                self.message = None;
            }
            (Change::Work(_), Ok(WorkspaceResponse::Work(WorkResult::PromptHistory(page)))) => {
                let capture = if page.availability == crate::workbench::PromptAvailability::Granted
                {
                    prompt::PromptCapture::Enabled
                } else {
                    prompt::PromptCapture::MetadataOnly
                };
                let mut nav = prompt::PromptNavigator::new(prompt::PromptScope::session(
                    &page.anchor,
                    capture,
                ));
                let token = nav.begin_load();
                self.prompt_nav = Some(nav);
                self.prompt_cursor = None;
                self.prompt_retry_cursor = None;
                self.accept_prompt_page(token, page);
                self.work = None;
            }
            (
                Change::PromptPage(token),
                Ok(WorkspaceResponse::Work(WorkResult::PromptHistory(page))),
            ) => {
                self.accept_prompt_page(token, page);
            }
            (Change::Work(target), Ok(WorkspaceResponse::Work(data))) => {
                if let Some(work) = &mut self.work {
                    work.apply(data);
                } else {
                    self.work = Some(Workbench::new(data, target));
                }
                if std::mem::take(&mut self.pending_task_search)
                    && let Some(work) = &mut self.work
                {
                    work.start_task_search();
                }
            }
            _ => self.message = Some("History reader returned an unexpected response".into()),
        }
    }

    fn history_request(
        &self,
        selection: HistorySelection,
        before: Option<ObservationPosition>,
    ) -> WorkspaceRequest {
        WorkspaceRequest::History {
            selection,
            before,
            include_reviews: self.include_reviews,
        }
    }

    fn toggle_review_visibility(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        self.request(
            sender,
            WorkspaceRequest::ReviewVisibility(!self.include_reviews),
            Change::ReviewVisibility,
        );
    }

    fn open(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, selection: HistorySelection) {
        if self.prompt_nav.is_some() {
            self.prompt_nav = None;
            self.prompt_read_epoch = None;
            self.prompt_read_key = None;
        }
        if let HistorySelection::Session(_, view) = &selection
            && inspector::Tab::from_view(*view).is_some()
        {
            // A fresh inspector starts on its tab strip; the content reader is
            // one Enter away. Section switches preserve the current focus.
            self.inspector = inspector::State::focused(*view);
        }
        self.request(
            sender,
            self.history_request(selection.clone(), None),
            Change::Open(selection),
        );
    }

    fn navigate(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, destination: Destination) {
        self.prompt_nav = None;
        self.prompt_read_epoch = None;
        self.prompt_read_key = None;
        let view = match destination {
            Destination::Agents => {
                self.history.clear();
                self.inspector = inspector::State::default();
                self.generation = self.generation.wrapping_add(1);
                self.message = None;
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
            Destination::Usage => WorkspaceView::Usage,
        };
        let selection = HistorySelection::All(view);
        self.request(
            sender,
            self.history_request(selection.clone(), None),
            Change::Navigate(selection),
        );
    }

    fn select_session_mode(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        session: RecordedSession,
        mode: SessionView,
    ) {
        if let Some(tab) = inspector::Tab::from_view(mode) {
            self.inspector.tab = tab;
        }
        let selection = HistorySelection::Session(session, mode);
        self.request(
            sender,
            self.history_request(selection.clone(), None),
            Change::SessionMode(selection),
        );
    }

    fn inspector_session(&self) -> Option<(RecordedSession, SessionView)> {
        match self.history.last().map(|view| &view.selection) {
            Some(HistorySelection::Session(row, view))
                if matches!(view, SessionView::Overview | SessionView::Details) =>
            {
                Some((row.clone(), *view))
            }
            _ => None,
        }
    }

    /// Enters the prompt navigator for the exact inspector session. This is the
    /// hook UI-04 owns: the shell supplies the exact anchor and does not read
    /// captured text itself.
    fn open_inspector_prompts(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        if self.pending.is_some() {
            return;
        }
        let Some((session, _)) = self.inspector_session() else {
            return;
        };
        let anchor = WorkAnchor::from(&session);
        self.inspector.tab = inspector::Tab::Prompts;
        self.request(
            sender,
            WorkspaceRequest::Work(WorkCommand::Prompts(anchor.clone())),
            Change::Work(Some(anchor)),
        );
    }

    fn enter_inspector(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        match self.inspector.tab {
            inspector::Tab::Prompts => self.open_inspector_prompts(sender),
            _ => self.inspector.tabs_focused = false,
        }
    }

    fn move_inspector_tab(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        forward: bool,
        focus_tabs: bool,
    ) {
        let Some((session, current)) = self.inspector_session() else {
            return;
        };
        if self.pending.is_some() {
            return;
        }
        let next = if forward {
            self.inspector.tab.next()
        } else {
            self.inspector.tab.previous()
        };
        self.inspector.tab = next;
        self.inspector.tabs_focused = focus_tabs;
        self.message = None;
        match next.view() {
            Some(view) if view != current => {
                self.select_session_mode(sender, session, view);
            }
            Some(_) => {}
            None => {
                self.message =
                    Some("Prompts · Enter or Down opens the recorded request navigator".into());
            }
        }
    }

    fn focus_inspector_tabs(&mut self) {
        if self.inspector_session().is_none() {
            return;
        }
        self.inspector.tabs_focused = true;
        self.message = Some("Inspector sections · ←/→ choose · Enter read".into());
    }

    fn open_inspector_activity(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        let Some((session, _)) = self.inspector_session() else {
            return;
        };
        self.open(
            sender,
            HistorySelection::Session(session, SessionView::Activity),
        );
    }

    fn open_inspector_usage(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        let Some((session, _)) = self.inspector_session() else {
            return;
        };
        self.open(
            sender,
            HistorySelection::Session(session, SessionView::Usage),
        );
    }

    fn open_inspector_metrics(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        let Some((session, _)) = self.inspector_session() else {
            return;
        };
        self.open(
            sender,
            HistorySelection::Session(session, SessionView::Metrics),
        );
    }

    /// Inspector key handling. The tab strip is one focus stop; content owns
    /// scrolling. Ctrl+Left/Right and `m` switch sections from content, and
    /// plain Left/Right never steals a reader's horizontal keys.
    fn inspector_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        key: event::KeyEvent,
    ) -> bool {
        if self.inspector_session().is_none() {
            return false;
        }
        let control = key.modifiers == KeyModifiers::CONTROL;
        let code = keymap::plain(key).unwrap_or(KeyCode::Null);
        if code == KeyCode::Char('K') {
            self.open_inspector_metrics(sender);
            return true;
        }
        if control && matches!(key.code, KeyCode::Left | KeyCode::Right) {
            self.move_inspector_tab(
                sender,
                key.code == KeyCode::Right,
                self.inspector.tabs_focused,
            );
            return true;
        }
        if self.inspector.tabs_focused {
            match code {
                KeyCode::Left => {
                    self.move_inspector_tab(sender, false, true);
                    return true;
                }
                KeyCode::Right => {
                    self.move_inspector_tab(sender, true, true);
                    return true;
                }
                KeyCode::Enter | KeyCode::Down => {
                    self.enter_inspector(sender);
                    return true;
                }
                KeyCode::Up if self.inspector.tab == inspector::Tab::Prompts => {
                    self.enter_inspector(sender);
                    return true;
                }
                KeyCode::Char('m') => {
                    self.move_inspector_tab(sender, true, true);
                    return true;
                }
                _ => {}
            }
        } else {
            match code {
                KeyCode::Char('m') => {
                    let forward = true;
                    self.move_inspector_tab(sender, forward, false);
                    return true;
                }
                KeyCode::Char('s' | 'A') if self.focus() != keymap::Focus::HistorySearch => {
                    self.open_inspector_activity(sender);
                    return true;
                }
                KeyCode::Char('u') => {
                    self.open_inspector_usage(sender);
                    return true;
                }
                _ => {}
            }
        }
        false
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

    /// Routes one key to the integrated prompt navigator. Reads reuse the
    /// existing grant-checked `RevealPrompt` path; the navigator only stores
    /// identifiers and accepts a delivery for the exact selected request.
    fn prompt_nav_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        key: event::KeyEvent,
    ) -> bool {
        let code = keymap::plain(key).unwrap_or(KeyCode::Null);
        // The navigator may drop/replace its body on these keys. Extract its
        // redaction acknowledgement BEFORE it mutates, and send only after the
        // next flushed public frame.
        let main_navigation = code == KeyCode::Char('m')
            && self.prompt_nav.as_ref().is_some_and(|nav| {
                nav.scope().worker.is_some() && nav.scope().main_session.is_some()
            });
        if matches!(
            code,
            KeyCode::Left | KeyCode::Esc | KeyCode::Char('[' | ']' | 'l')
        ) || main_navigation
        {
            self.redact_prompt_nav(false);
        }
        let Some(nav) = &mut self.prompt_nav else {
            return false;
        };
        match nav.key(code) {
            prompt::PromptInput::Ignored | prompt::PromptInput::Handled => true,
            prompt::PromptInput::NoRequests => {
                self.message = Some(nav.status_line());
                true
            }
            prompt::PromptInput::ReaderClosed => {
                self.deferred_prompt_read = None;
                self.prompt_read_epoch = None;
                self.prompt_read_key = None;
                self.prompt_receiver = None;
                true
            }
            prompt::PromptInput::Back => {
                self.deferred_prompt_read = None;
                self.prompt_nav = None;
                self.prompt_read_epoch = None;
                self.prompt_read_key = None;
                self.prompt_receiver = None;
                self.generation = self.generation.wrapping_add(1);
                self.message = Some("Prompts section · Esc returns to the roster".into());
                true
            }
            prompt::PromptInput::LoadEarlier => {
                self.load_prompt_page(sender, true);
                true
            }
            prompt::PromptInput::Retry => {
                self.load_prompt_page(sender, false);
                true
            }
            prompt::PromptInput::MainSession(anchor) => {
                let exists = self.agents.catalog.as_ref().is_some_and(|catalog| {
                    catalog.sessions.iter().any(|row| {
                        row.harness_id == anchor.source && row.session_id == anchor.session
                    })
                });
                if !exists {
                    if let Some(nav) = &mut self.prompt_nav {
                        nav.unavailable(
                            "The verified main session is no longer in this roster page",
                        );
                    }
                    return true;
                }
                self.prompt_nav = None;
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::Prompts(anchor.clone())),
                    Change::Work(Some(anchor)),
                );
                true
            }
            prompt::PromptInput::Read {
                entry,
                original,
                epoch,
            } => {
                if self.pending.is_some() {
                    self.deferred_prompt_read = Some((entry, epoch));
                    return true;
                }
                let key = (entry.event_id.clone(), entry.revision);
                let (delivery, receiver) = prompt_channel(PromptKind::User);
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::RevealPrompt(Box::new(entry), delivery)),
                    Change::Work(None),
                );
                self.prompt_receiver = Some(receiver);
                self.prompt_read_epoch = Some(epoch);
                self.prompt_read_key = Some(key);
                let _ = original;
                true
            }
        }
    }

    fn accept_prompt_page(&mut self, token: u64, page: crate::workbench::PromptHistoryPage) {
        use crate::workbench::PromptAvailability;
        let Some(nav) = &mut self.prompt_nav else {
            return;
        };
        let scope = nav.scope();
        if page.anchor.source != scope.source
            || page.anchor.session != scope.session
            || page.anchor.worker != scope.worker
            || page.anchor.turn != scope.turn
        {
            nav.out_of_scope("The prompt page returned a different scope");
            return;
        }
        match page.availability {
            PromptAvailability::Granted => {
                nav.set_capture(prompt::PromptCapture::Enabled);
                let accepted = nav.accept_page(
                    token,
                    page.entries
                        .iter()
                        .map(prompt::PromptRequest::from)
                        .collect(),
                    prompt::PromptPageMeta {
                        retained_cap: Some(page.retained_cap),
                        earlier_available: page.older.is_some(),
                        limited: page.limited,
                    },
                );
                if accepted {
                    self.prompt_cursor = page.older;
                }
            }
            PromptAvailability::MissingIdentity => nav.missing_identity(),
            PromptAvailability::Denied(reason) => nav.deny(reason),
            PromptAvailability::Unavailable(reason) => nav.unavailable(reason),
            PromptAvailability::Stale(reason) => nav.stale(reason),
        }
        self.message = None;
    }

    fn start_deferred_prompt_read(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        let Some((entry, epoch)) = self.deferred_prompt_read.take() else {
            return;
        };
        let key = (entry.event_id.clone(), entry.revision);
        if !self
            .prompt_nav
            .as_ref()
            .is_some_and(|nav| nav.pending_identity(epoch, &key))
        {
            return;
        }
        let (delivery, receiver) = prompt_channel(PromptKind::User);
        self.request(
            sender,
            WorkspaceRequest::Work(WorkCommand::RevealPrompt(Box::new(entry), delivery)),
            Change::Work(None),
        );
        self.prompt_receiver = Some(receiver);
        self.prompt_read_epoch = Some(epoch);
        self.prompt_read_key = Some(key);
    }

    fn load_prompt_page(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, older: bool) {
        if self.pending.is_some() {
            return;
        }
        let Some(nav) = &mut self.prompt_nav else {
            return;
        };
        let before = if older {
            self.prompt_cursor.clone()
        } else {
            self.prompt_retry_cursor.clone()
        };
        let token = if before.is_some() {
            let Some(token) = nav.begin_older() else {
                return;
            };
            token
        } else {
            nav.begin_retry()
        };
        let scope = nav.scope();
        let anchor = WorkAnchor {
            source: scope.source.clone(),
            session: scope.session.clone(),
            worker: scope.worker.clone(),
            turn: scope.turn.clone(),
        };
        self.prompt_retry_cursor = before.clone();
        self.request(
            sender,
            WorkspaceRequest::Work(WorkCommand::PromptPage { anchor, before }),
            Change::PromptPage(token),
        );
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

    fn expire_prompt_nav(&mut self, now: Instant) -> bool {
        let expired = self
            .prompt_nav
            .as_ref()
            .and_then(|nav| nav.reader_ref())
            .is_some_and(|reader| reader.expired(now));
        if expired {
            self.redact_prompt_nav(true);
            if let Some(nav) = &mut self.prompt_nav {
                nav.expired_status();
            }
        }
        expired
    }

    fn private_display_gate(&self) -> Option<Arc<Mutex<bool>>> {
        self.prompt
            .as_ref()
            .or_else(|| self.prompt_nav.as_ref().and_then(|nav| nav.reader_ref()))
            .filter(|reader| reader.acknowledge.is_some())
            .map(|reader| Arc::clone(&reader.active))
    }

    fn enforce_private_display_gate(&mut self, authorized: bool) {
        if !authorized {
            self.dismiss_prompt();
            self.redact_prompt_nav(true);
            self.message = Some("Content authorization expired before display".into());
        }
    }

    fn redact_prompt_nav(&mut self, close: bool) -> bool {
        let Some(nav) = &mut self.prompt_nav else {
            return false;
        };
        let reader = if close {
            nav.close_reader()
        } else {
            // Do not invalidate a pending read when no body is present.
            if !nav.reader_open() {
                return false;
            }
            nav.take_reader()
        };
        if let Some(mut reader) = reader {
            self.redact_then_acknowledge = reader.acknowledge.take();
            true
        } else {
            false
        }
    }

    fn acknowledge_redaction(&mut self) {
        if let Some(acknowledge) = self.redact_then_acknowledge.take() {
            let _ = acknowledge.send(());
        }
    }

    fn metadata_filter_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        key: KeyCode,
    ) -> bool {
        if self.prompt.is_some()
            || self.investigation.is_some()
            || self.view_io.form.is_some()
            || self.pending.is_some()
        {
            return false;
        }
        if let Some(work) = &mut self.work {
            return work.host_filter_key(key);
        }
        let Some(view) = self.history.last_mut() else {
            return false;
        };
        if view.page.ui.help
            || view.page.turn_picker.is_some()
            || view.page.operation_picker.is_some()
            || view.page.evaluation_picker.is_some()
        {
            return false;
        }
        if key == KeyCode::Char('F') && view.page.ui.activity_prompts_available() {
            if let Some(enabled) = view.page.ui.toggle_activity_prompts() {
                self.message = Some(if enabled {
                    "Activity filter · user prompts only".into()
                } else {
                    "Activity filter · all recorded events".into()
                });
            }
            return true;
        }
        if let Some(analytics::HistoryAnalytics::Metrics(browser)) = view.active_analytics_mut()
            && (key == KeyCode::Char('/') || browser.editing_filter())
        {
            let input = browser.filter_key(key, false);
            return self.handle_metric_input(sender, input, false);
        }
        if let Some(browser) = view.page.active_findings()
            && (key == KeyCode::Char('/') || browser.editing_filter())
        {
            return browser.key(key);
        }
        if let Some(table) = view.page.operation_table.active(&view.page.ui)
            && (key == KeyCode::Char('/') || table.editing_filter())
        {
            let scope = match &view.selection {
                HistorySelection::All(_) => Some(McpSearchScope::All),
                HistorySelection::Session(row, _) => {
                    Some(McpSearchScope::Session(Box::new(row.clone())))
                }
                HistorySelection::Turn(row) => Some(McpSearchScope::Turn(Box::new(row.clone()))),
                HistorySelection::TurnReview(row) => Some(McpSearchScope::Turn(row.clone())),
                HistorySelection::McpSearch { scope, .. } => Some(scope.clone()),
                HistorySelection::McpTraces { .. } | HistorySelection::ReviewEdits(_) => None,
            };
            let Some(scope) = scope else {
                return table.key(key);
            };
            match table.search_key(key) {
                operation_table::SearchInput::Unhandled => return false,
                operation_table::SearchInput::Handled => return true,
                operation_table::SearchInput::Submit(query) => {
                    let snapshot_id = view.page.snapshot_id;
                    if query.is_empty() {
                        if matches!(view.selection, HistorySelection::McpSearch { .. }) {
                            self.history.pop();
                            self.generation = self.generation.wrapping_add(1);
                            self.message = None;
                        }
                    } else {
                        self.open(
                            sender,
                            HistorySelection::McpSearch {
                                scope,
                                query,
                                snapshot_id,
                            },
                        );
                    }
                    return true;
                }
            }
        }
        if key == KeyCode::Char('/') || view.page.ui.evidence_filter_editing() {
            return view.page.ui.evidence_filter_key(key);
        }
        false
    }

    fn key(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, key: event::KeyEvent) -> bool {
        if keymap::interrupt(key) {
            if self.focus().prompt_reader() || self.help.is_open() || self.palette.is_open() {
                return true;
            }
            // Public evidence focus copies; editors and private content keep
            // the terminal interrupt meaning. Help documents this split.
            if self.focus().copyable()
                && self
                    .view_io_key(sender, plain_key('c'))
                    .is_some_and(|quit| !quit)
            {
                return false;
            }
            return true;
        }
        // Granted and pending inline readers own keys before help, palette,
        // export, filtering and navigation, just like the full-screen reader.
        if self
            .prompt_nav
            .as_ref()
            .is_some_and(|nav| nav.focus() == prompt::PromptFocus::Reader)
        {
            if keymap::plain(key) == Some(KeyCode::Char('q')) {
                return true;
            }
            self.prompt_nav_key(sender, key);
            return false;
        }
        if self.help.is_open() {
            if keymap::plain(key) == Some(KeyCode::Char('q')) && self.prompt.is_none() {
                return true;
            }
            self.help.key(key);
            return false;
        }
        if self.palette.is_open() {
            return match self.palette.key(key) {
                palette::Outcome::Open => false,
                palette::Outcome::Help => {
                    self.open_help();
                    false
                }
                palette::Outcome::Cancel => {
                    self.message = None;
                    false
                }
                palette::Outcome::Run(id) => self.run_action(sender, &id),
            };
        }
        let focus = self.focus();
        let colon_shortcut = key.code == KeyCode::Char(':')
            && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT);
        if (keymap::palette(key) || colon_shortcut) && !focus.text_entry() && !focus.chooser() {
            if self.prompt.is_none() {
                self.open_palette();
            }
            return false;
        }
        if keymap::is_help(key.code) && self.help_available(key) {
            self.open_help();
            return false;
        }
        let code = keymap::plain(key).unwrap_or(KeyCode::Null);
        if self.prompt.is_none() && self.prompt_nav.is_some() {
            if code == KeyCode::Char('q') {
                return true;
            }
            self.prompt_nav_key(sender, key);
            return false;
        }
        if self.prompt.is_none()
            && let Some(Change::FindingSource { cancelled }) = &mut self.pending
        {
            if code == KeyCode::Esc {
                *cancelled = true;
                self.prompt_receiver = None;
                self.message = None;
            }
            return code == KeyCode::Char('q');
        }
        if self.metadata_filter_key(sender, code) {
            return false;
        }
        if let Some(quit) = self.review_io_key(sender, key) {
            return quit;
        }
        if self.investigation.is_none()
            && let Some(quit) = self.view_io_key(sender, key)
        {
            return quit;
        }
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
                InvestigationInput::Reveal {
                    reference,
                    history_identity,
                } => {
                    let (delivery, receiver) = prompt_channel(PromptKind::Investigation);
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::RevealInvestigation {
                            reference,
                            history_identity,
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
            if code == KeyCode::Char('p') && work.workflow_refresh().is_some() && !work.editing() {
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
                    self.generation = self.generation.wrapping_add(1);
                    self.select_session_mode(sender, session, SessionView::Activity);
                }
                return false;
            }
            if code == KeyCode::Null && !work.editing() {
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
                WorkInput::Copy(text) => match crate::loaded_view::clipboard_request(&text) {
                    Ok(request) => {
                        self.view_io.clipboard = Some(request);
                        self.message = Some(
                            "Clipboard request sent · terminal permission/support required".into(),
                        );
                    }
                    Err(error) => self.message = Some(error),
                },
                WorkInput::None => {}
                WorkInput::Back => {
                    self.discard_work_result = self.pending.as_ref().is_some_and(Change::is_work);
                    self.message = None;
                }
                WorkInput::Close => {
                    self.discard_work_result = self.pending.as_ref().is_some_and(Change::is_work);
                    self.prompt_receiver = None;
                    self.queued_prompt = None;
                    self.work = None;
                    self.generation = self.generation.wrapping_add(1);
                    self.message = None;
                }
                WorkInput::Command(WorkCommand::WorkflowResume(item)) => {
                    let expected = Box::new(item.clone());
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::WorkflowResume(item)),
                        Change::WorkflowResume(expected),
                    );
                }
                WorkInput::Command(WorkCommand::RetryWorkflow {
                    authority,
                    key,
                    expected_revision,
                    expected_attempt,
                }) => {
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::RetryWorkflow {
                            authority: authority.clone(),
                            key,
                            expected_revision,
                            expected_attempt,
                        }),
                        Change::WorkflowRetry {
                            authority,
                            key,
                            revision: expected_revision,
                            attempt: expected_attempt,
                        },
                    );
                }
                WorkInput::Command(command) => {
                    self.request(sender, WorkspaceRequest::Work(command), Change::Work(None))
                }
            }
            return false;
        }
        if self.prompt_nav.is_some() {
            let _ = self.prompt_nav_key(sender, key);
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
        if let Some(picker) = &mut view.page.trace_picker {
            match code {
                KeyCode::Char('q') => return true,
                KeyCode::Esc => view.page.trace_picker = None,
                KeyCode::Down | KeyCode::Char('j') => {
                    move_selection(&mut picker.selection, picker.targets.len(), true);
                    self.message = None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    move_selection(&mut picker.selection, picker.targets.len(), false);
                    self.message = None;
                }
                KeyCode::Enter => {
                    let target = picker
                        .selection
                        .selected()
                        .and_then(|index| picker.targets.get(index))
                        .cloned();
                    let origin = picker.origin.clone();
                    view.page.trace_picker = None;
                    let result = target.map_or(Err("No linked span is selected"), |target| {
                        view.page.jump_to_trace_operation(&target, origin)
                    });
                    match result {
                        Ok(()) => self.message = None,
                        Err(message) => self.message = Some(message.into()),
                    }
                }
                _ => {}
            }
            return false;
        }
        if let Some((kind, picker)) = &mut view.page.operation_picker {
            let review = match &view.selection {
                HistorySelection::ReviewEdits(session) => Some(session),
                _ => None,
            };
            let kind = *kind;
            let entries = view
                .page
                .operation_details
                .iter()
                .filter(|entry| entry.kind == kind);
            match code {
                KeyCode::Char('q') => return true,
                KeyCode::Esc if review.is_some() => {
                    if self.pending.is_none() {
                        self.history.pop();
                        self.generation = self.generation.wrapping_add(1);
                    } else if self.pending.as_ref().is_some_and(Change::is_work) {
                        self.discard_work_result = true;
                        self.prompt_receiver = None;
                    }
                    self.message = None;
                }
                keymap::OLDER if review.is_some() && self.pending.is_none() => {
                    if let Some(before) = view.page.ui.older_cursor().cloned() {
                        let selection = view.selection.clone();
                        self.request(
                            sender,
                            self.history_request(selection, Some(before)),
                            Change::OlderHistory,
                        );
                    }
                }
                keymap::NEWER if review.is_some() && self.pending.is_none() => {
                    if let Some(newer) = view.newer.pop_back() {
                        view.page = newer;
                        self.message = None;
                    }
                }
                keymap::LATEST if review.is_some() && self.pending.is_none() => {
                    let selection = view.selection.clone();
                    self.request(
                        sender,
                        self.history_request(selection, None),
                        Change::LatestHistory,
                    );
                }
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
                        let command = if let Some(session) = review {
                            if !view.page.review_edit_preview {
                                self.message =
                                    Some("Edit preview is unavailable from this reader".into());
                                return false;
                            }
                            let Some(identity) =
                                view.page.history_identity.as_ref().filter(|identity| {
                                    session.history_identity.as_ref() == Some(*identity)
                                })
                            else {
                                self.message =
                                    Some("Review history changed. Reopen Review.".into());
                                return false;
                            };
                            if view.page.snapshot_id != session.snapshot_id
                                || operation.source_id != session.harness_id
                                || operation.session_id != session.session_id
                            {
                                self.message =
                                    Some("Edit does not match this Review selection".into());
                                return false;
                            }
                            WorkCommand::RevealReviewEdit(
                                Box::new(ReviewEditSelection {
                                    operation,
                                    snapshot_id: view.page.snapshot_id,
                                    history_identity: identity.clone(),
                                }),
                                delivery,
                            )
                        } else {
                            WorkCommand::RevealOperation(operation, delivery)
                        };
                        self.request(sender, WorkspaceRequest::Work(command), Change::Work(None));
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

        if self.review_controls_focused() {
            let focused = self
                .history
                .last()
                .is_some_and(|view| view.page.review_control.is_some());
            match code {
                KeyCode::Tab => {
                    self.move_review_control(true);
                    return false;
                }
                KeyCode::BackTab => {
                    self.move_review_control(false);
                    return false;
                }
                KeyCode::Enter | KeyCode::Char(' ') if focused => {
                    return self.activate_review_control(sender);
                }
                KeyCode::Esc if focused => {
                    if let Some(view) = self.history.last_mut() {
                        view.page.review_control = None;
                    }
                    self.message = Some("Review · Esc again returns one layer".into());
                    return false;
                }
                _ => {}
            }
        }

        if self.inspector_key(sender, key) {
            return false;
        }

        if self.analytics_key(sender, key, code) {
            return false;
        }
        let view = self.history.last_mut().expect("history exists");
        if keymap::back(key) {
            self.history.pop();
            self.generation = self.generation.wrapping_add(1);
            self.message = None;
            return false;
        }
        if keymap::review(key) {
            match &view.selection {
                HistorySelection::Session(session, _) => {
                    let selection = HistorySelection::Session(session.clone(), SessionView::Review);
                    self.open(sender, selection);
                }
                HistorySelection::Turn(turn) => {
                    let selection = HistorySelection::TurnReview(Box::new(turn.clone()));
                    self.open(sender, selection);
                }
                _ => {}
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

        if code == KeyCode::Char('s')
            && !view.page.ui.help
            && let Some(occurrence) = view
                .page
                .active_findings()
                .and_then(|browser| browser.inspected_occurrence())
        {
            if self.pending.is_none() && self.prompt_receiver.is_none() {
                let evidence_snapshot = occurrence.position.snapshot_id;
                let source = occurrence.inspected_source();
                let request = match (source, view.page.history_identity.clone()) {
                    (Some(crate::FindingSource::Command(reference)), Some(history_identity)) => {
                        let (delivery, receiver) = prompt_channel(PromptKind::Investigation);
                        Some((
                            WorkCommand::RevealInvestigation {
                                reference,
                                history_identity: Some(history_identity),
                                delivery,
                            },
                            receiver,
                        ))
                    }
                    (Some(crate::FindingSource::Telemetry(span)), Some(history_identity))
                        if span.history_identity == history_identity =>
                    {
                        let (delivery, receiver) = prompt_channel(PromptKind::Telemetry);
                        Some((WorkCommand::RevealTelemetry(span, delivery), receiver))
                    }
                    (Some(crate::FindingSource::Log(log)), Some(history_identity))
                        if log.history_identity == history_identity =>
                    {
                        let (delivery, receiver) = prompt_channel(PromptKind::TelemetryLog);
                        Some((WorkCommand::RevealTelemetryLog(log, delivery), receiver))
                    }
                    (Some(crate::FindingSource::Evaluation(detail)), Some(history_identity)) => {
                        let (delivery, receiver) = prompt_channel(PromptKind::Evaluation);
                        Some((
                            WorkCommand::RevealFailedEvalFinding {
                                detail,
                                history_identity,
                                evidence_snapshot,
                                delivery,
                            },
                            receiver,
                        ))
                    }
                    _ => None,
                };
                if let Some((command, receiver)) = request {
                    self.request(
                        sender,
                        WorkspaceRequest::Work(command),
                        Change::FindingSource { cancelled: false },
                    );
                    if self.pending.is_some() {
                        self.prompt_receiver = Some(receiver);
                    }
                } else {
                    self.message = Some("Protected source unavailable for this finding".into());
                }
            }
            return false;
        }

        if !view.page.ui.help
            && view
                .page
                .active_findings()
                .is_some_and(|browser| browser.key(code))
        {
            return false;
        }
        if view.page.ui.evidence_key(code) {
            return false;
        }
        if code == KeyCode::Char('o') && view.page.ui.selected_title() == Some("Traces") {
            let origin = view
                .page
                .ui
                .selected_evidence_export()
                .map(|(title, _, position, _)| (title.to_owned(), position));
            let targets = view
                .page
                .ui
                .selected_trace_operation_targets()
                .map(|targets| targets.to_vec())
                .unwrap_or_default();
            match (targets.as_slice(), origin) {
                ([], _) => {
                    self.message = Some("The selected trace has no exact linked operation".into());
                }
                ([target], Some(origin)) => {
                    match view.page.jump_to_trace_operation(target, origin) {
                        Ok(()) => self.message = None,
                        Err(message) => self.message = Some(message.into()),
                    }
                }
                ([_, _, ..], Some(origin)) => {
                    view.page.trace_picker = Some(TraceOperationPicker {
                        targets,
                        selection: ListState::default().with_selected(Some(0)),
                        origin,
                    });
                    self.message = None;
                }
                (_, None) => {
                    self.message = Some("The selected trace row is unavailable".into());
                }
            }
            return false;
        }
        if code == KeyCode::Esc
            && view.page.trace_return.is_some()
            && view
                .page
                .operation_table
                .active_ref(&view.page.ui)
                .is_some_and(|table| !table.expanded())
        {
            if let Some((title, position)) = view.page.trace_return.take() {
                view.page.ui.restore_view(Some(&title), position);
            }
            return false;
        }
        if let Some(table) = view.page.operation_table.active(&view.page.ui) {
            if code == KeyCode::Char('t')
                && table
                    .selected_row()
                    .is_some_and(|row| row.mcp_event.is_some())
            {
                if self.pending.is_none() {
                    if let Some((source, event_id, revision)) =
                        table.selected_row().and_then(|row| row.mcp_event.as_ref())
                        && let Ok(source_id) = ExtensionId::parse(source.clone())
                    {
                        let selection = HistorySelection::McpTraces {
                            source_id,
                            event_id: event_id.clone(),
                            revision: *revision,
                            snapshot_id: view.page.snapshot_id,
                        };
                        self.open(sender, selection);
                    } else {
                        self.message = Some("Select a retained MCP call to open its traces".into());
                    }
                }
                return false;
            }
            if table.key(code) {
                return false;
            }
        }
        if code == KeyCode::Char('v') && view.active_analytics().is_none() {
            self.toggle_review_visibility(sender);
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
        if code == KeyCode::Char('e')
            && !view.page.ui.help
            && view.page.reviewing()
            && let HistorySelection::Session(session, _) = &view.selection
        {
            let selection = HistorySelection::ReviewEdits(Box::new(session.clone()));
            self.open(sender, selection);
            return false;
        }
        let detail = match code {
            KeyCode::Char('M') => Some(OperationDetailKind::Mcp),
            KeyCode::Char('m') if mcp_pane => Some(OperationDetailKind::Mcp),
            KeyCode::Char('e') if view.page.ui.in_turn => Some(OperationDetailKind::Edit),
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
            KeyCode::Esc => {
                self.history.pop();
                self.generation = self.generation.wrapping_add(1);
                self.message = None;
            }
            KeyCode::Char('p') => self.toggle_pause(),
            KeyCode::Char('b' | 'P') => {
                if code == KeyCode::Char('P') {
                    self.inspector.tab = inspector::Tab::Prompts;
                }
                let target = match &view.selection {
                    HistorySelection::Session(session, _) => Some(WorkAnchor::from(session)),
                    HistorySelection::Turn(turn) => Some(WorkAnchor {
                        source: turn.harness_id.clone(),
                        session: turn.session_id.clone(),
                        worker: turn.worker_id.clone(),
                        turn: Some(turn.turn_id.clone()),
                    }),
                    HistorySelection::TurnReview(turn) => Some(WorkAnchor {
                        source: turn.harness_id.clone(),
                        session: turn.session_id.clone(),
                        worker: turn.worker_id.clone(),
                        turn: Some(turn.turn_id.clone()),
                    }),
                    HistorySelection::McpSearch { scope, .. } => scope.anchor(),
                    HistorySelection::All(_)
                    | HistorySelection::McpTraces { .. }
                    | HistorySelection::ReviewEdits(_) => None,
                };
                let command = if code == KeyCode::Char('b') {
                    Some(WorkCommand::Tasks(None))
                } else {
                    target.clone().map(WorkCommand::Prompts)
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
                        self.history_request(selection, Some(before)),
                        Change::OlderHistory,
                    );
                }
            }
            keymap::NEWER if self.pending.is_none() && !view.page.ui.help => {
                if let Some(mut newer) = view.newer.pop_back() {
                    newer.inherit_filters(&view.page);
                    view.page = newer;
                }
            }
            keymap::LATEST | KeyCode::Char('R') => {
                let selection = view.selection.clone();
                self.request(
                    sender,
                    self.history_request(selection, None),
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
        let (owner, identity, snapshot) = match self.history.last() {
            Some(view) => {
                if view.page.ui.help
                    || view.page.turn_picker.is_some()
                    || view.page.operation_picker.is_some()
                    || view.page.evaluation_picker.is_some()
                {
                    return;
                }
                let owner = match &view.selection {
                    HistorySelection::McpSearch { scope, .. } => scope.anchor(),
                    HistorySelection::All(_)
                    | HistorySelection::McpTraces { .. }
                    | HistorySelection::ReviewEdits(_) => None,
                    HistorySelection::Session(row, _) => Some(WorkAnchor::from(row)),
                    HistorySelection::Turn(row) => Some(WorkAnchor {
                        source: row.harness_id.clone(),
                        session: row.session_id.clone(),
                        worker: row.worker_id.clone(),
                        turn: Some(row.turn_id.clone()),
                    }),
                    HistorySelection::TurnReview(row) => Some(WorkAnchor {
                        source: row.harness_id.clone(),
                        session: row.session_id.clone(),
                        worker: row.worker_id.clone(),
                        turn: Some(row.turn_id.clone()),
                    }),
                };
                (
                    owner,
                    view.page.history_identity.clone(),
                    Some(view.page.snapshot_id),
                )
            }
            None => {
                if self.agents.help || self.agents.editing {
                    return;
                }
                let row = self.agents.current();
                (
                    row.map(WorkAnchor::from),
                    row.and_then(|row| row.history_identity.clone()),
                    row.map(|row| row.snapshot_id),
                )
            }
        };
        self.investigation = Some(Investigation::new(owner, identity, snapshot));
        self.message = None;
    }

    /// Opens the investigation overlay on a specific retained query kind. The
    /// kind order belongs to UI-04; the shell only selects the documented row.
    fn open_investigation_kind(&mut self, tabs: usize) {
        self.open_investigation();
        if let Some(investigation) = &mut self.investigation {
            for _ in 0..tabs {
                let _ = investigation.key(
                    event::KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                    false,
                );
            }
        }
    }

    fn context(&self) -> keymap::Context {
        if self.work.is_some() {
            keymap::Context::Workbench
        } else if self.investigation.is_some() {
            keymap::Context::Investigation
        } else if self.view_io.form.is_some() {
            keymap::Context::Form
        } else if !self.history.is_empty() {
            keymap::Context::History
        } else {
            keymap::Context::CommandCenter
        }
    }

    fn focus(&mut self) -> keymap::Focus {
        if self.help.is_open() {
            return keymap::Focus::Help;
        }
        if self.palette.is_open() {
            return if self.palette.kind() == Some(palette::Kind::Sessions) {
                keymap::Focus::SessionChooser
            } else {
                keymap::Focus::Palette
            };
        }
        if self.prompt.is_some() {
            return keymap::Focus::PrivatePrompt;
        }
        if let Some(nav) = &self.prompt_nav {
            return match nav.focus() {
                prompt::PromptFocus::Reader => keymap::Focus::PromptReader,
                prompt::PromptFocus::Chooser => keymap::Focus::PromptChooser,
                prompt::PromptFocus::Navigator => keymap::Focus::PromptNavigator,
            };
        }
        if self.view_io.form.is_some() || self.view_io.review_form.is_some() {
            return keymap::Focus::ExportForm;
        }
        if let Some(work) = &self.work {
            return if work.editing() {
                keymap::Focus::WorkbenchEditor
            } else {
                keymap::Focus::Workbench
            };
        }
        if self.investigation.is_some() {
            return keymap::Focus::Investigation;
        }
        if self.agents.editing {
            return keymap::Focus::AgentSearch;
        }
        if let Some(view) = self.history.last_mut() {
            if view.page.ui.help {
                return keymap::Focus::Help;
            }
            if view.page.turn_picker.is_some()
                || view.page.operation_picker.is_some()
                || view.page.evaluation_picker.is_some()
            {
                return keymap::Focus::HistoryChooser;
            }
            if view.page.ui.evidence_inspection.is_some() {
                return keymap::Focus::EvidenceInspection;
            }
            if view
                .page
                .operation_table
                .active(&view.page.ui)
                .is_some_and(|table| table.editing_filter())
                || view
                    .page
                    .active_findings()
                    .is_some_and(|browser| browser.editing_filter())
                || matches!(
                    view.active_analytics_mut(),
                    Some(analytics::HistoryAnalytics::Metrics(browser)) if browser.editing_filter()
                )
                || view.page.ui.evidence_filter_editing()
            {
                return keymap::Focus::HistorySearch;
            }
            return keymap::Focus::HistoryTable;
        }
        keymap::Focus::CommandCenter
    }

    fn selected_session(&self) -> Option<&RecordedSession> {
        match self.history.last().map(|view| &view.selection) {
            Some(HistorySelection::Session(row, _)) => Some(row),
            Some(HistorySelection::ReviewEdits(row)) => Some(row.as_ref()),
            Some(_) => None,
            None => self.agents.current(),
        }
    }

    fn current_anchor(&self) -> Option<WorkAnchor> {
        match self.history.last().map(|view| &view.selection) {
            Some(HistorySelection::Session(row, _)) => Some(WorkAnchor::from(row)),
            Some(HistorySelection::Turn(row)) => Some(WorkAnchor {
                source: row.harness_id.clone(),
                session: row.session_id.clone(),
                worker: row.worker_id.clone(),
                turn: Some(row.turn_id.clone()),
            }),
            Some(HistorySelection::TurnReview(row)) => Some(WorkAnchor {
                source: row.harness_id.clone(),
                session: row.session_id.clone(),
                worker: row.worker_id.clone(),
                turn: Some(row.turn_id.clone()),
            }),
            Some(HistorySelection::McpSearch { scope, .. }) => scope.anchor(),
            Some(
                HistorySelection::All(_)
                | HistorySelection::McpTraces { .. }
                | HistorySelection::ReviewEdits(_),
            ) => None,
            None => self.agents.current().map(WorkAnchor::from),
        }
    }

    /// Exact Review scope for copy/export/save. The shell never builds a
    /// provider identity; the anchor comes from the selected recorded row.
    fn review_anchor(&self) -> Option<WorkAnchor> {
        match self.history.last().map(|view| &view.selection) {
            Some(HistorySelection::Session(row, SessionView::Review)) => {
                Some(WorkAnchor::from(row))
            }
            Some(HistorySelection::TurnReview(turn)) => Some(WorkAnchor {
                source: turn.harness_id.clone(),
                session: turn.session_id.clone(),
                worker: turn.worker_id.clone(),
                turn: Some(turn.turn_id.clone()),
            }),
            _ => None,
        }
    }

    fn pinned_review_query(&self) -> Option<crate::workbench::PinnedReviewQuery> {
        let view = self.history.last()?;
        let anchor = self.review_anchor()?;
        Some(crate::workbench::PinnedReviewQuery {
            all_workers: matches!(
                view.selection,
                HistorySelection::Session(_, SessionView::Review)
            ),
            anchor,
            snapshot_id: view.page.snapshot_id,
            history_identity: view.page.history_identity.clone()?,
            version: match &view.selection {
                HistorySelection::Session(row, _) => Some(row.review_version),
                _ => None,
            },
            before: view.page.review_before.clone(),
            include_reviews: self.include_reviews,
            events: view.page.review_events.clone(),
        })
    }

    fn review_controls_focused(&self) -> bool {
        self.history.last().is_some_and(|view| {
            view.page.reviewing() || matches!(view.selection, HistorySelection::TurnReview(_))
        })
    }

    fn review_control_count(&self) -> usize {
        REVIEW_CONTROLS.len()
    }

    fn move_review_control(&mut self, forward: bool) {
        let count = self.review_control_count();
        if count == 0 {
            return;
        }
        let Some(view) = self.history.last_mut() else {
            return;
        };
        let next = match view.page.review_control {
            None if forward => 0,
            None => count - 1,
            Some(current) if forward => (current + 1).min(count - 1),
            Some(current) => current.saturating_sub(1),
        };
        view.page.review_control = Some(next);
        self.message = Some(format!(
            "Review controls · {} · Enter or Space activates",
            REVIEW_CONTROLS[next].0
        ));
    }

    fn activate_review_control(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) -> bool {
        let Some(view) = self.history.last() else {
            return false;
        };
        let Some(index) = view.page.review_control else {
            return false;
        };
        let key = REVIEW_CONTROLS.get(index).map(|(_, key)| *key);
        let depth = self.history.len();
        self.history
            .last_mut()
            .expect("history exists")
            .page
            .review_control = None;
        let quit = match key {
            Some(']') => self.send_key(sender, plain_code(keymap::OLDER)),
            Some('\u{0}') => self.send_key(sender, plain_code(KeyCode::Esc)),
            Some(key) => self.send_key(sender, plain_key(key)),
            None => false,
        };
        // Activating a button does not move focus into the evidence body.
        // Keep the originating control available after a read or nested form.
        if key != Some('\u{0}')
            && self.history.len() == depth
            && let Some(view) = self.history.last_mut()
        {
            view.page.review_control = Some(index);
        }
        quit
    }

    fn help_available(&mut self, key: event::KeyEvent) -> bool {
        if self.prompt.is_some() {
            return false;
        }
        if self.prompt_nav.is_some() && self.focus() == keymap::Focus::PromptReader {
            return false;
        }
        if key.code == KeyCode::F(1) {
            return true;
        }
        if key.code == KeyCode::Char('?') {
            let focus = self.focus();
            return !focus.text_entry() && !focus.chooser();
        }
        false
    }

    fn open_help(&mut self) {
        let context = self.context();
        let title = if self.palette.is_open() {
            match self.palette.kind() {
                Some(palette::Kind::Sessions) => "Choose agent session",
                _ => "Command palette",
            }
        } else {
            match context {
                keymap::Context::CommandCenter => "Command center",
                keymap::Context::History => "Session history and inspector",
                keymap::Context::Workbench => "Workbench",
                keymap::Context::Investigation => "Retained history investigation",
                keymap::Context::Form => "Export form",
            }
        };
        let mut sections = keymap::help_sections(context);
        let mut current = Vec::new();
        if let Some(help) = self
            .history
            .last()
            .and_then(|view| view.active_analytics())
            .map(|analytics| analytics.help())
        {
            current.extend(help.lines().map(str::to_owned));
        }
        if let Some(view) = self.history.last() {
            if let HistorySelection::Session(session, mode) = &view.selection {
                current.push(format!(
                    "Session {} / {} · {:?}",
                    visible_text(&session.harness_id),
                    visible_text(&session.session_id),
                    mode
                ));
                if matches!(mode, SessionView::Overview | SessionView::Details) {
                    current.push(format!(
                        "Inspector section {} · ←/→ tabs · Enter/Down read · m next · Esc roster",
                        self.inspector.tab.label()
                    ));
                    current.push(
                        "Overview links: s Activity · Ctrl+R Review · t Traces · u Usage · K Metrics · P Prompts"
                            .into(),
                    );
                    current.push(
                        "Recorded lifecycle describes stored evidence, not live process state."
                            .into(),
                    );
                }
            }
            if let Some(nav) = &self.prompt_nav {
                let scope = nav.scope();
                current.push(format!(
                    "Prompts · {} / {} · worker {} · turn {}",
                    visible_text(&scope.source),
                    visible_text(&scope.session),
                    scope.worker.as_deref().unwrap_or("session scope"),
                    scope.turn.as_deref().unwrap_or("all recorded turns"),
                ));
                current.push(format!("Prompt status · {}", nav.status_line()));
                current.push(
                    "Prompt Escape order: chooser → reader → Prompts section → roster".into(),
                );
            }
            if view.page.ui.in_turn {
                current.push("Recorded turn scope · Esc returns to the containing page".into());
            }
            if view.page.ui.turn_available {
                current.push("T selects one recorded turn from this page".into());
            }
            if view.page.reviewing() || matches!(view.selection, HistorySelection::TurnReview(_)) {
                current.push(
                    "Review actions: c copy receipt · x export · s save task · o excerpts · d draft · R re-read · Tab controls · y acknowledge · n decisions · e edits"
                        .into(),
                );
                current.push(
                    "Review Git capture and workspace mapping need a verified workspace binding; the shell refuses without one"
                        .into(),
                );
                if view.page.ui.selected_title().is_some() {
                    current.push(
                        "Review escapes one layer: excerpt chooser → edit list → Review → inspector/roster"
                            .into(),
                    );
                }
            }
            if view.page.ui.selected_evidence().is_some() {
                current.push(
                    "Evidence filter · / edits the page-local query · Enter applies · Esc clears; the committed filter follows older/newer pages"
                        .into(),
                );
                current.push(
                    "Evidence keys · Home/End select first/last · PgUp/PgDn scroll detail · Enter inspects · t follows a linked trace where the row carries one"
                        .into(),
                );
            }
            if telemetry_details(view) {
                current.push("d opens the retained captured text where authorized".into());
            }
            if evaluation_details(view) {
                current.push("E opens trace evaluations for the loaded traces".into());
            }
            if view
                .page
                .operation_details
                .iter()
                .any(|entry| entry.kind == OperationDetailKind::Mcp)
            {
                current.push("M opens retained MCP payloads for this page".into());
            }
        }
        if let Some(work) = &self.work {
            current.extend(work.context_help());
            current.push(
                "Workflow and saved-task editors keep unsaved drafts while help is open".into(),
            );
        }
        let priority = match context {
            keymap::Context::CommandCenter => Some("SESSION"),
            keymap::Context::History => Some("REVIEW AND PROMPTS"),
            keymap::Context::Workbench => Some("WORKFLOW AND SAVED WORK"),
            keymap::Context::Investigation => Some("SEARCH"),
            keymap::Context::Form => Some("SYSTEM"),
        };
        if let Some(priority) = priority
            && let Some(position) = sections.iter().position(|(group, _)| *group == priority)
        {
            let first = sections.remove(position);
            sections.insert(0, first);
        }
        if !current.is_empty() {
            // The current workflow appears first, mirroring the reference.
            sections.insert(0, ("THIS VIEW", current));
        }
        self.help.open(title, sections);
    }

    fn action_unavailable(&mut self, id: &str) -> Option<&'static str> {
        if self.work.as_ref().is_some_and(Workbench::editing)
            && !matches!(
                id,
                keymap::id::HELP | keymap::id::THEMES | keymap::id::PALETTE
            )
        {
            return Some("Finish or explicitly cancel the active editor first");
        }
        let catalog_ready = self
            .agents
            .catalog
            .as_ref()
            .is_some_and(|catalog| !catalog.sessions.is_empty());
        let session = self.selected_session().is_some();
        let anchor = self.current_anchor().is_some();
        let history = !self.history.is_empty();
        let loaded = self.loaded_view().is_some();
        let refreshing = self.pending.is_none();
        match id {
            keymap::id::SESSION_CHOOSER | keymap::id::SCOPE_TODAY | keymap::id::SCOPE_HISTORY => {
                (!catalog_ready).then_some("No session page is loaded")
            }
            keymap::id::SESSION_ERRORS => (!catalog_ready).then_some("No session page is loaded"),
            keymap::id::SESSION_SUBTREE => (self.agents.session_scope.is_none())
                .then_some("Choose an exact agent session first"),
            keymap::id::INSPECTOR_SECTIONS => (!matches!(
                self.history.last().map(|view| &view.selection),
                Some(HistorySelection::Session(
                    _,
                    SessionView::Overview | SessionView::Details
                ))
            ))
            .then_some("Open the session inspector first"),
            keymap::id::SCOPE_SELECTED => (!anchor).then_some("Select an exact session or turn"),
            keymap::id::SCOPE_ALL => (!history).then_some("Open a retained evidence view first"),
            keymap::id::LIVE_REFRESH | keymap::id::RETRY_CONNECTION => {
                (!refreshing).then_some("Wait for the current read to finish")
            }
            keymap::id::HISTORY_COMMANDS
            | keymap::id::HISTORY_INSTRUCTIONS
            | keymap::id::HISTORY_FAILED_COMMAND => {
                (!anchor).then_some("Private phrase search needs a selected session or turn")
            }
            keymap::id::REVIEW | keymap::id::PROMPTS => {
                (!anchor).then_some("Select an exact session or turn first")
            }
            keymap::id::REVIEW_NOTES | keymap::id::MARK_REVIEWED => {
                (!session).then_some("Select an exact session first")
            }
            keymap::id::TRACES => (!session
                && !matches!(
                    self.history.last().map(|view| &view.selection),
                    Some(HistorySelection::Turn(_) | HistorySelection::McpTraces { .. })
                ))
            .then_some("Select a session or recorded turn first"),
            keymap::id::TURN => (!history
                || !self
                    .history
                    .last()
                    .is_some_and(|view| view.page.ui.turn_available))
            .then_some("This page has no selectable recorded turn"),
            keymap::id::MCP_PAYLOAD => (!history
                || !self.history.last().is_some_and(|view| {
                    view.page
                        .operation_details
                        .iter()
                        .any(|entry| entry.kind == OperationDetailKind::Mcp)
                }))
            .then_some("No retained MCP payload on this page"),
            keymap::id::FILE_EDITS => (!history
                || !self.history.last().is_some_and(|view| {
                    view.page
                        .operation_details
                        .iter()
                        .any(|entry| entry.kind == OperationDetailKind::Edit)
                }))
            .then_some("No retained file edit in this context"),
            keymap::id::OLDER_PAGE => {
                let more = self
                    .history
                    .last()
                    .is_some_and(|view| view.page.ui.older_cursor().is_some())
                    || self
                        .agents
                        .catalog
                        .as_ref()
                        .is_some_and(|catalog| catalog.older.is_some());
                (!more).then_some("No older retained page")
            }
            keymap::id::NEWER_PAGE => {
                let more = self
                    .history
                    .last()
                    .is_some_and(|view| !view.newer.is_empty())
                    || !self.newer_sessions.is_empty();
                (!more).then_some("No newer loaded page")
            }
            keymap::id::LATEST_PAGE => (!history && self.agents.catalog.as_ref().is_none())
                .then_some("No loaded page to refresh"),
            keymap::id::COPY | keymap::id::EXPORT => {
                (!loaded).then_some("No loaded metadata view in this focus")
            }
            keymap::id::ATTENTION => (!catalog_ready).then_some("No session page is loaded"),
            keymap::id::MARK_SEEN => {
                let pending = self
                    .agents
                    .current()
                    .is_some_and(|row| row.pending_requests.iter().any(|request| !request.seen));
                (!pending).then_some("No current pending request for the selected session")
            }
            keymap::id::REVIEW_FILTER
            | keymap::id::GROUP
            | keymap::id::TREE
            | keymap::id::RESET
            | keymap::id::HOST_NOW => (!catalog_ready).then_some("No session page is loaded"),
            keymap::id::OVERVIEW
            | keymap::id::SESSION_ACTIVITY
            | keymap::id::SESSION_USAGE
            | keymap::id::SESSION_METRICS => (!session).then_some("Select an exact session first"),
            keymap::id::INSPECTOR_MODE => (!matches!(
                self.history.last().map(|view| &view.selection),
                Some(HistorySelection::Session(_, _))
            ))
            .then_some("Open a session inspector first"),
            keymap::id::REVIEW_REFRESH
            | keymap::id::REVIEW_CONTROLS
            | keymap::id::REVIEW_SAVE_TASK
            | keymap::id::REVIEW_EXCERPT
            | keymap::id::REVIEW_DRAFT
            | keymap::id::REVIEW_GIT
            | keymap::id::REVIEW_MAP => {
                (!self.review_controls_focused()).then_some("Open a session or turn Review first")
            }
            keymap::id::WORKFLOW_INCIDENTS => None,
            keymap::id::WORKFLOW_SEEN
            | keymap::id::WORKFLOW_LOCATE
            | keymap::id::WORKFLOW_RESUME
            | keymap::id::WORKFLOW_RETRY
            | keymap::id::WORKFLOW_SETTINGS => {
                (!self.work.as_ref().is_some_and(Workbench::workflow_visible))
                    .then_some("Open the Workflow inbox first")
            }
            keymap::id::TASK_DECISION_ADD | keymap::id::TASK_DECISION_SUPERSEDE => {
                (!self.work.as_ref().is_some_and(Workbench::decisions_visible))
                    .then_some("Open the task Decisions chooser first")
            }
            keymap::id::TASK_BRIEF | keymap::id::TASK_DECISIONS => {
                (!self.work.as_ref().is_some_and(Workbench::tasks_visible))
                    .then_some("Open Saved tasks first")
            }
            keymap::id::HANDOFF_COPY => (!self
                .work
                .as_ref()
                .is_some_and(Workbench::handoff_preview_visible))
            .then_some("Open a Handoff preview first"),
            keymap::id::HANDOFF_PREVIEW => {
                (!self.work.as_ref().is_some_and(Workbench::handoffs_visible))
                    .then_some("Open Handoffs first")
            }
            keymap::id::TASK_SEARCH => None,
            keymap::id::PROMPT_EARLIER_PAGE => self
                .prompt_cursor
                .is_none()
                .then_some("No earlier requests in the retained window"),
            keymap::id::PROMPT_MAIN_SESSION => self
                .prompt_nav
                .as_ref()
                .is_none_or(|nav| nav.scope().main_session.is_none())
                .then_some("No verified main-session navigation capability"),
            keymap::id::PROMPT_RETRY => self
                .prompt_nav
                .as_ref()
                .is_none_or(|nav| !nav.retry_available())
                .then_some("No failed prompt load to retry"),
            keymap::id::PROMPT_READ
            | keymap::id::PROMPT_OLDER
            | keymap::id::PROMPT_NEWER
            | keymap::id::PROMPT_LATEST
            | keymap::id::PROMPT_PRESENTATION
            | keymap::id::PROMPT_BACK => {
                (self.prompt_nav.is_none()).then_some("Open the inspector Prompts section first")
            }
            keymap::id::EVIDENCE_FILTER
            | keymap::id::EVIDENCE_HOME_END
            | keymap::id::EVIDENCE_PAGE => {
                (!history).then_some("Open a retained evidence page first")
            }
            keymap::id::ACTIVITY_PROMPTS => self
                .history
                .last()
                .is_none_or(|view| !view.page.ui.activity_prompts_available())
                .then_some("Select the Activity pane first"),
            keymap::id::SEARCH => {
                let focus = self.focus();
                (focus.text_entry() || focus.chooser())
                    .then_some("A text or chooser control owns this key")
            }
            keymap::id::INSPECT => {
                (!history && !self.agents.show_more_selected && self.agents.current().is_none())
                    .then_some("No selection to inspect")
            }
            keymap::id::BACK => (!history && self.work.is_none() && self.investigation.is_none())
                .then_some("Already at the cockpit root"),
            _ => None,
        }
    }

    fn palette_entries(&mut self) -> Vec<palette::Entry> {
        let context = self.context();
        keymap::actions(context)
            .filter(|action| {
                action.id != keymap::id::PALETTE && !keymap::PALETTE_EXCLUDED.contains(&action.id)
            })
            .map(|action| {
                let unavailable = self.action_unavailable(action.id);
                let label = if action.id == keymap::id::LIVE_PAUSE && self.agents.paused {
                    "Resume live updates"
                } else {
                    action.label
                };
                match unavailable {
                    Some(reason) => {
                        palette::Entry::unavailable(action.id, label, reason, action.group.label())
                    }
                    None => palette::Entry::command(
                        action.id,
                        label,
                        action.detail,
                        action.group.label(),
                    ),
                }
            })
            .collect()
    }

    fn open_palette(&mut self) {
        let entries = self.palette_entries();
        self.palette.open_commands(entries);
        self.message = None;
    }

    fn open_session_chooser(&mut self) {
        let Some(catalog) = self.agents.catalog.as_ref() else {
            self.message = Some("No session page is loaded".into());
            return;
        };
        let mut entries = vec![palette::Entry::command(
            "session|all",
            "All sessions",
            "Clear only the session filter",
            "SESSIONS",
        )];
        entries.extend(catalog.sessions.iter().map(|row| {
            let project = command_center::project(row);
            let native = row
                .native_context
                .as_deref()
                .map_or_else(String::new, |context| {
                    [
                        context.agent_name.as_deref(),
                        context.agent_role.as_deref(),
                        context.agent_path.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .map(visible_text)
                    .collect::<Vec<_>>()
                    .join(" · ")
                });
            let mut detail = format!(
                "{} · {}",
                command_center::provider(row),
                visible_text(project)
            );
            if !native.is_empty() {
                detail.push_str(" · ");
                detail.push_str(&native);
            }
            if let Some(worker) = row.worker_id.as_deref().filter(|id| !id.trim().is_empty()) {
                detail.push_str(" · worker ");
                detail.push_str(&visible_text(worker));
            }
            detail.push_str(&format!(
                " · {} · {}",
                visible_text(&row.harness_id),
                visible_text(&row.session_id)
            ));
            palette::Entry::command(
                format!("session|{}|{}", row.harness_id, row.session_id),
                command_center::name(row).to_string(),
                detail,
                "SESSIONS",
            )
        }));
        self.palette.open_sessions(entries);
        if let Some((source, id)) = &self.agents.session_scope {
            self.palette.select_key(&format!("session|{source}|{id}"));
        }
        self.message = None;
    }

    fn choose_session(&mut self, choice: &str) {
        if choice == "session|all" {
            self.agents.session_scope = None;
            self.agents.subtree_scope = false;
            let identity = self.agents.current().map(session_key);
            self.agents.apply_filters(identity.as_ref());
            self.message =
                Some("Session scope cleared · showing all sessions in this time scope".into());
            return;
        }
        let mut parts = choice.splitn(3, '|');
        let (Some("session"), Some(source), Some(id)) = (parts.next(), parts.next(), parts.next())
        else {
            self.message = Some("That session choice is not valid".into());
            return;
        };
        let identity = (source.to_owned(), id.to_owned());
        let exists = self.agents.catalog.as_ref().is_some_and(|catalog| {
            catalog
                .sessions
                .iter()
                .any(|row| session_key(row) == identity)
        });
        if !exists {
            self.message =
                Some("That session is no longer available. Choose another session.".into());
            return;
        }
        self.agents.session_scope = Some(identity.clone());
        self.agents.subtree_scope = false;
        self.agents.apply_filters(Some(&identity));
        if self.agents.visible.is_empty() {
            self.message = Some(
                "The chosen session is hidden by the active Today/status filters · choose All sessions"
                    .into(),
            );
        } else if self.agents.lost_selection {
            self.message =
                Some("The chosen session left the current page · kept the nearest row".into());
        } else {
            self.message = Some(format!(
                "Session scope · {}",
                self.agents
                    .current()
                    .map(|row| visible_text(&command_center::name(row)))
                    .unwrap_or_else(|| "selected session".into())
            ));
        }
    }

    fn set_time_scope(&mut self, scope: TimeScope) {
        let identity = self.agents.current().map(session_key);
        self.agents.time_scope = scope;
        self.agents.apply_filters(identity.as_ref());
        let mut message = format!(
            "Time scope: {} · {} session{} on this page",
            scope.label(),
            self.agents.visible.len(),
            if self.agents.visible.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        if scope == TimeScope::Today && self.agents.visible.is_empty() {
            message.push_str(" · nothing pending today · O History shows earlier work");
        }
        self.message = Some(message);
    }

    fn toggle_errors(&mut self) {
        self.agents.errors_only = !self.agents.errors_only;
        let identity = self.agents.current().map(session_key);
        self.agents.apply_filters(identity.as_ref());
        self.message = Some(if self.agents.errors_only {
            "Errors only · recorded failed or interrupted runs on this page".into()
        } else {
            "Errors filter cleared".into()
        });
    }

    fn toggle_subtree(&mut self) {
        if self.agents.session_scope.is_none() {
            self.message = Some("Choose an agent session before selecting a subtree scope".into());
            return;
        }
        self.agents.subtree_scope = !self.agents.subtree_scope;
        let identity = self.agents.current().map(session_key);
        self.agents.apply_filters(identity.as_ref());
        let mut message = if self.agents.subtree_scope {
            "Evidence scope: exact agent + confirmed delegated descendants".to_owned()
        } else {
            "Evidence scope: exact agent only".to_owned()
        };
        if self.agents.subtree_unsupported > 0 {
            message.push_str(&format!(
                " · {} non-delegated or cross-source relationship{} excluded",
                self.agents.subtree_unsupported,
                if self.agents.subtree_unsupported == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        if self.agents.subtree_outside_page > 0 {
            message.push_str(&format!(
                " · {} delegated parent{} outside this page",
                self.agents.subtree_outside_page,
                if self.agents.subtree_outside_page == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        self.message = Some(message);
    }

    fn refresh_current(&mut self, sender: &mpsc::Sender<WorkspaceRequest>) {
        if self.work.is_some() {
            let _ = self.key(
                sender,
                event::KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
            );
        } else if !self.history.is_empty() {
            let selection = self
                .history
                .last()
                .expect("history exists")
                .selection
                .clone();
            self.request(
                sender,
                self.history_request(selection, None),
                Change::LatestHistory,
            );
        } else {
            self.request(
                sender,
                WorkspaceRequest::Sessions(None),
                Change::RefreshSessions,
            );
        }
    }

    fn run_action(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, id: &str) -> bool {
        self.palette.close();
        if let Some(reason) = self.action_unavailable(id) {
            self.message = Some(reason.into());
            return false;
        }
        if let Some(choice) = id.strip_prefix("session|") {
            self.choose_session(&format!("session|{choice}"));
            return false;
        }
        match id {
            keymap::id::PALETTE => self.open_palette(),
            keymap::id::THEMES => self.theme_request = true,
            keymap::id::HELP => self.open_help(),
            keymap::id::GO_AGENTS => self.navigate(sender, Destination::Agents),
            keymap::id::GO_ACTIVITY => self.navigate(sender, Destination::Activity),
            keymap::id::GO_MCP => self.navigate(sender, Destination::Mcp),
            keymap::id::GO_TRACES => self.navigate(sender, Destination::Traces),
            keymap::id::GO_HOST => self.navigate(sender, Destination::Host),
            keymap::id::GO_METRICS => self.navigate(sender, Destination::Metrics),
            keymap::id::GO_FINDINGS => self.navigate(sender, Destination::Findings),
            keymap::id::GO_OPERATIONS => self.navigate(sender, Destination::Operations),
            keymap::id::GO_USAGE => self.navigate(sender, Destination::Usage),
            keymap::id::SESSION_CHOOSER => self.open_session_chooser(),
            keymap::id::SCOPE_TODAY => self.set_time_scope(TimeScope::Today),
            keymap::id::SCOPE_HISTORY => self.set_time_scope(TimeScope::All),
            keymap::id::SESSION_ERRORS => self.toggle_errors(),
            keymap::id::SESSION_SUBTREE => self.toggle_subtree(),
            keymap::id::SCOPE_SELECTED => {
                let turn = self.history.last().and_then(|view| match &view.selection {
                    HistorySelection::Turn(turn) => Some(turn.clone()),
                    _ => None,
                });
                if let Some(turn) = turn {
                    let selection = HistorySelection::Turn(turn);
                    self.open(sender, selection);
                } else if let Some(session) = self.selected_session().cloned() {
                    self.select_session_mode(sender, session, SessionView::Activity);
                } else {
                    self.navigate(sender, Destination::Activity);
                }
            }
            keymap::id::SCOPE_ALL => self.navigate(sender, Destination::Activity),
            keymap::id::LIVE_PAUSE => self.toggle_pause(),
            keymap::id::LIVE_REFRESH | keymap::id::RETRY_CONNECTION => self.refresh_current(sender),
            keymap::id::DIAGNOSTICS => self.navigate(sender, Destination::Findings),
            keymap::id::HISTORY_FAILURES => self.open_investigation_kind(0),
            keymap::id::HISTORY_INSTRUCTIONS => self.open_investigation_kind(1),
            keymap::id::HISTORY_PATHS => self.open_investigation_kind(2),
            keymap::id::HISTORY_COMMANDS => self.open_investigation_kind(3),
            keymap::id::HISTORY_FAILED_COMMAND => self.open_investigation_kind(4),
            keymap::id::REVIEW => {
                return self.send_key(
                    sender,
                    event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                );
            }
            keymap::id::REVIEW_REFRESH => return self.send_key(sender, plain_key('R')),
            keymap::id::REVIEW_CONTROLS => {
                if let Some(view) = self.history.last_mut()
                    && (view.page.reviewing()
                        || matches!(view.selection, HistorySelection::TurnReview(_)))
                {
                    view.page.review_control = Some(0);
                    self.message = Some(format!(
                        "Review controls · {} · Tab moves · Enter activates",
                        REVIEW_CONTROLS[0].0
                    ));
                } else {
                    self.message = Some("Open a Review to use its control order".into());
                }
            }
            keymap::id::REVIEW_SAVE_TASK => return self.send_key(sender, plain_key('s')),
            keymap::id::REVIEW_EXCERPT => return self.send_key(sender, plain_key('o')),
            keymap::id::REVIEW_DRAFT => return self.send_key(sender, plain_key('d')),
            keymap::id::REVIEW_GIT => return self.send_key(sender, plain_key('g')),
            keymap::id::REVIEW_MAP => return self.send_key(sender, plain_key('m')),
            keymap::id::WORKFLOW_INCIDENTS => {
                if self.work.is_some() {
                    return self.send_key(sender, plain_code(KeyCode::Tab));
                }
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::WorkflowAlerts),
                    Change::Work(None),
                );
            }
            keymap::id::WORKFLOW_SEEN => return self.send_key(sender, plain_key('r')),
            keymap::id::WORKFLOW_LOCATE => return self.send_key(sender, plain_key('l')),
            keymap::id::WORKFLOW_RESUME => return self.send_key(sender, plain_key('c')),
            keymap::id::WORKFLOW_RETRY => return self.send_key(sender, plain_key('T')),
            keymap::id::WORKFLOW_SETTINGS => return self.send_key(sender, plain_key('S')),
            keymap::id::TASK_SEARCH => {
                if self.work.is_some() {
                    return self.send_key(sender, plain_key('/'));
                }
                self.pending_task_search = true;
                self.request(
                    sender,
                    WorkspaceRequest::Work(WorkCommand::Tasks(None)),
                    Change::Work(None),
                );
            }
            keymap::id::TASK_BRIEF => return self.send_key(sender, plain_key('p')),
            keymap::id::TASK_DECISIONS => return self.send_key(sender, plain_key('v')),
            keymap::id::TASK_DECISION_ADD => return self.send_key(sender, plain_key('n')),
            keymap::id::TASK_DECISION_SUPERSEDE => return self.send_key(sender, plain_key('s')),
            keymap::id::HANDOFF_PREVIEW => return self.send_key(sender, plain_key('x')),
            keymap::id::HANDOFF_COPY => return self.send_key(sender, plain_key('c')),
            keymap::id::EVIDENCE_FILTER => return self.send_key(sender, plain_key('/')),
            keymap::id::ACTIVITY_PROMPTS => return self.send_key(sender, plain_key('F')),
            keymap::id::EVIDENCE_HOME_END => {
                return self.send_key(sender, plain_code(KeyCode::Home));
            }
            keymap::id::EVIDENCE_PAGE => {
                return self.send_key(sender, plain_code(KeyCode::PageDown));
            }
            keymap::id::PROMPT_READ => {
                return self.send_key(sender, plain_code(KeyCode::Enter));
            }
            keymap::id::PROMPT_OLDER => {
                return self.send_key(sender, plain_code(keymap::NEWER));
            }
            keymap::id::PROMPT_NEWER => {
                return self.send_key(sender, plain_code(keymap::OLDER));
            }
            keymap::id::PROMPT_LATEST => return self.send_key(sender, plain_code(keymap::LATEST)),
            keymap::id::PROMPT_PRESENTATION => return self.send_key(sender, plain_key('z')),
            keymap::id::PROMPT_BACK => return self.send_key(sender, plain_code(KeyCode::Esc)),
            keymap::id::PROMPT_MAIN_SESSION => return self.send_key(sender, plain_key('m')),
            keymap::id::PROMPT_EARLIER_PAGE => {
                return self.send_key(sender, plain_code(KeyCode::PageUp));
            }
            keymap::id::PROMPT_RETRY => return self.send_key(sender, plain_key('R')),
            keymap::id::PROMPTS => return self.send_key(sender, plain_key('P')),
            keymap::id::TRACES => return self.send_key(sender, plain_key('t')),
            keymap::id::TURN => return self.send_key(sender, plain_key('T')),
            keymap::id::MCP_PAYLOAD => return self.send_key(sender, plain_key('M')),
            keymap::id::FILE_EDITS => return self.send_key(sender, plain_key('e')),
            keymap::id::REVIEW_NOTES => return self.send_key(sender, plain_key('n')),
            keymap::id::MARK_REVIEWED => return self.send_key(sender, plain_key('y')),
            keymap::id::OLDER_PAGE => return self.send_key(sender, plain_code(keymap::OLDER)),
            keymap::id::NEWER_PAGE => return self.send_key(sender, plain_code(keymap::NEWER)),
            keymap::id::LATEST_PAGE => return self.send_key(sender, plain_code(keymap::LATEST)),
            keymap::id::COPY => return self.send_key(sender, plain_key('c')),
            keymap::id::EXPORT => return self.send_key(sender, plain_key('x')),
            keymap::id::INTERNAL_REVIEWS => return self.send_key(sender, plain_key('v')),
            keymap::id::WORKFLOW => return self.send_key(sender, plain_key('w')),
            keymap::id::TASKS => return self.send_key(sender, plain_key('b')),
            keymap::id::ATTENTION => {
                self.navigate(sender, Destination::Agents);
                return self.send_key(sender, plain_key('!'));
            }
            keymap::id::MARK_SEEN => return self.send_key(sender, plain_key('X')),
            keymap::id::REVIEW_FILTER => return self.send_key(sender, plain_key('Y')),
            keymap::id::GROUP => return self.send_key(sender, plain_key('G')),
            keymap::id::TREE => return self.send_key(sender, plain_key('f')),
            keymap::id::RESET => return self.send_key(sender, plain_key('a')),
            keymap::id::OVERVIEW => return self.send_key(sender, plain_key('g')),
            keymap::id::SESSION_ACTIVITY => {
                if let Some(session) = self.selected_session().cloned() {
                    self.select_session_mode(sender, session, SessionView::Activity);
                }
            }
            keymap::id::SESSION_USAGE => return self.send_key(sender, plain_key('u')),
            keymap::id::SESSION_METRICS => return self.send_key(sender, plain_key('K')),
            keymap::id::HOST_NOW => return self.send_key(sender, plain_key('H')),
            keymap::id::INSPECTOR_MODE => return self.send_key(sender, plain_key('m')),
            keymap::id::INSPECTOR_SECTIONS => self.focus_inspector_tabs(),
            keymap::id::SEARCH => return self.send_key(sender, plain_key('/')),
            keymap::id::INSPECT => return self.send_key(sender, plain_code(KeyCode::Enter)),
            keymap::id::BACK => return self.send_key(sender, plain_code(KeyCode::Esc)),
            keymap::id::QUIT => return true,
            _ => {}
        }
        false
    }

    fn send_key(&mut self, sender: &mpsc::Sender<WorkspaceRequest>, key: event::KeyEvent) -> bool {
        self.key(sender, key)
    }

    fn agent_key(
        &mut self,
        sender: &mpsc::Sender<WorkspaceRequest>,
        event: event::KeyEvent,
    ) -> bool {
        let key = keymap::plain(event).unwrap_or(KeyCode::Null);
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
            KeyCode::Char('G') => {
                let identity = self.agents.current().map(session_key);
                self.agents.grouping = self.agents.grouping.next();
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
            KeyCode::Char('T') => self.set_time_scope(TimeScope::Today),
            KeyCode::Char('O') => self.set_time_scope(TimeScope::All),
            KeyCode::Char('E') => self.toggle_errors(),
            KeyCode::Char('S') => self.toggle_subtree(),
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
            KeyCode::Char('v') => self.toggle_review_visibility(sender),
            KeyCode::Char('X') => {
                if let Some(session) = self.agents.current().cloned() {
                    self.request(
                        sender,
                        WorkspaceRequest::Work(WorkCommand::SessionAttention(session)),
                        Change::Work(None),
                    );
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
            KeyCode::Char('H') => self.request(
                sender,
                WorkspaceRequest::Work(WorkCommand::HostNow),
                Change::Work(None),
            ),
            KeyCode::Char('g' | 'U' | '8') => {
                let view = match key {
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
            KeyCode::Enter | KeyCode::Char('e' | 'm' | 'A' | 's' | 'V' | 'u' | 'K' | 't') => {
                if let Some(session) = self.agents.current().cloned() {
                    let view = match key {
                        KeyCode::Char('A' | 's') => SessionView::Activity,
                        KeyCode::Char('V') => SessionView::Review,
                        KeyCode::Char('u') => SessionView::Usage,
                        KeyCode::Char('K') => SessionView::Metrics,
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
    load: impl FnMut(WorkspaceRequest) -> Result<WorkspaceResponse, String> + Send,
) -> io::Result<()> {
    run_workspace_with_stop(load, Arc::new(AtomicBool::new(false)))
}

struct StopWorkspaceLoader<'a>(&'a AtomicBool);

impl Drop for StopWorkspaceLoader<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn load_workspace_requests(
    requests: mpsc::Receiver<WorkspaceRequest>,
    send: mpsc::Sender<Result<WorkspaceResponse, String>>,
    stop: &AtomicBool,
    mut load: impl FnMut(WorkspaceRequest) -> Result<WorkspaceResponse, String>,
) {
    while let Ok(request) = requests.recv() {
        if stop.load(Ordering::Acquire) || send.send(load(request)).is_err() {
            break;
        }
    }
}

/// Signal cancellation on terminal exit before waiting for the loader.
/// Long-running readers can observe the shared flag during a request.
pub fn run_workspace_with_stop(
    mut load: impl FnMut(WorkspaceRequest) -> Result<WorkspaceResponse, String> + Send,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    let review_preferences = review_visibility::path();
    let include_reviews = review_preferences
        .as_deref()
        .is_some_and(review_visibility::read);
    std::thread::scope(|scope| {
        let (request, requests) = mpsc::channel();
        let (send, results) = mpsc::channel();
        let loader_stop = Arc::clone(&stop);
        scope.spawn(move || {
            load_workspace_requests(requests, send, &loader_stop, |request| match request {
                WorkspaceRequest::ReviewVisibility(include) => {
                    review_visibility::update(review_preferences.as_deref(), include)
                        .map(|()| WorkspaceResponse::ReviewVisibility(include))
                }
                request => load(request),
            });
        });
        let stop_on_exit = StopWorkspaceLoader(&stop);
        let terminal_result = ratatui::run(|terminal| {
            let mut workspace = Workspace::new();
            workspace.include_reviews = include_reviews;
            let mut appearance = theme::Picker::new();
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
                        if let (Some(navigator), Some(epoch), Some(key)) = (
                            &mut workspace.prompt_nav,
                            workspace.prompt_read_epoch,
                            workspace.prompt_read_key.clone(),
                        ) {
                            match navigator.accept_reader(
                                epoch,
                                key,
                                text,
                                receiver,
                                Instant::now(),
                            ) {
                                prompt::PromptAccepted::Accepted => {}
                                prompt::PromptAccepted::Late(_) => {
                                    workspace.message =
                                        Some("Discarded a late prompt delivery".into());
                                }
                            }
                            workspace.prompt_read_epoch = None;
                            workspace.prompt_read_key = None;
                        } else {
                            workspace.prompt =
                                Some(PromptScreen::new(text, receiver, Instant::now()));
                        }
                    } else {
                        workspace.message = Some("Invalid private content delivery".into());
                    }
                    dirty = true;
                }
                if let Ok(result) = results.try_recv() {
                    workspace.finish(result);
                    if workspace.pending.is_none() {
                        workspace.start_deferred_prompt_read(&request);
                    }
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
                if workspace.pending.is_none()
                    && !workspace.agents.paused
                    && Instant::now() >= workspace.refresh_after
                    && let Some(refresh) = workspace
                        .work
                        .as_ref()
                        .and_then(Workbench::workflow_refresh)
                {
                    workspace.refresh_after = Instant::now() + REFRESH_INTERVAL;
                    workspace.request(
                        &request,
                        WorkspaceRequest::Work(refresh),
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
                dirty |= workspace.expire_prompt_nav(Instant::now());
                if dirty {
                    let active = workspace.private_display_gate();
                    let gate = active.as_ref().and_then(|active| active.lock().ok());
                    if active.is_some() {
                        workspace.enforce_private_display_gate(
                            gate.as_deref().copied().unwrap_or(false),
                        );
                    }
                    terminal.draw(|frame| {
                        draw_workspace(frame, &mut workspace);
                        if workspace.prompt.is_none()
                            && !workspace
                                .prompt_nav
                                .as_ref()
                                .is_some_and(|nav| nav.reader_open())
                        {
                            appearance.draw(frame);
                        }
                    })?;
                    workspace.acknowledge_redaction();
                    drop(gate);
                    dirty = false;
                }
                // Async work can queue a public resume command without a
                // further key event. Flush only after public redraw/ACK.
                if workspace.prompt.is_none()
                    && !workspace
                        .prompt_nav
                        .as_ref()
                        .is_some_and(|nav| nav.reader_open())
                    && workspace.prompt_receiver.is_none()
                    && workspace.redact_then_acknowledge.is_none()
                    && let Some(bytes) = workspace.view_io.clipboard.take()
                {
                    use std::io::Write;
                    let mut output = io::stdout().lock();
                    output.write_all(&bytes)?;
                    output.flush()?;
                }
                if event::poll(Duration::from_millis(80))? {
                    match event::read()? {
                        Event::Key(key)
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            if workspace.prompt.is_none()
                                && !workspace
                                    .prompt_nav
                                    .as_ref()
                                    .is_some_and(|nav| nav.reader_open() || nav.read_pending())
                                && appearance.key(key)
                            {
                                dirty = true;
                                continue;
                            }
                            let quit = workspace.key(&request, key);
                            if std::mem::take(&mut workspace.theme_request) {
                                appearance.open();
                            }
                            if quit {
                                stop.store(true, Ordering::Release);
                                // Keep the proof fence until a flushed non-private
                                // frame replaces the modal, including q and Ctrl-C.
                                let redacted = workspace.dismiss_prompt();
                                let inline_redacted = workspace.redact_prompt_nav(true);
                                if redacted
                                    || inline_redacted
                                    || workspace.redact_then_acknowledge.is_some()
                                {
                                    terminal.draw(|frame| {
                                        draw_workspace(frame, &mut workspace);
                                        if workspace.prompt.is_none() {
                                            appearance.draw(frame);
                                        }
                                    })?;
                                    workspace.acknowledge_redaction();
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
        drop(stop_on_exit);
        drop(request);
        terminal_result
    })
}

fn pending_evidence_titles(selection: &HistorySelection) -> Option<(&'static str, &'static str)> {
    match selection {
        HistorySelection::ReviewEdits(_) => Some(("Review file edits", "Retained edit evidence")),
        HistorySelection::Session(_, SessionView::Overview) => {
            Some(("Session inspector", "Session overview"))
        }
        HistorySelection::Session(_, SessionView::Details) => {
            Some(("Session inspector", "Session details"))
        }
        HistorySelection::All(WorkspaceView::Activity)
        | HistorySelection::Session(_, SessionView::Activity) => Some(("Activity", "Event detail")),
        HistorySelection::All(WorkspaceView::Findings) => Some(("Findings", "Evidence inspector")),
        HistorySelection::All(WorkspaceView::Host) => Some(("Host now", "Collection status")),
        HistorySelection::All(WorkspaceView::Usage)
        | HistorySelection::Session(_, SessionView::Usage) => Some(("Usage", "Metric inspector")),
        HistorySelection::Session(_, SessionView::Metrics) => {
            Some(("Metrics", "All authorized OTLP sources"))
        }
        HistorySelection::All(WorkspaceView::Traces)
        | HistorySelection::Session(_, SessionView::Traces)
        | HistorySelection::McpTraces { .. } => Some(("Traces", "Execution tree")),
        _ => None,
    }
}

struct ReviewEditView<'a> {
    session: &'a RecordedSession,
    entries: &'a [OperationDetailEntry],
    older: bool,
    newer: bool,
    preview: bool,
    pending: bool,
    message: Option<&'a str>,
}

fn draw_review_edits(frame: &mut Frame, view: ReviewEditView<'_>, selection: &mut ListState) {
    let [context, list, receipt, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(" Review file edits").style(heading_style()),
            Line::from(format!(
                " Source  {}",
                visible_text(&view.session.harness_id)
            )),
            Line::from(format!(
                " Session  {}",
                visible_text(&view.session.session_id)
            )),
            Line::from(format!(
                " Evidence {} · Snapshot {}",
                view.session.review_version.receipt_sequence, view.session.snapshot_id,
            )),
        ]),
        context,
    );
    if view.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(if view.older {
                "No eligible edits in this range. ] to look further back."
            } else {
                "No eligible edits in this retained range."
            })
            .wrap(Wrap { trim: true })
            .block(panel(" Codex file edits ")),
            list,
        );
    } else {
        frame.render_stateful_widget(
            List::new(view.entries.iter().map(|entry| {
                ListItem::new(format!(
                    "{} · rev {}",
                    visible_text(&entry.label),
                    entry.revision
                ))
            }))
            .block(panel(" Codex file edits "))
            .highlight_symbol("› ")
            .highlight_style(selection_style()),
            list,
            selection,
        );
    }
    if let Some(entry) = selection
        .selected()
        .and_then(|index| view.entries.get(index))
    {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " Main worker · Turn {}",
                    visible_text(&entry.turn_id)
                )),
                Line::from(format!(" Operation {}", visible_text(&entry.operation_id))),
                Line::from(format!(
                    " Event {} · rev {}",
                    visible_text(&entry.event_id),
                    entry.revision
                )),
            ]),
            receipt,
        );
    }
    let mut navigation = String::from("↑↓ select · Esc Review");
    if view.preview && !view.entries.is_empty() {
        navigation.push_str(" · Enter reveal");
    }
    let mut paging = String::new();
    if view.older {
        paging.push_str("] older · ");
    }
    if view.newer {
        paging.push_str("[ newer · ");
    }
    paging.push_str("l latest at this snapshot");
    let status = if view.pending {
        "Reading selected evidence…"
    } else if !view.preview {
        "Recorded metadata · private preview unavailable"
    } else if view.older {
        "More retained history available"
    } else {
        "Reached the beginning of retained history"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(visible_text(view.message.unwrap_or(status))),
            Line::from(navigation).style(label_style()),
            Line::from(paging).style(label_style()),
        ]),
        footer,
    );
}

fn draw_workspace(frame: &mut Frame, workspace: &mut Workspace) {
    draw_workspace_base(frame, workspace);
    if workspace.help.is_open() {
        workspace.help.draw(frame);
    } else if workspace.palette.is_open() {
        workspace.palette.draw(frame);
    }
}

fn draw_workspace_base(frame: &mut Frame, workspace: &mut Workspace) {
    frame.render_widget(
        Block::default().style(surface(theme::palette().canvas)),
        frame.area(),
    );
    if let Some(prompt) = &mut workspace.prompt {
        prompt.draw(frame);
        return;
    }
    if let Some(form) = &workspace.view_io.review_form {
        view_io::draw_review_export(frame, form, workspace.message.as_deref());
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
    if let Some(navigator) = &mut workspace.prompt_nav {
        if let Some(HistoryView {
            selection: HistorySelection::Session(session, _),
            ..
        }) = workspace.history.last()
        {
            inspector::draw_prompts(
                frame,
                session,
                workspace.inspector,
                navigator,
                workspace.pending.is_some(),
                workspace.message.as_deref(),
            );
        } else {
            let [body, status, keys] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
            navigator.draw(frame, body);
            frame.render_widget(Paragraph::new(navigator.status_line()), status);
            frame.render_widget(Paragraph::new(navigator.hints_for(area.width)), keys);
        }
        return;
    }
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
        if let Some(picker) = &mut view.page.trace_picker {
            if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
                frame.render_widget(
                    Paragraph::new("Terminal too small · widen the window or press Esc")
                        .centered()
                        .style(theme::foreground(theme::palette().primary)),
                    area,
                );
                return;
            }
            let [body, footer] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);
            let items = picker.targets.iter().map(|target| {
                let worker = target
                    .worker_id
                    .as_deref()
                    .map_or_else(String::new, |worker| format!(" · worker {worker}"));
                let turn = target
                    .turn_id
                    .as_deref()
                    .map_or_else(String::new, |turn| format!(" · turn {turn}"));
                ListItem::new(format!(
                    "span {}  →  operation {} · source {} · session {}{}{}",
                    visible_text(&target.span_id),
                    visible_text(&target.operation_id),
                    visible_text(&target.source_id),
                    visible_text(&target.session_id),
                    worker,
                    turn,
                ))
            });
            frame.render_stateful_widget(
                List::new(items)
                    .block(panel("Select linked operation · trace spans"))
                    .highlight_symbol("› ")
                    .highlight_style(selection_style()),
                body,
                &mut picker.selection,
            );
            frame.render_widget(
                Paragraph::new("↑↓ choose span · Enter open operation · Esc back"),
                footer,
            );
            return;
        }
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
                    .block(panel("Trace evaluations · loaded traces"))
                    .highlight_symbol("› ")
                    .highlight_style(selection_style()),
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
            if let HistorySelection::ReviewEdits(session) = &view.selection {
                draw_review_edits(
                    frame,
                    ReviewEditView {
                        session,
                        entries: &view.page.operation_details,
                        older: view.page.ui.older_cursor().is_some(),
                        newer: !view.newer.is_empty(),
                        preview: view.page.review_edit_preview,
                        pending: workspace.pending.is_some(),
                        message: workspace.message.as_deref(),
                    },
                    selection,
                );
                return;
            }
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
                    .block(panel(match kind {
                        OperationDetailKind::Mcp => "MCP operations",
                        OperationDetailKind::Edit => "File edits",
                    }))
                    .highlight_style(selection_style()),
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
        if area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && view.page.turn_picker.is_none()
            && let Some(analytics) = view.active_analytics_mut()
        {
            let loading = workspace
                .pending
                .as_ref()
                .is_some_and(|change| !matches!(change, Change::MetricHistory { cancelled: true }));
            analytics.draw(frame, loading, workspace.message.as_deref());
            return;
        }
        if area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && view.page.turn_picker.is_none()
            && let Some(navigator) = &mut workspace.prompt_nav
            && let HistorySelection::Session(session, _) = &view.selection
        {
            inspector::draw_prompts(
                frame,
                session,
                workspace.inspector,
                navigator,
                workspace.pending.is_some(),
                workspace.message.as_deref(),
            );
            return;
        }
        if area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && view.page.turn_picker.is_none()
            && let HistorySelection::Session(session, session_view) = &view.selection
            && inspector::Tab::from_view(*session_view).is_some()
        {
            view.page.ui.set_viewport(area.width, area.height);
            inspector::draw(
                frame,
                session,
                &view.page.ui,
                workspace.inspector,
                !view.newer.is_empty(),
                workspace.pending.is_some(),
                workspace.message.as_deref(),
            );
            return;
        }
        view.page.ui.set_viewport(area.width, area.height);
        let draw_findings = !view.page.ui.help
            && view.page.turn_picker.is_none()
            && area.width >= MIN_WIDTH
            && area.height >= MIN_HEIGHT
            && view.page.active_findings().is_some();
        if draw_findings {
            let [head, body, keys, _] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
            evidence_view::header(frame, &view.page.ui, head);
            let browser = view
                .page
                .active_findings()
                .expect("selected findings browser exists");
            browser.draw(frame, body);
            frame.render_widget(Paragraph::new(browser.hints()), keys);
        } else if !view.page.ui.help
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
            let hint = if view.page.trace_return.is_some() && table.expanded() {
                "↑↓ scroll · Esc close detail · Esc return to trace"
            } else if view.page.trace_return.is_some() {
                "↑↓ select · Enter inspect · Esc return to trace"
            } else {
                table.key_hint()
            };
            frame.render_widget(Paragraph::new(hint), keys);
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
            let back_pages = keymap::hint(&[
                keymap::id::BACK,
                keymap::id::NEWER_PAGE,
                keymap::id::OLDER_PAGE,
            ]);
            let status = if let Some(error) = workspace.message.as_deref() {
                error.to_owned()
            } else if workspace.pending.is_some() {
                "Loading history…".to_owned()
            } else if draw_findings {
                back_pages.clone()
            } else if view.page.ui.evidence_inspection.is_some() {
                "↑↓ scroll · PgUp/PgDn page · Esc back".to_owned()
            } else if evaluation_details(view) && telemetry_details(view) {
                format!(
                    "{} · {} · {back_pages}",
                    keymap::hint(&[keymap::id::FILE_EDITS]),
                    keymap::hint(&[keymap::id::SEARCH])
                )
            } else if evaluation_details(view) || telemetry_details(view) {
                format!("{} · {back_pages}", keymap::hint(&[keymap::id::FILE_EDITS]))
            } else if let HistorySelection::Session(session, _) = &view.selection
                && view.page.reviewing()
            {
                if session.reviewed {
                    keymap::hint(&[
                        keymap::id::COPY,
                        keymap::id::EXPORT,
                        keymap::id::REVIEW_SAVE_TASK,
                        keymap::id::REVIEW_EXCERPT,
                        keymap::id::REVIEW_DRAFT,
                        keymap::id::REVIEW_REFRESH,
                        keymap::id::REVIEW_CONTROLS,
                        keymap::id::FILE_EDITS,
                        keymap::id::REVIEW_NOTES,
                        keymap::id::TRACES,
                        keymap::id::BACK,
                    ])
                } else {
                    keymap::hint(&[
                        keymap::id::COPY,
                        keymap::id::EXPORT,
                        keymap::id::REVIEW_SAVE_TASK,
                        keymap::id::REVIEW_EXCERPT,
                        keymap::id::REVIEW_DRAFT,
                        keymap::id::REVIEW_REFRESH,
                        keymap::id::REVIEW_CONTROLS,
                        keymap::id::MARK_REVIEWED,
                        keymap::id::FILE_EDITS,
                        keymap::id::REVIEW_NOTES,
                        keymap::id::TRACES,
                        keymap::id::BACK,
                    ])
                }
            } else if has_details(OperationDetailKind::Edit) {
                if has_details(OperationDetailKind::Mcp) {
                    format!(
                        "{} · {} · {back_pages}",
                        keymap::hint(&[keymap::id::FILE_EDITS]),
                        keymap::hint(&[keymap::id::MCP_PAYLOAD])
                    )
                } else {
                    format!("{} · {back_pages}", keymap::hint(&[keymap::id::FILE_EDITS]))
                }
            } else if has_details(OperationDetailKind::Mcp) {
                format!(
                    "{} · {back_pages}",
                    keymap::hint(&[keymap::id::MCP_PAYLOAD])
                )
            } else {
                back_pages
            };
            let footer = Rect::new(0, area.height.saturating_sub(1), area.width, 1);
            let status = if let Some(index) = view.page.review_control {
                format!(
                    "Review controls › {} · Tab/Shift+Tab · Enter · {status}",
                    REVIEW_CONTROLS
                        .get(index)
                        .map(|(label, _)| *label)
                        .unwrap_or("Earlier")
                )
            } else if view.show_evidence
                && workspace.message.is_none()
                && workspace.pending.is_none()
            {
                format!("B chart / series · {status}")
            } else {
                visible_text(&status)
            };
            frame.render_widget(Paragraph::new(status), footer);
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
    if nanos == 0 {
        return "Unknown".into();
    }
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
            | HistorySelection::McpTraces { .. }
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
            native_context: None,
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
            worker_id: None,
            pending_requests: Vec::new(),
        }
    }

    fn catalog(sessions: Vec<RecordedSession>) -> SessionCatalog {
        SessionCatalog {
            sessions,
            older: None,
            notice: None,
            // Fixed far-future boundary: a page test opts into outcomes by
            // pinning its own day start.
            day_start_unix_ns: u64::MAX,
        }
    }

    fn catalog_at(sessions: Vec<RecordedSession>, day_start_unix_ns: u64) -> SessionCatalog {
        SessionCatalog {
            sessions,
            older: None,
            notice: None,
            day_start_unix_ns,
        }
    }

    fn session_history_page(titles: &[&str]) -> HistoryPage {
        let panes = titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let id =
                    agenturm_kernel::ExtensionId::parse(format!("app.session-{index}")).unwrap();
                let pane = agenturm_kernel::PaneModel {
                    title: (*title).into(),
                    rows: vec![MetricRow {
                        label: format!("{title} field"),
                        value: format!("{title} value"),
                    }],
                };
                (id, pane)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let initial_pane = panes.keys().next().cloned();
        HistoryPage {
            features: Default::default(),
            history_identity: None,
            snapshot: Snapshot {
                panes,
                ..Snapshot::default()
            },
            turns: Vec::new(),
            initial_pane,
            review_pane: None,
            operation_details: Vec::new(),
            private_telemetry_history: None,
            operation_tables: Default::default(),
            evidence_tables: Default::default(),
        }
    }

    fn empty_history_page() -> HistoryPage {
        HistoryPage {
            features: Default::default(),
            history_identity: None,
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
    fn trace_span_picker_cancel_and_jump_restore_exact_filtered_origin() {
        let traces_id = ExtensionId::parse("app.session-traces").unwrap();
        let operations_id = ExtensionId::parse("app.session-operations").unwrap();
        let target = |span: &str, operation: &str, worker: &str, turn: &str| TraceOperationTarget {
            trace_id: "trace-2".into(),
            span_id: span.into(),
            source_id: "harness.codex".into(),
            session_id: "session-2".into(),
            worker_id: Some(worker.into()),
            turn_id: Some(turn.into()),
            operation_id: operation.into(),
        };
        let targets = vec![
            vec![target("span-a", "op-a", "worker-a", "turn-a")],
            vec![
                target("span-b", "op-b", "worker-b", "turn-b"),
                target("span-c", "op-c", "worker-c", "turn-c"),
            ],
            vec![target("span-d", "op-d", "worker-d", "turn-d")],
        ];
        let trace_rows = ["trace-1", "trace-2", "trace-3"]
            .into_iter()
            .map(|trace| EvidenceRow {
                cells: vec!["Ok".into(), trace.into(), "2".into(), "—".into()],
                details: vec![MetricRow {
                    label: "Trace".into(),
                    value: trace.into(),
                }],
            })
            .collect::<Vec<_>>();
        let mut trace_table = EvidenceSpec {
            title: "Traces",
            inspector: "Trace details",
            columns: &[("Status", 10), ("Trace", 34), ("Spans", 8), ("Tokens", 10)],
            empty: "No traces",
        }
        .table(trace_rows);
        trace_table.operation_targets = targets;

        let operations = [
            ("op-a", "worker-a", "turn-a"),
            ("op-b", "worker-b", "turn-b"),
            ("op-c", "worker-c", "turn-c"),
            ("op-d", "worker-d", "turn-d"),
        ]
        .into_iter()
        .map(|(operation, worker, turn)| OperationRow {
            time: "Recorded".into(),
            server: None,
            tool: operation.into(),
            result: Some("succeeded".into()),
            latency: None,
            evidence: vec![MetricRow {
                label: "Operation detail".into(),
                value: operation.into(),
            }],
            mcp_event: None,
            identity: Some(OperationIdentity {
                source_id: "harness.codex".into(),
                session_id: "session-2".into(),
                worker_id: Some(worker.into()),
                turn_id: Some(turn.into()),
                operation_id: operation.into(),
            }),
        })
        .collect();

        let mut page = empty_history_page();
        page.snapshot.panes.insert(
            traces_id.clone(),
            agenturm_kernel::PaneModel {
                title: "Traces".into(),
                rows: vec![
                    MetricRow {
                        label: "Trace".into(),
                        value: "trace-1".into(),
                    },
                    MetricRow {
                        label: "Trace".into(),
                        value: "trace-2".into(),
                    },
                    MetricRow {
                        label: "Trace".into(),
                        value: "trace-3".into(),
                    },
                ],
            },
        );
        page.snapshot.panes.insert(
            operations_id.clone(),
            agenturm_kernel::PaneModel {
                title: "Operations".into(),
                rows: Vec::new(),
            },
        );
        page.initial_pane = Some(traces_id.clone());
        page.evidence_tables.insert(traces_id, trace_table);
        page.operation_tables.insert(operations_id, operations);

        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Operations),
            page,
        ));
        let (sender, _) = mpsc::channel();
        {
            let view = workspace.history.last_mut().unwrap();
            view.page.ui.set_viewport(120, 32);
            assert!(view.page.ui.evidence_filter_key(KeyCode::Char('/')));
            for ch in "trace".chars() {
                assert!(view.page.ui.evidence_filter_key(KeyCode::Char(ch)));
            }
            assert!(view.page.ui.evidence_filter_key(KeyCode::Enter));
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Down));
        {
            let view = workspace.history.last().unwrap();
            assert_eq!(view.page.ui.selected_title(), Some("Traces"));
            assert_eq!(view.page.ui.scroll, 1);
            assert_eq!(
                view.page.ui.selected_evidence_export().unwrap().1.cells[1],
                "trace-2"
            );
        }

        assert!(!press(&mut workspace, &sender, KeyCode::Char('o')));
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let picker = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for label in [
            "span span-b",
            "operation op-b",
            "source harness.codex",
            "session session-2",
            "worker worker-b",
            "turn turn-b",
        ] {
            assert!(
                picker.contains(label),
                "missing {label} from picker:\n{picker}"
            );
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Down));
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        {
            let view = workspace.history.last().unwrap();
            assert_eq!(view.page.ui.selected_title(), Some("Traces"));
            assert_eq!(view.page.ui.scroll, 1);
            assert_eq!(
                view.page.ui.selected_evidence_export().unwrap().1.cells[1],
                "trace-2"
            );
            assert!(
                view.page
                    .ui
                    .evidence_filter_label()
                    .unwrap()
                    .contains("trace")
            );
        }

        assert!(!press(&mut workspace, &sender, KeyCode::Char('o')));
        assert!(!press(&mut workspace, &sender, KeyCode::Down));
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        {
            let view = workspace.history.last().unwrap();
            assert_eq!(view.page.ui.selected_title(), Some("Operations"));
            assert_eq!(
                view.page
                    .operation_table
                    .active_ref(&view.page.ui)
                    .unwrap()
                    .selected_row()
                    .unwrap()
                    .identity
                    .as_ref()
                    .unwrap()
                    .operation_id,
                "op-c"
            );
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert!(
            workspace
                .history
                .last()
                .unwrap()
                .page
                .operation_table
                .active_ref(&workspace.history.last().unwrap().page.ui)
                .unwrap()
                .expanded()
        );
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(
            !workspace
                .history
                .last()
                .unwrap()
                .page
                .operation_table
                .active_ref(&workspace.history.last().unwrap().page.ui)
                .unwrap()
                .expanded()
        );
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        let view = workspace.history.last().unwrap();
        assert_eq!(view.page.ui.selected_title(), Some("Traces"));
        assert_eq!(view.page.ui.scroll, 1);
        assert_eq!(
            view.page.ui.selected_evidence_export().unwrap().1.cells[1],
            "trace-2"
        );
        assert!(
            view.page
                .ui
                .evidence_filter_label()
                .unwrap()
                .contains("trace")
        );
    }

    #[test]
    fn review_session_edits_keep_the_pinned_occurrence_and_return_to_decisions() {
        let (sender, requests) = mpsc::channel();
        let mut session = session("harness.codex.fixture", "selected", "completed", None);
        session.history_identity = Some("review-history".into());
        let pane = ExtensionId::parse("app.session-review").unwrap();
        let mut review = empty_history_page();
        review.snapshot.history_snapshot_id = session.snapshot_id;
        review.history_identity = session.history_identity.clone();
        review.initial_pane = Some(pane.clone());
        review.review_pane = Some(pane.clone());
        review.snapshot.panes.insert(
            pane,
            agenturm_kernel::PaneModel {
                title: "Review".into(),
                rows: Vec::new(),
            },
        );
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(session.clone(), SessionView::Review),
            review,
        ));
        press(&mut workspace, &sender, KeyCode::Char('e'));
        assert!(matches!(requests.try_recv(), Ok(WorkspaceRequest::History {
            selection: HistorySelection::ReviewEdits(row), before: None, ..
        }) if *row == session));

        let mut page = empty_history_page();
        page.history_identity = session.history_identity.clone();
        page.snapshot.history_snapshot_id = session.snapshot_id;
        page.snapshot.observations_truncated = true;
        let cursor = ObservationPosition {
            observed_at_unix_ns: 5,
            harness_id: session.harness_id.clone(),
            event_id: "first-unrelated".into(),
            revision: 0,
            snapshot_id: session.snapshot_id,
        };
        page.features.continuation = Some(Box::new(cursor.clone()));
        workspace.finish(Ok(WorkspaceResponse::History(page)));
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("No eligible edits in this range"));
        assert!(rendered.contains("Evidence 7 · Snapshot 9") && rendered.contains("] older"));
        press(&mut workspace, &sender, KeyCode::Char(']'));
        assert!(matches!(requests.try_recv(), Ok(WorkspaceRequest::History {
            selection: HistorySelection::ReviewEdits(row), before: Some(before), ..
        }) if *row == session && before == cursor));

        let mut page = empty_history_page();
        page.history_identity = session.history_identity.clone();
        page.snapshot.history_snapshot_id = session.snapshot_id;
        page.features.review_edit_preview = true;
        page.operation_details = [("turn-b", "edit-b", 4), ("turn-a", "edit-a", 2)]
            .into_iter()
            .map(|(turn, event, revision)| OperationDetailEntry {
                kind: OperationDetailKind::Edit,
                source_id: session.harness_id.clone(),
                session_id: session.session_id.clone(),
                turn_id: turn.into(),
                event_id: event.into(),
                revision,
                operation_id: "same-operation".into(),
                label: "One file changed".into(),
            })
            .collect();
        workspace.finish(Ok(WorkspaceResponse::History(page)));
        press(&mut workspace, &sender, KeyCode::Down);
        press(&mut workspace, &sender, KeyCode::Enter);
        assert!(matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(
            WorkCommand::RevealReviewEdit(entry, _)
        )) if entry.operation.event_id == "edit-a" && entry.operation.turn_id == "turn-a"
            && entry.operation.revision == 2 && entry.operation.source_id == session.harness_id
            && entry.operation.session_id == session.session_id && entry.snapshot_id == session.snapshot_id
            && Some(&entry.history_identity) == session.history_identity.as_ref()));
        workspace.finish(Err("No current edit approval".into()));
        assert_eq!(
            workspace
                .history
                .last()
                .unwrap()
                .page
                .operation_picker
                .as_ref()
                .unwrap()
                .1
                .selected(),
            Some(1)
        );
        assert!(workspace.loaded_view().is_none());
        press(&mut workspace, &sender, KeyCode::Esc);
        assert!(workspace.history.last().unwrap().page.reviewing());
        press(&mut workspace, &sender, KeyCode::Char('n'));
        assert!(matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(
            WorkCommand::ReviewNotes(row)
        )) if row == session));
    }

    #[test]
    fn fully_hidden_review_page_keeps_exact_older_continuation() {
        let (sender, requests) = mpsc::channel();
        let mut workspace = Workspace::new();
        let mut page = empty_history_page();
        page.snapshot.observations_truncated = true;
        page.snapshot.history_snapshot_id = 8.into();
        let continuation = ObservationPosition {
            observed_at_unix_ns: 50,
            harness_id: "harness.codex.review-1".into(),
            event_id: "internal-review-1".into(),
            revision: 0,
            snapshot_id: page.snapshot.history_snapshot_id,
        };
        page.features.continuation = Some(Box::new(continuation.clone()));
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Activity),
            page,
        ));
        press(&mut workspace, &sender, keymap::OLDER);
        assert!(matches!(requests.try_recv(), Ok(WorkspaceRequest::History {
            selection: HistorySelection::All(WorkspaceView::Activity),
            before: Some(cursor),
            include_reviews: false,
        }) if cursor == continuation));
    }

    #[test]
    fn internal_review_visibility_is_explicit_and_does_not_change_selected_agent() {
        let (sender, requests) = mpsc::channel();
        let mut workspace = Workspace::new();
        let selected = session("harness.codex", "selected", "waiting", None);
        workspace.agents.update(catalog(vec![selected.clone()]));
        press(&mut workspace, &sender, KeyCode::Char('2'));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                include_reviews: false,
                ..
            })
        ));
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        press(&mut workspace, &sender, KeyCode::Char('v'));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::ReviewVisibility(true))
        ));
        assert!(!workspace.include_reviews);
        workspace.finish(Ok(WorkspaceResponse::ReviewVisibility(true)));
        let (reload, change) = workspace.deferred_request.take().unwrap();
        assert!(matches!(
            reload,
            WorkspaceRequest::History {
                include_reviews: true,
                selection: HistorySelection::All(WorkspaceView::Activity),
                ..
            }
        ));
        assert!(workspace.history.is_empty());
        assert_eq!(workspace.agents.current(), Some(&selected));
        workspace.request(&sender, reload, change);
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        assert!(workspace.include_reviews);
    }

    #[test]
    fn metric_read_cancellation_and_export_keep_the_current_scope() {
        use agenturm_kernel::metric::{MetricKind, MetricSeries, MetricTemporality};
        use agenturm_kernel::retained_history::{
            MetricHistoryPage, MetricHistoryQuery, MetricSeriesEntry, MetricSeriesKey,
        };
        let (sender, requests) = mpsc::channel();
        let snapshot = 5.into();
        let source = ExtensionId::parse("otlp.fixture").unwrap();
        let key = MetricSeriesKey {
            source_id: source,
            series_id: "1".repeat(64),
        };
        let series = MetricSeries {
            id: key.series_id.clone(),
            name: "fixture.latency".into(),
            unit: "ms".into(),
            service: None,
            scope: None,
            kind: MetricKind::Gauge,
            temporality: MetricTemporality::Unspecified,
            monotonic: None,
        };
        let mut page = empty_history_page();
        page.snapshot.history_snapshot_id = snapshot;
        page.features.metric_history = Some(Box::new(MetricHistoryPage::Series {
            snapshot_id: snapshot,
            rows: vec![MetricSeriesEntry {
                key: key.clone(),
                series,
                latest_unix_ns: 20,
                description: None,
            }],
            next: None,
        }));
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Metrics),
            page,
        ));
        assert_eq!(
            workspace.loaded_view().unwrap().rows[0].label,
            "fixture.latency"
        );
        press(&mut workspace, &sender, KeyCode::Char('/'));
        for ch in "historical description".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert!(requests.try_recv().is_err());
        press(&mut workspace, &sender, KeyCode::Enter);
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::MetricHistory(query))
            if matches!(query.as_ref(), MetricHistoryQuery::Series {
                search: Some(search), snapshot_id: Some(at), after: None, ..
            } if search == "historical description" && *at == snapshot))
        );
        press(&mut workspace, &sender, KeyCode::Esc);
        workspace.finish(Err("Cancelled search must not replace the catalog".into()));
        assert!(workspace.message.is_none());
        assert_eq!(
            workspace.loaded_view().unwrap().rows[0].label,
            "fixture.latency"
        );
        press(&mut workspace, &sender, KeyCode::Enter);
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::MetricHistory(query))
            if matches!(query.as_ref(), MetricHistoryQuery::Points { series, snapshot_id, .. } if series == &key && *snapshot_id == snapshot))
        );
        press(&mut workspace, &sender, KeyCode::Esc);
        assert_eq!(workspace.history.len(), 1);
        workspace.finish(Err("Cancelled read must not replace the view".into()));
        assert!(workspace.message.is_none());
        assert_eq!(
            workspace.loaded_view().unwrap().title,
            "Metrics · OTLP series"
        );
        press(&mut workspace, &sender, KeyCode::Char('B'));
        assert_eq!(workspace.loaded_view().unwrap().title, "Metrics");
        press(&mut workspace, &sender, KeyCode::Char('B'));
        assert_eq!(
            workspace.loaded_view().unwrap().rows[0].label,
            "fixture.latency"
        );
    }

    #[test]
    fn mcp_trace_open_pins_the_selected_call_and_returns_to_it() {
        let (sender, requests) = mpsc::channel();
        let source = "harness.codex";
        let pane = ExtensionId::parse("app.workspace-mcp").unwrap();
        let snapshot = 8.into();
        let mut page = empty_history_page();
        page.snapshot.history_snapshot_id = snapshot;
        page.snapshot.panes.insert(
            pane.clone(),
            agenturm_kernel::PaneModel {
                title: "MCP".into(),
                rows: Vec::new(),
            },
        );
        page.initial_pane = Some(pane.clone());
        page.operation_tables.insert(
            pane,
            [("event-1", 13), ("event-2", 5)]
                .into_iter()
                .map(|(event, revision)| OperationRow {
                    time: "Recorded".into(),
                    server: Some("fixture".into()),
                    tool: "call".into(),
                    result: None,
                    latency: None,
                    evidence: Vec::new(),
                    mcp_event: Some((source.into(), event.into(), revision)),
                    identity: None,
                })
                .collect(),
        );
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Mcp),
            page,
        ));
        press(&mut workspace, &sender, KeyCode::Down);
        press(&mut workspace, &sender, KeyCode::Char('t'));
        let WorkspaceRequest::History { selection, .. } = requests.try_recv().unwrap() else {
            panic!("MCP traces query")
        };
        assert!(
            matches!(&selection, HistorySelection::McpTraces { source_id, event_id, revision: 13, snapshot_id }
            if source_id.as_str() == source && event_id == "event-1" && *snapshot_id == snapshot)
        );
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        assert_eq!(workspace.history.len(), 2);
        press(&mut workspace, &sender, KeyCode::Esc);
        press(&mut workspace, &sender, KeyCode::Char('t'));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::History { selection: HistorySelection::McpTraces { event_id, revision: 13, .. }, .. }) if event_id == "event-1")
        );
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('/')));
        for ch in "query type".chars() {
            assert!(!press(&mut workspace, &sender, KeyCode::Char(ch)));
        }
        assert!(requests.try_recv().is_err());
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::McpSearch {
                    scope: McpSearchScope::All,
                    query,
                    snapshot_id,
                },
                before: None,
                ..
            }) if query == "query type" && snapshot_id == snapshot
        ));
        workspace.finish(Ok(WorkspaceResponse::History(empty_history_page())));
        assert_eq!(workspace.history.len(), 2);
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('t')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::McpTraces { event_id, revision: 13, .. },
                ..
            }) if event_id == "event-1"
        ));
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
    fn shutdown_signals_an_active_load_and_skips_queued_requests() {
        let stop = AtomicBool::new(false);
        let (request, requests) = mpsc::channel();
        let (send, results) = mpsc::channel();
        let (started, first_started) = mpsc::channel();
        let (release, wait_for_exit) = mpsc::channel();
        request.send(WorkspaceRequest::Sessions(None)).unwrap();
        request.send(WorkspaceRequest::Sessions(None)).unwrap();
        drop(request);
        std::thread::scope(|scope| {
            let stop = &stop;
            scope.spawn(move || {
                load_workspace_requests(requests, send, stop, |_| {
                    started.send(()).unwrap();
                    if wait_for_exit.recv_timeout(Duration::from_secs(5)).is_err() {
                        return Err("test exit signal not received".into());
                    }
                    assert!(stop.load(Ordering::Acquire));
                    Ok(WorkspaceResponse::ReviewSaved)
                });
            });
            first_started.recv_timeout(Duration::from_secs(5)).unwrap();
            drop(StopWorkspaceLoader(stop));
            release.send(()).unwrap();
        });
        assert!(matches!(
            results.try_recv(),
            Ok(Ok(WorkspaceResponse::ReviewSaved))
        ));
        assert!(matches!(
            results.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(matches!(
            first_started.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn retry_changes_only_the_requested_receipt_without_moving_the_workflow_cursor() {
        use agenturm_kernel::workflow_notifications::{WorkflowDelivery, WorkflowDeliveryState};

        let selected = RequestEntry {
            owner: WorkAnchor {
                source: "harness.codex.fixture".into(),
                session: "fixture-session".into(),
                worker: None,
                turn: Some("fixture-turn".into()),
            },
            event_id: "selected-approval".into(),
            revision: 3,
            timestamp: 1,
            reason: "approval".into(),
            seen: false,
            proof: RequestProof {
                snapshot_id: 99.into(),
                history_identity: Some("fixture-history".into()),
            },
        };
        let mut sibling = selected.clone();
        sibling.event_id = "sibling-approval".into();
        let delivery = WorkflowDelivery {
            authority: WorkflowNotificationAuthority {
                history_identity: "fixture-history".into(),
                snapshot_id: 99.into(),
            },
            key: [1; 32],
            policy_revision: 7,
            attempt: 1,
            state: WorkflowDeliveryState::Failed,
        };
        let mut sibling_delivery = delivery.clone();
        sibling_delivery.key = [2; 32];
        let mut workspace = Workspace::new();
        workspace.work = Some(Workbench::new(
            WorkResult::Workflow {
                items: vec![sibling.clone(), selected.clone()],
                partial: false,
                deliveries: vec![
                    (sibling, sibling_delivery.clone()),
                    (selected, delivery.clone()),
                ],
            },
            None,
        ));
        let (sender, requests) = mpsc::channel();
        let retry = |workspace: &mut Workspace| {
            press(workspace, &sender, KeyCode::Char('T'));
            match requests.try_recv() {
                Ok(WorkspaceRequest::Work(WorkCommand::RetryWorkflow {
                    authority,
                    key,
                    expected_revision,
                    expected_attempt,
                })) => Some((authority, key, expected_revision, expected_attempt)),
                Err(mpsc::TryRecvError::Empty) => None,
                _ => panic!("unexpected request"),
            }
        };
        let response = |delivery: WorkflowDelivery| {
            Ok(WorkspaceResponse::Work(
                WorkResult::WorkflowDeliveryUpdated(delivery),
            ))
        };
        let expected = |attempt| {
            Some((
                delivery.authority.clone(),
                delivery.key,
                delivery.policy_revision,
                attempt,
            ))
        };
        press(&mut workspace, &sender, KeyCode::Down);
        workspace.pending = Some(Change::Work(None));
        let mut compact = Terminal::new(TestBackend::new(60, 18)).unwrap();
        compact
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        assert!(compact.backend().to_string().contains("Delivery Failed"));
        workspace.finish(response(delivery.clone()));
        assert_eq!(retry(&mut workspace), expected(1));
        sibling_delivery.attempt = 2;
        workspace.finish(response(sibling_delivery.clone()));
        assert_eq!(retry(&mut workspace), expected(1));
        let mut next_failure = delivery.clone();
        next_failure.attempt = 2;
        let mut stale = next_failure.clone();
        stale.authority.snapshot_id = 98.into();
        workspace.finish(response(stale));
        assert_eq!(retry(&mut workspace), expected(1));
        let mut stale = next_failure.clone();
        stale.policy_revision -= 1;
        workspace.finish(response(stale));
        assert_eq!(retry(&mut workspace), expected(1));
        workspace.finish(response(delivery.clone()));
        assert_eq!(retry(&mut workspace), expected(1));
        let mut ahead = next_failure.clone();
        ahead.attempt += 1;
        workspace.finish(response(ahead));
        assert_eq!(retry(&mut workspace), expected(1));
        workspace.finish(response(next_failure.clone()));
        assert_eq!(retry(&mut workspace), expected(2));
        workspace.finish(response(next_failure.clone()));
        assert_eq!(retry(&mut workspace), expected(2));
        next_failure.attempt = 3;
        next_failure.state = WorkflowDeliveryState::Delivered;
        workspace.finish(response(next_failure));
        assert_eq!(
            workspace.message.as_deref(),
            Some("Notification retry · Delivered")
        );
        assert_eq!(retry(&mut workspace), None);
        compact
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        assert!(compact.backend().to_string().contains("Delivery Delivered"));
        press(&mut workspace, &sender, KeyCode::Up);
        assert_eq!(
            retry(&mut workspace),
            Some((
                sibling_delivery.authority,
                sibling_delivery.key,
                sibling_delivery.policy_revision,
                1,
            ))
        );
    }

    #[test]
    fn workflow_resume_copies_only_the_exact_requested_row_and_preserves_selection() {
        let selected = RequestEntry {
            owner: WorkAnchor {
                source: "harness.codex.fixture".into(),
                session: "fixture-session".into(),
                worker: None,
                turn: Some("fixture-turn".into()),
            },
            event_id: "selected-approval".into(),
            revision: 3,
            timestamp: 1,
            reason: "approval".into(),
            seen: false,
            proof: RequestProof {
                snapshot_id: 99.into(),
                history_identity: Some("fixture-history".into()),
            },
        };
        let mut sibling = selected.clone();
        sibling.owner.source = "harness.codex.sibling".into();
        sibling.revision = 4;
        let response = |request| {
            Ok(WorkspaceResponse::Work(WorkResult::WorkflowResume(
                Box::new(crate::WorkflowResume {
                    request,
                    runtime: crate::ResumeRuntime::Codex,
                }),
            )))
        };
        let (sender, requests) = mpsc::channel();
        let request = |workspace: &mut Workspace| {
            assert!(!press(workspace, &sender, KeyCode::Char('c')));
            let WorkspaceRequest::Work(WorkCommand::WorkflowResume(item)) =
                requests.try_recv().unwrap()
            else {
                panic!("expected exact native resume request");
            };
            item
        };
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut workspace = Workspace::new();
            workspace.work = Some(Workbench::new(
                WorkResult::Workflow {
                    items: vec![sibling.clone(), selected.clone()],
                    partial: false,
                    deliveries: Vec::new(),
                },
                None,
            ));
            assert!(!press(&mut workspace, &sender, KeyCode::Down));
            assert_eq!(request(&mut workspace), selected);
            assert!(!press(&mut workspace, &sender, KeyCode::Up));
            workspace.finish(response(sibling.clone()));
            assert!(workspace.view_io.clipboard.is_none());
            assert!(!press(&mut workspace, &sender, KeyCode::Down));
            assert_eq!(request(&mut workspace), selected);
            let mut wrong_proof = selected.clone();
            wrong_proof.proof.snapshot_id = 100.into();
            workspace.finish(response(wrong_proof));
            assert!(workspace.view_io.clipboard.is_none());
            assert_eq!(request(&mut workspace), selected);
            workspace.finish(response(selected.clone()));
            assert_eq!(
                workspace.view_io.clipboard.take(),
                Some(
                    crate::loaded_view::clipboard_request("codex resume fixture-session").unwrap()
                )
            );
            terminal
                .draw(|frame| draw_workspace(frame, &mut workspace))
                .unwrap();
            let display = terminal.backend().to_string();
            assert!(display.contains("Native resume command"));
            assert!(display.contains("codex resume fixture-session"));
            assert!(!press(&mut workspace, &sender, KeyCode::Down));
            assert!(!press(&mut workspace, &sender, KeyCode::Esc));
            assert_eq!(request(&mut workspace), selected);
            assert!(!press(&mut workspace, &sender, KeyCode::Esc));
            workspace.finish(response(selected.clone()));
            assert!(workspace.view_io.clipboard.is_none());
        }
    }

    #[test]
    fn finding_source_uses_the_exact_occurrence_and_redacts_before_return() {
        use std::io::Write;
        let pane = ExtensionId::parse("app.workspace-findings").unwrap();
        let source = ExtensionId::parse("harness.codex.fixture").unwrap();
        let reference = "opaque-checked-reference";
        let pinned = 72.into();
        let mut workspace = Workspace::new();
        let mut page = empty_history_page();
        page.history_identity = Some("finding-history".into());
        page.snapshot.history_snapshot_id = pinned;
        page.initial_pane = Some(pane.clone());
        page.snapshot.panes.insert(
            pane.clone(),
            agenturm_kernel::PaneModel {
                title: "Findings".into(),
                rows: Vec::new(),
            },
        );
        let occurrence = |id: &str, protected| crate::FindingOccurrence {
            position: ObservationPosition {
                observed_at_unix_ns: 12,
                harness_id: source.as_str().into(),
                event_id: id.into(),
                revision: 1,
                snapshot_id: pinned,
            },
            identity: agenturm_kernel::ObservationIdentity::session("fixture-session"),
            evaluation: None,
            source: protected,
            details: Vec::new(),
        };
        page.features.findings = Some((
            pane,
            crate::FindingsPage {
                groups: vec![crate::FindingGroup {
                    source_id: source.clone(),
                    kind: "Check",
                    severity: "error".into(),
                    signal: "Failed check".into(),
                    occurrences: vec![
                        occurrence("unsupported-sibling", None),
                        occurrence(
                            "selected-check",
                            Some(crate::FindingSource::Command(reference.into())),
                        ),
                    ],
                }],
                snapshot_id: pinned,
                scanned: 2,
                has_older: false,
                evaluation_partial: false,
            },
        ));
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Findings),
            page,
        ));
        let (sender, requests) = mpsc::channel();
        let key = |workspace: &mut Workspace, code| {
            assert!(!press(workspace, &sender, code));
        };
        key(&mut workspace, KeyCode::Char('s'));
        key(&mut workspace, KeyCode::Enter);
        key(&mut workspace, KeyCode::Enter);
        key(&mut workspace, KeyCode::Char('s'));
        assert!(requests.try_recv().is_err());
        assert_eq!(
            workspace.message.as_deref(),
            Some("Protected source unavailable for this finding")
        );
        key(&mut workspace, KeyCode::Esc);
        key(&mut workspace, KeyCode::Down);
        key(&mut workspace, KeyCode::Enter);
        let request = |workspace: &mut Workspace| {
            key(workspace, KeyCode::Char('s'));
            let WorkspaceRequest::Work(WorkCommand::RevealInvestigation {
                reference: selected,
                history_identity,
                delivery,
            }) = requests.try_recv().unwrap()
            else {
                panic!("expected gated finding source reveal")
            };
            assert_eq!(selected, reference);
            assert_eq!(history_identity.as_deref(), Some("finding-history"));
            delivery
        };

        let mut cancelled = request(&mut workspace);
        key(&mut workspace, KeyCode::Esc);
        assert_eq!(
            cancelled.flush().unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        workspace.finish(Err("late callback failure".into()));
        assert!(workspace.message.is_none());

        let mut delivery = request(&mut workspace);
        let body = "synthetic-granted-finding-command";
        serde_json::to_writer(&mut delivery, body).unwrap();
        let send = std::thread::spawn(move || delivery.flush());
        let receiver = workspace.prompt_receiver.take().unwrap();
        let bytes = receiver
            .frames
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let text: String = serde_json::from_slice(&bytes).unwrap();
        workspace.prompt = Some(PromptScreen::new(text, receiver, Instant::now()));
        key(&mut workspace, KeyCode::Char('c'));
        key(&mut workspace, KeyCode::Char('x'));
        assert!(workspace.view_io.clipboard.is_none());
        assert!(workspace.view_io.form.is_none());
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        assert!(terminal.backend().to_string().contains(body));
        key(&mut workspace, KeyCode::Esc);
        assert!(!send.is_finished());
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let redacted = terminal.backend().to_string();
        assert!(!redacted.contains(body));
        assert!(!redacted.contains(reference));
        assert!(redacted.contains("selected-check"));
        workspace.acknowledge_redaction();
        assert!(send.join().unwrap().is_ok());
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Status(
            String::new(),
        ))));
        let selected = workspace
            .history
            .last_mut()
            .unwrap()
            .page
            .active_findings()
            .and_then(|browser| browser.inspected_occurrence())
            .unwrap();
        assert_eq!(selected.position.event_id, "selected-check");
        assert!(
            !workspace
                .loaded_view()
                .unwrap()
                .plain_text()
                .unwrap()
                .contains(reference)
        );
    }

    #[test]
    fn selected_log_requires_the_page_history_and_redacts_before_acknowledgement() {
        use std::io::Write;
        let pane = ExtensionId::parse("app.workspace-findings").unwrap();
        let source_id = ExtensionId::parse("otlp.logs").unwrap();
        let event_id = format!("otlp-error-log:{}", "a".repeat(64));
        let snapshot_id = 83.into();
        let row_identity = agenturm_kernel::ObservationIdentity::source();
        let log = crate::TelemetryLogEntry {
            history_identity: "history-identity".into(),
            source_id: source_id.clone(),
            native_source_id: None,
            event_id: event_id.clone(),
            revision: 0,
            observed_at_unix_ns: 0,
            identity: row_identity.clone(),
            snapshot_id,
        };
        let occurrence = |selected: crate::TelemetryLogEntry| crate::FindingOccurrence {
            position: ObservationPosition {
                observed_at_unix_ns: 0,
                harness_id: source_id.as_str().into(),
                event_id: event_id.clone(),
                revision: 0,
                snapshot_id,
            },
            identity: row_identity.clone(),
            evaluation: None,
            details: Vec::new(),
            source: Some(crate::FindingSource::Log(Box::new(selected))),
        };
        let mut wrong_history = log.clone();
        wrong_history.history_identity = "other-history".into();
        let mut wrong_scope = log.clone();
        wrong_scope.identity = agenturm_kernel::ObservationIdentity::session("unrelated");
        let mut page = empty_history_page();
        page.history_identity = Some(log.history_identity.clone());
        page.snapshot.history_snapshot_id = snapshot_id;
        page.initial_pane = Some(pane.clone());
        page.snapshot.panes.insert(
            pane.clone(),
            agenturm_kernel::PaneModel {
                title: "Findings".into(),
                rows: Vec::new(),
            },
        );
        page.features.findings = Some((
            pane,
            crate::FindingsPage {
                groups: vec![crate::FindingGroup {
                    source_id: source_id.clone(),
                    kind: "Error log",
                    severity: "error".into(),
                    signal: "Recorded error log".into(),
                    occurrences: vec![
                        occurrence(wrong_history),
                        occurrence(wrong_scope),
                        occurrence(log.clone()),
                    ],
                }],
                snapshot_id,
                scanned: 3,
                has_older: false,
                evaluation_partial: false,
            },
        ));
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Findings),
            page,
        ));
        let (sender, requests) = mpsc::channel();
        let key = |workspace: &mut Workspace, code| assert!(!press(workspace, &sender, code));
        key(&mut workspace, KeyCode::Enter);
        for _ in 0..2 {
            key(&mut workspace, KeyCode::Enter);
            key(&mut workspace, KeyCode::Char('s'));
            assert!(requests.try_recv().is_err());
            key(&mut workspace, KeyCode::Esc);
            key(&mut workspace, KeyCode::Down);
        }
        key(&mut workspace, KeyCode::Enter);
        key(&mut workspace, KeyCode::Char('s'));
        let WorkspaceRequest::Work(WorkCommand::RevealTelemetryLog(selected, mut delivery)) =
            requests.try_recv().unwrap()
        else {
            panic!("expected exact log source request");
        };
        assert_eq!(*selected, log);
        let body = "synthetic private log text";
        serde_json::to_writer(&mut delivery, body).unwrap();
        let send = std::thread::spawn(move || delivery.flush());
        let receiver = workspace.prompt_receiver.take().unwrap();
        let bytes = receiver
            .frames
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        workspace.prompt = Some(PromptScreen::new(
            serde_json::from_slice::<String>(&bytes).unwrap(),
            receiver,
            Instant::now(),
        ));
        for code in [KeyCode::Char('c'), KeyCode::Char('x')] {
            key(&mut workspace, code);
        }
        assert!(workspace.view_io.clipboard.is_none());
        assert!(workspace.view_io.form.is_none());
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        assert!(terminal.backend().to_string().contains(body));
        key(&mut workspace, KeyCode::Esc);
        assert!(!send.is_finished());
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let redacted = terminal.backend().to_string();
        assert!(!redacted.contains(body));
        assert!(redacted.contains("Recorded error log"));
        workspace.acknowledge_redaction();
        assert!(send.join().unwrap().is_ok());
        assert!(
            !workspace
                .loaded_view()
                .unwrap()
                .plain_text()
                .unwrap()
                .contains(body)
        );
    }

    #[test]
    fn investigation_keeps_exact_turn_scope_and_returns_to_the_same_inspector() {
        let mut workspace = Workspace::new();
        let mut session = session("harness.pi", "retained-session", "completed", None);
        session.history_identity = Some("catalog-history".into());
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
        page.history_identity = Some("inspector-history".into());
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
        assert_eq!(read.history_identity.as_deref(), Some("inspector-history"));
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
                history_identity: Some("inspector-history".into()),
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
        let WorkspaceRequest::Work(WorkCommand::RevealInvestigation {
            reference,
            history_identity,
            ..
        }) = requests.try_recv().unwrap()
        else {
            panic!("expected an exact protected reveal request");
        };
        assert_eq!(reference, "opaque-reference");
        assert_eq!(history_identity.as_deref(), Some("inspector-history"));
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Status(
            String::new(),
        ))));
        assert!(!press(&mut workspace, &sender, KeyCode::PageDown));
        let WorkspaceRequest::Work(WorkCommand::Investigate(read)) = requests.try_recv().unwrap()
        else {
            panic!("expected a continuation request");
        };
        assert_eq!(read.cursor.as_deref(), Some("older-page"));
        assert_eq!(read.history_identity.as_deref(), Some("inspector-history"));
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
        assert_eq!(read.history_identity.as_deref(), Some("catalog-history"));
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
                ..
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
            matches!(requests.try_recv(), Ok(WorkspaceRequest::History { selection: HistorySelection::Session(row, SessionView::Review), before: None, .. }) if row.session_id == "selected")
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
                guarded_active: Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
        // `m` cycles sections while keeping the tab strip focused; Prompts is
        // an entry point, so Enter/Down opens the UI-04 navigator.
        assert_eq!(workspace.inspector.tab, inspector::Tab::Overview);
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert_eq!(workspace.inspector.tab, inspector::Tab::Prompts);
        assert!(requests.try_recv().is_err());
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(WorkCommand::Prompts(owner))) if owner.source == "harness.pi" && owner.session == "selected")
        );
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Prompts(Vec::new()))));
        assert!(workspace.prompt_nav.is_some());
        assert!(workspace.work.is_none());
        // The UI-04 navigator is an overlay; Esc returns to the same inspector
        // section before a section switch is available.
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(workspace.prompt_nav.is_none());
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::History { selection: HistorySelection::Session(row, SessionView::Details), .. }) if row.session_id == "selected")
        );
        assert_eq!(workspace.inspector.tab, inspector::Tab::Details);
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
            assert!(text.contains("Codex"), "{text}");
            assert!(text.contains("Pi session"), "{text}");
        }
    }

    #[test]
    fn native_names_context_and_duplicates_are_readable_at_every_supported_size() {
        let mut first = session(
            "harness.codex.11111111111111111111111111111111",
            "opaque-one",
            "running",
            None,
        );
        first.task_title = Some("Release checks".into());
        first.project_path = Some("/work/agenturm".into());
        first.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        first.native_context = Some(Box::new(NativeSessionContext {
            agent_name: Some("Mira".into()),
            agent_role: Some("reviewer".into()),
            model_provider: Some("openai".into()),
            ..NativeSessionContext::default()
        }));
        let mut second = first.clone();
        second.session_id = "opaque-two".into();
        second.native_context.as_mut().unwrap().agent_name = Some("Sol".into());
        let mut agents = AgentList::default();
        agents.update(catalog(vec![first, second]));
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
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
                "Release checks",
                "Codex",
                "Mira",
                "Sol",
                "/work/agenturm",
                "Same name",
            ] {
                assert!(
                    text.contains(expected),
                    "{width}x{height}: {expected}: {text}"
                );
            }
            assert!(!text.contains('●'), "{width}x{height}: {text}");
            assert!(!text.contains('○'), "{width}x{height}: {text}");
            assert!(!text.contains("! "), "{width}x{height}: {text}");
            assert!(
                !text.contains("opaque-one"),
                "IDs belong in inspector/copy: {text}"
            );
            assert!(
                !text.contains("opaque-two"),
                "IDs belong in inspector/copy: {text}"
            );
        }
    }

    #[test]
    fn wide_command_center_keeps_task_list_and_selected_details_visible() {
        let mut task = session("harness.codex", "private-session-id", "running", None);
        task.task_title = Some("Repair retry handling".into());
        task.project_path = Some("/work/agenturm".into());
        task.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        task.native_context = Some(Box::new(NativeSessionContext {
            model_provider: Some("openai".into()),
            last_activity: Some("Recorded shell activity".into()),
            ..NativeSessionContext::default()
        }));
        let mut agents = AgentList::default();
        agents.update(catalog(vec![task]));
        let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
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
            "Tasks",
            "Status",
            "Updated",
            "Repair retry handling",
            "Selected task",
            "Project",
            "Native session workspace",
            "/work/agenturm",
            "openai",
            "Recent activity",
            "Recorded shell activity",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert_eq!(text.matches("Repair retry handling").count(), 2, "{text}");
        assert!(!text.contains("private-session-id"), "{text}");
    }

    #[test]
    fn command_center_layout_switches_cleanly_at_the_master_detail_breakpoint() {
        for (width, height, split) in [
            (119, 32, false),
            (120, 32, true),
            (121, 32, true),
            (160, 23, false),
            (160, 24, true),
            (160, 18, false),
        ] {
            let mut task = session("harness.codex", "layout-session", "running", None);
            task.task_title = Some("Keep title readable".into());
            let mut agents = AgentList::default();
            agents.update(catalog(vec![task]));
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
            assert!(text.contains("Keep title readable"), "{width}: {text}");
            assert_eq!(text.contains("Selected task"), split, "{width}: {text}");
        }
    }

    #[test]
    fn command_center_explains_its_minimum_terminal_size() {
        for (width, height) in [(59, 24), (60, 17)] {
            let mut agents = AgentList::default();
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
            assert!(
                text.contains("Terminal too small"),
                "{width}x{height}: {text}"
            );
            assert!(text.contains("60×18"), "{width}x{height}: {text}");
            assert!(
                !text.contains("Agenturm command center"),
                "{width}x{height}: {text}"
            );
        }
    }

    #[test]
    fn show_more_selection_does_not_leave_the_previous_task_in_the_detail_pane() {
        let mut page = catalog(vec![session("harness.codex", "older-row", "running", None)]);
        page.older = Some(SessionCursor {
            harness_id: "harness.codex".into(),
            session_id: "older-row".into(),
            updated_at_unix_ns: 1,
            snapshot_id: 9.into(),
        });
        let mut agents = AgentList::default();
        agents.update(page);
        agents.show_more_selected = true;
        let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
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
        assert!(text.contains("Older tasks"), "{text}");
        assert!(!text.contains("Selected task"), "{text}");
    }

    #[test]
    fn task_titles_inherit_the_same_row_background_as_status_and_context() {
        let mut alpha = session("harness.codex", "alpha", "running", None);
        alpha.task_title = Some("First task".into());
        let mut beta = session("harness.codex", "beta", "completed", None);
        beta.task_title = Some("Second task".into());
        let mut agents = AgentList::default();
        agents.update(catalog(vec![alpha, beta]));
        for (width, height) in [(120, 32), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_agents(frame, &mut agents, false, None))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let row = (0..height)
                .find(|&y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .contains("Second task")
                })
                .expect("the second task is visible");
            let status = (0..width)
                .find(|&x| buffer[(x, row)].symbol() == "R")
                .unwrap();
            assert_eq!(buffer[(8, row)].bg, buffer[(status, row)].bg);
            assert_eq!(buffer[(8, row)].bg, buffer[(8, row + 1)].bg);
        }
    }

    #[test]
    fn native_rename_reorders_rows_without_changing_exact_selection_or_inspector_binding() {
        let mut selected = session("harness.codex.alpha", "shared", "running", None);
        selected.task_title = Some("Before rename".into());
        selected.project_path = Some("/work/z-project".into());
        selected.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        let other = session("harness.codex.beta", "shared", "completed", None);
        let mut workspace = Workspace::new();
        workspace
            .agents
            .update(catalog(vec![selected.clone(), other.clone()]));
        let exact = session_key(&selected);
        workspace.agents.apply_filters(Some(&exact));
        selected.task_title = Some("After rename".into());
        selected.project_path = Some("/work/a-project".into());
        workspace.agents.update(catalog(vec![other, selected]));
        assert_eq!(session_key(workspace.agents.current().unwrap()), exact);
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let Ok(WorkspaceRequest::History {
            selection: HistorySelection::Session(row, SessionView::Overview),
            ..
        }) = requests.try_recv()
        else {
            panic!("expected exact session inspector request")
        };
        assert_eq!(session_key(&row), exact);
        assert_eq!(row.task_title.as_deref(), Some("After rename"));
        assert_eq!(row.project_path.as_deref(), Some("/work/a-project"));
    }

    #[test]
    fn primary_names_prefer_tasks_then_native_nicknames_then_human_context() {
        let mut row = session("harness.codex", "opaque-id", "running", None);
        row.native_context = Some(Box::new(NativeSessionContext {
            agent_name: Some("Nash".into()),
            agent_role: Some("worker".into()),
            ..NativeSessionContext::default()
        }));
        assert_eq!(row.display_name(), "Nash");
        row.task_title = Some("Finish migration".into());
        assert_eq!(row.display_name(), "Finish migration");
        row.task_title = None;
        row.worker_id = Some("opaque-id".into());
        row.native_context.as_mut().unwrap().agent_name = None;
        assert_eq!(row.display_name(), "Codex worker");
        assert!(!row.display_name().contains("opaque-id"));
    }

    #[test]
    fn recency_orders_groups_and_rows_without_status_or_hierarchy_priority() {
        let mut parent = session("harness.codex", "parent", "failed", None);
        parent.updated_at_unix_ns = 10;
        parent.project_path = Some("/work/older-name".into());
        parent.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        let mut child = parent.clone();
        child.session_id = "child".into();
        child.parent_session_id = Some("parent".into());
        child.relationship_kind = Some(RelationshipKind::Delegated);
        child.updated_at_unix_ns = 100;
        child.state = Some("running".into());
        let mut other = session("harness.pi", "other", "completed", None);
        other.updated_at_unix_ns = 50;
        other.project_path = Some("/work/alphabetically-first".into());
        other.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        let mut agents = AgentList::default();
        agents.update(catalog(vec![parent.clone(), other.clone(), child.clone()]));
        let ids = |agents: &AgentList| {
            agents
                .visible
                .iter()
                .map(|item| {
                    agents.catalog.as_ref().unwrap().sessions[item.index]
                        .session_id
                        .clone()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&agents), ["child", "parent", "other"]);
        let identity = session_key(&child);
        agents.grouping = command_center::Grouping::None;
        agents.apply_filters(Some(&identity));
        assert_eq!(ids(&agents), ["child", "other", "parent"]);
        assert_eq!(agents.current().unwrap().session_id, "child");
        agents.update(catalog(vec![other, child, parent]));
        assert_eq!(ids(&agents), ["child", "other", "parent"]);
        assert_eq!(agents.current().unwrap().session_id, "child");
    }

    #[test]
    fn native_context_search_uses_nicknames_without_activity_or_ids() {
        let mut row = session("harness.claude.selected", "opaque-id", "completed", None);
        row.project_path = Some("/work/alpha-project".into());
        row.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        row.native_context = Some(Box::new(NativeSessionContext {
            agent_name: Some("Mira".into()),
            agent_role: Some("reviewer".into()),
            model_provider: Some("anthropic".into()),
            last_activity: Some("MCP tool call".into()),
            ..NativeSessionContext::default()
        }));
        let mut agents = AgentList::default();
        agents.update(catalog(vec![row]));
        for query in ["MIRA", "reviewer", "anthropic", "alpha-project"] {
            agents.search = query.into();
            agents.apply_filters(None);
            assert_eq!(agents.visible.len(), 1, "{query}");
            assert_eq!(command_center::name(agents.current().unwrap()), "Mira");
        }
        agents.search = "absent".into();
        agents.apply_filters(None);
        assert!(agents.visible.is_empty());
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
                    features: Default::default(),
                    history_identity: None,
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
            let WorkspaceRequest::History {
                selection, before, ..
            } = requests.try_recv().unwrap()
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
                features: Default::default(),
                history_identity: None,
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
            // The inspector opens on its tab strip; Enter enters the content
            // reader before Left/Right/Down reach panes or scrolling.
            assert!(!press(&mut workspace, KeyCode::Enter));
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
        let position = workspace
            .agents
            .visible
            .iter()
            .position(|item| {
                workspace.agents.catalog.as_ref().unwrap().sessions[item.index] == selected
            })
            .unwrap();
        workspace.agents.selection.select(Some(position));
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
            selection: HistorySelection::Session(ref session, _), before: None, ..
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
            [(1, 0), (2, 0), (0, 1)]
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
                "Codex session",
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
                "No recorded project",
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
        for (width, height) in [(160, 32), (120, 32), (80, 24), (60, 18)] {
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
            assert!(text.contains("Codex session"));
            assert!(text.contains("Receiver disconnected"));
        }
    }
    #[test]
    fn command_center_help_points_at_the_shared_inventory_at_compact_sizes() {
        // The command center no longer owns a second help body: the shell's
        // scrollable reference is built from keymap::ACTIONS. A direct render
        // still shows a deterministic pointer instead of duplicating text.
        let inventory = keymap::help_sections(keymap::Context::CommandCenter)
            .into_iter()
            .flat_map(|(heading, lines)| std::iter::once(heading.to_owned()).chain(lines))
            .collect::<Vec<_>>()
            .join("\n");
        for key in [
            "Ctrl+F",
            "Mark request seen",
            "Grouping",
            "Refresh now",
            "Saved tasks",
            "Workflow inbox",
            "Copy selected view",
            "Export selected view",
            "Quit",
            "Internal reviews",
            "Choose agent session",
            "Show Today",
            "Errors only",
        ] {
            assert!(inventory.contains(key), "missing {key} in shared inventory");
        }
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut agents = AgentList {
                help: true,
                ..AgentList::default()
            };
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
            assert!(
                text.contains("F1 / ? opens the scrollable cockpit reference"),
                "{width}: {text}"
            );
        }
    }

    fn ctrl(code: KeyCode) -> event::KeyEvent {
        event::KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_p_opens_the_searchable_palette_and_themes_stay_on_f2() {
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![session(
            "harness.codex",
            "one",
            "running",
            None,
        )]));
        let (sender, requests) = mpsc::channel();
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('p'))));
        assert!(workspace.palette.is_open());
        assert_eq!(workspace.palette.kind(), Some(palette::Kind::Commands));
        assert!(
            workspace
                .palette
                .entries()
                .iter()
                .any(|entry| entry.key == keymap::id::THEMES)
        );
        assert!(
            workspace
                .palette
                .entries()
                .iter()
                .any(|entry| entry.key == keymap::id::SESSION_CHOOSER)
        );
        // Literal query characters stay in the search field.
        for ch in "them".chars() {
            assert!(!workspace.key(&sender, plain_key(ch)));
        }
        assert_eq!(workspace.palette.query(), "them");
        assert!(!workspace.key(
            &sender,
            event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ));
        assert!(workspace.theme_request);
        assert!(!workspace.palette.is_open());
        assert!(requests.try_recv().is_err(), "palette navigation is local");
        // The old Ctrl+P appearance binding is gone; F2 still owns themes.
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('p'))));
        assert!(workspace.palette.is_open());
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(!workspace.palette.is_open());
    }

    #[test]
    fn colon_opens_commands_but_stays_literal_in_text_search() {
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![session(
            "harness.codex",
            "one",
            "running",
            None,
        )]));
        let (sender, requests) = mpsc::channel();
        assert!(!workspace.key(&sender, plain_key(':')));
        assert!(workspace.palette.is_open());
        for ch in "theme".chars() {
            workspace.key(&sender, plain_key(ch));
        }
        assert_eq!(
            workspace.palette.selected().unwrap().key,
            keymap::id::THEMES
        );
        workspace.key(&sender, plain_code(KeyCode::Esc));
        assert!(!workspace.palette.is_open());

        workspace.agents.editing = true;
        workspace.agents.search = "codex".into();
        assert!(!workspace.key(&sender, plain_key(':')));
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('p'))));
        assert!(!workspace.palette.is_open());
        assert_eq!(workspace.agents.search, "codex:");
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn session_chooser_keeps_exact_identity_and_owns_escape() {
        let mut workspace = Workspace::new();
        let first = session("harness.codex", "first", "running", None);
        let mut second = session("harness.pi", "second", "completed", None);
        second.task_title = Some("Second review".into());
        workspace
            .agents
            .update(catalog(vec![first.clone(), second.clone()]));
        let (sender, requests) = mpsc::channel();
        let opened = workspace.run_action(&sender, keymap::id::SESSION_CHOOSER);
        assert!(!opened);
        assert_eq!(workspace.palette.kind(), Some(palette::Kind::Sessions));
        // F1 opens help over the chooser and returns to the frozen choices.
        assert!(!workspace.key(&sender, plain_code(KeyCode::F(1))));
        assert!(workspace.help.is_open());
        assert!(workspace.palette.is_open());
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(!workspace.help.is_open());
        assert!(workspace.palette.is_open());
        // Escape belongs to the chooser before the monitor or an enclosing form.
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(!workspace.palette.is_open());
        assert!(workspace.agents.session_scope.is_none());
        assert!(requests.try_recv().is_err());

        workspace.run_action(&sender, keymap::id::SESSION_CHOOSER);
        for ch in "pi".chars() {
            workspace.key(&sender, plain_key(ch));
        }
        workspace.key(
            &sender,
            event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(
            workspace.agents.session_scope,
            Some(("harness.pi".into(), "second".into()))
        );
        assert_eq!(workspace.agents.visible.len(), 1);
        assert_eq!(workspace.agents.current(), Some(&second));

        workspace.key(&sender, ctrl(KeyCode::Char('p')));
        workspace.run_action(&sender, keymap::id::SESSION_CHOOSER);
        workspace.palette.select_key("session|all");
        workspace.key(
            &sender,
            event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(workspace.agents.session_scope.is_none());
        assert_eq!(workspace.agents.visible.len(), 2);
    }

    #[test]
    fn help_preserves_text_drafts_scroll_and_returns_to_the_same_control() {
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![
            session("harness.codex", "first", "running", None),
            session("harness.codex", "selected", "completed", None),
        ]));
        let (sender, _requests) = mpsc::channel();
        assert!(!workspace.key(&sender, plain_key('/')));
        for ch in "a?[4".chars() {
            assert!(!workspace.key(&sender, plain_key(ch)));
        }
        assert_eq!(workspace.agents.search, "a?[4");
        let selected = workspace.agents.current().cloned();
        assert!(!workspace.key(&sender, plain_code(KeyCode::F(1))));
        assert!(workspace.help.is_open());
        assert_eq!(workspace.help.title(), "Command center");
        assert!(
            workspace
                .help
                .lines()
                .iter()
                .any(|line| line.text.contains("Operations (Rust)"))
        );
        assert!(!workspace.key(&sender, plain_code(KeyCode::Down)));
        assert!(workspace.help.scroll() <= 1);
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(!workspace.help.is_open());
        assert!(workspace.agents.editing, "help returns to the text control");
        assert_eq!(workspace.agents.search, "a?[4");
        assert_eq!(workspace.agents.current().cloned(), selected);
        // Literal question marks never open help while editing.
        assert!(!workspace.key(&sender, plain_key('?')));
        assert!(!workspace.help.is_open());
        assert_eq!(workspace.agents.search, "a?[4?");
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        // Outside text entry, `?` is the help shortcut.
        assert!(!workspace.key(&sender, plain_key('?')));
        assert!(workspace.help.is_open());
        assert!(!workspace.key(&sender, plain_key('?')));
        assert!(!workspace.help.is_open());
    }

    #[test]
    fn palette_marks_current_view_actions_unavailable_without_a_selection() {
        let mut workspace = Workspace::new();
        let (sender, _requests) = mpsc::channel();
        workspace.open_palette();
        let review = workspace
            .palette
            .entries()
            .iter()
            .find(|entry| entry.key == keymap::id::REVIEW)
            .expect("review action is always listed");
        assert!(!review.enabled);
        assert!(review.detail.contains("Select an exact session"));
        assert!(
            workspace
                .palette
                .entries()
                .iter()
                .any(|entry| entry.key == keymap::id::GO_OPERATIONS && entry.enabled)
        );
        assert!(
            workspace
                .palette
                .entries()
                .iter()
                .any(|entry| entry.key == keymap::id::GO_USAGE && entry.enabled)
        );
        workspace.palette.close();

        let selected = session("harness.codex", "selected", "completed", None);
        workspace.agents.update(catalog(vec![selected.clone()]));
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(selected, SessionView::Overview),
            empty_history_page(),
        ));
        workspace.open_palette();
        let review = workspace
            .palette
            .entries()
            .iter()
            .find(|entry| entry.key == keymap::id::REVIEW)
            .expect("review action is always listed");
        assert!(review.enabled);
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
    }

    #[test]
    fn refreshing_a_pane_keeps_its_scroll_and_reports_a_missing_identity() {
        let (sender, requests) = mpsc::channel();
        let mut workspace = Workspace::new();
        let page = |titles: &[&str]| {
            let mut page = empty_history_page();
            for title in titles {
                page.snapshot.panes.insert(
                    agenturm_kernel::ExtensionId::parse(title.to_ascii_lowercase()).unwrap(),
                    agenturm_kernel::PaneModel {
                        title: (*title).into(),
                        rows: (0..40)
                            .map(|index| MetricRow {
                                label: format!("Row {index}"),
                                value: index.to_string(),
                            })
                            .collect(),
                    },
                );
            }
            page
        };
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Activity),
            page(&["Alpha", "Beta"]),
        ));
        workspace.history[0].page.ui.set_viewport(60, 18);
        workspace.history[0].page.ui.restore_view(Some("Beta"), 4);
        assert_eq!(workspace.history[0].page.ui.scroll, 4);
        assert!(!workspace.key(&sender, plain_key('R')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History { .. })
        ));
        workspace.finish(Ok(WorkspaceResponse::History(page(&["Alpha", "Beta"]))));
        assert_eq!(workspace.history[0].page.ui.selected_title(), Some("Beta"));
        assert_eq!(workspace.history[0].page.ui.scroll, 4);

        workspace.key(&sender, plain_key('R'));
        let _ = requests.try_recv();
        workspace.finish(Ok(WorkspaceResponse::History(page(&["Alpha", "Gamma"]))));
        assert_eq!(workspace.history[0].page.ui.selected_title(), Some("Gamma"));
        assert!(
            workspace
                .message
                .as_deref()
                .is_some_and(|message| message.contains("Beta is no longer"))
        );
    }

    #[test]
    fn late_responses_for_a_closed_context_are_discarded() {
        let (sender, requests) = mpsc::channel();
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Activity),
            empty_history_page(),
        ));
        workspace.key(&sender, plain_key('R'));
        let _ = requests.try_recv();
        assert!(workspace.pending.is_some());
        // Simulate the reader returning after the lane closed the view.
        workspace.generation = workspace.generation.wrapping_add(1);
        let mut next = empty_history_page();
        next.snapshot.panes.insert(
            agenturm_kernel::ExtensionId::parse("late").unwrap(),
            agenturm_kernel::PaneModel {
                title: "Late".into(),
                rows: Vec::new(),
            },
        );
        workspace.finish(Ok(WorkspaceResponse::History(next)));
        assert!(workspace.pending.is_none());
        assert!(
            workspace
                .message
                .as_deref()
                .is_some_and(|message| message.contains("Discarded a late response"))
        );
        assert_eq!(
            workspace.history[0].page.ui.selected_title(),
            None,
            "a superseded page must not replace the current view"
        );
    }

    #[test]
    fn ctrl_c_copies_public_evidence_but_keeps_interrupt_in_text_entry() {
        let mut workspace = Workspace::new();
        let mut page = empty_history_page();
        page.snapshot.panes.insert(
            agenturm_kernel::ExtensionId::parse("activity").unwrap(),
            agenturm_kernel::PaneModel {
                title: "Activity".into(),
                rows: vec![MetricRow {
                    label: "Event".into(),
                    value: "recorded".into(),
                }],
            },
        );
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Activity),
            page,
        ));
        let (sender, _requests) = mpsc::channel();
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('c'))));
        assert!(
            workspace.view_io.clipboard.is_some(),
            "public evidence focus copies"
        );
        let _ = workspace.view_io.clipboard.take();

        workspace.history.clear();
        workspace.agents.update(catalog(vec![session(
            "harness.codex",
            "selected",
            "running",
            None,
        )]));
        workspace.key(&sender, plain_key('/'));
        assert!(workspace.key(&sender, ctrl(KeyCode::Char('c'))));
    }

    #[test]
    fn today_and_session_scope_survive_refresh_without_moving_identity() {
        let mut workspace = Workspace::new();
        let active = session("harness.codex", "active", "running", None);
        let old = session("harness.pi", "old", "completed", None);
        workspace
            .agents
            .update(catalog(vec![old.clone(), active.clone()]));
        let (sender, _requests) = mpsc::channel();
        workspace.run_action(&sender, keymap::id::SCOPE_TODAY);
        assert_eq!(workspace.agents.time_scope, TimeScope::Today);
        assert_eq!(workspace.agents.visible.len(), 1);
        assert_eq!(workspace.agents.current(), Some(&active));
        workspace.run_action(
            &sender,
            &format!("session|{}|{}", active.harness_id, active.session_id),
        );
        assert_eq!(workspace.agents.session_scope, Some(session_key(&active)));
        workspace.agents.update(catalog(vec![active.clone(), old]));
        assert_eq!(workspace.agents.time_scope, TimeScope::Today);
        assert_eq!(workspace.agents.session_scope, Some(session_key(&active)));
        assert_eq!(workspace.agents.current(), Some(&active));
        workspace.run_action(&sender, keymap::id::SCOPE_HISTORY);
        assert_eq!(workspace.agents.time_scope, TimeScope::All);
        assert_eq!(
            workspace.agents.visible.len(),
            1,
            "History does not clear the independent session scope"
        );
        workspace.run_action(&sender, "session|all");
        assert_eq!(workspace.agents.visible.len(), 2);
    }

    #[test]
    fn help_from_a_compact_chooser_and_export_form_returns_with_state_intact() {
        let mut workspace = Workspace::new();
        let session = session("harness.codex", "selected", "completed", None);
        let turn = RecordedTurn {
            harness_id: session.harness_id.clone(),
            session_id: session.session_id.clone(),
            turn_id: "turn-a".into(),
            worker_id: None,
            snapshot_id: 9.into(),
            lifecycle: None,
        };
        let mut page = empty_history_page();
        page.snapshot.history_snapshot_id = turn.snapshot_id;
        page.turns = vec![turn];
        page.snapshot.panes.insert(
            agenturm_kernel::ExtensionId::parse("activity").unwrap(),
            agenturm_kernel::PaneModel {
                title: "Activity".into(),
                rows: vec![MetricRow {
                    label: "Event".into(),
                    value: "recorded".into(),
                }],
            },
        );
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(session, SessionView::Overview),
            page,
        ));
        let (sender, _requests) = mpsc::channel();
        assert!(!workspace.key(&sender, plain_key('T')));
        assert!(workspace.history[0].page.turn_picker.is_some());
        assert!(!workspace.key(&sender, plain_code(KeyCode::F(1))));
        assert!(workspace.help.is_open());
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(!workspace.help.is_open());
        assert!(
            workspace.history[0].page.turn_picker.is_some(),
            "help returns to the compact chooser"
        );
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(workspace.history[0].page.turn_picker.is_none());

        // Export form drafts also survive help.
        assert!(!workspace.key(&sender, plain_key('x')));
        assert!(workspace.view_io.form.is_some());
        for ch in ['a', '?', '[', '4'] {
            assert!(!workspace.key(&sender, plain_key(ch)));
        }
        assert!(workspace.view_io.form.is_some());
        assert!(!workspace.key(&sender, plain_code(KeyCode::F(1))));
        assert!(workspace.help.is_open());
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(!workspace.help.is_open());
        assert!(
            workspace.view_io.form.is_some(),
            "help returns to the export form with its draft"
        );
        assert!(!workspace.key(&sender, plain_code(KeyCode::Esc)));
        assert!(workspace.view_io.form.is_none());
    }

    #[test]
    fn palette_runs_a_clipped_destination_and_reports_it_in_help() {
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![session(
            "harness.codex",
            "selected",
            "completed",
            None,
        )]));
        let (sender, requests) = mpsc::channel();
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('p'))));
        for ch in "operations".chars() {
            workspace.key(&sender, plain_key(ch));
        }
        workspace.key(
            &sender,
            event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::All(WorkspaceView::Operations),
                before: None,
                ..
            })
        ));
        workspace.pending = None;
        workspace.history.clear();
        workspace.open_help();
        let rendered: Vec<_> = workspace
            .help
            .lines()
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        for expected in [
            "1",
            "Agents",
            "7",
            "Findings",
            "9",
            "Operations (Rust)",
            "U",
            "Usage (Rust)",
            "Ctrl+P",
            "F2",
        ] {
            assert!(
                rendered.iter().any(|line| line.contains(expected)),
                "help is missing {expected}: {rendered:?}"
            );
        }
    }

    #[test]
    fn palette_help_and_theme_overlays_stay_usable_at_the_floor() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut workspace = Workspace::new();
            workspace.agents.update(catalog(vec![session(
                "harness.codex",
                "selected",
                "running",
                None,
            )]));
            workspace.open_palette();
            workspace.open_help();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_workspace(frame, &mut workspace))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Help · Command palette"), "{width}: {text}");
            assert!(text.contains("F1 or ? close help"), "{width}: {text}");

            workspace.help.close();
            workspace.open_palette();
            terminal
                .draw(|frame| draw_workspace(frame, &mut workspace))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Command palette"), "{width}: {text}");
            assert!(text.contains("Esc cancel"), "{width}: {text}");
        }
    }
    fn pending_request(owner: &RecordedSession) -> RequestEntry {
        RequestEntry {
            owner: WorkAnchor::from(owner),
            event_id: "approval".into(),
            revision: 1,
            timestamp: 1,
            reason: "approval".into(),
            seen: false,
            proof: RequestProof {
                snapshot_id: owner.snapshot_id,
                history_identity: owner.history_identity.clone(),
            },
        }
    }

    #[test]
    fn today_scope_uses_the_host_local_boundary_and_keeps_active_work_of_any_age() {
        let day = 1_700_000_000_000_000_000u64;
        let mut active = session("harness.codex", "active", "running", None);
        active.updated_at_unix_ns = day - 10 * 86_400 * 1_000_000_000;
        let mut pending = session("harness.claude", "pending", "waiting", None);
        pending.updated_at_unix_ns = day - 10 * 86_400 * 1_000_000_000;
        pending.pending_requests.push(pending_request(&pending));
        let mut reviewed_pending = session("harness.claude", "reviewed-input", "waiting", None);
        reviewed_pending.updated_at_unix_ns = day - 10 * 86_400 * 1_000_000_000;
        let mut request = pending_request(&reviewed_pending);
        request.seen = true;
        reviewed_pending.pending_requests.push(request);
        let mut outcome_today = session("harness.pi", "outcome-today", "completed", None);
        outcome_today.updated_at_unix_ns = day + 1;
        let mut outcome_yesterday = session("harness.pi", "outcome-yesterday", "completed", None);
        outcome_yesterday.updated_at_unix_ns = day - 1;
        let mut reviewed_failure = session("harness.pi", "reviewed-failure", "failed", None);
        reviewed_failure.updated_at_unix_ns = day + 1;
        reviewed_failure.reviewed = true;

        let mut agents = AgentList::default();
        agents.update(catalog_at(
            vec![
                active.clone(),
                pending.clone(),
                reviewed_pending,
                outcome_today.clone(),
                outcome_yesterday,
                reviewed_failure,
            ],
            day,
        ));
        assert_eq!(agents.time_scope, TimeScope::All);
        agents.time_scope = TimeScope::Today;
        agents.apply_filters(None);
        let visible = agents
            .visible
            .iter()
            .map(|item| {
                let row = &agents.catalog.as_ref().unwrap().sessions[item.index];
                row.session_id.as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(visible, ["outcome-today", "pending", "active"]);
        // Selecting the outcome and returning to History keeps exact identity,
        // and the reviewed failure returns only in History.
        let selected = outcome_today.clone();
        agents.selection.select(Some(2));
        agents.time_scope = TimeScope::All;
        agents.apply_filters(Some(&session_key(&selected)));
        assert_eq!(agents.current(), Some(&selected));
        assert_eq!(agents.visible.len(), 6);
        assert!(!agents.lost_selection);
        assert_eq!(agents.time_scope, TimeScope::All);
        // Active work of any age is still Today, not a liveness claim.
        assert_eq!(active.state.as_deref(), Some("running"));
    }

    #[test]
    fn errors_and_subtree_scopes_publish_unsupported_relationships_and_keep_identity() {
        let root = session("harness.codex", "root", "running", None);
        let child = session("harness.codex", "child", "running", Some("root"));
        let grandchild = session("harness.codex", "grandchild", "completed", Some("child"));
        let mut forked = session("harness.codex", "forked", "completed", Some("root"));
        forked.relationship_kind = Some(RelationshipKind::Forked);
        let mut cross_source = session("harness.pi", "cross-source", "failed", Some("root"));
        cross_source.relationship_kind = Some(RelationshipKind::Delegated);
        let mut orphan = session("harness.codex", "orphan", "completed", Some("missing"));
        orphan.relationship_kind = Some(RelationshipKind::Delegated);
        let failed = session("harness.pi", "failed", "failed", None);
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![
            root.clone(),
            child.clone(),
            grandchild.clone(),
            forked,
            cross_source,
            orphan,
            failed.clone(),
        ]));
        let (sender, _requests) = mpsc::channel();
        workspace.run_action(
            &sender,
            &format!("session|{}|{}", root.harness_id, root.session_id),
        );
        assert_eq!(workspace.agents.visible.len(), 1);
        workspace.run_action(&sender, keymap::id::SESSION_SUBTREE);
        let visible = workspace
            .agents
            .visible
            .iter()
            .map(|item| {
                let row = &workspace.agents.catalog.as_ref().unwrap().sessions[item.index];
                row.session_id.as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(visible, ["root", "child", "grandchild"]);
        assert_eq!(workspace.agents.subtree_unsupported, 2);
        assert_eq!(workspace.agents.subtree_outside_page, 1);
        let message = workspace.message.clone().unwrap();
        assert!(message.contains("2 non-delegated or cross-source relationships excluded"));
        assert!(message.contains("1 delegated parent outside this page"));

        // Exact identity survives a subtree refresh with a title change.
        let position = workspace
            .agents
            .visible
            .iter()
            .position(|item| {
                workspace.agents.catalog.as_ref().unwrap().sessions[item.index].session_id
                    == "child"
            })
            .unwrap();
        workspace.agents.selection.select(Some(position));
        let mut renamed = child.clone();
        renamed.task_title = Some("Renamed child".into());
        workspace.agents.update(catalog_at(
            vec![
                root.clone(),
                renamed.clone(),
                grandchild.clone(),
                failed.clone(),
            ],
            u64::MAX,
        ));
        assert_eq!(workspace.agents.current(), Some(&renamed));
        assert!(!workspace.agents.lost_selection);

        // Errors is an independent facet over recorded failure states.
        workspace.agents.session_scope = None;
        workspace.agents.subtree_scope = false;
        workspace.agents.apply_filters(Some(&session_key(&failed)));
        workspace.run_action(&sender, keymap::id::SESSION_ERRORS);
        assert!(workspace.agents.errors_only);
        assert_eq!(workspace.agents.current(), Some(&failed));
        assert!(
            workspace
                .agents
                .visible
                .iter()
                .filter_map(|item| workspace
                    .agents
                    .catalog
                    .as_ref()
                    .map(|c| &c.sessions[item.index]))
                .all(errors_only)
        );
        workspace.run_action(&sender, keymap::id::SESSION_ERRORS);
        assert!(!workspace.agents.errors_only);
        assert_eq!(workspace.agents.current(), Some(&failed));
    }

    #[test]
    fn session_chooser_searches_worker_identity_and_stays_usable_at_the_floor() {
        let mut first = session("harness.codex", "session-a", "running", None);
        first.task_title = Some("Payments API".into());
        first.project_path = Some("/work/payments-api".into());
        first.project_origin = Some(ProjectOrigin::OperatorTaskWorkspace);
        first.worker_id = Some("worker-9f2a".into());
        let mut second = session("harness.claude", "session-b", "completed", None);
        second.task_title = Some("Payments API".into());
        second.project_path = Some("/work/payments-api".into());
        second.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        second.worker_id = Some("worker-other".into());
        let mut workspace = Workspace::new();
        workspace
            .agents
            .update(catalog(vec![first.clone(), second.clone()]));
        let (sender, _requests) = mpsc::channel();
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('p'))));
        for ch in "choose agent session".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert_eq!(workspace.palette.kind(), Some(palette::Kind::Sessions));
        for ch in "claude".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert_eq!(
            workspace.palette.selected().map(|entry| entry.key.clone()),
            Some("session|harness.claude|session-b".into())
        );
        workspace.key(&sender, ctrl(KeyCode::Char('a')));
        for ch in "worker-9f2a".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert_eq!(
            workspace.palette.selected().map(|entry| entry.key.clone()),
            Some("session|harness.codex|session-a".into())
        );
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Choose agent session"), "{text}");
        assert!(text.contains("Payments API"), "{text}");
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert_eq!(
            workspace.agents.session_scope,
            Some(("harness.codex".into(), "session-a".into()))
        );
        assert_eq!(workspace.agents.current(), Some(&first));

        // An empty result stays open and clear/cancel work at 60x18.
        assert!(!workspace.key(&sender, ctrl(KeyCode::Char('p'))));
        for ch in "choose agent session".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        press(&mut workspace, &sender, KeyCode::Enter);
        for ch in "zzzz".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert!(workspace.palette.is_open());
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("No matches"), "{text}");
        workspace.key(&sender, ctrl(KeyCode::Char('a')));
        assert!(workspace.palette.query().is_empty());
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(!workspace.palette.is_open());
        assert_eq!(
            workspace.agents.session_scope,
            Some(("harness.codex".into(), "session-a".into()))
        );
    }

    #[test]
    fn inspector_sections_navigate_enter_and_return_with_the_same_roster_state() {
        let selected = session("harness.codex", "selected", "running", None);
        let mut workspace = Workspace::new();
        workspace.agents.update(catalog(vec![
            session("harness.pi", "other", "completed", None),
            selected.clone(),
        ]));
        workspace.agents.display_offset = 0;
        workspace.agents.filter = Filter::Active;
        workspace
            .agents
            .apply_filters(Some(&session_key(&selected)));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::Session(_, SessionView::Overview),
                ..
            })
        ));
        workspace.finish(Ok(WorkspaceResponse::History(session_history_page(&[
            "Session overview",
            "Session details",
        ]))));
        assert_eq!(workspace.inspector.tab, inspector::Tab::Overview);
        assert!(workspace.inspector.tabs_focused);

        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "Overview",
            "Prompts",
            "Details",
            "READ: Enter",
            "Codex session",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }

        // Right selects Prompts without loading; Down enters the UI-04
        // navigator; Esc returns to the same tab strip.
        assert!(!press(&mut workspace, &sender, KeyCode::Right));
        assert_eq!(workspace.inspector.tab, inspector::Tab::Prompts);
        assert!(requests.try_recv().is_err());
        assert!(!press(&mut workspace, &sender, KeyCode::Down));
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(WorkCommand::Prompts(owner))) if owner.session == "selected")
        );
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::Prompts(Vec::new()))));
        assert!(workspace.prompt_nav.is_some());
        press(&mut workspace, &sender, KeyCode::Esc);
        assert!(workspace.prompt_nav.is_none());

        // `m` from the section content loads Details in place and preserves
        // content focus. Enter from the tab strip sets read focus.
        assert!(!press(&mut workspace, &sender, KeyCode::Char('m')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::Session(_, SessionView::Details),
                ..
            })
        ));
        workspace.finish(Ok(WorkspaceResponse::History(session_history_page(&[
            "Session details",
        ]))));
        assert_eq!(workspace.inspector.tab, inspector::Tab::Details);
        terminal
            .draw(|frame| draw_workspace(frame, &mut workspace))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Session details value"), "{text}");

        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert!(!workspace.inspector.tabs_focused);
        // Ctrl+Left/Right switch sections from content without returning to the
        // tab strip or losing content focus.
        assert!(!workspace.key(&sender, ctrl(KeyCode::Right)));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History {
                selection: HistorySelection::Session(_, SessionView::Overview),
                ..
            })
        ));
        workspace.finish(Ok(WorkspaceResponse::History(session_history_page(&[
            "Session overview",
        ]))));
        assert_eq!(workspace.inspector.tab, inspector::Tab::Overview);
        assert!(!workspace.inspector.tabs_focused);
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(workspace.history.is_empty());
        assert_eq!(workspace.agents.current(), Some(&selected));
        assert_eq!(workspace.agents.filter, Filter::Active);
        assert_eq!(workspace.agents.current(), Some(&selected));
        assert_eq!(workspace.agents.display_offset, 0);
    }

    #[test]
    fn unnamed_sessions_render_human_context_never_a_name_unavailable_label() {
        let mut unnamed = session("harness.codex", "unnamed", "running", None);
        unnamed.project_path = Some("/work/payments-api".into());
        unnamed.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        unnamed.worker_id = Some("w-1".into());
        unnamed.native_context = Some(Box::new(NativeSessionContext {
            last_activity: Some("Checking budget race".into()),
            ..NativeSessionContext::default()
        }));
        let mut nickname_only = session("harness.claude", "nickname", "running", None);
        nickname_only.native_context = Some(Box::new(NativeSessionContext {
            agent_name: Some("payments-helper".into()),
            ..NativeSessionContext::default()
        }));
        nickname_only.project_path = Some("/work/agenturm".into());
        nickname_only.project_origin = Some(ProjectOrigin::NativeRecordedWorkspace);
        let mut agents = AgentList::default();
        agents.update(catalog(vec![unnamed, nickname_only]));
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_agents(frame, &mut agents, false, None))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                text.contains("Codex worker · payments-api"),
                "{width}: {text}"
            );
            assert!(text.contains("Checking budget race"), "{width}: {text}");
            assert!(!text.contains("w-1"), "{width}: {text}");
            assert!(text.contains("payments-helper"), "{width}: {text}");
            assert!(!text.contains("Name unavailable"), "{width}: {text}");
            if width < 120 {
                assert!(
                    !text.contains("Codex · Checking budget race"),
                    "{width}: {text}"
                );
            }
        }
    }

    fn review_page() -> HistoryPage {
        let id = agenturm_kernel::ExtensionId::parse("app.session-review").unwrap();
        let mut page = session_history_page(&[]);
        page.history_identity = Some("review-history".into());
        page.snapshot.history_snapshot_id = 9.into();
        page.snapshot.panes.insert(
            id.clone(),
            agenturm_kernel::PaneModel {
                title: "Review".into(),
                rows: vec![MetricRow {
                    label: "Checks".into(),
                    value: "1 passed".into(),
                }],
            },
        );
        page.initial_pane = Some(id.clone());
        page.review_pane = Some(id);
        page
    }

    #[test]
    fn review_copy_export_and_save_use_the_exact_anchor_and_keep_review_selected() {
        let row = session("harness.codex", "review-me", "completed", None);
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(row.clone(), SessionView::Review),
            review_page(),
        ));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Char('c')));
        let WorkspaceRequest::Work(WorkCommand::ReviewReceipt(request)) =
            requests.try_recv().unwrap()
        else {
            panic!("expected review receipt copy")
        };
        assert_eq!(request.query.anchor.source, "harness.codex");
        assert_eq!(request.query.anchor.session, "review-me");
        assert_eq!(request.format, crate::workbench::ReviewFormat::Text);
        assert!(request.directory.is_none());
        assert!(matches!(
            workspace.history.last().map(|view| &view.selection),
            Some(HistorySelection::Session(_, SessionView::Review))
        ));

        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::ReviewDocument {
            title: "Review receipt".into(),
            text: "review receipt body".into(),
            scope: "harness.codex / review-me".into(),
        })));
        assert!(workspace.view_io.clipboard.is_some());
        assert!(
            workspace
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("copied")
        );
        assert!(matches!(
            workspace.history.last().map(|view| &view.selection),
            Some(HistorySelection::Session(_, SessionView::Review))
        ));

        assert!(!press(&mut workspace, &sender, KeyCode::Char('x')));
        let form = workspace.view_io.review_form.as_ref().unwrap();
        assert_eq!(form.query.anchor.session, "review-me");
        for ch in "/tmp/review-export".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let WorkspaceRequest::Work(WorkCommand::ReviewReceipt(request)) =
            requests.try_recv().unwrap()
        else {
            panic!("expected review receipt export")
        };
        assert_eq!(
            request.directory.as_deref(),
            Some("/tmp/review-export"),
            "exact destination is carried"
        );
        workspace.finish(Err("Cannot export review receipt: AlreadyExists".into()));
        assert!(
            workspace.view_io.review_form.is_some(),
            "a collision keeps the destination form open"
        );
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let _retry = requests
            .try_recv()
            .expect("the form retries the same directory");
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::ReviewDocument {
            title: "Review receipt".into(),
            text: "receipt".into(),
            scope: "harness.codex / review-me / exported".into(),
        })));
        assert!(workspace.view_io.review_form.is_none());

        assert!(!press(&mut workspace, &sender, KeyCode::Char('s')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::Work(WorkCommand::DraftReviewTask { anchor, .. }))
                if anchor.session == "review-me"
        ));
        assert!(!press(&mut workspace, &sender, KeyCode::Char('g')));
        assert!(
            workspace
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("workspace mapping")
        );
        assert!(matches!(
            workspace.history.last().map(|view| &view.selection),
            Some(HistorySelection::Session(_, SessionView::Review))
        ));
    }

    #[test]
    fn review_control_order_is_deterministic_and_escape_returns_to_review() {
        let row = session("harness.codex", "controls", "completed", None);
        let mut workspace = Workspace::new();
        let mut page = review_page();
        page.snapshot.observations_truncated = true;
        page.features.continuation = Some(Box::new(ObservationPosition {
            observed_at_unix_ns: 7,
            harness_id: "harness.codex".into(),
            event_id: "prior-page".into(),
            revision: 0,
            snapshot_id: row.snapshot_id,
        }));
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(row, SessionView::Review),
            page,
        ));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Tab));
        assert_eq!(
            workspace.history.last().unwrap().page.review_control,
            Some(0)
        );
        assert!(!press(&mut workspace, &sender, KeyCode::Tab));
        assert_eq!(
            workspace.history.last().unwrap().page.review_control,
            Some(1)
        );
        assert!(!press(&mut workspace, &sender, KeyCode::BackTab));
        assert_eq!(
            workspace.history.last().unwrap().page.review_control,
            Some(0)
        );
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let request = match requests.try_recv() {
            Ok(request) => request,
            Err(error) => panic!("review control sent no request: {error}"),
        };
        match request {
            WorkspaceRequest::History { selection, .. } => assert!(matches!(
                selection,
                HistorySelection::Session(_, SessionView::Review)
            )),
            _ => panic!("review control did not request the same Review page"),
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(
            workspace
                .history
                .last()
                .unwrap()
                .page
                .review_control
                .is_none()
        );
        assert_eq!(workspace.history.len(), 1);
    }

    #[test]
    fn turn_review_route_keeps_the_exact_turn_and_returns_to_it() {
        let row = session("harness.codex", "turn-session", "completed", None);
        let turn = RecordedTurn {
            harness_id: "harness.codex".into(),
            session_id: "turn-session".into(),
            turn_id: "turn-7".into(),
            worker_id: Some("worker-a".into()),
            snapshot_id: row.snapshot_id,
            lifecycle: Some(("completed".into(), None)),
        };
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::Turn(turn.clone()),
            session_history_page(&["Turn activity"]),
        ));
        let (sender, requests) = mpsc::channel();
        assert!(!workspace.key(
            &sender,
            event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
        ));
        let WorkspaceRequest::History {
            selection: HistorySelection::TurnReview(pinned),
            ..
        } = requests.try_recv().unwrap()
        else {
            panic!("expected a turn-scoped Review request")
        };
        assert_eq!(pinned.turn_id, "turn-7");
        assert_eq!(pinned.worker_id.as_deref(), Some("worker-a"));
        workspace.finish(Ok(WorkspaceResponse::History(review_page())));
        assert!(matches!(
            workspace.history.last().map(|view| &view.selection),
            Some(HistorySelection::TurnReview(turn)) if turn.turn_id == "turn-7"
        ));
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(matches!(
            workspace.history.last().map(|view| &view.selection),
            Some(HistorySelection::Turn(turn)) if turn.turn_id == "turn-7"
        ));
    }

    #[test]
    fn inspector_prompts_open_the_navigator_and_escape_returns_to_the_section() {
        let row = session("harness.codex", "prompt-session", "running", None);
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(row, SessionView::Overview),
            session_history_page(&["Session overview", "Session details"]),
        ));
        let (sender, requests) = mpsc::channel();
        assert!(!press(&mut workspace, &sender, KeyCode::Char('P')));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::Work(WorkCommand::Prompts(anchor)))
                if anchor.session == "prompt-session"
        ));
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::PromptHistory(
            crate::workbench::PromptHistoryPage {
                anchor: WorkAnchor {
                    source: "harness.codex".into(),
                    session: "prompt-session".into(),
                    worker: None,
                    turn: None,
                },
                retained: 1,
                retained_cap: 256,
                limited: false,
                older: None,
                availability: crate::workbench::PromptAvailability::Granted,
                entries: vec![PromptEntry {
                    anchor: WorkAnchor {
                        source: "harness.codex".into(),
                        session: "prompt-session".into(),
                        worker: None,
                        turn: Some("turn-1".into()),
                    },
                    event_id: "event-1".into(),
                    revision: 0,
                    timestamp: 1,
                    snapshot_id: None,
                    history_identity: None,
                }],
            },
        ))));
        let navigator = workspace.prompt_nav.as_ref().unwrap();
        assert_eq!(navigator.status_line(), "1 loaded · oldest first");
        assert_eq!(workspace.focus(), keymap::Focus::PromptNavigator);
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::Work(WorkCommand::RevealPrompt(entry, _)))
                if entry.event_id == "event-1"
        ));
        assert!(workspace.prompt_read_epoch.is_some());
        // Reader focus owns the first Escape (closing the pending read); the
        // navigator owns the next one and leaves the Prompts section.
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(workspace.prompt_nav.is_some());
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(workspace.prompt_nav.is_none());
        assert!(matches!(
            workspace.history.last().map(|view| &view.selection),
            Some(HistorySelection::Session(_, SessionView::Overview))
        ));
        assert!(!press(&mut workspace, &sender, KeyCode::Esc));
        assert!(
            workspace.history.is_empty(),
            "the next Esc returns to the roster"
        );
    }

    #[test]
    fn evidence_filter_is_pane_local_and_survives_a_page_replacement() {
        let width = 80;
        let id = agenturm_kernel::ExtensionId::parse("app.session-activity").unwrap();
        let mut page = session_history_page(&[]);
        page.snapshot.panes.insert(
            id.clone(),
            agenturm_kernel::PaneModel {
                title: "Activity".into(),
                rows: Vec::new(),
            },
        );
        page.initial_pane = Some(id.clone());
        page.evidence_tables.insert(
            id.clone(),
            crate::EvidenceSpec {
                title: "Activity",
                inspector: "Event detail",
                columns: &[("Action", 2)],
                empty: "No activity",
            }
            .table(vec![
                crate::EvidenceRow {
                    cells: vec!["tool read".into()],
                    details: vec![MetricRow {
                        label: "Result".into(),
                        value: "passed".into(),
                    }],
                },
                crate::EvidenceRow {
                    cells: vec!["tool write".into()],
                    details: vec![MetricRow {
                        label: "Result".into(),
                        value: "failed".into(),
                    }],
                },
            ]),
        );
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::All(WorkspaceView::Activity),
            page,
        ));
        let (sender, _requests) = mpsc::channel();
        let view = workspace.history.last().unwrap();
        assert_eq!(view.page.ui.evidence_visible(), vec![0, 1]);
        assert!(!press(&mut workspace, &sender, KeyCode::Char('/')));
        for ch in "write".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        assert!(!press(&mut workspace, &sender, KeyCode::Enter));
        let view = workspace.history.last().unwrap();
        assert_eq!(view.page.ui.evidence_visible(), vec![1]);
        assert!(
            view.page
                .ui
                .evidence_filter_label()
                .unwrap()
                .contains("write")
        );
        // A new page of the same pane keeps the committed filter.
        let mut next = session_history_page(&[]);
        next.snapshot.panes.insert(
            id.clone(),
            agenturm_kernel::PaneModel {
                title: "Activity".into(),
                rows: Vec::new(),
            },
        );
        next.initial_pane = Some(id.clone());
        next.evidence_tables.insert(
            id,
            crate::EvidenceSpec {
                title: "Activity",
                inspector: "Event detail",
                columns: &[("Action", 2)],
                empty: "No activity",
            }
            .table(vec![
                crate::EvidenceRow {
                    cells: vec!["tool write".into()],
                    details: vec![],
                },
                crate::EvidenceRow {
                    cells: vec!["tool read".into()],
                    details: vec![],
                },
            ]),
        );
        let mut replacement = PageView::new(next, false);
        replacement.inherit_filters(&workspace.history.last().unwrap().page);
        assert_eq!(replacement.ui.evidence_visible(), vec![0]);
        let _ = width;
    }

    #[test]
    fn palette_exposes_review_workflow_tasks_search_and_incidents() {
        let mut workspace = Workspace::new();
        workspace.agents.catalog = Some(catalog(vec![session(
            "harness.codex",
            "palette",
            "running",
            None,
        )]));
        workspace.open_palette();
        let labels = workspace
            .palette
            .entries()
            .iter()
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "Review recorded work",
            "Workflow inbox",
            "Saved tasks",
            "Search saved tasks",
            "Workflow incidents",
            "To review",
        ] {
            assert!(labels.contains(&expected), "missing {expected}: {labels:?}");
        }
    }

    fn prompt_page(anchor: WorkAnchor) -> crate::workbench::PromptHistoryPage {
        crate::workbench::PromptHistoryPage {
            entries: vec![PromptEntry {
                anchor: WorkAnchor {
                    turn: Some(anchor.turn.clone().unwrap_or("turn-1".into())),
                    ..anchor.clone()
                },
                event_id: "prompt-event".into(),
                revision: 0,
                timestamp: 1,
                snapshot_id: Some(9.into()),
                history_identity: Some("history".into()),
            }],
            anchor,
            retained: 1,
            retained_cap: 256,
            limited: false,
            older: None,
            availability: crate::workbench::PromptAvailability::Granted,
        }
    }

    fn workspace_text(workspace: &mut Workspace, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_workspace(frame, workspace))
            .unwrap();
        terminal.backend().to_string()
    }

    fn inline_private_fixture() -> (Workspace, mpsc::Receiver<()>, Arc<AtomicBool>) {
        let anchor = WorkAnchor {
            source: "harness.fixture".into(),
            session: "private-session".into(),
            worker: None,
            turn: None,
        };
        let page = prompt_page(anchor.clone());
        let mut nav = prompt::PromptNavigator::new(prompt::PromptScope::session(
            &anchor,
            prompt::PromptCapture::Enabled,
        ));
        let token = nav.begin_load();
        nav.accept_page(
            token,
            page.entries
                .iter()
                .map(prompt::PromptRequest::from)
                .collect(),
            prompt::PromptPageMeta::default(),
        );
        let prompt::PromptInput::Read { epoch, entry, .. } = nav.key(KeyCode::Right) else {
            panic!("read requested");
        };
        let (_frames, received) = mpsc::channel();
        let (acknowledgement, ack) = mpsc::channel();
        let guarded = Arc::new(AtomicBool::new(true));
        let receiver = PromptReceiver {
            frames: received,
            acknowledgement,
            active: Arc::new(Mutex::new(true)),
            guarded_active: Arc::clone(&guarded),
            kind: PromptKind::User,
        };
        assert_eq!(
            nav.accept_reader_presentation(
                epoch,
                (entry.event_id, entry.revision),
                prompt::PromptPresentation::normalized(
                    "PRIVATE_FIRST_LINE_MARKER".into(),
                    Some("PRIVATE_FIRST_LINE_MARKER".into()),
                    0,
                    false
                ),
                receiver,
                Instant::now()
            ),
            prompt::PromptAccepted::Accepted
        );
        let mut workspace = Workspace::new();
        workspace.prompt_nav = Some(nav);
        (workspace, ack, guarded)
    }

    #[test]
    fn inline_private_reader_redacts_before_ack_and_never_caches_a_captured_label() {
        for action in 0..5 {
            let (mut workspace, ack, guarded) = inline_private_fixture();
            let (sender, _requests) = mpsc::channel();
            for (width, height) in [(120, 32), (80, 24), (60, 18)] {
                assert!(
                    workspace_text(&mut workspace, width, height)
                        .contains("PRIVATE_FIRST_LINE_MARKER")
                );
            }
            workspace.key(
                &sender,
                event::KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            );
            press(&mut workspace, &sender, KeyCode::F(1));
            assert!(!workspace.palette.is_open() && !workspace.help.is_open());
            match action {
                0 => {
                    press(&mut workspace, &sender, KeyCode::Esc);
                }
                1 => {
                    press(&mut workspace, &sender, KeyCode::Char('['));
                }
                2 => {
                    assert!(workspace.expire_prompt_nav(Instant::now() + Duration::from_secs(26)));
                }
                3 => {
                    guarded.store(false, Ordering::Release);
                    assert!(workspace.expire_prompt_nav(Instant::now()));
                }
                _ => {
                    assert!(workspace.key(
                        &sender,
                        event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
                    ));
                    assert!(workspace.redact_prompt_nav(true));
                }
            }
            assert!(
                matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)),
                "ACK must stay connected until redraw"
            );
            for (width, height) in [(120, 32), (80, 24), (60, 18)] {
                assert!(
                    !workspace_text(&mut workspace, width, height)
                        .contains("PRIVATE_FIRST_LINE_MARKER")
                );
            }
            workspace.acknowledge_redaction();
            assert_eq!(ack.try_recv(), Ok(()));
        }
    }

    #[test]
    fn inline_private_reader_checks_the_same_locked_display_gate_as_fullscreen() {
        let (mut workspace, ack, _guarded) = inline_private_fixture();
        let active = workspace.private_display_gate().unwrap();
        *active.lock().unwrap() = false;
        let gate = active.lock().unwrap();
        workspace.enforce_private_display_gate(*gate);
        assert!(!workspace_text(&mut workspace, 80, 24).contains("PRIVATE_FIRST_LINE_MARKER"));
        assert!(matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)));
        workspace.acknowledge_redaction();
        assert_eq!(ack.try_recv(), Ok(()));
    }

    #[test]
    fn an_unavailable_main_session_key_does_not_mutate_a_private_reader() {
        let (mut workspace, ack, _guarded) = inline_private_fixture();
        let (sender, requests) = mpsc::channel();
        press(&mut workspace, &sender, KeyCode::Char('m'));
        assert!(workspace_text(&mut workspace, 80, 24).contains("PRIVATE_FIRST_LINE_MARKER"));
        assert!(workspace.redact_then_acknowledge.is_none());
        assert!(matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn cancelled_review_export_cannot_copy_or_consume_a_replacement_form() {
        let mut workspace = Workspace::new();
        workspace.history.push(HistoryView::new(
            HistorySelection::Session(
                session("harness.fixture", "review", "completed", None),
                SessionView::Review,
            ),
            review_page(),
        ));
        let (sender, requests) = mpsc::channel();
        press(&mut workspace, &sender, KeyCode::Char('x'));
        for ch in "/original".chars() {
            press(&mut workspace, &sender, KeyCode::Char(ch));
        }
        press(&mut workspace, &sender, KeyCode::Enter);
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::Work(WorkCommand::ReviewReceipt(_)))
        ));
        press(&mut workspace, &sender, KeyCode::Esc);
        press(&mut workspace, &sender, KeyCode::Char('x'));
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::ReviewDocument {
            title: "Review".into(),
            text: "OLD_EXPORT_MUST_NOT_COPY".into(),
            scope: "original".into(),
        })));
        assert!(workspace.view_io.clipboard.is_none());
        assert_eq!(
            workspace.view_io.review_form.as_ref().unwrap().directory,
            ""
        );
        assert!(workspace.message.as_ref().unwrap().contains("Discarded"));
    }

    #[test]
    fn roster_and_exact_turn_prompt_routes_render_and_restore_their_origin() {
        for from_turn in [false, true] {
            let mut workspace = Workspace::new();
            let row = session("harness.fixture", "scoped", "running", None);
            workspace.agents.update(catalog(vec![row.clone()]));
            if from_turn {
                let turn = RecordedTurn {
                    harness_id: row.harness_id.clone(),
                    session_id: row.session_id.clone(),
                    worker_id: None,
                    turn_id: "exact-turn".into(),
                    snapshot_id: row.snapshot_id,
                    lifecycle: None,
                };
                workspace.history.push(HistoryView::new(
                    HistorySelection::Turn(turn),
                    session_history_page(&["Turn"]),
                ));
            }
            let (sender, requests) = mpsc::channel();
            press(&mut workspace, &sender, KeyCode::Char('P'));
            let WorkspaceRequest::Work(WorkCommand::Prompts(anchor)) = requests.try_recv().unwrap()
            else {
                panic!("prompt route");
            };
            assert_eq!(anchor.turn.as_deref(), from_turn.then_some("exact-turn"));
            workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::PromptHistory(
                prompt_page(anchor),
            ))));
            for (width, height) in [(120, 32), (80, 24), (60, 18)] {
                assert!(workspace_text(&mut workspace, width, height).contains("Request"));
            }
            press(&mut workspace, &sender, KeyCode::Esc);
            assert!(workspace.prompt_nav.is_none());
            assert_eq!(workspace.history.len(), usize::from(from_turn));
            assert_eq!(workspace.agents.current(), Some(&row));
        }
    }

    #[test]
    fn agents_navigation_supersedes_a_pending_destination_instead_of_ignoring_the_key() {
        let mut workspace = Workspace::new();
        let (sender, requests) = mpsc::channel();
        workspace.navigate(&sender, Destination::Activity);
        assert!(matches!(
            requests.try_recv(),
            Ok(WorkspaceRequest::History { .. })
        ));
        assert!(workspace.pending.is_some());
        press(&mut workspace, &sender, KeyCode::Char('1'));
        workspace.finish(Ok(WorkspaceResponse::History(session_history_page(&[
            "Activity",
        ]))));
        assert!(workspace.history.is_empty());
        assert!(workspace.pending.is_none());
        assert!(workspace.message.as_deref().unwrap().contains("Discarded"));
    }

    #[test]
    fn palette_actions_use_their_real_context_instead_of_conflicting_letters() {
        let mut workspace = Workspace::new();
        let (sender, requests) = mpsc::channel();
        workspace.work = Some(Workbench::new(
            WorkResult::Tasks {
                items: vec![],
                older: None,
            },
            None,
        ));
        workspace.run_action(&sender, keymap::id::TASK_DECISION_ADD);
        assert!(requests.try_recv().is_err());
        assert!(!workspace.work.as_ref().unwrap().editing());
        assert!(workspace.message.as_ref().unwrap().contains("Decisions"));
        workspace.work = None;
        let anchor = WorkAnchor {
            source: "harness.fixture".into(),
            session: "prompt".into(),
            worker: None,
            turn: None,
        };
        workspace.request(
            &sender,
            WorkspaceRequest::Work(WorkCommand::Prompts(anchor.clone())),
            Change::Work(Some(anchor.clone())),
        );
        requests.try_recv().unwrap();
        let mut page = prompt_page(anchor.clone());
        page.older = Some(crate::workbench::PromptHistoryCursor {
            timestamp: 1,
            event_id: "prompt-event".into(),
            snapshot_id: 9.into(),
            history_identity: "history".into(),
        });
        workspace.finish(Ok(WorkspaceResponse::Work(WorkResult::PromptHistory(page))));
        workspace.run_action(&sender, keymap::id::PROMPT_EARLIER_PAGE);
        assert!(
            matches!(requests.try_recv(), Ok(WorkspaceRequest::Work(WorkCommand::PromptPage { anchor: selected, before: Some(_) })) if selected == anchor)
        );
    }
}
