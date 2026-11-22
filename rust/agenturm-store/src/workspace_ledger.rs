//! Explicit, local cockpit tasks and cross-session handoffs.

use crate::{MAX_EVENT_ID_BYTES, MAX_SESSION_KEY_BYTES, Store, StoreError, validate_identifier};
use agenturm_kernel::ExtensionId;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params, types::Type,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const APPLICATION_ID: i64 = 0x4157_5350;
const SCHEMA_VERSION: i64 = 2;
pub const MAX_WORKSPACE_PAGE_SIZE: usize = 200;
const MAX_TITLE_BYTES: usize = 256;
const MAX_NOTE_BYTES: usize = 4 * 1024;

type Result<T> = std::result::Result<T, WorkspaceError>;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Debug)]
pub enum WorkspaceError {
    Invalid(&'static str),
    MissingAnchor,
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
        })
    }

    pub fn create_task(
        &self,
        anchor: Option<&WorkspaceAnchor>,
        title: &str,
        note: &str,
    ) -> Result<SavedTask> {
        validate_title(title)?;
        validate_note(note, true)?;
        if let Some(anchor) = anchor {
            self.require_anchors(&[anchor])?;
        }
        let id = self
            .write(true, |connection| {
                let id = connection.query_row(
                    "INSERT INTO agenturm_saved_tasks
                    (history_binding, source_id, session_id, worker_id, turn_id, title, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
                    params![
                        self.binding,
                        anchor.map(|anchor| anchor.harness_id.as_str()),
                        anchor.map(|anchor| anchor.session_id.as_str()),
                        anchor.and_then(|anchor| anchor.worker_id.as_deref()),
                        anchor.and_then(|anchor| anchor.turn_id.as_deref()),
                        title,
                        note
                    ],
                    |row| row.get::<_, i64>(0),
                )?;
                if let Some(anchor) = anchor {
                    insert_task_link(connection, self.binding, id, anchor)?;
                }
                Ok(id as u64)
            })?
            .ok_or(WorkspaceError::UnrecognizedLedger)?;
        Ok(SavedTask {
            id,
            anchor: anchor.cloned(),
            title: title.into(),
            note: note.into(),
            completed: false,
        })
    }

    pub fn update_task(&self, id: u64, title: &str, note: &str) -> Result<bool> {
        let id = positive_id(id)?;
        validate_title(title)?;
        validate_note(note, true)?;
        self.change(|connection| {
            connection.execute(
                "UPDATE agenturm_saved_tasks SET title = ?3, note = ?4
                 WHERE history_binding = ?1 AND id = ?2",
                params![self.binding, id, title, note],
            )
        })
    }

    pub fn set_task_completed(&self, id: u64, completed: bool) -> Result<bool> {
        let id = positive_id(id)?;
        self.change(|connection| {
            connection.execute(
                "UPDATE agenturm_saved_tasks SET completed = ?3
                 WHERE history_binding = ?1 AND id = ?2",
                params![self.binding, id, completed],
            )
        })
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
        self.list(
            "SELECT id, source_id, session_id, worker_id, turn_id, title, note, completed
             FROM agenturm_saved_tasks
             WHERE history_binding = ?1 AND id <= ?2
             ORDER BY id DESC LIMIT ?3",
            limit,
            before,
            |row, id| {
                Ok(SavedTask {
                    id,
                    anchor: decode_optional_anchor(row, 1)?,
                    title: row.get(5)?,
                    note: row.get(6)?,
                    completed: row.get(7)?,
                })
            },
        )
    }

    /// Adds a verified session or exact worker turn to a saved task.
    /// Returns false for duplicate links or a task that was removed.
    pub fn link_task(&self, id: u64, anchor: &WorkspaceAnchor) -> Result<bool> {
        let id = positive_id(id)?;
        self.require_anchors(&[anchor])?;
        self.change(|connection| {
            let changed = insert_task_link(connection, self.binding, id, anchor)?;
            if changed != 0 {
                connection.execute(
                    "UPDATE agenturm_saved_tasks
                     SET source_id = ?3, session_id = ?4, worker_id = ?5, turn_id = ?6
                     WHERE history_binding = ?1 AND id = ?2 AND source_id IS NULL",
                    params![
                        self.binding,
                        id,
                        anchor.harness_id.as_str(),
                        anchor.session_id,
                        anchor.worker_id,
                        anchor.turn_id
                    ],
                )?;
            }
            Ok(changed)
        })
    }

    /// Removes the exact link. If it was primary, the newest remaining link
    /// becomes primary. Links to expired history can still be removed.
    pub fn unlink_task(&self, id: u64, anchor: &WorkspaceAnchor) -> Result<bool> {
        let id = positive_id(id)?;
        validate_anchor(anchor)?;
        self.change(|connection| {
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
            Ok(changed)
        })
    }

    pub fn task_links(&self, id: u64, limit: usize) -> Result<Vec<WorkspaceAnchor>> {
        let id = positive_id(id)?;
        if !(1..=MAX_WORKSPACE_PAGE_SIZE).contains(&limit) {
            return Err(WorkspaceError::Invalid(
                "workspace task-link limit is invalid",
            ));
        }
        let mut links = Vec::new();
        let Some(mut connection) = self.connect(false)? else {
            return Ok(links);
        };
        let transaction = connection.transaction()?;
        match schema_version(&transaction)? {
            1 => {
                let anchor = transaction
                    .query_row(
                        "SELECT source_id, session_id, worker_id, turn_id FROM agenturm_saved_tasks
                     WHERE history_binding = ?1 AND id = ?2",
                        params![self.binding, id],
                        |row| decode_optional_anchor(row, 0),
                    )
                    .optional()?
                    .flatten();
                links.extend(anchor);
            }
            SCHEMA_VERSION => {
                let mut statement = transaction.prepare(
                    "SELECT link.source_id, link.session_id, NULLIF(link.worker_key, ''),
                            NULLIF(link.turn_key, '')
                     FROM agenturm_task_links AS link JOIN agenturm_saved_tasks AS task
                          ON task.id = link.task_id
                     WHERE task.history_binding = ?1 AND task.id = ?2
                     ORDER BY link.id DESC LIMIT ?3",
                )?;
                links = statement
                    .query_map(params![self.binding, id, limit as i64], |row| {
                        decode_anchor(row, 0)
                    })?
                    .collect::<rusqlite::Result<_>>()?;
            }
            _ => {}
        }
        self.verify_history()?;
        transaction.commit()?;
        Ok(links)
    }

    pub fn save_task_workspace_snapshot(
        &self,
        id: u64,
        snapshot: &WorkspaceGitSnapshot,
    ) -> Result<bool> {
        let id = positive_id(id)?;
        validate_git_snapshot(snapshot)?;
        self.change(|connection| {
            connection.execute(
                "INSERT INTO agenturm_task_git_snapshots
                    (task_id, repository_root, branch, head_commit, staged_changes,
                     unstaged_changes, untracked_files, captured_at_unix_ns)
                 SELECT id, ?3, ?4, ?5, ?6, ?7, ?8, ?9 FROM agenturm_saved_tasks
                 WHERE history_binding = ?1 AND id = ?2
                 ON CONFLICT (task_id) DO UPDATE SET
                     repository_root = excluded.repository_root, branch = excluded.branch,
                     head_commit = excluded.head_commit, staged_changes = excluded.staged_changes,
                     unstaged_changes = excluded.unstaged_changes, untracked_files = excluded.untracked_files,
                     captured_at_unix_ns = excluded.captured_at_unix_ns",
                params![self.binding, id, snapshot.repository_root, snapshot.branch, snapshot.head_commit,
                    snapshot.staged_changes, snapshot.unstaged_changes, snapshot.untracked_files,
                    snapshot.captured_at_unix_ns],
            )
        })
    }

    pub fn task_workspace_snapshot(&self, id: u64) -> Result<Option<WorkspaceGitSnapshot>> {
        let id = positive_id(id)?;
        let Some(mut connection) = self.connect(false)? else {
            return Ok(None);
        };
        let transaction = connection.transaction()?;
        let snapshot =
            if schema_version(&transaction)? == SCHEMA_VERSION {
                transaction.query_row(
                "SELECT git.repository_root, git.branch, git.head_commit, git.staged_changes,
                        git.unstaged_changes, git.untracked_files, git.captured_at_unix_ns
                 FROM agenturm_task_git_snapshots AS git JOIN agenturm_saved_tasks AS task
                      ON task.id = git.task_id
                 WHERE task.history_binding = ?1 AND task.id = ?2",
                params![self.binding, id],
                |row| Ok(WorkspaceGitSnapshot {
                    repository_root: row.get(0)?,
                    branch: row.get(1)?,
                    head_commit: row.get(2)?,
                    staged_changes: row.get(3)?,
                    unstaged_changes: row.get(4)?,
                    untracked_files: row.get(5)?,
                    captured_at_unix_ns: row.get(6)?,
                }),
            ).optional()?
            } else {
                None
            };
        self.verify_history()?;
        transaction.commit()?;
        Ok(snapshot)
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
            "SELECT id, from_source_id, from_session_id, from_worker_id, from_turn_id,
                    to_source_id, to_session_id, to_worker_id, to_turn_id, note
             FROM agenturm_saved_handoffs
             WHERE history_binding = ?1 AND id <= ?2
             ORDER BY id DESC LIMIT ?3",
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
        let history = Store::open_read_only(&self.history).map_err(WorkspaceError::History)?;
        let found = history.connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agenturm_observations
                 WHERE source_id = ?1 AND session_id = ?2 AND worker_id IS ?3 AND turn_id IS ?4
                   AND event_id = ?5 AND revision = ?6
             )",
            params![
                anchor.harness_id.as_str(),
                anchor.session_id,
                anchor.worker_id,
                anchor.turn_id,
                event_id,
                revision
            ],
            |row| row.get::<_, bool>(0),
        )?;
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
        sql: &str,
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
        if initialized(&transaction)? {
            let mut statement = transaction.prepare(sql)?;
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
        let history = Store::open_read_only(&self.history).map_err(WorkspaceError::History)?;
        let transaction = history.connection.unchecked_transaction()?;
        let mut lookup = transaction.prepare(
            "SELECT EXISTS(
                SELECT 1 FROM agenturm_observations
                WHERE source_id = ?1 AND session_id = ?2
                  AND (?3 IS NULL OR (turn_id = ?3 AND worker_id IS ?4))
             )",
        )?;
        for anchor in anchors {
            validate_anchor(anchor)?;
            if anchor.worker_id.is_some() && anchor.turn_id.is_none() {
                return Err(WorkspaceError::Invalid(
                    "workspace worker anchors require a turn",
                ));
            }
            let found = lookup.query_row(
                params![
                    anchor.harness_id.as_str(),
                    anchor.session_id,
                    anchor.turn_id,
                    anchor.worker_id
                ],
                |row| row.get::<_, bool>(0),
            )?;
            if !found {
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

    fn write<T>(
        &self,
        create: bool,
        action: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        if create {
            self.verify_history()?;
            create_private_file(&self.path)?;
        }
        let Some(mut connection) = self.connect(true)? else {
            return Ok(None);
        };
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match schema_version(&transaction)? {
            0 if !create => {
                return Ok(None);
            }
            0 => {
                create_schema(&transaction)?;
                create_task_extensions(&transaction)?;
            }
            1 => create_task_extensions(&transaction)?,
            _ => {}
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
        Ok(Some(connection))
    }
}

fn decode_anchor(row: &Row<'_>, column: usize) -> rusqlite::Result<WorkspaceAnchor> {
    decode_scoped_anchor(row, column, row.get(column)?)
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
        (APPLICATION_ID, 1 | SCHEMA_VERSION) => Ok(version),
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
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
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
