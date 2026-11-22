//! Approved task continuations retain exact source references and operator text.

use super::*;

const MAX_APPROVED_DRAFTS: usize = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSourceProof {
    pub anchor: WorkspaceAnchor,
    pub event_id: String,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
    pub grant_sha256: String,
    pub excerpt_sha256: String,
}

impl TaskSourceProof {
    pub(super) fn validate(&self) -> Result<()> {
        validate_request_key(&self.anchor, &self.event_id)?;
        if self.anchor.turn_id.is_none()
            || self.observed_at_unix_ns > i64::MAX as u64
            || !valid_digest(&self.grant_sha256)
            || !valid_digest(&self.excerpt_sha256)
        {
            return Err(WorkspaceError::Invalid("Invalid exact task source proof"));
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct TaskDraftApproval {
    pub draft_sha256: String,
    pub source: TaskSourceProof,
    pub continuation: String,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct ApprovedTaskDraft {
    pub id: u64,
    pub draft_sha256: String,
    pub approved_at_unix_ns: u64,
    pub source: TaskSourceProof,
    pub continuation: String,
}

impl WorkspaceLedger {
    /// A stable authority identifier for artifacts prepared against this ledger.
    pub fn task_draft_binding(&self) -> Result<[u8; 32]> {
        self.verify_history()?;
        Ok(self.binding)
    }

    /// Called by the task host while the exact source proof is fenced.
    pub fn approve_draft_for_new_task(
        &self,
        title: &str,
        workspace_directory: Option<&str>,
        approval: &TaskDraftApproval,
    ) -> Result<(SavedTask, ApprovedTaskDraft)> {
        validate_title(title)?;
        if let Some(directory) = workspace_directory {
            validate_workspace_directory(directory)?;
        }
        validate_approval(approval)?;
        self.require_anchors(&[&approval.source.anchor])?;
        self.write_workspace(true, |connection| {
            let id = insert_task_row(
                connection,
                self.binding,
                Some(&approval.source.anchor),
                title,
                "",
                workspace_directory,
            )?;
            let approved = insert_approval(connection, id, approval)?;
            let task = select_task(connection, &self.binding, id, SCHEMA_VERSION)?
                .ok_or(WorkspaceError::UnrecognizedLedger)?;
            Ok((task, approved))
        })?
        .ok_or(WorkspaceError::UnrecognizedLedger)
    }

    /// Called by the task host while the exact source proof is fenced.
    pub fn approve_draft_for_existing_task(
        &self,
        id: u64,
        expected_revision: u64,
        approval: &TaskDraftApproval,
    ) -> Result<Option<(SavedTask, ApprovedTaskDraft)>> {
        let signed_id = positive_id(id)?;
        validate_approval(approval)?;
        self.require_anchors(&[&approval.source.anchor])?;
        self.edit_task(id, Some(expected_revision), |connection, _| {
            let approved = insert_approval(connection, signed_id, approval)?;
            add_task_link(connection, self.binding, signed_id, &approval.source.anchor)?;
            Ok(approved)
        })
    }
}

fn validate_approval(approval: &TaskDraftApproval) -> Result<()> {
    approval.source.validate()?;
    validate_note(&approval.continuation, true)?;
    if !valid_digest(&approval.draft_sha256) {
        return Err(WorkspaceError::Invalid("Invalid continuity draft binding"));
    }
    Ok(())
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn encode_proof(proof: &TaskSourceProof) -> Result<String> {
    proof.validate()?;
    serde_json::to_string(proof)
        .map_err(|_| WorkspaceError::Invalid("Cannot encode task source proof"))
}

pub(super) fn decode_proof(
    row: &Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<TaskSourceProof>> {
    row.get::<_, Option<String>>(column)?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

fn insert_approval(
    connection: &Connection,
    task_id: i64,
    approval: &TaskDraftApproval,
) -> Result<ApprovedTaskDraft> {
    let (count, repeated): (usize, bool) = connection.query_row(
        "SELECT (SELECT count(*) FROM agenturm_task_continuity WHERE task_id = ?1),
                EXISTS(SELECT 1 FROM agenturm_task_continuity WHERE draft_sha256 = ?2)",
        params![task_id, approval.draft_sha256],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if repeated {
        return Err(WorkspaceError::Invalid(
            "This continuity draft already has an approval",
        ));
    }
    if count >= MAX_APPROVED_DRAFTS {
        return Err(WorkspaceError::Invalid(
            "This task has reached its approved continuity history limit",
        ));
    }
    let approved_at_unix_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| u64::try_from(time.as_nanos()).ok())
        .filter(|time| (1..=i64::MAX as u64).contains(time))
        .ok_or(WorkspaceError::Invalid(
            "Cannot timestamp the continuity approval",
        ))?;
    let id = connection.query_row(
        "INSERT INTO agenturm_task_continuity(task_id, draft_sha256, approved_at_unix_ns, source_json, continuation)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
        params![task_id, approval.draft_sha256, approved_at_unix_ns, encode_proof(&approval.source)?, approval.continuation],
        |row| row.get(0),
    )?;
    Ok(ApprovedTaskDraft {
        id,
        draft_sha256: approval.draft_sha256.clone(),
        approved_at_unix_ns,
        source: approval.source.clone(),
        continuation: approval.continuation.clone(),
    })
}

pub(super) fn load_approvals(
    connection: &Connection,
    task_id: u64,
    version: i64,
) -> Result<Vec<ApprovedTaskDraft>> {
    if version < 4 {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT id, draft_sha256, approved_at_unix_ns, source_json, continuation
         FROM agenturm_task_continuity WHERE task_id = ?1 ORDER BY id LIMIT ?2",
    )?;
    Ok(statement
        .query_map(params![task_id, MAX_APPROVED_DRAFTS], |row| {
            Ok(ApprovedTaskDraft {
                id: row.get(0)?,
                draft_sha256: row.get(1)?,
                approved_at_unix_ns: row.get(2)?,
                source: decode_proof(row, 3)?.ok_or(rusqlite::Error::InvalidQuery)?,
                continuation: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "ALTER TABLE agenturm_task_decisions ADD COLUMN source_json TEXT;
         CREATE TABLE agenturm_task_continuity (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES agenturm_saved_tasks(id) ON DELETE CASCADE,
            draft_sha256 TEXT NOT NULL UNIQUE CHECK (length(draft_sha256) = 64),
            approved_at_unix_ns INTEGER NOT NULL CHECK (approved_at_unix_ns > 0),
            source_json TEXT NOT NULL,
            continuation TEXT NOT NULL
         );
         CREATE INDEX agenturm_task_continuity_by_task ON agenturm_task_continuity(task_id, id);",
    )?;
    connection.pragma_update(None, "user_version", 4)?;
    Ok(())
}
