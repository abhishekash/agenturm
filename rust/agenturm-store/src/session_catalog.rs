//! Source-scoped session summaries over the durable observation history.

use crate::{
    MAX_SESSION_KEY_BYTES, MAX_SOURCE_ID_BYTES, ObservationKind, Result, ReviewVersion, Store,
    StoreError, retention, validate_identifier,
};
use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::{ExtensionId, RelationshipKind};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_SESSION_PAGE_SIZE: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionQuery {
    pub harness_id: Option<String>,
    pub limit: usize,
    pub snapshot_id: Option<ReadSnapshot>,
    pub before: Option<SessionPosition>,
}

impl SessionQuery {
    pub fn latest(limit: usize) -> Self {
        Self {
            harness_id: None,
            limit,
            snapshot_id: None,
            before: None,
        }
    }
}

/// An exclusive position ordered by last observed time, source, then session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPosition {
    pub observed_at_unix_ns: u64,
    pub harness_id: String,
    pub session_id: String,
    pub snapshot_id: ReadSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPage {
    /// Newest first, with descending source and session IDs as tie-breakers.
    pub sessions: Vec<SessionSummary>,
    pub snapshot_id: ReadSnapshot,
    pub next_before: Option<SessionPosition>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    pub harness_id: ExtensionId,
    pub session_id: String,
    pub observed_at_unix_ns: u64,
    pub review_version: ReviewVersion,
    /// Most recent recorded lifecycle fact, regardless of when activity
    /// last arrived. The fact's worker and turn anchors remain explicit.
    pub lifecycle: Option<SessionLifecycle>,
    /// Most recent explicit relationship in the retained, pinned source.
    /// The parent is scoped to `harness_id` and may be outside this page.
    pub parent_session_id: Option<String>,
    pub relationship_kind: Option<RelationshipKind>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLifecycle {
    pub observed_at_unix_ns: u64,
    pub state: String,
    pub reason_code: Option<String>,
    pub worker_id: Option<String>,
    pub turn_id: Option<String>,
}

impl Store {
    /// Reads at most the requested number of source-scoped sessions at a
    /// pinned observation watermark. Source-wide diagnostics are excluded.
    /// An active traversal keeps its ordering and lifecycle evidence when
    /// later collection commits. Pruning expires older snapshots.
    pub fn query_sessions(&self, query: &SessionQuery) -> Result<SessionPage> {
        self.query_sessions_impl(query, None)
    }

    pub fn query_sessions_from_sources(
        &self,
        query: &SessionQuery,
        allowed_sources: &BTreeSet<ExtensionId>,
    ) -> Result<SessionPage> {
        self.query_sessions_impl(query, Some(allowed_sources))
    }

    fn query_sessions_impl(
        &self,
        query: &SessionQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<SessionPage> {
        if !(1..=MAX_SESSION_PAGE_SIZE).contains(&query.limit) {
            return Err(StoreError::Invalid("session page limit is invalid"));
        }
        if let Some(harness_id) = &query.harness_id {
            parse_harness_id(harness_id)?;
        }
        if let Some(before) = &query.before {
            parse_harness_id(&before.harness_id)?;
            validate_identifier(&before.session_id, MAX_SESSION_KEY_BYTES, "session ID")?;
            if before.observed_at_unix_ns > i64::MAX as u64 {
                return Err(StoreError::Invalid("session cursor timestamp is invalid"));
            }
            if query.snapshot_id != Some(before.snapshot_id) {
                return Err(StoreError::Invalid(
                    "session cursor snapshot does not match",
                ));
            }
            if query
                .harness_id
                .as_ref()
                .is_some_and(|id| *id != before.harness_id)
            {
                return Err(StoreError::Invalid("session cursor source does not match"));
            }
        }

        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, receipt_watermark) =
            retention::resolve_read_snapshot(&transaction, query.snapshot_id)?;
        let mut list = transaction.prepare_cached(
            "SELECT o.source_id, o.session_id, MAX(o.observed_at_unix_ns) AS latest,
                    MAX(h.sequence), COUNT(*)
             FROM agenturm_observations AS o
             JOIN agenturm_history_batches AS h ON h.request_id = o.request_id
             WHERE o.session_id IS NOT NULL AND h.sequence <= ?1
               AND (?2 IS NULL OR o.source_id = ?2)
               AND (?7 IS NULL OR o.source_id IN (SELECT value FROM json_each(?7)))
             GROUP BY o.session_id, o.source_id
             HAVING ?3 IS NULL OR (latest, o.source_id, o.session_id) < (?3, ?4, ?5)
             ORDER BY latest DESC, o.source_id DESC, o.session_id DESC
             LIMIT ?6",
        )?;
        let before = query.before.as_ref();
        let mut rows = list.query_map(
            params![
                receipt_watermark,
                query.harness_id,
                before.map(|position| position.observed_at_unix_ns as i64),
                before.map(|position| position.harness_id.as_str()),
                before.map(|position| position.session_id.as_str()),
                (query.limit + 1) as i64,
                allowed_sources.map(serde_json::to_string).transpose()?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    ReviewVersion {
                        receipt_sequence: row.get(3)?,
                        retained_observations: row.get(4)?,
                    },
                ))
            },
        )?;

        // Load lifecycle and relationship facts only for these summaries.
        // The session/time index can walk backward to a matching occurrence.
        let mut latest_fact = transaction.prepare_cached(
            "SELECT o.kind_json, o.observed_at_unix_ns, o.worker_id, o.turn_id
             FROM agenturm_observations AS o
             JOIN agenturm_history_batches AS h ON h.request_id = o.request_id
             WHERE o.session_id = ?1 AND o.source_id = ?2 AND h.sequence <= ?3
               AND json_extract(o.kind_json, '$.kind') = ?4
             ORDER BY o.observed_at_unix_ns DESC, o.event_id DESC, o.revision DESC
             LIMIT 1",
        )?;
        let mut sessions = Vec::<SessionSummary>::with_capacity(query.limit);
        for packed in rows.by_ref().take(query.limit) {
            let (harness_id, session_id, observed_at_unix_ns, review_version) = packed?;
            let lifecycle = latest_fact
                .query_row(
                    params![session_id, harness_id, receipt_watermark, "lifecycle"],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?
                .map(
                    |(kind, observed_at_unix_ns, worker_id, turn_id)| -> Result<SessionLifecycle> {
                        let ObservationKind::Lifecycle { state, reason_code } =
                            serde_json::from_str(&kind)?
                        else {
                            return Err(StoreError::Invalid("invalid stored session lifecycle"));
                        };
                        Ok(SessionLifecycle {
                            observed_at_unix_ns,
                            state,
                            reason_code,
                            worker_id,
                            turn_id,
                        })
                    },
                )
                .transpose()?;
            let relationship = latest_fact
                .query_row(
                    params![session_id, harness_id, receipt_watermark, "relationship"],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let (parent_session_id, relationship_kind) = if let Some(kind) = relationship {
                let ObservationKind::Relationship {
                    relationship_kind,
                    parent_session_id,
                } = serde_json::from_str(&kind)?
                else {
                    return Err(StoreError::Invalid("invalid stored session relationship"));
                };
                (Some(parent_session_id), Some(relationship_kind))
            } else {
                (None, None)
            };
            sessions.push(SessionSummary {
                harness_id: parse_harness_id(&harness_id)?,
                session_id,
                observed_at_unix_ns,
                review_version,
                lifecycle,
                parent_session_id,
                relationship_kind,
            });
        }
        let next_before = if rows.next().transpose()?.is_some() {
            sessions.last().map(|last| SessionPosition {
                observed_at_unix_ns: last.observed_at_unix_ns,
                harness_id: last.harness_id.as_str().to_owned(),
                session_id: last.session_id.clone(),
                snapshot_id,
            })
        } else {
            None
        };
        Ok(SessionPage {
            sessions,
            snapshot_id,
            next_before,
        })
    }
}

fn parse_harness_id(value: &str) -> Result<ExtensionId> {
    validate_identifier(value, MAX_SOURCE_ID_BYTES, "harness ID")?;
    ExtensionId::parse(value).map_err(|_| StoreError::Invalid("harness ID"))
}
