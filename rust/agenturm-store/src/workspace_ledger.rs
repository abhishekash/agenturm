//! Explicit, local cockpit tasks and cross-session handoffs.

mod continuity;
pub use continuity::{ApprovedTaskDraft, TaskDraftApproval, TaskSourceProof};

use crate::{MAX_EVENT_ID_BYTES, MAX_SESSION_KEY_BYTES, Store, StoreError, validate_identifier};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params, types::Type,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APPLICATION_ID: i64 = 0x4157_5350;
const SCHEMA_VERSION: i64 = 5;
pub const MAX_WORKSPACE_GIT_DIFF_BYTES: usize = 64 * 1024;
pub const MAX_WORKSPACE_PAGE_SIZE: usize = 200;
const MAX_TITLE_BYTES: usize = 256;
const MAX_ROSTER_PROJECT_PATH_BYTES: usize = 512;
const MAX_NOTE_BYTES: usize = 4 * 1024;
const MAX_DECISION_VERSIONS: usize = 128;
const MAX_DECISION_BYTES: usize = 480 * 1024;

type Result<T> = std::result::Result<T, WorkspaceError>;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAnchor {
    pub harness_id: ExtensionId,
    pub session_id: String,
    pub worker_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SavedTask {
    pub id: u64,
    pub anchor: Option<WorkspaceAnchor>,
    pub title: String,
    pub note: String,
    pub completed: bool,
    pub archived: bool,
    pub revision: u64,
    pub workspace_directory: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBrief {
    pub intent: String,
    pub acceptance_criteria: String,
    pub constraints: String,
    pub open_questions: String,
    pub next_action: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBriefUpdate {
    #[serde(default, deserialize_with = "present_brief_text")]
    pub intent: Option<String>,
    #[serde(default, deserialize_with = "present_brief_text")]
    pub acceptance_criteria: Option<String>,
    #[serde(default, deserialize_with = "present_brief_text")]
    pub constraints: Option<String>,
    #[serde(default, deserialize_with = "present_brief_text")]
    pub open_questions: Option<String>,
    #[serde(default, deserialize_with = "present_brief_text")]
    pub next_action: Option<String>,
}

fn present_brief_text<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDecisionKind {
    Accepted,
    Rejected,
}

impl TaskDecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct TaskDecisionInput {
    pub kind: TaskDecisionKind,
    pub text: String,
    pub rationale: String,
    pub scope: String,
    pub supersedes_id: Option<u64>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct SavedTaskDecision {
    pub id: u64,
    pub created_at_unix_ns: u64,
    #[serde(flatten)]
    pub decision: TaskDecisionInput,
    pub source: Option<TaskSourceProof>,
}

/// One consistent, bounded read of the operator-authored task handoff.
pub struct TaskHandoff {
    pub task: SavedTask,
    pub brief: TaskBrief,
    pub decisions: Vec<SavedTaskDecision>,
    pub links: Vec<WorkspaceAnchor>,
    pub links_truncated: bool,
    pub git_snapshot: Option<WorkspaceGitSnapshot>,
    pub git_diff: Option<WorkspaceGitDiff>,
    pub continuity: Vec<ApprovedTaskDraft>,
}

impl fmt::Debug for SavedTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedTask")
            .field("id", &self.id)
            .field("anchor", &self.anchor)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SavedHandoff {
    pub id: u64,
    pub from: WorkspaceAnchor,
    pub to: WorkspaceAnchor,
    pub note: String,
}

impl fmt::Debug for SavedHandoff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedHandoff")
            .field("id", &self.id)
            .field("from", &self.from)
            .field("to", &self.to)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspacePage<T> {
    /// Newest saved items first, ordered by stable, increasing creation ID.
    pub items: Vec<T>,
    pub next_before: Option<u64>,
}

/// A local summary saved by the operator. The counters cover staged and
/// unstaged changes and untracked files, without retaining the file listings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceGitSnapshot {
    pub repository_root: String,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
    pub staged_changes: u32,
    pub unstaged_changes: u32,
    pub untracked_files: u32,
    pub captured_at_unix_ns: u64,
}

/// Private text patches from a single explicit task capture.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct WorkspaceGitDiff {
    pub staged: String,
    pub unstaged: String,
}

impl WorkspaceGitDiff {
    pub fn byte_len(&self) -> usize {
        self.staged.len().saturating_add(self.unstaged.len())
    }

    fn validate(&self) -> Result<()> {
        if self.byte_len() > MAX_WORKSPACE_GIT_DIFF_BYTES
            || !display_safe(&self.staged, true)
            || !display_safe(&self.unstaged, true)
        {
            return Err(WorkspaceError::Invalid(
                "Git patch is oversized or contains unsafe characters",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Invalid(&'static str),
    MissingAnchor,
    HistoryUnavailable(String),
    TaskRevisionConflict { expected: u64, current: u64 },
    HistoryChanged,
    UnrecognizedLedger,
    Io(io::Error),
    Sqlite(rusqlite::Error),
    History(StoreError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::MissingAnchor => f.write_str("the selected history anchor is unavailable"),
            Self::HistoryUnavailable(message) => {
                write!(f, "Cannot verify workspace history: {message}")
            }
            Self::TaskRevisionConflict { expected, current } => write!(
                f,
                "Task revision conflict: expected {expected}, current {current}. Reload the task before saving."
            ),
            Self::HistoryChanged => {
                f.write_str("the history file changed since opening the workspace")
            }
            Self::UnrecognizedLedger => {
                f.write_str("the workspace file is not a private Agenturm workspace ledger")
            }
            Self::Io(error) => error.fmt(f),
            Self::Sqlite(error) => error.fmt(f),
            Self::History(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::History(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for WorkspaceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for WorkspaceError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// A separate, private SQLite database next to the history it references.
/// Opening and listing do not create it; the first explicit save does.
pub struct WorkspaceLedger {
    history: PathBuf,
    path: PathBuf,
    binding: [u8; 32],
    verifier: Option<Box<dyn WorkspaceHistory + Send + Sync>>,
}

/// Checks that a workspace reference still belongs to the bound history.
pub trait WorkspaceHistory {
    fn contains_anchor(&self, anchor: &WorkspaceAnchor) -> std::result::Result<bool, String>;
    fn contains_event(
        &self,
        anchor: &WorkspaceAnchor,
        event_id: &str,
        revision: u32,
    ) -> std::result::Result<bool, String>;
}

impl WorkspaceHistory for Store {
    fn contains_anchor(&self, anchor: &WorkspaceAnchor) -> std::result::Result<bool, String> {
        contains_anchor_in(&self.connection, anchor).map_err(|error| error.to_string())
    }

    fn contains_event(
        &self,
        anchor: &WorkspaceAnchor,
        event_id: &str,
        revision: u32,
    ) -> std::result::Result<bool, String> {
        contains_event_in(&self.connection, anchor, event_id, revision)
            .map_err(|error| error.to_string())
    }
}

impl WorkspaceLedger {
    pub(super) fn backup_binding(&self) -> [u8; 32] {
        self.binding
    }

    pub(super) fn open_for_backup(&self) -> Result<Option<Connection>> {
        self.connect(false)
    }

    pub fn open(history_path: impl AsRef<Path>) -> Result<Self> {
        let history = history_path.as_ref().canonicalize()?;
        let binding = history_binding(&history)?;
        let mut path = history.as_os_str().to_owned();
        path.push(".workspace.sqlite");
        Ok(Self {
            history,
            path: path.into(),
            binding,
            verifier: None,
        })
    }

    /// Uses an explicit history authority and binds local state to a private
    /// identity file for that authority.
    pub fn open_with_history(
        marker: impl AsRef<Path>,
        verifier: impl WorkspaceHistory + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut ledger = Self::open(marker)?;
        ledger.verifier = Some(Box::new(verifier));
        Ok(ledger)
    }

    pub fn create_task(
        &self,
        anchor: Option<&WorkspaceAnchor>,
        title: &str,
        note: &str,
    ) -> Result<SavedTask> {
        self.create_task_in_workspace(anchor, title, note, None)
    }

    pub fn create_task_in_workspace(
        &self,
        anchor: Option<&WorkspaceAnchor>,
        title: &str,
        note: &str,
        workspace_directory: Option<&str>,
    ) -> Result<SavedTask> {
        validate_title(title)?;
        validate_note(note, true)?;
        if let Some(directory) = workspace_directory {
            validate_workspace_directory(directory)?;
        }
        if let Some(anchor) = anchor {
            self.require_anchors(&[anchor])?;
        }
        let id = self
            .write(true, |connection| {
                insert_task_row(
                    connection,
                    self.binding,
                    anchor,
                    title,
                    note,
                    workspace_directory,
                )
            })?
            .ok_or(WorkspaceError::UnrecognizedLedger)?;
        Ok(SavedTask {
            id: id as u64,
            anchor: anchor.cloned(),
            title: title.into(),
            note: note.into(),
            completed: false,
            archived: false,
            revision: 1,
            workspace_directory: workspace_directory
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        })
    }

    pub fn update_task(&self, id: u64, title: &str, note: &str) -> Result<bool> {
        Ok(self
            .update_task_fields(id, Some(title), Some(note), None, None)?
            .is_some())
    }

    pub fn update_task_fields(
        &self,
        id: u64,
        title: Option<&str>,
        note: Option<&str>,
        workspace_directory: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Result<Option<SavedTask>> {
        if title.is_none() && note.is_none() && workspace_directory.is_none() {
            return Err(WorkspaceError::Invalid(
                "Choose at least one task field to update",
            ));
        }
        if let Some(title) = title {
            validate_title(title)?;
        }
        if let Some(note) = note {
            validate_note(note, true)?;
        }
        if let Some(directory) = workspace_directory {
            validate_workspace_directory(directory)?;
        }
        self.edit_task(id, expected_revision, |connection, current| {
            if workspace_directory.is_some_and(|directory| directory.is_empty() || Some(directory) != current.workspace_directory.as_deref()) {
                connection.execute("DELETE FROM agenturm_task_git_snapshots WHERE task_id = ?1", [id])?;
            }
            connection.execute(
                "UPDATE agenturm_saved_tasks
                 SET title = COALESCE(?2, title), note = COALESCE(?3, note),
                     workspace_directory = CASE WHEN ?4 IS NULL THEN workspace_directory ELSE NULLIF(?4, '') END
                 WHERE id = ?1",
                params![id, title, note, workspace_directory],
            )?;
            Ok(())
        }).map(|saved| saved.map(|(task, ())| task))
    }

    pub fn set_task_archived(
        &self,
        id: u64,
        archived: bool,
        expected_revision: Option<u64>,
    ) -> Result<Option<SavedTask>> {
        self.edit_task(id, expected_revision, |connection, _| {
            connection.execute(
                "UPDATE agenturm_saved_tasks SET archived = ?2 WHERE id = ?1",
                params![id, archived],
            )?;
            Ok(())
        })
        .map(|saved| saved.map(|(task, ())| task))
    }

    pub fn set_task_completed(&self, id: u64, completed: bool) -> Result<bool> {
        Ok(self
            .set_task_completed_if_current(id, completed, None)?
            .is_some())
    }

    pub fn set_task_completed_if_current(
        &self,
        id: u64,
        completed: bool,
        expected_revision: Option<u64>,
    ) -> Result<Option<SavedTask>> {
        self.edit_task(id, expected_revision, |connection, _| {
            connection.execute(
                "UPDATE agenturm_saved_tasks SET completed = ?2 WHERE id = ?1",
                params![id, completed],
            )?;
            Ok(())
        })
        .map(|saved| saved.map(|(task, ())| task))
    }

    pub fn delete_task(&self, id: u64) -> Result<bool> {
        let id = positive_id(id)?;
        self.change(|connection| {
            connection.execute(
                "DELETE FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id = ?2",
                params![self.binding, id],
            )
        })
    }

    pub fn list_tasks(
        &self,
        limit: usize,
        before: Option<u64>,
    ) -> Result<WorkspacePage<SavedTask>> {
        self.list_tasks_including_archived(limit, before, false)
    }

    /// Bounded title-only projection of current, session-level task anchors.
    /// Task excerpts, notes, and turn or worker links stay outside this read.
    pub fn session_task_titles(
        &self,
        local_history: Option<&Store>,
    ) -> Result<Vec<(ExtensionId, String, String)>> {
        let page = self.list(
            |version| match version {
                3..=SCHEMA_VERSION => {
                    "SELECT t.id, l.source_id, l.session_id, t.title
                    FROM agenturm_saved_tasks AS t JOIN agenturm_task_links AS l ON l.task_id=t.id
                    WHERE t.history_binding = ?1 AND t.id <= ?2 AND t.archived = 0
                        AND l.worker_key = '' AND l.turn_key = '' ORDER BY t.id DESC, l.id DESC LIMIT ?3"
                }
                2 => {
                    "SELECT t.id, l.source_id, l.session_id, t.title
                    FROM agenturm_saved_tasks AS t JOIN agenturm_task_links AS l ON l.task_id=t.id
                    WHERE t.history_binding = ?1 AND t.id <= ?2
                        AND l.worker_key = '' AND l.turn_key = '' ORDER BY t.id DESC, l.id DESC LIMIT ?3"
                }
                _ => {
                    "SELECT id, source_id, session_id, title
                    FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id <= ?2
                        AND source_id IS NOT NULL AND worker_id IS NULL AND turn_id IS NULL
                        ORDER BY id DESC LIMIT ?3"
                }
            },
            MAX_WORKSPACE_PAGE_SIZE,
            None,
            |row, _| {
                let source: String = row.get(1)?;
                let id = ExtensionId::parse(source).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
                })?;
                Ok((id, row.get(2)?, row.get(3)?))
            },
        )?;
        if page.next_before.is_some() {
            // A page that omitted tasks cannot resolve conflicting titles.
            return Ok(Vec::new());
        }
        if let Some(expected) = local_history.filter(|_| !page.items.is_empty()) {
            self.verify_local_history(expected, None)?;
        }
        Ok(page.items)
    }

    /// Project candidates from current operator-saved Git snapshots in this
    /// local history. Each nullable root is for an explicit session-level task
    /// link. An invalid candidate stays distinguishable from an absent capture
    /// so it cannot hide a conflict when the caller merges links.
    pub fn session_task_project_roots(
        &self,
        local_history: &Store,
        snapshot: ReadSnapshot,
    ) -> Result<Vec<(WorkspaceAnchor, Option<String>)>> {
        let page = self.list(
            |version| match version {
                3..=SCHEMA_VERSION => {
                    "SELECT t.id, l.source_id, l.session_id, g.repository_root
                    FROM agenturm_saved_tasks AS t JOIN agenturm_task_links AS l ON l.task_id=t.id
                    JOIN agenturm_task_git_snapshots AS g ON g.task_id=t.id
                    WHERE t.history_binding = ?1 AND t.id <= ?2 AND t.archived = 0
                        AND l.worker_key = '' AND l.turn_key = '' ORDER BY t.id DESC, l.id DESC LIMIT ?3"
                }
                2 => {
                    "SELECT t.id, l.source_id, l.session_id, g.repository_root
                    FROM agenturm_saved_tasks AS t JOIN agenturm_task_links AS l ON l.task_id=t.id
                    JOIN agenturm_task_git_snapshots AS g ON g.task_id=t.id
                    WHERE t.history_binding = ?1 AND t.id <= ?2
                        AND l.worker_key = '' AND l.turn_key = '' ORDER BY t.id DESC, l.id DESC LIMIT ?3"
                }
                _ => {
                    "SELECT id, source_id, session_id, NULL FROM agenturm_saved_tasks
                    WHERE history_binding = ?1 AND id <= ?2 AND 0 LIMIT ?3"
                }
            },
            MAX_WORKSPACE_PAGE_SIZE,
            None,
            |row, _| {
                let source: String = row.get(1)?;
                let harness_id = ExtensionId::parse(source).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
                })?;
                let root = row.get_ref(3)?.as_str().ok().filter(|root| {
                    let path = Path::new(root);
                    root.len() <= MAX_ROSTER_PROJECT_PATH_BYTES
                        && path.is_absolute()
                        && display_safe(root, false)
                        && !path.components().any(|part| part == Component::ParentDir)
                });
                Ok((
                    WorkspaceAnchor {
                        harness_id,
                        session_id: row.get(2)?,
                        worker_id: None,
                        turn_id: None,
                    },
                    root.map(str::to_owned),
                ))
            },
        )?;
        if page.next_before.is_some() {
            // Another link outside the bound could disagree with any root.
            return Ok(Vec::new());
        }
        if !page.items.is_empty() {
            self.verify_local_history(local_history, Some(snapshot))?;
        }
        Ok(page.items)
    }

    fn verify_local_history(&self, expected: &Store, snapshot: Option<ReadSnapshot>) -> Result<()> {
        let current = Store::open_read_only(&self.history).map_err(WorkspaceError::History)?;
        let transaction = current.connection.unchecked_transaction()?;
        if expected
            .history_identity()
            .map_err(WorkspaceError::History)?
            != current
                .history_identity()
                .map_err(WorkspaceError::History)?
            || expected
                .read_incarnation()
                .map_err(WorkspaceError::History)?
                != current
                    .read_incarnation()
                    .map_err(WorkspaceError::History)?
        {
            return Err(WorkspaceError::HistoryChanged);
        }
        if let Some(snapshot) = snapshot {
            crate::retention::resolve_read_snapshot(&transaction, Some(snapshot))
                .map_err(WorkspaceError::History)?;
        }
        self.verify_history()?;
        Ok(())
    }

    pub fn list_tasks_including_archived(
        &self,
        limit: usize,
        before: Option<u64>,
        include_archived: bool,
    ) -> Result<WorkspacePage<SavedTask>> {
        self.list(
            |version| match (version, include_archived) {
                (3..=SCHEMA_VERSION, true) => "SELECT id, source_id, session_id, worker_id, turn_id, title, note, completed, archived, revision, workspace_directory
                    FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id <= ?2 ORDER BY id DESC LIMIT ?3",
                (3..=SCHEMA_VERSION, false) => "SELECT id, source_id, session_id, worker_id, turn_id, title, note, completed, archived, revision, workspace_directory
                    FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id <= ?2 AND archived = 0 ORDER BY id DESC LIMIT ?3",
                _ => "SELECT id, source_id, session_id, worker_id, turn_id, title, note, completed, 0, 1, NULL
                    FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id <= ?2 ORDER BY id DESC LIMIT ?3",
            },
            limit,
            before,
            decode_task,
        )
    }

    /// Finds the exact task, including an archived task hidden by the default list.
    pub fn task(&self, id: u64) -> Result<Option<SavedTask>> {
        self.read_task(id, |_, task, _| Ok(task.clone()))
    }

    pub fn task_handoff(&self, id: u64) -> Result<Option<TaskHandoff>> {
        self.task_handoff_with_git_diff(id, false)
    }

    pub fn task_handoff_with_git_diff(
        &self,
        id: u64,
        include_diff: bool,
    ) -> Result<Option<TaskHandoff>> {
        self.read_task(id, |connection, task, version| {
            let mut links =
                load_task_links(connection, task, version, MAX_WORKSPACE_PAGE_SIZE + 1)?;
            let links_truncated = links.len() > MAX_WORKSPACE_PAGE_SIZE;
            links.truncate(MAX_WORKSPACE_PAGE_SIZE);
            let (brief, decisions) = if version >= 3 {
                (
                    load_task_brief(connection, id)?,
                    load_task_decisions(connection, id, version)?,
                )
            } else {
                (TaskBrief::default(), Vec::new())
            };
            Ok(TaskHandoff {
                task: task.clone(),
                brief,
                decisions,
                links,
                links_truncated,
                git_snapshot: load_task_git_snapshot(connection, id, version)?,
                git_diff: if include_diff && version >= 5 {
                    load_task_git_diff(connection, id)?
                } else {
                    None
                },
                continuity: continuity::load_approvals(connection, id, version)?,
            })
        })
    }

    pub fn task_brief(&self, id: u64) -> Result<Option<(SavedTask, TaskBrief)>> {
        self.read_task(id, |connection, task, version| {
            let brief = if version >= 3 {
                load_task_brief(connection, id)?
            } else {
                TaskBrief::default()
            };
            Ok((task.clone(), brief))
        })
    }

    pub fn update_task_brief(
        &self,
        id: u64,
        patch: &TaskBriefUpdate,
        expected_revision: Option<u64>,
    ) -> Result<Option<(SavedTask, TaskBrief)>> {
        for value in [
            &patch.intent,
            &patch.acceptance_criteria,
            &patch.constraints,
            &patch.open_questions,
            &patch.next_action,
        ]
        .into_iter()
        .flatten()
        {
            validate_note(value, true)?;
        }
        self.edit_task(id, expected_revision, |connection, _| {
            let mut brief = load_task_brief(connection, id)?;
            for (field, update) in [
                (&mut brief.intent, &patch.intent),
                (&mut brief.acceptance_criteria, &patch.acceptance_criteria),
                (&mut brief.constraints, &patch.constraints),
                (&mut brief.open_questions, &patch.open_questions),
                (&mut brief.next_action, &patch.next_action),
            ] {
                if let Some(update) = update {
                    field.clone_from(update);
                }
            }
            let encoded = serde_json::to_string(&brief)
                .map_err(|_| WorkspaceError::Invalid("Cannot encode this task brief"))?;
            connection.execute(
                "INSERT INTO agenturm_task_briefs(task_id, brief_json) VALUES (?1, ?2)
                 ON CONFLICT(task_id) DO UPDATE SET brief_json = excluded.brief_json",
                params![id, encoded],
            )?;
            Ok(brief)
        })
    }

    pub fn task_decisions(&self, id: u64) -> Result<Option<(SavedTask, Vec<SavedTaskDecision>)>> {
        self.read_task(id, |connection, task, version| {
            let decisions = if version >= 3 {
                load_task_decisions(connection, id, version)?
            } else {
                Vec::new()
            };
            Ok((task.clone(), decisions))
        })
    }

    pub fn add_task_decision(
        &self,
        id: u64,
        decision: &TaskDecisionInput,
        expected_revision: Option<u64>,
    ) -> Result<Option<(SavedTask, SavedTaskDecision)>> {
        self.add_task_decision_with_source(id, decision, expected_revision, None)
    }

    pub fn add_task_decision_with_source(
        &self,
        id: u64,
        decision: &TaskDecisionInput,
        expected_revision: Option<u64>,
        source: Option<&TaskSourceProof>,
    ) -> Result<Option<(SavedTask, SavedTaskDecision)>> {
        validate_note(&decision.text, false)?;
        validate_note(&decision.rationale, true)?;
        validate_note(&decision.scope, true)?;
        let supersedes = decision.supersedes_id.map(positive_id).transpose()?;
        let source_json = source.map(continuity::encode_proof).transpose()?;
        let created_at_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_nanos()).ok())
            .filter(|time| *time <= i64::MAX as u64 && *time > 0)
            .ok_or(WorkspaceError::Invalid(
                "Cannot timestamp the task decision",
            ))?;
        self.edit_task(id, expected_revision, |connection, _| {
            let (count, bytes): (usize, usize) = connection.query_row(
                "SELECT count(*), COALESCE(sum(length(CAST(text AS BLOB)) + length(CAST(rationale AS BLOB))
                     + length(CAST(scope AS BLOB))), 0) FROM agenturm_task_decisions WHERE task_id = ?1",
                [id], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let appended = decision.text.len() + decision.rationale.len() + decision.scope.len();
            if count >= MAX_DECISION_VERSIONS || bytes.saturating_add(appended) > MAX_DECISION_BYTES {
                return Err(WorkspaceError::Invalid("This task has reached its retained decision history limit"));
            }
            if let Some(previous) = supersedes {
                let valid = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM agenturm_task_decisions WHERE task_id = ?1 AND id = ?2)
                        AND NOT EXISTS(SELECT 1 FROM agenturm_task_decisions WHERE task_id = ?1 AND supersedes_id = ?2)",
                    params![id, previous], |row| row.get::<_, bool>(0),
                )?;
                if !valid { return Err(WorkspaceError::Invalid("Supersede a current decision in this task")); }
            }
            let saved = connection.query_row(
                "INSERT INTO agenturm_task_decisions(task_id, kind, text, rationale, scope, created_at_unix_ns, supersedes_id, source_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
                params![id, decision.kind.as_str(), decision.text, decision.rationale, decision.scope,
                    created_at_unix_ns, supersedes, source_json], |row| row.get(0),
            )?;
            Ok(SavedTaskDecision { id: saved, created_at_unix_ns, decision: decision.clone(), source: source.cloned() })
        })
    }

    /// Adds a verified session or exact worker turn to a saved task.
    /// Returns false for duplicate links or a task that was removed.
    pub fn link_task(&self, id: u64, anchor: &WorkspaceAnchor) -> Result<bool> {
        Ok(self
            .link_task_if_current(id, anchor, None)?
            .is_some_and(|(_, changed)| changed))
    }

    pub fn link_task_if_current(
        &self,
        id: u64,
        anchor: &WorkspaceAnchor,
        expected_revision: Option<u64>,
    ) -> Result<Option<(SavedTask, bool)>> {
        let signed_id = positive_id(id)?;
        self.require_anchors(&[anchor])?;
        self.edit_task_with_change(id, expected_revision, |connection, _| {
            let changed = add_task_link(connection, self.binding, signed_id, anchor)?;
            Ok((changed, changed))
        })
    }

    /// Removes the exact link. If it was primary, the newest remaining link
    /// becomes primary. Links to expired history can still be removed.
    pub fn unlink_task(&self, id: u64, anchor: &WorkspaceAnchor) -> Result<bool> {
        Ok(self
            .unlink_task_if_current(id, anchor, None)?
            .is_some_and(|(_, changed)| changed))
    }

    pub fn unlink_task_if_current(
        &self,
        id: u64,
        anchor: &WorkspaceAnchor,
        expected_revision: Option<u64>,
    ) -> Result<Option<(SavedTask, bool)>> {
        validate_anchor(anchor)?;
        self.edit_task_with_change(id, expected_revision, |connection, _| {
            let changed = connection.execute(
                "DELETE FROM agenturm_task_links
                 WHERE task_id = ?2 AND source_id = ?3 AND session_id = ?4
                   AND worker_key = ?5 AND turn_key = ?6
                   AND task_id IN (SELECT id FROM agenturm_saved_tasks WHERE history_binding = ?1)",
                params![
                    self.binding,
                    id,
                    anchor.harness_id.as_str(),
                    anchor.session_id,
                    anchor.worker_id.as_deref().unwrap_or(""),
                    anchor.turn_id.as_deref().unwrap_or("")
                ],
            )?;
            if changed != 0 {
                connection.execute(
                    "UPDATE agenturm_saved_tasks SET (source_id, session_id, worker_id, turn_id) = (
                       SELECT source_id, session_id, NULLIF(worker_key, ''), NULLIF(turn_key, '')
                       FROM agenturm_task_links WHERE task_id = ?2 ORDER BY id DESC LIMIT 1
                     )
                     WHERE history_binding = ?1 AND id = ?2 AND source_id = ?3 AND session_id = ?4
                       AND worker_id IS ?5 AND turn_id IS ?6",
                    params![self.binding, id, anchor.harness_id.as_str(), anchor.session_id,
                        anchor.worker_id, anchor.turn_id],
                )?;
            }
            Ok((changed != 0, changed != 0))
        })
    }

    pub fn task_links(&self, id: u64, limit: usize) -> Result<Vec<WorkspaceAnchor>> {
        if !(1..=MAX_WORKSPACE_PAGE_SIZE).contains(&limit) {
            return Err(WorkspaceError::Invalid(
                "workspace task-link limit is invalid",
            ));
        }
        Ok(self
            .read_task(id, |connection, task, version| {
                load_task_links(connection, task, version, limit)
            })?
            .unwrap_or_default())
    }

    pub fn save_task_workspace_snapshot(
        &self,
        id: u64,
        snapshot: &WorkspaceGitSnapshot,
    ) -> Result<bool> {
        Ok(self
            .save_task_workspace_snapshot_if_current(id, snapshot, None)?
            .is_some())
    }

    pub fn save_task_workspace_snapshot_if_current(
        &self,
        id: u64,
        snapshot: &WorkspaceGitSnapshot,
        expected_revision: Option<u64>,
    ) -> Result<Option<SavedTask>> {
        self.save_task_workspace_capture_if_current(id, snapshot, None, expected_revision)
    }

    pub fn save_task_workspace_capture_if_current(
        &self,
        id: u64,
        snapshot: &WorkspaceGitSnapshot,
        diff: Option<&WorkspaceGitDiff>,
        expected_revision: Option<u64>,
    ) -> Result<Option<SavedTask>> {
        validate_git_snapshot(snapshot)?;
        if let Some(diff) = diff {
            diff.validate()?;
        }
        self.edit_task(id, expected_revision, |connection, _| {
            connection.execute(
                "INSERT INTO agenturm_task_git_snapshots
                    (task_id, repository_root, branch, head_commit, staged_changes,
                     unstaged_changes, untracked_files, captured_at_unix_ns)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (task_id) DO UPDATE SET
                     repository_root = excluded.repository_root, branch = excluded.branch,
                     head_commit = excluded.head_commit, staged_changes = excluded.staged_changes,
                     unstaged_changes = excluded.unstaged_changes, untracked_files = excluded.untracked_files,
                     captured_at_unix_ns = excluded.captured_at_unix_ns",
                params![id, snapshot.repository_root, snapshot.branch, snapshot.head_commit,
                    snapshot.staged_changes, snapshot.unstaged_changes, snapshot.untracked_files,
                    snapshot.captured_at_unix_ns],
            )?;
            if let Some(diff) = diff {
                connection.execute(
                    "INSERT INTO agenturm_task_git_diffs (task_id, staged, unstaged)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (task_id) DO UPDATE SET staged = excluded.staged, unstaged = excluded.unstaged",
                    params![id, diff.staged, diff.unstaged],
                )?;
            } else {
                connection.execute("DELETE FROM agenturm_task_git_diffs WHERE task_id = ?1", [id])?;
            }
            Ok(())
        }).map(|saved| saved.map(|(task, ())| task))
    }

    pub fn task_workspace_snapshot(&self, id: u64) -> Result<Option<WorkspaceGitSnapshot>> {
        self.read_task(id, |connection, _, version| {
            load_task_git_snapshot(connection, id, version)
        })
        .map(Option::flatten)
    }

    pub fn create_handoff(
        &self,
        from: &WorkspaceAnchor,
        to: &WorkspaceAnchor,
        note: &str,
    ) -> Result<SavedHandoff> {
        if from.harness_id == to.harness_id && from.session_id == to.session_id {
            return Err(WorkspaceError::Invalid(
                "a handoff requires two distinct sessions",
            ));
        }
        validate_note(note, false)?;
        self.require_anchors(&[from, to])?;
        let id = self
            .write(true, |connection| {
                connection.query_row(
                    "INSERT INTO agenturm_saved_handoffs
                    (history_binding, from_source_id, from_session_id, from_worker_id, from_turn_id,
                     to_source_id, to_session_id, to_worker_id, to_turn_id, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
                    params![
                        self.binding,
                        from.harness_id.as_str(),
                        from.session_id,
                        from.worker_id,
                        from.turn_id,
                        to.harness_id.as_str(),
                        to.session_id,
                        to.worker_id,
                        to.turn_id,
                        note
                    ],
                    |row| row.get(0),
                )
            })?
            .ok_or(WorkspaceError::UnrecognizedLedger)?;
        Ok(SavedHandoff {
            id,
            from: from.clone(),
            to: to.clone(),
            note: note.into(),
        })
    }

    pub fn update_handoff(&self, id: u64, note: &str) -> Result<bool> {
        let id = positive_id(id)?;
        validate_note(note, false)?;
        self.change(|connection| {
            connection.execute(
                "UPDATE agenturm_saved_handoffs SET note = ?3
                 WHERE history_binding = ?1 AND id = ?2",
                params![self.binding, id, note],
            )
        })
    }

    pub fn delete_handoff(&self, id: u64) -> Result<bool> {
        let id = positive_id(id)?;
        self.change(|connection| {
            connection.execute(
                "DELETE FROM agenturm_saved_handoffs WHERE history_binding = ?1 AND id = ?2",
                params![self.binding, id],
            )
        })
    }

    pub fn list_handoffs(
        &self,
        limit: usize,
        before: Option<u64>,
    ) -> Result<WorkspacePage<SavedHandoff>> {
        self.list(
            |_| {
                "SELECT id, from_source_id, from_session_id, from_worker_id, from_turn_id,
                    to_source_id, to_session_id, to_worker_id, to_turn_id, note
             FROM agenturm_saved_handoffs
             WHERE history_binding = ?1 AND id <= ?2
             ORDER BY id DESC LIMIT ?3"
            },
            limit,
            before,
            |row, id| {
                Ok(SavedHandoff {
                    id,
                    from: decode_anchor(row, 1)?,
                    to: decode_anchor(row, 5)?,
                    note: row.get(9)?,
                })
            },
        )
    }

    /// Marks the exact retained revision viewed by the operator. The caller
    /// classifies which evidence is a request. A revised event is a new key.
    pub fn mark_request_seen(
        &self,
        anchor: &WorkspaceAnchor,
        event_id: &str,
        revision: u32,
    ) -> Result<bool> {
        validate_request_key(anchor, event_id)?;
        let found = if let Some(verifier) = &self.verifier {
            verifier
                .contains_event(anchor, event_id, revision)
                .map_err(WorkspaceError::HistoryUnavailable)?
        } else {
            let history = Store::open_read_only(&self.history).map_err(WorkspaceError::History)?;
            contains_event_in(&history.connection, anchor, event_id, revision)?
        };
        if !found {
            return Err(WorkspaceError::MissingAnchor);
        }
        let changed = self.write(true, |connection| {
            connection.execute(
                "INSERT INTO agenturm_seen_requests
                    (history_binding, source_id, session_id, worker_id, turn_id, event_id, revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (history_binding, source_id, event_id, revision) DO NOTHING",
                params![
                    self.binding,
                    anchor.harness_id.as_str(),
                    anchor.session_id,
                    anchor.worker_id,
                    anchor.turn_id,
                    event_id,
                    revision
                ],
            )
        })?;
        Ok(changed.is_some_and(|count| count != 0))
    }

    pub fn seen_requests(
        &self,
        keys: &[(WorkspaceAnchor, String, u32)],
    ) -> Result<BTreeSet<(WorkspaceAnchor, String, u32)>> {
        if keys.len() > MAX_WORKSPACE_PAGE_SIZE {
            return Err(WorkspaceError::Invalid(
                "workspace request lookup exceeds its page limit",
            ));
        }
        for (anchor, event_id, _) in keys {
            validate_request_key(anchor, event_id)?;
        }
        let mut seen = BTreeSet::new();
        if keys.is_empty() {
            return Ok(seen);
        }
        let Some(mut connection) = self.connect(false)? else {
            return Ok(seen);
        };
        let transaction = connection.transaction()?;
        if initialized(&transaction)? {
            let mut query = transaction.prepare(
                "SELECT EXISTS(
                     SELECT 1 FROM agenturm_seen_requests
                     WHERE history_binding = ?1 AND source_id = ?2 AND event_id = ?3
                       AND revision = ?4 AND session_id = ?5 AND worker_id IS ?6 AND turn_id IS ?7
                 )",
            )?;
            for key @ (anchor, event_id, revision) in keys {
                let found = query.query_row(
                    params![
                        self.binding,
                        anchor.harness_id.as_str(),
                        event_id,
                        revision,
                        anchor.session_id,
                        anchor.worker_id,
                        anchor.turn_id
                    ],
                    |row| row.get::<_, bool>(0),
                )?;
                if found {
                    seen.insert(key.clone());
                }
            }
        }
        self.verify_history()?;
        transaction.commit()?;
        Ok(seen)
    }

    fn list<T>(
        &self,
        sql: impl FnOnce(i64) -> &'static str,
        limit: usize,
        before: Option<u64>,
        decode: impl Fn(&Row<'_>, u64) -> rusqlite::Result<T>,
    ) -> Result<WorkspacePage<T>> {
        if !(1..=MAX_WORKSPACE_PAGE_SIZE).contains(&limit) {
            return Err(WorkspaceError::Invalid("workspace page limit is invalid"));
        }
        let upper_id = before
            .map(positive_id)
            .transpose()?
            .map_or(i64::MAX, |id| id - 1);
        let mut page = WorkspacePage {
            items: Vec::new(),
            next_before: None,
        };
        let Some(mut connection) = self.connect(false)? else {
            return Ok(page);
        };
        let transaction = connection.transaction()?;
        let version = schema_version(&transaction)?;
        if version != 0 {
            let mut statement = transaction.prepare(sql(version))?;
            let mut rows = statement.query(params![self.binding, upper_id, (limit + 1) as i64])?;
            let mut last = None;
            while let Some(row) = rows.next()? {
                if page.items.len() == limit {
                    page.next_before = last;
                    break;
                }
                let id = row.get(0)?;
                page.items.push(decode(row, id)?);
                last = Some(id);
            }
        }
        self.verify_history()?;
        transaction.commit()?;
        Ok(page)
    }

    fn require_anchors(&self, anchors: &[&WorkspaceAnchor]) -> Result<()> {
        if let Some(verifier) = &self.verifier {
            for anchor in anchors {
                validate_link_anchor(anchor)?;
                if !verifier
                    .contains_anchor(anchor)
                    .map_err(WorkspaceError::HistoryUnavailable)?
                {
                    return Err(WorkspaceError::MissingAnchor);
                }
            }
            return Ok(());
        }
        let history = Store::open_read_only(&self.history).map_err(WorkspaceError::History)?;
        let transaction = history.connection.unchecked_transaction()?;
        for anchor in anchors {
            if !contains_anchor_in(&transaction, anchor)? {
                return Err(WorkspaceError::MissingAnchor);
            }
        }
        Ok(())
    }

    fn change(&self, action: impl FnOnce(&Connection) -> rusqlite::Result<usize>) -> Result<bool> {
        Ok(self
            .write(false, action)?
            .is_some_and(|changed| changed != 0))
    }

    fn read_task<T>(
        &self,
        id: u64,
        action: impl FnOnce(&Connection, &SavedTask, i64) -> Result<T>,
    ) -> Result<Option<T>> {
        let id = positive_id(id)?;
        let Some(mut connection) = self.connect(false)? else {
            return Ok(None);
        };
        let transaction = connection.transaction()?;
        let version = schema_version(&transaction)?;
        let result = select_task(&transaction, &self.binding, id, version)?
            .map(|task| action(&transaction, &task, version))
            .transpose()?;
        self.verify_history()?;
        transaction.commit()?;
        Ok(result)
    }

    fn edit_task<T>(
        &self,
        id: u64,
        expected: Option<u64>,
        action: impl FnOnce(&Connection, &SavedTask) -> Result<T>,
    ) -> Result<Option<(SavedTask, T)>> {
        self.edit_task_with_change(id, expected, |connection, task| {
            action(connection, task).map(|value| (value, true))
        })
    }

    fn edit_task_with_change<T>(
        &self,
        id: u64,
        expected: Option<u64>,
        action: impl FnOnce(&Connection, &SavedTask) -> Result<(T, bool)>,
    ) -> Result<Option<(SavedTask, T)>> {
        let id = positive_id(id)?;
        self.write_workspace(false, |connection| {
            let Some(task) = select_task(connection, &self.binding, id, SCHEMA_VERSION)? else {
                return Ok(None);
            };
            if let Some(expected) = expected
                && expected != task.revision
            {
                return Err(WorkspaceError::TaskRevisionConflict {
                    expected,
                    current: task.revision,
                });
            }
            let (value, changed) = action(connection, &task)?;
            if !changed {
                return Ok(Some((task, value)));
            }
            connection.execute(
                "UPDATE agenturm_saved_tasks SET revision = revision + 1 WHERE id = ?1",
                [id],
            )?;
            let updated = select_task(connection, &self.binding, id, SCHEMA_VERSION)?
                .ok_or(WorkspaceError::UnrecognizedLedger)?;
            Ok(Some((updated, value)))
        })
        .map(Option::flatten)
    }

    fn write<T>(
        &self,
        create: bool,
        action: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.write_workspace(create, |connection| Ok(action(connection)?))
    }

    fn write_workspace<T>(
        &self,
        create: bool,
        action: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<Option<T>> {
        if create {
            self.verify_history()?;
            create_private_file(&self.path)?;
        }
        let Some(mut connection) = self.connect(true)? else {
            return Ok(None);
        };
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version = schema_version(&transaction)?;
        if version == 0 {
            if !create {
                return Ok(None);
            }
            create_schema(&transaction)?;
        }
        if version < 2 {
            create_task_extensions(&transaction)?;
        }
        if version < 3 {
            create_task_planning(&transaction)?;
        }
        if version < 4 {
            continuity::create_schema(&transaction)?;
        }
        if version < 5 {
            create_task_git_diffs(&transaction)?;
        }
        let value = action(&transaction)?;
        self.verify_history()?;
        transaction.commit()?;
        Ok(Some(value))
    }

    fn verify_history(&self) -> Result<()> {
        if history_binding(&self.history)? != self.binding {
            return Err(WorkspaceError::HistoryChanged);
        }
        Ok(())
    }

    fn connect(&self, write: bool) -> Result<Option<Connection>> {
        self.verify_history()?;
        let file = match fs::symlink_metadata(&self.path) {
            Ok(file) if file.is_file() => file,
            Ok(_) => return Err(WorkspaceError::UnrecognizedLedger),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if file.nlink() != 1 || file.mode() & 0o077 != 0 {
                return Err(WorkspaceError::UnrecognizedLedger);
            }
        }
        #[cfg(not(unix))]
        let _ = file;
        let mode = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let connection =
            Connection::open_with_flags(&self.path, mode | OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
        connection.busy_timeout(Duration::from_millis(500))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        if write {
            connection.pragma_update(None, "secure_delete", true)?;
        }
        Ok(Some(connection))
    }
}

fn contains_anchor_in(connection: &Connection, anchor: &WorkspaceAnchor) -> Result<bool> {
    validate_link_anchor(anchor)?;
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agenturm_observations
            WHERE source_id = ?1 AND session_id = ?2
              AND (?3 IS NULL OR (turn_id = ?3 AND worker_id IS ?4)))",
        params![
            anchor.harness_id.as_str(),
            anchor.session_id,
            anchor.turn_id,
            anchor.worker_id
        ],
        |row| row.get(0),
    )?)
}

fn contains_event_in(
    connection: &Connection,
    anchor: &WorkspaceAnchor,
    event_id: &str,
    revision: u32,
) -> Result<bool> {
    validate_request_key(anchor, event_id)?;
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agenturm_observations
            WHERE source_id = ?1 AND session_id = ?2 AND worker_id IS ?3 AND turn_id IS ?4
              AND event_id = ?5 AND revision = ?6)",
        params![
            anchor.harness_id.as_str(),
            anchor.session_id,
            anchor.worker_id,
            anchor.turn_id,
            event_id,
            revision
        ],
        |row| row.get(0),
    )?)
}

