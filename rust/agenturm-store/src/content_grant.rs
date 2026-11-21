//! Durable authorization state for host-controlled private source requests.

use crate::{ObservationKind, Result, SCHEMA_VERSION, Store, StoreError};
use agenturm_kernel::ExtensionId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentKind {
    PiUserText,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::PiUserText => "pi.user_text",
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

/// Metadata from the exact retained user message and its still-current
/// committed source batch. The host must independently verify source bytes.
#[derive(Debug, Eq, PartialEq)]
pub struct PiUserTextEvidence {
    pub cursor: String,
    pub session_id: String,
    pub worker_id: Option<String>,
    pub turn_id: String,
    pub observed_at_unix_ns: u64,
}

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE agenturm_content_grants (
            source_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind = 'pi.user_text'),
            source_fingerprint BLOB NOT NULL CHECK (length(source_fingerprint) = 32),
            epoch INTEGER NOT NULL CHECK (typeof(epoch) = 'integer' AND epoch > 0),
            nonce BLOB NOT NULL CHECK (length(nonce) = 16),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            PRIMARY KEY (source_id, kind)
        );",
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
        9 | SCHEMA_VERSION => Ok(true),
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

    /// Selects a retained observation in the same commit as the current
    /// source cursor. Older batches cannot supply admission-time byte proof
    /// through this API. The caller holds storage's writer fence through use.
    pub fn current_pi_user_text_evidence(
        &self,
        scope: &ContentGrantScope,
        grant: &ContentGrant,
        event_id: &str,
        revision: u32,
    ) -> Result<Option<PiUserTextEvidence>> {
        #[cfg(target_os = "linux")]
        if self.writer_lease.is_none() {
            return Err(StoreError::WriterLease(std::io::Error::other(
                "Pi preview requires a writer lease",
            )));
        }
        self.verify_write_admission()?;
        self.verify_content_grant(scope, grant)?;
        let version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(version));
        }
        let row = self
            .connection
            .query_row(
                "SELECT state.cursor, o.session_id, o.worker_id, o.turn_id,
                    o.observed_at_unix_ns, o.kind_json
             FROM agenturm_observations AS o
             JOIN agenturm_history_batches AS history ON history.request_id = o.request_id
             JOIN agenturm_source_state AS state ON state.source_id = o.source_id
             WHERE o.source_id = ?1 AND o.event_id = ?2 AND o.revision = ?3
               AND o.scope = 'session' AND o.turn_id IS NOT NULL AND o.operation_id IS NULL
               AND history.sequence = (
                   SELECT newest.sequence FROM agenturm_history_batches AS newest
                   JOIN agenturm_ingest_receipts AS receipt USING (request_id)
                   WHERE receipt.source_id = o.source_id ORDER BY newest.sequence DESC LIMIT 1
               )
               AND NOT EXISTS (
                   SELECT 1 FROM agenturm_observations AS newer
                   WHERE newer.source_id = o.source_id AND newer.event_id = o.event_id
                         AND newer.revision > o.revision
                   UNION ALL
                   SELECT 1 FROM agenturm_retired_observations AS newer
                   WHERE newer.source_id = o.source_id AND newer.event_id = o.event_id
                         AND newer.revision > o.revision
               )",
                params![scope.source_id.as_str(), event_id, revision],
                |row| {
                    let evidence = PiUserTextEvidence {
                        cursor: row.get(0)?,
                        session_id: row.get(1)?,
                        worker_id: row.get(2)?,
                        turn_id: row.get(3)?,
                        observed_at_unix_ns: row.get(4)?,
                    };
                    Ok((evidence, row.get::<_, String>(5)?))
                },
            )
            .optional()?;
        let Some((evidence, kind)) = row else {
            return Ok(None);
        };
        let kind = serde_json::from_str::<ObservationKind>(&kind)?;
        Ok(matches!(kind, ObservationKind::Message { role } if role == "user").then_some(evidence))
    }
}
