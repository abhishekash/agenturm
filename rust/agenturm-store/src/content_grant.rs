//! Durable authorization state for host-controlled private source requests.

use crate::{Result, SCHEMA_VERSION, Store, StoreError};
use agenturm_kernel::ExtensionId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

const MAX_REFERENCES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContentKind {
    #[serde(rename = "pi.user_text")]
    PiUserText,
    #[serde(rename = "codex.user_text")]
    CodexUserText,
    #[serde(rename = "claude.user_text")]
    ClaudeUserText,
    #[serde(rename = "kiro.user_text")]
    KiroUserText,
    #[serde(rename = "opencode.user_text")]
    OpenCodeUserText,
    #[serde(rename = "codex.mcp_payload")]
    CodexMcpPayload,
    #[serde(rename = "codex.edit_detail")]
    CodexEditDetail,
    #[serde(rename = "codex.command_detail")]
    CodexCommandDetail,
    #[serde(rename = "telemetry.span_payload")]
    TelemetrySpanPayload,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::PiUserText => "pi.user_text",
            Self::CodexUserText => "codex.user_text",
            Self::ClaudeUserText => "claude.user_text",
            Self::KiroUserText => "kiro.user_text",
            Self::OpenCodeUserText => "opencode.user_text",
            Self::CodexMcpPayload => "codex.mcp_payload",
            Self::CodexEditDetail => "codex.edit_detail",
            Self::CodexCommandDetail => "codex.command_detail",
            Self::TelemetrySpanPayload => "telemetry.span_payload",
        }
    }
}

/// An exact source and content kind, bound to a fingerprint of the trusted
/// current source configuration. The host derives the fingerprint from the
/// expected artifact and source identity. Adapters never supply this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentGrantScope {
    source_id: ExtensionId,
    kind: ContentKind,
    source_fingerprint: [u8; 32],
}

impl ContentGrantScope {
    pub fn new(source_id: ExtensionId, kind: ContentKind, source_fingerprint: [u8; 32]) -> Self {
        Self {
            source_id,
            kind,
            source_fingerprint,
        }
    }

    pub fn source_id(&self) -> &ExtensionId {
        &self.source_id
    }

    pub fn kind(&self) -> ContentKind {
        self.kind
    }

    pub fn source_fingerprint(&self) -> [u8; 32] {
        self.source_fingerprint
    }
}

/// Authority for one exact approved source configuration. A private content
/// caller must use a fresh source scope to validate this before use and again
/// before disclosure, alongside its independent source evidence checks.
#[derive(Clone, Eq, PartialEq)]
pub struct ContentGrant {
    scope: ContentGrantScope,
    epoch: i64,
    nonce: [u8; 16],
}

impl ContentGrant {
    pub fn epoch(&self) -> i64 {
        self.epoch
    }

    /// Binds derived evidence to this approval without exporting its nonce.
    pub fn binding_fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"agenturm-content-approval/1");
        hash.update(self.scope.source_fingerprint);
        hash.update(self.epoch.to_be_bytes());
        hash.update(self.nonce);
        hash.finalize().into()
    }
}

impl fmt::Debug for ContentGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentGrant")
            .field("source_id", &self.scope.source_id)
            .field("kind", &self.scope.kind)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Metadata from the exact retained user message and its admission and
/// current source checkpoints. The host independently verifies source bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserTextEvidence {
    pub cursor: String,
    pub admitted_cursor: Option<String>,
    pub session_id: String,
    pub worker_id: Option<String>,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
    pub revision: u32,
}

/// The exact durable MCP occurrence to be checked against a native source.
pub struct CodexOperationSelection<'a> {
    pub event_id: &'a str,
    pub revision: u32,
    pub session_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub operation_id: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexOperationEvidence {
    pub cursor: String,
    pub admitted_cursor: String,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
}