fn validate_link_anchor(anchor: &WorkspaceAnchor) -> Result<()> {
    validate_anchor(anchor)?;
    if anchor.worker_id.is_some() && anchor.turn_id.is_none() {
        return Err(WorkspaceError::Invalid(
            "workspace worker anchors require a turn",
        ));
    }
    Ok(())
}

fn decode_anchor(row: &Row<'_>, column: usize) -> rusqlite::Result<WorkspaceAnchor> {
    decode_scoped_anchor(row, column, row.get(column)?)
}

fn decode_task(row: &Row<'_>, id: u64) -> rusqlite::Result<SavedTask> {
    Ok(SavedTask {
        id,
        anchor: decode_optional_anchor(row, 1)?,
        title: row.get(5)?,
        note: row.get(6)?,
        completed: row.get(7)?,
        archived: row.get(8)?,
        revision: row.get(9)?,
        workspace_directory: row.get(10)?,
    })
}

fn select_task(
    connection: &Connection,
    binding: &[u8; 32],
    id: i64,
    version: i64,
) -> Result<Option<SavedTask>> {
    let sql = match version {
        0 => return Ok(None),
        1..=2 => "SELECT id, source_id, session_id, worker_id, turn_id, title, note, completed, 0, 1, NULL
                  FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id = ?2",
        _ => "SELECT id, source_id, session_id, worker_id, turn_id, title, note, completed, archived, revision, workspace_directory
              FROM agenturm_saved_tasks WHERE history_binding = ?1 AND id = ?2",
    };
    Ok(connection
        .query_row(sql, params![binding, id], |row| decode_task(row, id as u64))
        .optional()?)
}

