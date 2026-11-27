//! Retained, source-bound attention metadata and immutable admission receipts.

use crate::workspace_ledger::WorkspaceAnchor;
use crate::{Result, Store, StoreError, read_history_identity, retention};
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

mod execution_clock;
pub use execution_clock::{
    ExecutionClockPage, ExecutionClockQuery, ExecutionClockStart, MAX_EXECUTION_CLOCK_SCOPES,
};

pub use agenturm_kernel::native_attention::{
    NativeAttentionCapture, NativeAttentionKind, NativeAttentionReceipt, NativeAttentionScope,
    is_attention_event_id,
};

const MAX_SENDERS: i64 = 1_000;
const MAX_EVENTS: i64 = 65_536;
const MAX_PENDING: usize = 1_000;

const SENDERS_SQL: &str = "CREATE TABLE agenturm_native_attention_senders (
    collector_id TEXT NOT NULL,
    sender_id TEXT NOT NULL,
    scope_digest BLOB NOT NULL CHECK(length(scope_digest)=32),
    last_sequence INTEGER NOT NULL CHECK(last_sequence>0),
    PRIMARY KEY(collector_id,sender_id)
) WITHOUT ROWID";
const EVENTS_SQL: &str = "CREATE TABLE agenturm_native_attention_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    event_digest BLOB NOT NULL CHECK(length(event_digest)=32),
    collector_id TEXT NOT NULL,
    sender_id TEXT NOT NULL,
    source_sequence INTEGER NOT NULL CHECK(source_sequence>0),
    history_sequence INTEGER NOT NULL UNIQUE REFERENCES agenturm_history_batches(sequence),
    kind TEXT NOT NULL CHECK(kind IN ('approval_requested','reply_requested','request_resolved','turn_started','turn_completed','turn_interrupted')),
    request_digest BLOB CHECK(request_digest IS NULL OR length(request_digest)=32),
    receipt_time_unix_ns INTEGER CHECK(receipt_time_unix_ns IS NULL OR receipt_time_unix_ns>0),
    source_time_unix_ns INTEGER CHECK(source_time_unix_ns IS NULL OR source_time_unix_ns>=0),
    source_id TEXT,
    session_id TEXT,
    turn_id TEXT,
    worker_id TEXT,
    CHECK((kind IN ('approval_requested','reply_requested','request_resolved')) = (request_digest IS NOT NULL)),
    CHECK((source_id IS NULL AND session_id IS NULL AND turn_id IS NULL AND worker_id IS NULL AND receipt_time_unix_ns IS NULL AND source_time_unix_ns IS NULL)
        OR (source_id IS NOT NULL AND session_id IS NOT NULL AND turn_id IS NOT NULL AND receipt_time_unix_ns IS NOT NULL)),
    FOREIGN KEY(collector_id,sender_id) REFERENCES agenturm_native_attention_senders(collector_id,sender_id),
    UNIQUE(collector_id,sender_id,source_sequence)
) WITHOUT ROWID";
const REQUEST_INDEX_SQL: &str = "CREATE INDEX agenturm_native_attention_by_request
    ON agenturm_native_attention_events(collector_id,sender_id,request_digest,kind,history_sequence)";
const TARGET_INDEX_SQL: &str = "CREATE INDEX agenturm_native_attention_by_target
    ON agenturm_native_attention_events(source_id,session_id,history_sequence)
    WHERE source_id IS NOT NULL AND kind IN ('approval_requested','reply_requested')";

const PENDING: &str = "o.kind IN ('approval_requested','reply_requested')
    AND o.source_id IS NOT NULL
    AND o.history_sequence<=?1
    AND NOT EXISTS (
        SELECT 1 FROM agenturm_native_attention_events resolution
        WHERE resolution.collector_id=o.collector_id AND resolution.sender_id=o.sender_id
          AND resolution.request_digest=o.request_digest AND resolution.kind='request_resolved'
          AND resolution.history_sequence<=?1
    )
    AND EXISTS (
        SELECT 1 FROM agenturm_observations support
        JOIN agenturm_history_batches b ON b.request_id=support.request_id
        WHERE support.source_id=o.source_id AND support.session_id=o.session_id
          AND support.scope='session' AND support.turn_id=o.turn_id
          AND support.worker_id IS o.worker_id AND b.sequence<=?1
    )";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAttentionQuery {
    pub source_id: Option<ExtensionId>,
    pub session_id: Option<String>,
    pub snapshot_id: Option<ReadSnapshot>,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_receiver_clock: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAttentionPending {
    pub scope: NativeAttentionScope,
    pub event_id: String,
    pub kind: NativeAttentionKind,
    pub receipt_time_unix_ns: u64,
    /// Caller-reported clock metadata. Admission and eligibility use the receiver clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_time_unix_ns: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAttentionPage {
    pub snapshot_id: ReadSnapshot,
    pub pending: Vec<NativeAttentionPending>,
    pub truncated: bool,
    /// Time of the authenticated latest-snapshot read on the receiver host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_time_unix_ns: Option<u64>,
}