pub(super) fn create_schema(connection: &Connection, version: i64) -> Result<()> {
    let rule = match version {
        9 => "kind = 'pi.user_text'",
        11 => {
            "kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text')"
        }
        12 => {
            "kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text', 'codex.mcp_payload')"
        }
        13 => {
            "kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text', 'codex.mcp_payload', 'codex.edit_detail')"
        }
        14 | 15 => {
            "kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text', 'codex.mcp_payload', 'codex.edit_detail', 'codex.command_detail')"
        }
        16 => {
            "kind IN ('pi.user_text', 'codex.user_text', 'claude.user_text', 'kiro.user_text', 'opencode.user_text', 'codex.mcp_payload', 'codex.edit_detail', 'codex.command_detail', 'telemetry.span_payload')"
        }
        _ => return Err(StoreError::UnsupportedSchema(version)),
    };
    connection.execute_batch(&format!(
        "CREATE TABLE agenturm_content_grants (
            source_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK ({rule}),
            source_fingerprint BLOB NOT NULL CHECK (length(source_fingerprint) = 32),
            epoch INTEGER NOT NULL CHECK (typeof(epoch) = 'integer' AND epoch > 0),
            nonce BLOB NOT NULL CHECK (length(nonce) = 16),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            PRIMARY KEY (source_id, kind)
        );"
    ))?;
    Ok(())
}

pub(super) fn create_receipt_proof_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_content_cursor_proofs (
            request_id TEXT PRIMARY KEY NOT NULL REFERENCES agenturm_ingest_receipts(request_id),
            cursor TEXT NOT NULL
        );",
    )?;
    Ok(())
}

pub(super) fn record_receipt_proof(
    connection: &Connection,
    request_id: &str,
    cursor: &str,
) -> Result<()> {
    let proof = if let Some(native) = cursor.strip_prefix("v2:") {
        jsonl_receipt_proof(native)
    } else if let Some(native) = cursor.strip_prefix("oc4:") {
        sqlite_receipt_proof(native)
    } else {
        None
    };
    let Some(proof) = proof else {
        return Ok(());
    };
    connection.execute(
        "INSERT INTO agenturm_content_cursor_proofs(request_id, cursor) VALUES (?1, ?2)",
        params![request_id, proof],
    )?;
    Ok(())
}

fn jsonl_receipt_proof(native: &str) -> Option<String> {
    let fields: Vec<_> = native.splitn(7, ':').collect();
    if fields.len() != 7
        || fields[..3]
            .iter()
            .any(|value| value.parse::<u64>().is_err())
        || fields[3..6].iter().any(|value| !valid_digest(value))
    {
        return None;
    }
    // Keep only the file identity, hash lengths and digests. Adapter
    // checkpoint details are deliberately absent from historical proofs.
    Some(format!("v2:{}:", fields[..6].join(":")))
}