fn load_task_brief(connection: &Connection, id: u64) -> Result<TaskBrief> {
    let encoded: Option<String> = connection
        .query_row(
            "SELECT brief_json FROM agenturm_task_briefs WHERE task_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    encoded
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map(|brief| brief.unwrap_or_default())
        .map_err(|_| WorkspaceError::UnrecognizedLedger)
}

fn load_task_links(
    connection: &Connection,
    task: &SavedTask,
    version: i64,
    limit: usize,
) -> Result<Vec<WorkspaceAnchor>> {
    if version == 1 {
        return Ok(task.anchor.clone().into_iter().collect());
    }
    let mut statement = connection.prepare(
        "SELECT source_id, session_id, NULLIF(worker_key, ''), NULLIF(turn_key, '')
         FROM agenturm_task_links WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    Ok(statement
        .query_map(params![task.id, limit], |row| decode_anchor(row, 0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn load_task_git_snapshot(
    connection: &Connection,
    id: u64,
    version: i64,
) -> Result<Option<WorkspaceGitSnapshot>> {
    if version < 2 {
        return Ok(None);
    }
    Ok(connection.query_row(
        "SELECT repository_root, branch, head_commit, staged_changes, unstaged_changes, untracked_files, captured_at_unix_ns
         FROM agenturm_task_git_snapshots WHERE task_id = ?1", [id],
        |row| Ok(WorkspaceGitSnapshot {
            repository_root: row.get(0)?, branch: row.get(1)?, head_commit: row.get(2)?,
            staged_changes: row.get(3)?, unstaged_changes: row.get(4)?, untracked_files: row.get(5)?, captured_at_unix_ns: row.get(6)?,
        }),
    ).optional()?)
}

fn load_task_git_diff(connection: &Connection, id: u64) -> Result<Option<WorkspaceGitDiff>> {
    let diff = connection
        .query_row(
            "SELECT staged, unstaged FROM agenturm_task_git_diffs WHERE task_id = ?1",
            [id],
            |row| {
                Ok(WorkspaceGitDiff {
                    staged: row.get(0)?,
                    unstaged: row.get(1)?,
                })
            },
        )
        .optional()?;
    if let Some(diff) = &diff {
        diff.validate()?;
    }
    Ok(diff)
}

fn load_task_decisions(
    connection: &Connection,
    id: u64,
    version: i64,
) -> Result<Vec<SavedTaskDecision>> {
    let sql = if version >= 4 {
        "SELECT id, kind, text, rationale, scope, created_at_unix_ns, supersedes_id, source_json
         FROM agenturm_task_decisions WHERE task_id = ?1 ORDER BY id LIMIT ?2"
    } else {
        "SELECT id, kind, text, rationale, scope, created_at_unix_ns, supersedes_id, NULL
         FROM agenturm_task_decisions WHERE task_id = ?1 ORDER BY id LIMIT ?2"
    };
    let mut statement = connection.prepare(sql)?;
    Ok(statement
        .query_map(params![id, MAX_DECISION_VERSIONS], |row| {
            let kind: String = row.get(1)?;
            let kind = match kind.as_str() {
                "accepted" => TaskDecisionKind::Accepted,
                "rejected" => TaskDecisionKind::Rejected,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(SavedTaskDecision {
                id: row.get(0)?,
                created_at_unix_ns: row.get(5)?,
                decision: TaskDecisionInput {
                    kind,
                    text: row.get(2)?,
                    rationale: row.get(3)?,
                    scope: row.get(4)?,
                    supersedes_id: row.get(6)?,
                },
                source: continuity::decode_proof(row, 7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn decode_optional_anchor(
    row: &Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<WorkspaceAnchor>> {
    row.get::<_, Option<String>>(column)?
        .map(|source| decode_scoped_anchor(row, column, source))
        .transpose()
}

fn decode_scoped_anchor(
    row: &Row<'_>,
    column: usize,
    source: String,
) -> rusqlite::Result<WorkspaceAnchor> {
    let harness_id = ExtensionId::parse(source).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })?;
    Ok(WorkspaceAnchor {
        harness_id,
        session_id: row.get(column + 1)?,
        worker_id: row.get(column + 2)?,
        turn_id: row.get(column + 3)?,
    })
}

fn validate_request_key(anchor: &WorkspaceAnchor, event_id: &str) -> Result<()> {
    validate_anchor(anchor)?;
    validate_identifier(event_id, MAX_EVENT_ID_BYTES, "request event ID")
        .map_err(|_| WorkspaceError::Invalid("request event ID is invalid"))
}

fn validate_anchor(anchor: &WorkspaceAnchor) -> Result<()> {
    for id in [
        Some(anchor.session_id.as_str()),
        anchor.worker_id.as_deref(),
        anchor.turn_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_identifier(id, MAX_SESSION_KEY_BYTES, "workspace anchor")
            .map_err(|_| WorkspaceError::Invalid("workspace anchor is invalid"))?;
    }
    Ok(())
}

fn validate_title(title: &str) -> Result<()> {
    if title.trim().is_empty() || title.len() > MAX_TITLE_BYTES || !display_safe(title, false) {
        return Err(WorkspaceError::Invalid(
            "task title is empty, too long, or contains control characters",
        ));
    }
    Ok(())
}

fn validate_note(note: &str, allow_empty: bool) -> Result<()> {
    if (!allow_empty && note.trim().is_empty())
        || note.len() > MAX_NOTE_BYTES
        || !display_safe(note, true)
    {
        return Err(WorkspaceError::Invalid(
            "workspace note is empty, too long, or contains control characters",
        ));
    }
    Ok(())
}

fn display_safe(text: &str, multiline: bool) -> bool {
    text.chars().all(|c| {
        (multiline && matches!(c, '\n' | '\t'))
            || (!c.is_control()
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    })
}

fn positive_id(id: u64) -> Result<i64> {
    i64::try_from(id)
        .ok()
        .filter(|id| *id > 0)
        .ok_or(WorkspaceError::Invalid("workspace item ID is invalid"))
}

fn initialized(connection: &Connection) -> Result<bool> {
    Ok(schema_version(connection)? != 0)
}

pub(super) fn schema_version(connection: &Connection) -> Result<i64> {
    let application_id = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match (application_id, version) {
        (APPLICATION_ID, 1..=SCHEMA_VERSION) => Ok(version),
        (0, 0) => {
            let has_schema =
                connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get::<_, bool>(0)
                })?;
            if has_schema {
                Err(WorkspaceError::UnrecognizedLedger)
            } else {
                Ok(0)
            }
        }
        _ => Err(WorkspaceError::UnrecognizedLedger),
    }
}

fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_saved_tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            history_binding BLOB NOT NULL CHECK (length(history_binding) = 32),
            source_id TEXT,
            session_id TEXT,
            worker_id TEXT,
            turn_id TEXT,
            title TEXT NOT NULL,
            note TEXT NOT NULL,
            completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
            CHECK ((source_id IS NULL AND session_id IS NULL AND worker_id IS NULL AND turn_id IS NULL)
                OR (source_id IS NOT NULL AND session_id IS NOT NULL AND
                    (worker_id IS NULL OR turn_id IS NOT NULL)))
         );
         CREATE INDEX agenturm_saved_tasks_by_history ON agenturm_saved_tasks(history_binding, id);
         CREATE TABLE agenturm_saved_handoffs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            history_binding BLOB NOT NULL CHECK (length(history_binding) = 32),
            from_source_id TEXT NOT NULL,
            from_session_id TEXT NOT NULL,
            from_worker_id TEXT,
            from_turn_id TEXT,
            to_source_id TEXT NOT NULL,
            to_session_id TEXT NOT NULL,
            to_worker_id TEXT,
            to_turn_id TEXT,
            note TEXT NOT NULL
         );
         CREATE INDEX agenturm_saved_handoffs_by_history ON agenturm_saved_handoffs(history_binding, id);
         CREATE TABLE agenturm_seen_requests (
            history_binding BLOB NOT NULL CHECK (length(history_binding) = 32),
            source_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            worker_id TEXT,
            turn_id TEXT,
            event_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision >= 0),
            PRIMARY KEY (history_binding, source_id, event_id, revision)
         ) WITHOUT ROWID;",
    )?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", 1)?;
    Ok(())
}

fn create_task_extensions(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_task_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES agenturm_saved_tasks(id) ON DELETE CASCADE,
            source_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            worker_key TEXT NOT NULL,
            turn_key TEXT NOT NULL,
            UNIQUE(task_id, source_id, session_id, worker_key, turn_key)
         );
         CREATE INDEX agenturm_task_links_by_task ON agenturm_task_links(task_id, id);
         INSERT INTO agenturm_task_links (task_id, source_id, session_id, worker_key, turn_key)
         SELECT id, source_id, session_id, COALESCE(worker_id, ''), COALESCE(turn_id, '')
         FROM agenturm_saved_tasks WHERE source_id IS NOT NULL;
         CREATE TABLE agenturm_task_git_snapshots (
            task_id INTEGER PRIMARY KEY REFERENCES agenturm_saved_tasks(id) ON DELETE CASCADE,
            repository_root TEXT NOT NULL,
            branch TEXT,
            head_commit TEXT,
            staged_changes INTEGER NOT NULL CHECK (staged_changes >= 0),
            unstaged_changes INTEGER NOT NULL CHECK (unstaged_changes >= 0),
            untracked_files INTEGER NOT NULL CHECK (untracked_files >= 0),
            captured_at_unix_ns INTEGER NOT NULL CHECK (captured_at_unix_ns > 0)
         );",
    )?;
    connection.pragma_update(None, "user_version", 2)?;
    Ok(())
}

fn create_task_planning(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "ALTER TABLE agenturm_saved_tasks ADD COLUMN archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1));
         ALTER TABLE agenturm_saved_tasks ADD COLUMN revision INTEGER NOT NULL DEFAULT 1
            CHECK (typeof(revision) = 'integer' AND revision > 0);
         ALTER TABLE agenturm_saved_tasks ADD COLUMN workspace_directory TEXT;
         CREATE TABLE agenturm_task_briefs (
            task_id INTEGER PRIMARY KEY REFERENCES agenturm_saved_tasks(id) ON DELETE CASCADE,
            brief_json TEXT NOT NULL
         );
         CREATE TABLE agenturm_task_decisions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES agenturm_saved_tasks(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK (kind IN ('accepted', 'rejected')),
            text TEXT NOT NULL,
            rationale TEXT NOT NULL,
            scope TEXT NOT NULL,
            created_at_unix_ns INTEGER NOT NULL CHECK (created_at_unix_ns > 0),
            supersedes_id INTEGER,
            UNIQUE(task_id, id),
            UNIQUE(task_id, supersedes_id),
            FOREIGN KEY (task_id, supersedes_id) REFERENCES agenturm_task_decisions(task_id, id)
         );",
    )?;
    connection.pragma_update(None, "user_version", 3)?;
    Ok(())
}

fn create_task_git_diffs(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_task_git_diffs (
            task_id INTEGER PRIMARY KEY REFERENCES agenturm_task_git_snapshots(task_id) ON DELETE CASCADE,
            staged TEXT NOT NULL,
            unstaged TEXT NOT NULL,
            CHECK (length(CAST(staged AS BLOB)) + length(CAST(unstaged AS BLOB)) <= 65536)
         );",
    )?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn insert_task_row(
    connection: &Connection,
    binding: [u8; 32],
    anchor: Option<&WorkspaceAnchor>,
    title: &str,
    note: &str,
    workspace_directory: Option<&str>,
) -> rusqlite::Result<i64> {
    let id = connection.query_row(
        "INSERT INTO agenturm_saved_tasks
         (history_binding, source_id, session_id, worker_id, turn_id, title, note, workspace_directory)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULLIF(?8, '')) RETURNING id",
        params![binding, anchor.map(|anchor| anchor.harness_id.as_str()), anchor.map(|anchor| anchor.session_id.as_str()),
            anchor.and_then(|anchor| anchor.worker_id.as_deref()), anchor.and_then(|anchor| anchor.turn_id.as_deref()),
            title, note, workspace_directory],
        |row| row.get(0),
    )?;
    if let Some(anchor) = anchor {
        insert_task_link(connection, binding, id, anchor)?;
    }
    Ok(id)
}

fn add_task_link(
    connection: &Connection,
    binding: [u8; 32],
    id: i64,
    anchor: &WorkspaceAnchor,
) -> rusqlite::Result<bool> {
    let changed = insert_task_link(connection, binding, id, anchor)? != 0;
    if changed {
        connection.execute(
            "UPDATE agenturm_saved_tasks SET source_id = ?3, session_id = ?4, worker_id = ?5, turn_id = ?6
             WHERE history_binding = ?1 AND id = ?2 AND source_id IS NULL",
            params![binding, id, anchor.harness_id.as_str(), anchor.session_id, anchor.worker_id, anchor.turn_id],
        )?;
    }
    Ok(changed)
}

fn insert_task_link(
    connection: &Connection,
    binding: [u8; 32],
    id: i64,
    anchor: &WorkspaceAnchor,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO agenturm_task_links (task_id, source_id, session_id, worker_key, turn_key)
         SELECT id, ?3, ?4, ?5, ?6 FROM agenturm_saved_tasks
         WHERE history_binding = ?1 AND id = ?2
         ON CONFLICT (task_id, source_id, session_id, worker_key, turn_key) DO NOTHING",
        params![
            binding,
            id,
            anchor.harness_id.as_str(),
            anchor.session_id,
            anchor.worker_id.as_deref().unwrap_or(""),
            anchor.turn_id.as_deref().unwrap_or("")
        ],
    )
}

fn validate_git_snapshot(snapshot: &WorkspaceGitSnapshot) -> Result<()> {
    if !Path::new(&snapshot.repository_root).is_absolute()
        || snapshot.repository_root.len() > MAX_NOTE_BYTES
        || !display_safe(&snapshot.repository_root, false)
    {
        return Err(WorkspaceError::Invalid("workspace Git path is invalid"));
    }
    if snapshot.branch.as_deref().is_some_and(|branch| {
        branch.is_empty() || branch.len() > MAX_SESSION_KEY_BYTES || !display_safe(branch, false)
    }) {
        return Err(WorkspaceError::Invalid("workspace Git branch is invalid"));
    }
    if snapshot.head_commit.as_deref().is_some_and(|commit| {
        !matches!(commit.len(), 40 | 64)
            || !commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(WorkspaceError::Invalid("workspace Git commit is invalid"));
    }
    if snapshot.captured_at_unix_ns == 0 || snapshot.captured_at_unix_ns > i64::MAX as u64 {
        return Err(WorkspaceError::Invalid(
            "workspace Git capture time is invalid",
        ));
    }
    Ok(())
}

fn validate_workspace_directory(directory: &str) -> Result<()> {
    if !directory.is_empty()
        && (!Path::new(directory).is_absolute()
            || directory.len() > MAX_NOTE_BYTES
            || !display_safe(directory, false))
    {
        return Err(WorkspaceError::Invalid(
            "Task workspace directory is invalid",
        ));
    }
    Ok(())
}

fn create_private_file(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn history_binding(path: &Path) -> Result<[u8; 32]> {
    let file = fs::symlink_metadata(path)?;
    if !file.is_file() {
        return Err(WorkspaceError::Invalid(
            "workspace history must be a regular file",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(b"agenturm-workspace-history-v1");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(file.dev().to_be_bytes());
        hash.update(file.ino().to_be_bytes());
    }
    match file.created() {
        Ok(created) => hash.update(
            created
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes(),
        ),
        #[cfg(unix)]
        Err(_) => {}
        #[cfg(not(unix))]
        Err(error) => return Err(error.into()),
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ObservationBatch, ObservationIdentity, ObservationKind, StoredObservation, StoredScope,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn roster_titles_keep_session_anchors_and_reject_a_different_history() {
        let scratch = Scratch::new();
        let path = scratch.0.join("history.sqlite");
        let mut store = Store::open_collector(&path).unwrap();
        let worker = anchor("harness.codex");
        seed(&mut store, &worker, &[0]);
        let session = WorkspaceAnchor {
            worker_id: None,
            turn_id: None,
            ..worker.clone()
        };
        let sibling = anchor("harness.pi");
        seed(&mut store, &sibling, &[0]);
        let ledger = WorkspaceLedger::open(&path).unwrap();
        assert!(ledger.session_task_titles(Some(&store)).unwrap().is_empty());
        let visible = ledger
            .create_task(Some(&session), "Session title", "private note")
            .unwrap();
        ledger
            .create_task(Some(&worker), "Worker-only title", "")
            .unwrap();
        ledger
            .create_task(Some(&sibling), "Other source worker title", "")
            .unwrap();
        ledger.create_task(None, "Unlinked title", "").unwrap();
        let archived = ledger
            .create_task(Some(&session), "Archived title", "")
            .unwrap();
        assert!(
            ledger
                .set_task_archived(archived.id, true, None)
                .unwrap()
                .is_some()
        );
        let rows = ledger.session_task_titles(Some(&store)).unwrap();
        assert_eq!(
            rows,
            [(
                session.harness_id.clone(),
                session.session_id.clone(),
                visible.title.clone()
            )]
        );
        let sibling_session = WorkspaceAnchor {
            worker_id: None,
            turn_id: None,
            ..sibling.clone()
        };
        assert!(ledger.link_task(visible.id, &sibling_session).unwrap());
        let rows = ledger.session_task_titles(Some(&store)).unwrap();
        assert!(rows.contains(&(
            sibling_session.harness_id,
            sibling_session.session_id,
            visible.title
        )));

        let other = scratch.0.join("other.sqlite");
        let mut other = Store::open_collector(&other).unwrap();
        seed(&mut other, &worker, &[0]);
        seed(&mut other, &sibling, &[0]);
        assert_eq!(
            store.observation_page(1).unwrap().snapshot_id,
            other.observation_page(1).unwrap().snapshot_id
        );
        assert_eq!(
            store.read_incarnation().unwrap(),
            other.read_incarnation().unwrap()
        );
        assert!(matches!(
            ledger.session_task_titles(Some(&other)),
            Err(WorkspaceError::HistoryChanged)
        ));
    }

    #[test]
    fn roster_projects_require_current_exact_active_session_captures() {
        let scratch = Scratch::new();
        let path = scratch.0.join("history.sqlite");
        let mut store = Store::open_collector(&path).unwrap();
        let worker = anchor("harness.codex");
        let session = WorkspaceAnchor {
            worker_id: None,
            turn_id: None,
            ..worker.clone()
        };
        let sibling = WorkspaceAnchor {
            worker_id: None,
            turn_id: None,
            ..anchor("harness.pi")
        };
        let turn_only = WorkspaceAnchor {
            worker_id: None,
            ..anchor("harness.claude")
        };
        seed(&mut store, &worker, &[0]);
        seed(&mut store, &sibling, &[0]);
        seed(&mut store, &turn_only, &[0]);
        let snapshot = store.observation_page(1).unwrap().snapshot_id;
        let ledger = WorkspaceLedger::open(&path).unwrap();
        assert!(
            ledger
                .session_task_project_roots(&store, snapshot)
                .unwrap()
                .is_empty()
        );

        let save = |anchor, root: &str| {
            let task = ledger
                .create_task(anchor, "Operator task", "private note")
                .unwrap();
            assert!(
                ledger
                    .save_task_workspace_snapshot(
                        task.id,
                        &WorkspaceGitSnapshot {
                            repository_root: root.into(),
                            branch: None,
                            head_commit: None,
                            staged_changes: 0,
                            unstaged_changes: 0,
                            untracked_files: 0,
                            captured_at_unix_ns: 1,
                        }
                    )
                    .unwrap()
            );
            task.id
        };
        save(Some(&session), "/work/codex");
        save(Some(&sibling), "/work/pi");
        save(Some(&worker), "/workers/hidden");
        save(Some(&turn_only), "/turns/hidden");
        save(None, "/unlinked/hidden");
        let archived = save(Some(&session), "/archived/hidden");
        assert!(
            ledger
                .set_task_archived(archived, true, None)
                .unwrap()
                .is_some()
        );

        assert_eq!(
            ledger.session_task_project_roots(&store, snapshot).unwrap(),
            [
                (sibling.clone(), Some("/work/pi".into())),
                (session.clone(), Some("/work/codex".into()))
            ]
        );

        let long = format!("/{}", "x".repeat(MAX_ROSTER_PROJECT_PATH_BYTES));
        save(Some(&session), &long);
        let corrupt = save(Some(&sibling), "/previously-valid");
        Connection::open(&ledger.path)
            .unwrap()
            .execute(
                "UPDATE agenturm_task_git_snapshots SET repository_root = ?2 WHERE task_id = ?1",
                params![corrupt, "relative/prohibited"],
            )
            .unwrap();
        let rows = ledger.session_task_project_roots(&store, snapshot).unwrap();
        assert_eq!(
            &rows[..2],
            &[(sibling.clone(), None), (session.clone(), None)]
        );
        assert_eq!(rows.len(), 4);
        assert!(
            ledger
                .session_task_project_roots(
                    &store,
                    ReadSnapshot::new(snapshot.watermark() + 1, snapshot.incarnation())
                )
                .is_err()
        );

        let alternate_incarnation = "0123456789abcdef0123456789abcdef".parse().unwrap();
        assert!(matches!(
            ledger.session_task_project_roots(
                &store,
                ReadSnapshot::new(snapshot.watermark(), Some(alternate_incarnation))
            ),
            Err(WorkspaceError::History(StoreError::SnapshotReplaced))
        ));

        let other = scratch.0.join("other.sqlite");
        let mut other = Store::open_collector(&other).unwrap();
        seed(&mut other, &worker, &[0]);
        seed(&mut other, &sibling, &[0]);
        seed(&mut other, &turn_only, &[0]);
        let matching_number = other.observation_page(1).unwrap().snapshot_id;
        assert_eq!(snapshot, matching_number);
        assert!(matches!(
            ledger.session_task_project_roots(&other, matching_number),
            Err(WorkspaceError::HistoryChanged)
        ));
    }

    #[test]
    fn tasks_reopen_page_and_mutate_independently_of_a_live_collector() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        let mut collector = Store::open_collector(&history).unwrap();
        let anchor = anchor("harness.codex");
        seed(&mut collector, &anchor, &[0]);
        let workspace = WorkspaceLedger::open(&history).unwrap();
        assert!(workspace.list_tasks(2, None).unwrap().items.is_empty());
        assert!(!workspace.delete_task(1).unwrap());
        assert!(matches!(
            workspace.create_task(None, "", "ignored"),
            Err(WorkspaceError::Invalid(_))
        ));
        let mut wrong = anchor.clone();
        wrong.worker_id = None;
        assert!(matches!(
            workspace.create_task(Some(&wrong), "wrong worker", ""),
            Err(WorkspaceError::MissingAnchor)
        ));
        let sidecar = scratch.0.join("history.sqlite.workspace.sqlite");
        assert!(!sidecar.exists());

        let blank = workspace.create_task(None, "Blank task", "").unwrap();
        let linked = workspace
            .create_task(Some(&anchor), "Investigate", "local private note")
            .unwrap();
        assert!(!format!("{linked:?}").contains("local private note"));
        assert!(workspace.set_task_completed(linked.id, true).unwrap());
        assert!(
            workspace
                .update_task(linked.id, "Follow up", "edited")
                .unwrap()
        );
        assert_eq!(collector.observations().unwrap().len(), 1);

        let reopened = WorkspaceLedger::open(history).unwrap();
        let first = reopened.list_tasks(1, None).unwrap();
        assert_eq!(first.next_before, Some(linked.id));
        assert_eq!(
            first.items,
            [SavedTask {
                title: "Follow up".into(),
                note: "edited".into(),
                completed: true,
                revision: linked.revision + 2,
                ..linked.clone()
            }]
        );
        let older = reopened.list_tasks(1, first.next_before).unwrap();
        assert_eq!(older.items, [blank]);
        assert!(older.next_before.is_none());
        assert!(reopened.delete_task(linked.id).unwrap());
        assert!(!workspace.set_task_completed(linked.id, false).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(sidecar).unwrap().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn handoffs_keep_exact_anchors_across_sources_even_when_native_ids_match() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        let mut collector = Store::open(&history).unwrap();
        let from = anchor("harness.codex");
        let to = anchor("harness.kiro");
        seed(&mut collector, &from, &[0]);
        seed(&mut collector, &to, &[0]);
        let workspace = WorkspaceLedger::open(&history).unwrap();
        let mut nonexistent = to.clone();
        nonexistent.turn_id = Some("missing-turn".into());
        assert!(matches!(
            workspace.create_handoff(&from, &nonexistent, "Create a task"),
            Err(WorkspaceError::MissingAnchor)
        ));
        assert!(matches!(
            workspace.create_handoff(&from, &from, "same session"),
            Err(WorkspaceError::Invalid(_))
        ));
        let handoff = workspace
            .create_handoff(&from, &to, "Pass this along")
            .unwrap();
        assert!(
            workspace
                .update_handoff(handoff.id, "Updated local note")
                .unwrap()
        );
        let reopened = WorkspaceLedger::open(&history).unwrap();
        assert_eq!(
            reopened.list_handoffs(1, None).unwrap().items,
            [SavedHandoff {
                note: "Updated local note".into(),
                ..handoff.clone()
            }]
        );
        assert!(reopened.delete_handoff(handoff.id).unwrap());
        assert!(workspace.list_handoffs(1, None).unwrap().items.is_empty());
    }

    #[test]
    fn seen_requests_are_idempotent_and_revision_worker_source_exact() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        let mut collector = Store::open(&history).unwrap();
        let codex = anchor("harness.codex");
        let kiro = anchor("harness.kiro");
        seed(&mut collector, &codex, &[0, 1]);
        seed(&mut collector, &kiro, &[0]);
        let workspace = WorkspaceLedger::open(&history).unwrap();
        let mut wrong = codex.clone();
        wrong.worker_id = None;
        assert!(matches!(
            workspace.mark_request_seen(&wrong, "request", 0),
            Err(WorkspaceError::MissingAnchor)
        ));
        assert!(workspace.mark_request_seen(&codex, "request", 0).unwrap());
        assert!(!workspace.mark_request_seen(&codex, "request", 0).unwrap());
        let keys = [
            (codex.clone(), "request".into(), 0),
            (codex, "request".into(), 1),
            (kiro, "request".into(), 0),
            (wrong, "request".into(), 0),
        ];
        let reopened = WorkspaceLedger::open(history).unwrap();
        assert_eq!(
            reopened.seen_requests(&keys).unwrap(),
            BTreeSet::from([keys[0].clone()])
        );
        assert!(matches!(
            reopened.seen_requests(&vec![keys[0].clone(); MAX_WORKSPACE_PAGE_SIZE + 1]),
            Err(WorkspaceError::Invalid(_))
        ));
    }

    #[test]
    fn links_distinguish_sessions_from_exact_worker_turns_and_survive_reopen() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        let mut collector = Store::open_collector(&history).unwrap();
        let worker = anchor("harness.codex");
        seed(&mut collector, &worker, &[0]);
        let session = WorkspaceAnchor {
            worker_id: None,
            turn_id: None,
            ..worker.clone()
        };
        let workspace = WorkspaceLedger::open(&history).unwrap();
        let task = workspace.create_task(Some(&worker), "Task", "").unwrap();
        let mut wrong_worker = worker.clone();
        wrong_worker.worker_id = None;
        assert!(matches!(
            workspace.link_task(task.id, &wrong_worker),
            Err(WorkspaceError::MissingAnchor)
        ));
        assert!(!workspace.link_task(task.id, &worker).unwrap());
        assert!(workspace.link_task(task.id, &session).unwrap());
        let reopened = WorkspaceLedger::open(&history).unwrap();
        assert_eq!(
            reopened.task_links(task.id, 2).unwrap(),
            [session.clone(), worker.clone()]
        );
        assert!(reopened.unlink_task(task.id, &worker).unwrap());
        assert!(!reopened.unlink_task(task.id, &worker).unwrap());
        assert_eq!(
            reopened.list_tasks(1, None).unwrap().items[0].anchor,
            Some(session.clone())
        );
        assert!(reopened.unlink_task(task.id, &session).unwrap());
        assert!(
            reopened.list_tasks(1, None).unwrap().items[0]
                .anchor
                .is_none()
        );
    }

    #[test]
    fn old_ledger_reads_its_anchor_and_migrates_atomically_when_saving_git() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        drop(Store::open(&history).unwrap());
        let workspace = WorkspaceLedger::open(&history).unwrap();
        let original = anchor("harness.codex");
        create_private_file(&workspace.path).unwrap();
        let connection = workspace.connect(true).unwrap().unwrap();
        create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO agenturm_saved_tasks
                (history_binding, source_id, session_id, worker_id, turn_id, title, note)
             VALUES (?1, ?2, ?3, ?4, ?5, 'Prior task', 'original note')",
                params![
                    workspace.binding,
                    original.harness_id.as_str(),
                    original.session_id,
                    original.worker_id,
                    original.turn_id
                ],
            )
            .unwrap();
        let id = connection.last_insert_rowid() as u64;
        assert_eq!(
            workspace.task_links(id, 1).unwrap().as_slice(),
            std::slice::from_ref(&original)
        );
        assert_eq!(schema_version(&connection).unwrap(), 1);
        assert!(workspace.task_workspace_snapshot(id).unwrap().is_none());
        drop(connection);

        let snapshot = WorkspaceGitSnapshot {
            repository_root: scratch.0.display().to_string(),
            branch: Some("main".into()),
            head_commit: Some("a".repeat(40)),
            staged_changes: 1,
            unstaged_changes: 2,
            untracked_files: 3,
            captured_at_unix_ns: 123,
        };
        assert!(
            workspace
                .save_task_workspace_snapshot(id, &snapshot)
                .unwrap()
        );
        let reopened = WorkspaceLedger::open(&history).unwrap();
        assert_eq!(
            reopened.task_workspace_snapshot(id).unwrap(),
            Some(snapshot)
        );
        assert_eq!(reopened.task_links(id, 1).unwrap(), [original]);
        assert_eq!(
            reopened.list_tasks(1, None).unwrap().items[0].note,
            "original note"
        );
        assert!(reopened.delete_task(id).unwrap());
        let connection = reopened.connect(false).unwrap().unwrap();
        assert_eq!(schema_version(&connection).unwrap(), SCHEMA_VERSION);
        let dependent_rows: i64 = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM agenturm_task_links) +
                    (SELECT COUNT(*) FROM agenturm_task_git_snapshots)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dependent_rows, 0);
    }

    fn anchor(harness_id: &str) -> WorkspaceAnchor {
        WorkspaceAnchor {
            harness_id: ExtensionId::parse(harness_id).unwrap(),
            session_id: "session".into(),
            worker_id: Some("worker".into()),
            turn_id: Some("turn".into()),
        }
    }

    fn seed(store: &mut Store, anchor: &WorkspaceAnchor, revisions: &[u32]) {
        let observations = revisions
            .iter()
            .map(|&revision| StoredObservation {
                event_id: "request".into(),
                revision,
                identity: ObservationIdentity {
                    scope: StoredScope::Session,
                    session_id: Some(anchor.session_id.clone()),
                    worker_id: anchor.worker_id.clone(),
                    turn_id: anchor.turn_id.clone(),
                    operation_id: None,
                },
                observed_at_unix_ns: 1,
                kind: ObservationKind::Diagnostic {
                    code: "kiro_permission_unrepresented".into(),
                    severity: "warning".into(),
                },
            })
            .collect();
        let batch = ObservationBatch::new_deterministic(
            anchor.harness_id.as_str(),
            None,
            "next",
            observations,
        )
        .unwrap();
        store.commit_batch(&batch).unwrap();
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "agenturm-workspace-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