fn receiver_time_unix_ns() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| i64::try_from(time.as_nanos()).ok())
        .filter(|at| *at > 0)
}

pub(super) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "{SENDERS_SQL};\n{EVENTS_SQL};\n{REQUEST_INDEX_SQL};\n{TARGET_INDEX_SQL};"
    ))?;
    Ok(())
}

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    for (name, expected) in [
        ("agenturm_native_attention_senders", SENDERS_SQL),
        ("agenturm_native_attention_events", EVENTS_SQL),
        ("agenturm_native_attention_by_request", REQUEST_INDEX_SQL),
        ("agenturm_native_attention_by_target", TARGET_INDEX_SQL),
    ] {
        let sql: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        if sql.as_deref() != Some(expected) {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    Ok(())
}

pub(super) fn prune_before(connection: &Connection, cutoff: i64) -> Result<()> {
    connection.execute(
        "UPDATE agenturm_native_attention_events
            SET receipt_time_unix_ns=NULL, source_time_unix_ns=NULL, source_id=NULL,
                session_id=NULL, turn_id=NULL, worker_id=NULL
            WHERE receipt_time_unix_ns<?1",
        [cutoff],
    )?;
    Ok(())
}

#[cfg(test)]
fn retain_sources(connection: &Connection, allowed: &BTreeSet<ExtensionId>) -> Result<()> {
    if !table_exists(connection)? {
        return Ok(());
    }
    let allowed = serde_json::to_string(allowed)?;
    connection.execute_batch(
        "CREATE TEMP TABLE denied_native_attention_receipts(
            history_sequence INTEGER PRIMARY KEY, event_id TEXT NOT NULL);",
    )?;
    connection.execute(
        "INSERT INTO denied_native_attention_receipts(history_sequence,event_id)
         SELECT e.history_sequence,e.event_id FROM agenturm_native_attention_events e
         WHERE e.collector_id NOT IN (SELECT value FROM json_each(?1))
            OR e.source_id NOT IN (SELECT value FROM json_each(?1))
            OR (e.source_id IS NULL AND NOT EXISTS (
                SELECT 1 FROM agenturm_native_attention_events retained
                WHERE retained.collector_id=e.collector_id AND retained.sender_id=e.sender_id
                  AND retained.source_id IN (SELECT value FROM json_each(?1))
            ))",
        [allowed],
    )?;
    connection.execute_batch(
        "DELETE FROM agenturm_native_attention_events
           WHERE history_sequence IN (SELECT history_sequence FROM denied_native_attention_receipts);
         DELETE FROM agenturm_native_attention_senders
           WHERE NOT EXISTS (
                SELECT 1 FROM agenturm_native_attention_events e
                WHERE e.collector_id=agenturm_native_attention_senders.collector_id
                  AND e.sender_id=agenturm_native_attention_senders.sender_id
           );
         DELETE FROM agenturm_history_batches
           WHERE sequence IN (SELECT history_sequence FROM denied_native_attention_receipts);
         DELETE FROM agenturm_ingest_receipts
           WHERE request_id IN (SELECT event_id FROM denied_native_attention_receipts);
         DROP TABLE denied_native_attention_receipts;",
    )?;
    Ok(())
}

pub fn canonical_event_id(collector: &ExtensionId, capture: &NativeAttentionCapture) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agenturm.native-attention-event/1\0");
    digest.update(collector.as_str().as_bytes());
    digest.update([0]);
    digest.update(capture.sender_id.as_bytes());
    digest.update([0]);
    digest.update(capture.event_id.as_bytes());
    format!("native-attention:{:x}", digest.finalize())
}

fn binding(connection: &Connection) -> Result<String> {
    let identity = read_history_identity(connection)?;
    let incarnation = retention::read_incarnation(connection)?;
    Ok(incarnation.map_or_else(|| identity.clone(), |marker| format!("{identity}:{marker}")))
}