fn sqlite_receipt_proof(native: &str) -> Option<String> {
    let fields: Vec<_> = native.split(':').skip(12).collect();
    if !(1..=6).contains(&fields.len()) || fields.iter().any(|value| !valid_digest(value)) {
        return None;
    }
    Some(format!("oc4:{}", fields.join(":")))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn migrate_v10(connection: &Connection) -> Result<()> {
    migrate_grant_schema(connection, 11)?;
    create_receipt_proof_schema(connection)
}

pub(super) fn migrate_v11(connection: &Connection) -> Result<()> {
    migrate_grant_schema(connection, 12)
}

pub(super) fn migrate_v12(connection: &Connection) -> Result<()> {
    migrate_grant_schema(connection, 13)
}

pub(super) fn migrate_v13(connection: &Connection) -> Result<()> {
    migrate_grant_schema(connection, 14)
}

pub(super) fn migrate_grant_schema(connection: &Connection, version: i64) -> Result<()> {
    connection.execute_batch(
        "ALTER TABLE agenturm_content_grants RENAME TO agenturm_prior_content_grants;",
    )?;
    create_schema(connection, version)?;
    connection.execute_batch(
        "INSERT INTO agenturm_content_grants
             SELECT source_id, kind, source_fingerprint, epoch, nonce, enabled
             FROM agenturm_prior_content_grants;
         DROP TABLE agenturm_prior_content_grants;",
    )?;
    Ok(())
}

pub(super) fn disable_backup_grants(connection: &Connection) -> Result<()> {
    // Backups from read-only v8 databases have no grant state. A detached
    // snapshot keeps prior epochs but must require fresh authorization.
    if has_grant_schema(connection)? {
        connection.execute(
            "UPDATE agenturm_content_grants SET enabled = 0 WHERE enabled = 1",
            [],
        )?;
    }
    Ok(())
}

fn has_grant_schema(connection: &Connection) -> Result<bool> {
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    match version {
        9..=SCHEMA_VERSION => Ok(true),
        8 => Ok(false),
        _ => Err(StoreError::UnsupportedSchema(version)),
    }
}

impl Store {
    /// Authorizes an exact host scope. Repeating the same active approval is
    /// idempotent. Re-approval or a changed fingerprint creates a fresh epoch.
    pub fn grant_content(&mut self, scope: &ContentGrantScope) -> Result<ContentGrant> {
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (epoch, nonce) = transaction.query_row(
            "INSERT INTO agenturm_content_grants
                (source_id, kind, source_fingerprint, epoch, nonce, enabled)
             VALUES (?1, ?2, ?3, 1, randomblob(16), 1)
             ON CONFLICT(source_id, kind) DO UPDATE SET
                source_fingerprint = excluded.source_fingerprint,
                epoch = CASE WHEN source_fingerprint = excluded.source_fingerprint AND enabled = 1
                         THEN epoch ELSE epoch + 1 END,
                nonce = CASE WHEN source_fingerprint = excluded.source_fingerprint AND enabled = 1
                         THEN nonce ELSE excluded.nonce END,
                enabled = 1
             RETURNING epoch, nonce",
            params![
                scope.source_id.as_str(),
                scope.kind.as_str(),
                scope.source_fingerprint
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        transaction.commit()?;
        Ok(ContentGrant {
            scope: scope.clone(),
            epoch,
            nonce,
        })
    }

    /// Revokes the source and kind across all bindings. Returns whether an
    /// active approval changed. The epoch is retained for future re-approval.
    pub fn revoke_content(&mut self, source_id: &ExtensionId, kind: ContentKind) -> Result<bool> {
        self.verify_write_admission()?;
        let changed = self.connection.execute(
            "UPDATE agenturm_content_grants SET enabled = 0
             WHERE source_id = ?1 AND kind = ?2 AND enabled = 1",
            params![source_id.as_str(), kind.as_str()],
        )?;
        Ok(changed != 0)
    }

    /// Revokes only the exact binding the caller named. A delayed revoke for
    /// a replaced source cannot revoke its replacement approval.
    pub fn revoke_bound_content(&mut self, scope: &ContentGrantScope) -> Result<bool> {
        self.verify_write_admission()?;
        let changed = self.connection.execute(
            "UPDATE agenturm_content_grants SET enabled = 0
             WHERE source_id = ?1 AND kind = ?2 AND source_fingerprint = ?3 AND enabled = 1",
            params![
                scope.source_id.as_str(),
                scope.kind.as_str(),
                scope.source_fingerprint
            ],
        )?;
        Ok(changed != 0)
    }

    /// Revokes every kind associated with this configured source ID.
    pub fn revoke_source_content(&mut self, source_id: &ExtensionId) -> Result<bool> {
        self.verify_write_admission()?;
        let changed = self.connection.execute(
            "UPDATE agenturm_content_grants SET enabled = 0 WHERE source_id = ?1 AND enabled = 1",
            [source_id.as_str()],
        )?;
        Ok(changed != 0)
    }

    /// Returns the approval for the exact current scope. Absent, revoked, or
    /// differently bound sources deny access.
    pub fn current_content_grant(&self, scope: &ContentGrantScope) -> Result<Option<ContentGrant>> {
        if !has_grant_schema(&self.connection)? {
            return Ok(None);
        }
        self.connection
            .query_row(
                "SELECT epoch, nonce FROM agenturm_content_grants
                 WHERE source_id = ?1 AND kind = ?2 AND source_fingerprint = ?3 AND enabled = 1",
                params![
                    scope.source_id.as_str(),
                    scope.kind.as_str(),
                    scope.source_fingerprint
                ],
                |row| {
                    Ok(ContentGrant {
                        scope: scope.clone(),
                        epoch: row.get(0)?,
                        nonce: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Re-reads current approval and rejects stale, revoked, foreign-store, or
    /// source-replacement tokens. The caller supplies its freshly checked scope.
    pub fn verify_content_grant(
        &self,
        scope: &ContentGrantScope,
        grant: &ContentGrant,
    ) -> Result<()> {
        if scope != &grant.scope || self.current_content_grant(scope)?.as_ref() != Some(grant) {
            return Err(StoreError::StaleContentGrant);
        }
        Ok(())
    }

    /// Retains the writer lease and serializes proof and delivery with
    /// admission, grants, and retention on other same-process connections.
    pub fn with_user_content_fence<T>(&self, use_current: impl FnOnce(&Self) -> T) -> Result<T> {
        self.require_content_writer()?;
        let fence =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let response = use_current(self);
        fence.rollback()?;
        Ok(response)
    }

    /// Selects a retained observation and its admission proof. Older rows
    /// require their own receipt proof. Legacy rows can use the latest
    /// commit only. The caller holds storage's writer fence through use.
    pub fn current_user_text_evidence(
        &self,
        scope: &ContentGrantScope,
        grant: &ContentGrant,
        event_id: &str,
        revision: u32,
    ) -> Result<Option<UserTextEvidence>> {
        let mut found =
            self.current_user_text_candidates(scope, grant, Some(event_id), None, Some(revision))?;
        Ok(found.pop().map(|(_, evidence)| evidence))
    }

    /// Lists a bounded page of recent user messages from one session.
    /// Each entry still needs native byte proof before disclosure.
    pub fn current_user_text_references(
        &self,
        scope: &ContentGrantScope,
        grant: &ContentGrant,
        session_id: &str,
    ) -> Result<Vec<(String, UserTextEvidence)>> {
        self.current_user_text_candidates(scope, grant, None, Some(session_id), None)
    }

    /// Joins the selected occurrence to its byte proof while the disclosure
    /// caller holds the writer fence. User-text approvals grant no access here.
    pub fn current_codex_operation_evidence(
        &self,
        scope: &ContentGrantScope,
        grant: &ContentGrant,
        selection: &CodexOperationSelection<'_>,
    ) -> Result<Option<CodexOperationEvidence>> {
        self.require_content_writer()?;
        let observation_kind = match scope.kind {
            ContentKind::CodexMcpPayload => "mcp_call",
            ContentKind::CodexEditDetail if selection.turn_id.is_some() => "file_edit",
            ContentKind::CodexCommandDetail if selection.turn_id.is_some() => "tool",
            _ => return Err(StoreError::StaleContentGrant),
        };
        self.verify_content_grant(scope, grant)?;
        self.connection
            .query_row(
                "SELECT state.cursor, COALESCE(proof.cursor, state.cursor), o.turn_id, o.observed_at_unix_ns
                 FROM agenturm_observations AS o
                 JOIN agenturm_source_state AS state ON state.source_id = o.source_id
                 LEFT JOIN agenturm_content_cursor_proofs AS proof ON proof.request_id = o.request_id
                 WHERE o.source_id = ?1 AND o.event_id = ?2 AND o.revision = ?3
                   AND o.session_id = ?4 AND o.operation_id = ?5
                   AND o.scope = 'session' AND o.worker_id IS NULL AND o.turn_id IS NOT NULL
                   AND (?6 IS NULL OR o.turn_id = ?6)
                   AND (proof.cursor GLOB 'v2:*' OR (state.cursor GLOB 'v2:*' AND o.request_id = (
                       SELECT newest.request_id FROM agenturm_history_batches AS newest
                       JOIN agenturm_ingest_receipts AS receipt USING (request_id)
                       WHERE receipt.source_id = o.source_id ORDER BY newest.sequence DESC LIMIT 1
                   )))
                   AND json_extract(o.kind_json, '$.kind') = ?7
                   AND (?7 <> 'tool' OR json_extract(o.kind_json, '$.category') = 'shell')
                   AND NOT EXISTS (
                       SELECT 1 FROM agenturm_observations AS newer
                       WHERE newer.source_id = o.source_id AND newer.event_id = o.event_id
                             AND newer.revision > o.revision
                       UNION ALL
                       SELECT 1 FROM agenturm_retired_observations AS newer
                       WHERE newer.source_id = o.source_id AND newer.event_id = o.event_id
                             AND newer.revision > o.revision
                   )",
                params![
                    scope.source_id.as_str(),
                    selection.event_id,
                    selection.revision,
                    selection.session_id,
                    selection.operation_id,
                    selection.turn_id,
                    observation_kind,
                ],
                |row| {
                    Ok(CodexOperationEvidence {
                        cursor: row.get(0)?,
                        admitted_cursor: row.get(1)?,
                        turn_id: row.get(2)?,
                        observed_at_unix_ns: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn require_content_writer(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self.writer_lease.is_none() {
            return Err(StoreError::WriterLease(std::io::Error::other(
                "Prompt preview requires a writer lease",
            )));
        }
        self.verify_write_admission()
    }

    fn current_user_text_candidates(
        &self,
        scope: &ContentGrantScope,
        grant: &ContentGrant,
        event_id: Option<&str>,
        session_id: Option<&str>,
        revision: Option<u32>,
    ) -> Result<Vec<(String, UserTextEvidence)>> {
        self.require_content_writer()?;
        if matches!(
            scope.kind,
            ContentKind::CodexMcpPayload
                | ContentKind::CodexEditDetail
                | ContentKind::CodexCommandDetail
                | ContentKind::TelemetrySpanPayload
        ) {
            return Err(StoreError::StaleContentGrant);
        }
        self.verify_content_grant(scope, grant)?;
        let version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(version));
        }
        let mut statement = self.connection.prepare(
            "SELECT o.event_id, state.cursor, o.session_id, o.worker_id, o.turn_id,
                    o.observed_at_unix_ns, o.revision, proof.cursor
             FROM agenturm_observations AS o
             JOIN agenturm_source_state AS state ON state.source_id = o.source_id
             LEFT JOIN agenturm_content_cursor_proofs AS proof ON proof.request_id = o.request_id
                AND ((?6 AND proof.cursor GLOB 'oc4:*') OR (NOT ?6 AND proof.cursor GLOB 'v2:*'))
             WHERE o.source_id = ?1 AND (?2 IS NULL OR o.event_id = ?2) AND (?3 IS NULL OR o.revision = ?3)
               AND (?4 IS NULL OR o.session_id = ?4)
               AND o.scope = 'session' AND o.turn_id IS NOT NULL AND o.operation_id IS NULL
               AND json_extract(o.kind_json, '$.kind') = 'message'
               AND json_extract(o.kind_json, '$.role') = 'user'
               AND (proof.request_id IS NOT NULL OR ((NOT ?6 OR state.cursor GLOB 'oc3:*') AND o.request_id = (
                   SELECT newest.request_id FROM agenturm_history_batches AS newest
                   JOIN agenturm_ingest_receipts AS receipt USING (request_id)
                   WHERE receipt.source_id = ?1 ORDER BY newest.sequence DESC LIMIT 1
               )))
               AND NOT EXISTS (
                   SELECT 1 FROM agenturm_observations AS newer
                   WHERE newer.source_id = o.source_id AND newer.event_id = o.event_id
                         AND newer.revision > o.revision
                         AND NOT (?6 AND ?2 IS NOT NULL AND proof.request_id IS NOT NULL)
                   UNION ALL
                   SELECT 1 FROM agenturm_retired_observations AS newer
                   WHERE newer.source_id = o.source_id AND newer.event_id = o.event_id
                         AND newer.revision > o.revision
               )
             ORDER BY o.observed_at_unix_ns DESC, o.event_id DESC
             LIMIT ?5",
        )?;
        let rows = statement.query_map(
            params![
                scope.source_id.as_str(),
                event_id,
                revision,
                session_id,
                MAX_REFERENCES,
                scope.kind == ContentKind::OpenCodeUserText
            ],
            |row| {
                let evidence = UserTextEvidence {
                    cursor: row.get(1)?,
                    session_id: row.get(2)?,
                    worker_id: row.get(3)?,
                    turn_id: row.get(4)?,
                    observed_at_unix_ns: row.get(5)?,
                    revision: row.get(6)?,
                    admitted_cursor: row.get(7)?,
                };
                Ok((row.get(0)?, evidence))
            },
        )?;
        let mut found = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        found.reverse();
        Ok(found)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn command_only_receipts_remain_provable_after_followup_and_unrelated_pruning() {
        use agenturm_kernel::{
            Observation, ObservationIdentity, ObservationKind, ObservationStore,
        };
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-command-only-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let db = directory.join("history.sqlite");
        let mut store = Store::open(&db).unwrap();
        let source = ExtensionId::parse("harness.codex").unwrap();
        let scope =
            ContentGrantScope::new(source.clone(), ContentKind::CodexCommandDetail, [7; 32]);
        let grant = store.grant_content(&scope).unwrap();
        let cursor = format!(
            "v2:1:2:3:{}:{}:{}:opaque",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64)
        );
        let mut identity = ObservationIdentity::session("session-one");
        identity.turn_id = Some("turn-one".into());
        identity.operation_id = Some("command-one".into());
        let command = Observation {
            harness_id: source.clone(),
            event_id: "command-event".into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 30,
            kind: ObservationKind::Tool {
                category: "shell".into(),
                result: "succeeded".into(),
            },
        };
        store
            .commit(source.as_str(), None, &cursor, &[command])
            .unwrap();
        let selection = CodexOperationSelection {
            session_id: "session-one",
            event_id: "command-event",
            revision: 0,
            operation_id: "command-one",
            turn_id: Some("turn-one"),
        };
        let proven = |store: &Store| {
            store
                .with_user_content_fence(|store| {
                    store
                        .current_codex_operation_evidence(&scope, &grant, &selection)
                        .unwrap()
                        .is_some()
                })
                .unwrap()
        };
        assert!(proven(&store));
        // Older v14 stores did not retain proofs for command-only batches.
        // Only their latest batch may use its current native cursor as proof.
        store
            .connection
            .execute("DELETE FROM agenturm_content_cursor_proofs", [])
            .unwrap();
        assert!(proven(&store));
        let command = Observation {
            harness_id: source.clone(),
            event_id: "command-event-two".into(),
            revision: 0,
            identity: ObservationIdentity::session("session-two"),
            observed_at_unix_ns: 31,
            kind: ObservationKind::SessionStarted,
        };
        // Record the original proof again as new collection now does, then
        // advance the source beyond it with a non-content batch.
        let receipt: String = store
            .connection
            .query_row(
                "SELECT request_id FROM agenturm_history_batches ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        record_receipt_proof(&store.connection, &receipt, &cursor).unwrap();
        store
            .commit(source.as_str(), Some(&cursor), "followup", &[command])
            .unwrap();
        let unrelated = Observation {
            harness_id: ExtensionId::parse("harness.pi").unwrap(),
            event_id: "old-unrelated".into(),
            revision: 0,
            identity: ObservationIdentity::session("other"),
            observed_at_unix_ns: 1,
            kind: ObservationKind::SessionStarted,
        };
        store
            .commit("harness.pi", None, "one", &[unrelated])
            .unwrap();
        assert_eq!(
            store
                .prune_observations_before(20)
                .unwrap()
                .removed_observations,
            1
        );
        assert!(proven(&store));
        store.prune_observations_before(31).unwrap();
        assert!(!proven(&store));
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn content_delivery_serializes_same_process_revocation() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-content-fence-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let db = directory.join("history.sqlite");
        let mut authorizer = Store::open(&db).unwrap();
        let mut revoker = Store::open(&db).unwrap();
        revoker.connection.busy_timeout(Duration::ZERO).unwrap();
        let source = ExtensionId::parse("harness.pi").unwrap();
        let scope = ContentGrantScope::new(source.clone(), ContentKind::PiUserText, [7; 32]);
        let grant = authorizer.grant_content(&scope).unwrap();
        authorizer
            .with_user_content_fence(|store| {
                assert!(matches!(
                    revoker.revoke_content(&source, ContentKind::PiUserText),
                    Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(err, _)))
                        if err.code == rusqlite::ErrorCode::DatabaseBusy
                ));
                store.verify_content_grant(&scope, &grant).unwrap();
            })
            .unwrap();
        assert!(
            revoker
                .revoke_content(&source, ContentKind::PiUserText)
                .unwrap()
        );
        assert!(matches!(
            authorizer.verify_content_grant(&scope, &grant),
            Err(StoreError::StaleContentGrant)
        ));
        drop(revoker);
        drop(authorizer);
        fs::remove_dir_all(directory).unwrap();
    }
}