fn exact_anchor(
    connection: &Connection,
    scope: &NativeAttentionScope,
    watermark: i64,
) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agenturm_observations support
         JOIN agenturm_history_batches b ON b.request_id=support.request_id
         WHERE support.source_id=?1 AND support.session_id=?2 AND support.scope='session'
           AND support.turn_id=?3 AND support.worker_id IS ?4 AND b.sequence<=?5)",
        params![
            scope.source_id,
            scope.session_id,
            scope.turn_id,
            scope.worker_id,
            watermark
        ],
        |row| row.get(0),
    )?)
}

fn table_exists(connection: &Connection) -> Result<bool> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(version >= 19)
}

impl Store {
    pub fn admit_native_attention(
        &mut self,
        collector: &ExtensionId,
        capture: &NativeAttentionCapture,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<NativeAttentionReceipt> {
        let target = ExtensionId::parse(&capture.scope.source_id)
            .map_err(|_| StoreError::Invalid("invalid native attention source"))?;
        if !collector.as_str().starts_with("otlp.") || !capture.is_valid() {
            return Err(StoreError::Invalid("invalid native attention capture"));
        }
        if allowed_sources
            .is_some_and(|sources| !sources.contains(collector) || !sources.contains(&target))
        {
            return Err(StoreError::Invalid("native attention scope is unavailable"));
        }
        self.verify_write_admission()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let history_binding = binding(&transaction)?;
        if capture.history_binding != history_binding {
            return Err(StoreError::Invalid(
                "native attention history binding changed",
            ));
        }
        let event_id = canonical_event_id(collector, capture);
        let digest: [u8; 32] = Sha256::digest(serde_json::to_vec(capture)?).into();
        let prior: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT event_digest FROM agenturm_native_attention_events WHERE event_id=?1",
                [&event_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            if prior != digest {
                return Err(StoreError::RequestConflict);
            }
            return Ok(NativeAttentionReceipt {
                history_binding,
                event_id,
                replayed: true,
            });
        }
        let watermark = retention::receipt_head(&transaction)?;
        if !exact_anchor(&transaction, &capture.scope, watermark)? {
            return Err(StoreError::Invalid(
                "native attention requires an exact retained turn and worker",
            ));
        }
        let scope_digest: [u8; 32] = Sha256::digest(serde_json::to_vec(&capture.scope)?).into();
        let previous: Option<(Vec<u8>, u64)> = transaction.query_row(
            "SELECT scope_digest,last_sequence FROM agenturm_native_attention_senders WHERE collector_id=?1 AND sender_id=?2",
            params![collector.as_str(), capture.sender_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((scope, previous)) = previous {
            if scope != scope_digest || capture.sequence <= previous {
                return Err(StoreError::RequestConflict);
            }
        } else {
            let senders: i64 = transaction.query_row(
                "SELECT count(*) FROM agenturm_native_attention_senders",
                [],
                |r| r.get(0),
            )?;
            if senders >= MAX_SENDERS {
                return Err(StoreError::Invalid("native attention sender limit reached"));
            }
        }
        let events: i64 = transaction.query_row(
            "SELECT count(*) FROM agenturm_native_attention_events",
            [],
            |r| r.get(0),
        )?;
        if events >= MAX_EVENTS {
            return Err(StoreError::Invalid(
                "native attention receipt limit reached",
            ));
        }

        let request_digest = capture
            .request_id
            .as_ref()
            .map(|id| <[u8; 32]>::from(Sha256::digest(id.as_bytes())));
        if let Some(request) = request_digest {
            validate_request(&transaction, collector, capture, request)?;
        }
        let timestamp = receiver_time_unix_ns().ok_or(StoreError::Invalid(
            "native attention receipt clock is unavailable",
        ))?;
        if timestamp < retention::read_state(&transaction)?.cutoff_unix_ns {
            return Err(StoreError::Invalid(
                "native attention receipt is outside retention",
            ));
        }
        // A restored pre-clock history has no saved incarnation floor. Its
        // first new authenticated admission establishes a floor before any
        // new event becomes eligible. Replays return before this write.
        if retention::execution_clock_floor(&transaction)?.is_none() {
            retention::set_execution_clock_floor(&transaction, watermark)?;
        }

        let sequence = retention::next_receipt_sequence(&transaction)?;
        transaction.execute(
            "INSERT INTO agenturm_ingest_receipts(request_id,source_id,batch_digest,event_count)
             VALUES(?1,?2,?3,0)",
            params![event_id, collector.as_str(), encode_digest(digest)],
        )?;
        transaction.execute(
            "INSERT INTO agenturm_history_batches(sequence,request_id,admitted_at_unix_ns)
             VALUES(?1,?2,?3)",
            params![sequence, event_id, timestamp],
        )?;
        transaction.execute(
            "INSERT INTO agenturm_native_attention_senders(collector_id,sender_id,scope_digest,last_sequence)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(collector_id,sender_id) DO UPDATE SET last_sequence=excluded.last_sequence",
            params![collector.as_str(), capture.sender_id, scope_digest, capture.sequence],
        )?;
        transaction.execute(
            "INSERT INTO agenturm_native_attention_events
             (event_id,event_digest,collector_id,sender_id,source_sequence,history_sequence,kind,request_digest,
              receipt_time_unix_ns,source_id,session_id,turn_id,worker_id,source_time_unix_ns)
              VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![event_id, digest, collector.as_str(), capture.sender_id, capture.sequence, sequence,
                    capture.kind.as_str(), request_digest, timestamp, capture.scope.source_id,
                    capture.scope.session_id, capture.scope.turn_id, capture.scope.worker_id,
                    capture.source_time_unix_ns],
        )?;
        transaction.commit()?;
        Ok(NativeAttentionReceipt {
            history_binding,
            event_id,
            replayed: false,
        })
    }

    pub fn query_native_attention(
        &self,
        query: &NativeAttentionQuery,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<NativeAttentionPage> {
        if !(1..=MAX_PENDING).contains(&query.limit)
            || query.session_id.as_ref().is_some_and(|id| {
                query.source_id.is_none()
                    || id.is_empty()
                    || id.len() > 512
                    || id.chars().any(char::is_control)
            })
            || query.source_id.as_ref().is_some_and(|source| {
                allowed_sources.is_some_and(|sources| !sources.contains(source))
            })
        {
            return Err(StoreError::Invalid(
                "invalid native attention read scope or page limit",
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let (snapshot_id, watermark) =
            retention::resolve_read_snapshot(&transaction, query.snapshot_id)?;
        let mut pending = Vec::new();
        if table_exists(&transaction)? {
            let sql = format!(
                "SELECT event_id,o.kind,receipt_time_unix_ns,source_id,session_id,turn_id,worker_id,source_time_unix_ns
                FROM agenturm_native_attention_events o WHERE {PENDING}
                  AND (?2 IS NULL OR o.source_id=?2)
                  AND (?3 IS NULL OR o.session_id=?3)
                  AND (?4 IS NULL OR (o.collector_id IN (SELECT value FROM json_each(?4))
                                      AND o.source_id IN (SELECT value FROM json_each(?4))))
                ORDER BY o.history_sequence DESC LIMIT ?5"
            );
            let mut statement = transaction.prepare(&sql)?;
            let mut rows = statement.query(params![
                watermark,
                query.source_id.as_ref().map(ExtensionId::as_str),
                query.session_id,
                allowed_sources.map(serde_json::to_string).transpose()?,
                (query.limit + 1) as i64,
            ])?;
            while let Some(row) = rows.next()? {
                let kind = match row.get::<_, String>(1)?.as_str() {
                    "approval_requested" => NativeAttentionKind::ApprovalRequested,
                    "reply_requested" => NativeAttentionKind::ReplyRequested,
                    _ => {
                        return Err(StoreError::Invalid(
                            "unexpected retained native attention kind",
                        ));
                    }
                };
                pending.push(NativeAttentionPending {
                    event_id: row.get(0)?,
                    kind,
                    receipt_time_unix_ns: row.get(2)?,
                    source_time_unix_ns: row.get(7)?,
                    scope: NativeAttentionScope {
                        source_id: row.get(3)?,
                        session_id: row.get(4)?,
                        turn_id: row.get(5)?,
                        worker_id: row.get(6)?,
                    },
                });
            }
        }
        let truncated = pending.len() > query.limit;
        pending.truncate(query.limit);
        Ok(NativeAttentionPage {
            snapshot_id,
            pending,
            truncated,
            receiver_time_unix_ns: if query.include_receiver_clock && query.snapshot_id.is_none() {
                receiver_time_unix_ns().map(|time| time as u64)
            } else {
                None
            },
        })
    }

    pub fn contains_native_attention_event(
        &self,
        anchor: &WorkspaceAnchor,
        event_id: &str,
        revision: u32,
        allowed_sources: Option<&BTreeSet<ExtensionId>>,
    ) -> Result<bool> {
        let transaction = self.connection.unchecked_transaction()?;
        contains_event_in(&transaction, anchor, event_id, revision, allowed_sources)
    }
}

pub(super) fn contains_event_in(
    connection: &Connection,
    anchor: &WorkspaceAnchor,
    event_id: &str,
    revision: u32,
    allowed_sources: Option<&BTreeSet<ExtensionId>>,
) -> Result<bool> {
    if revision != 0
        || !is_attention_event_id(event_id)
        || anchor.turn_id.is_none()
        || !table_exists(connection)?
    {
        return Ok(false);
    }
    let watermark = retention::receipt_head(connection)?;
    let query = format!(
        "SELECT EXISTS(SELECT 1 FROM agenturm_native_attention_events o
        WHERE {PENDING} AND o.event_id=?2 AND o.source_id=?3 AND o.session_id=?4
            AND o.turn_id=?5 AND o.worker_id IS ?6
            AND (?7 IS NULL OR (o.collector_id IN (SELECT value FROM json_each(?7))
                                AND o.source_id IN (SELECT value FROM json_each(?7)))))"
    );
    Ok(connection.query_row(
        &query,
        params![
            watermark,
            event_id,
            anchor.harness_id.as_str(),
            anchor.session_id,
            anchor.turn_id,
            anchor.worker_id,
            allowed_sources.map(serde_json::to_string).transpose()?
        ],
        |row| row.get(0),
    )?)
}

fn validate_request(
    connection: &Connection,
    collector: &ExtensionId,
    capture: &NativeAttentionCapture,
    request: [u8; 32],
) -> Result<()> {
    let (requested, resolved, retained): (i64, i64, i64) = connection.query_row(
        "SELECT COALESCE(SUM(kind IN ('approval_requested','reply_requested')),0),
                COALESCE(SUM(kind='request_resolved'),0),
                COALESCE(SUM(kind IN ('approval_requested','reply_requested') AND source_id IS NOT NULL),0)
         FROM agenturm_native_attention_events
         WHERE collector_id=?1 AND sender_id=?2 AND request_digest=?3",
        params![collector.as_str(), capture.sender_id, request],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if capture.kind.is_request() && requested == 0 && resolved == 0
        || capture.kind == NativeAttentionKind::RequestResolved
            && requested == 1
            && resolved == 0
            && retained == 1
    {
        Ok(())
    } else {
        Err(StoreError::RequestConflict)
    }
}

fn encode_digest(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(64);
    for byte in digest {
        write!(&mut text, "{byte:02x}").expect("write to string");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::{Observation, ObservationIdentity, ObservationKind, ObservationStore};

    fn setup() -> (Store, ExtensionId, ExtensionId, NativeAttentionCapture) {
        let mut store = Store::open(":memory:").unwrap();
        let collector = ExtensionId::parse("otlp.attention").unwrap();
        let target = ExtensionId::parse("harness.native").unwrap();
        let mut anchor = Observation {
            harness_id: target.clone(),
            event_id: "fixture-anchor".into(),
            revision: 0,
            identity: ObservationIdentity::session("fixture-session"),
            observed_at_unix_ns: 3,
            kind: ObservationKind::Activity {
                category: "tool".into(),
            },
        };
        anchor.identity.turn_id = Some("fixture-turn".into());
        let mut sibling = anchor.clone();
        sibling.event_id = "sibling-anchor".into();
        sibling.identity.worker_id = Some("sibling-worker".into());
        ObservationStore::commit(
            &mut store,
            target.as_str(),
            None,
            "retained",
            &[anchor, sibling],
        )
        .unwrap();
        let capture = NativeAttentionCapture {
            history_binding: binding(&store.connection).unwrap(),
            scope: NativeAttentionScope {
                source_id: target.as_str().into(),
                session_id: "fixture-session".into(),
                turn_id: "fixture-turn".into(),
                worker_id: None,
            },
            sender_id: "1".repeat(32),
            event_id: "native-private-id".into(),
            sequence: 1,
            kind: NativeAttentionKind::ApprovalRequested,
            request_id: Some("native-request-id".into()),
            source_time_unix_ns: Some(0),
        };
        (store, collector, target, capture)
    }

    fn query(snapshot_id: Option<ReadSnapshot>) -> NativeAttentionQuery {
        NativeAttentionQuery {
            source_id: None,
            session_id: None,
            snapshot_id,
            limit: 16,
            include_receiver_clock: false,
        }
    }

    #[test]
    fn the_clock_is_opt_in_and_bound_to_the_latest_read_transaction() {
        let (store, _, _, _) = setup();
        let default_query = query(None);
        let plain = store.query_native_attention(&default_query, None).unwrap();
        assert_eq!(plain.receiver_time_unix_ns, None);
        let wire = serde_json::to_value(&plain).unwrap();
        assert!(wire.get("receiver_time_unix_ns").is_none());
        assert_eq!(
            serde_json::from_value::<NativeAttentionPage>(wire).unwrap(),
            plain
        );

        let clock_query = NativeAttentionQuery {
            include_receiver_clock: true,
            ..default_query
        };
        let started = receiver_time_unix_ns().unwrap() as u64;
        let timed = store.query_native_attention(&clock_query, None).unwrap();
        let finished = receiver_time_unix_ns().unwrap() as u64;
        assert!((started..=finished).contains(&timed.receiver_time_unix_ns.unwrap()));
        assert_eq!(timed.snapshot_id, plain.snapshot_id);
        let historical = NativeAttentionQuery {
            snapshot_id: Some(timed.snapshot_id),
            ..clock_query
        };
        assert_eq!(
            store
                .query_native_attention(&historical, None)
                .unwrap()
                .receiver_time_unix_ns,
            None
        );
    }

    #[test]
    fn exact_admission_binds_scope_history_order_and_request_resolution() {
        let (mut store, collector, target, capture) = setup();
        let before = store
            .query_native_attention(&query(None), None)
            .unwrap()
            .snapshot_id;
        let allowed = BTreeSet::from([collector.clone(), target.clone()]);
        let receipt = store
            .admit_native_attention(&collector, &capture, Some(&allowed))
            .unwrap();
        assert!(!receipt.replayed && is_attention_event_id(&receipt.event_id));
        assert_eq!(receipt.event_id, canonical_event_id(&collector, &capture));
        assert_eq!(
            store.source_cursor(target.as_str()).unwrap().as_deref(),
            Some("retained")
        );
        assert_eq!(store.source_cursor(collector.as_str()).unwrap(), None);
        assert!(
            store
                .query_native_attention(&query(Some(before)), None)
                .unwrap()
                .pending
                .is_empty()
        );
        let pending = store
            .query_native_attention(&query(None), Some(&allowed))
            .unwrap();
        assert_eq!(pending.pending.len(), 1);
        assert_eq!(pending.pending[0].scope, capture.scope);
        assert!(pending.pending[0].receipt_time_unix_ns > 1);
        assert_eq!(pending.pending[0].source_time_unix_ns, Some(0));
        let replay = store
            .admit_native_attention(&collector, &capture, Some(&allowed))
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.event_id, receipt.event_id);
        assert_eq!(
            store.query_native_attention(&query(None), None).unwrap(),
            pending
        );

        let denied_target = BTreeSet::from([collector.clone()]);
        assert!(
            store
                .query_native_attention(&query(None), Some(&denied_target))
                .unwrap()
                .pending
                .is_empty()
        );
        let denied_collector = BTreeSet::from([target.clone()]);
        assert!(
            store
                .query_native_attention(&query(None), Some(&denied_collector))
                .unwrap()
                .pending
                .is_empty()
        );
        assert!(
            store
                .admit_native_attention(&collector, &capture, Some(&denied_collector))
                .is_err()
        );

        let mut changed = capture.clone();
        changed.history_binding = "2".repeat(32);
        assert!(
            store
                .admit_native_attention(&collector, &changed, None)
                .is_err()
        );
        changed = capture.clone();
        changed.kind = NativeAttentionKind::ReplyRequested;
        assert!(matches!(
            store.admit_native_attention(&collector, &changed, None),
            Err(StoreError::RequestConflict)
        ));
        changed = capture.clone();
        changed.event_id = "replacement".into();
        changed.sequence = 2;
        changed.scope.worker_id = Some("sibling-worker".into());
        assert!(matches!(
            store.admit_native_attention(&collector, &changed, None),
            Err(StoreError::RequestConflict)
        ));
        changed.scope.worker_id = None;
        changed.kind = NativeAttentionKind::TurnCompleted;
        changed.request_id = None;
        store
            .admit_native_attention(&collector, &changed, None)
            .unwrap();
        assert_eq!(
            store
                .query_native_attention(&query(None), None)
                .unwrap()
                .pending
                .len(),
            1
        );

        let anchor = WorkspaceAnchor {
            harness_id: target,
            session_id: capture.scope.session_id.clone(),
            turn_id: Some(capture.scope.turn_id.clone()),
            worker_id: None,
        };
        let mut resolved = capture.clone();
        resolved.event_id = "native-resolution".into();
        resolved.sequence = 3;
        resolved.kind = NativeAttentionKind::RequestResolved;
        resolved.sender_id = "2".repeat(32);
        assert!(matches!(
            store.admit_native_attention(&collector, &resolved, None),
            Err(StoreError::RequestConflict)
        ));
        assert!(
            store
                .contains_native_attention_event(&anchor, &receipt.event_id, 0, Some(&allowed))
                .unwrap()
        );
        assert!(
            !store
                .contains_native_attention_event(
                    &anchor,
                    &receipt.event_id,
                    0,
                    Some(&denied_collector)
                )
                .unwrap()
        );
        resolved.sender_id = capture.sender_id.clone();
        store
            .admit_native_attention(&collector, &resolved, None)
            .unwrap();
        assert!(
            store
                .query_native_attention(&query(None), None)
                .unwrap()
                .pending
                .is_empty()
        );
        assert_eq!(
            store
                .query_native_attention(&query(Some(pending.snapshot_id)), None)
                .unwrap(),
            pending
        );
        assert!(
            !store
                .contains_native_attention_event(&anchor, &receipt.event_id, 0, Some(&allowed))
                .unwrap()
        );
        assert!(
            store
                .admit_native_attention(&collector, &capture, None)
                .unwrap()
                .replayed
        );
    }

    #[test]
    fn scoped_backup_keeps_resolution_tombstone_with_its_allowed_sender() {
        let (mut store, collector, target, capture) = setup();
        store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        let mut resolved = capture.clone();
        resolved.event_id = "resolution-before-clock-adjustment".into();
        resolved.sequence = 2;
        resolved.kind = NativeAttentionKind::RequestResolved;
        let resolved = store
            .admit_native_attention(&collector, &resolved, None)
            .unwrap();
        // Move only the receipt clock backward to model an operator clock adjustment.
        store.connection.execute(
            "UPDATE agenturm_native_attention_events SET receipt_time_unix_ns=1 WHERE event_id=?1",
            [&resolved.event_id],
        ).unwrap();
        prune_before(&store.connection, 2).unwrap();
        retain_sources(
            &store.connection,
            &BTreeSet::from([collector.clone(), target]),
        )
        .unwrap();
        assert!(
            store
                .query_native_attention(&query(None), None)
                .unwrap()
                .pending
                .is_empty()
        );
        retain_sources(&store.connection, &BTreeSet::from([collector])).unwrap();
        let event_count: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM agenturm_native_attention_events",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 0);
    }

    #[test]
    fn retention_and_source_filtered_backup_leave_no_visible_denied_scope() {
        let (mut store, collector, target, capture) = setup();
        let receipt = store
            .admit_native_attention(&collector, &capture, None)
            .unwrap();
        let pending = store.query_native_attention(&query(None), None).unwrap();
        let cutoff = pending.pending[0].receipt_time_unix_ns + 1;
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        retention::prune_in(&transaction, cutoff).unwrap();
        transaction.commit().unwrap();
        assert!(
            store
                .query_native_attention(&query(None), None)
                .unwrap()
                .pending
                .is_empty()
        );
        assert!(
            store
                .admit_native_attention(&collector, &capture, None)
                .unwrap()
                .replayed
        );
        let retained: (Option<String>, Option<u64>) = store
            .connection
            .query_row(
                "SELECT source_id,source_time_unix_ns FROM agenturm_native_attention_events WHERE event_id=?1",
                [&receipt.event_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(retained, (None, None));
        retain_sources(&store.connection, &BTreeSet::from([collector, target])).unwrap();
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM agenturm_native_attention_senders",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM agenturm_ingest_receipts WHERE request_id=?1",
                    [&receipt.event_id],
                    |row| row.get::<_, usize>(0),
                )
                .unwrap(),
            0
        );
    }
}
