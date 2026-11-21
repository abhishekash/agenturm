//! A small Rust-owned SQLite store for kernel observations.
//!
//! This crate proves typed atomic persistence, request replay, event identity,
//! and close/reopen reads. It is not yet a production storage authority: it
//! has no service principal, cross-process writer fence, product policy,
//! retention, backup protocol, or integration with the Agenturm app.
//!
//! The current kernel observation type does not include an event ID, so this
//! API is not yet a direct kernel port. The adapter-to-kernel contract must
//! preserve stable adapter-native event identity scoped by source; do not
//! synthesize identity from timestamps or session keys.

#![forbid(unsafe_code)]

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::Path;

const SCHEMA_VERSION: i64 = 1;
const MAX_BATCH_EVENTS: usize = 2_048;
const MAX_SOURCE_ID_BYTES: usize = 96;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_EVENT_ID_BYTES: usize = 256;
const MAX_SESSION_KEY_BYTES: usize = 512;
const MAX_CATEGORY_BYTES: usize = 64;
const MAX_CURSOR_BYTES: usize = 512;
const STORE_MARKER: &str = "agenturm.observation-store.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationKind {
    SessionStarted,
    SessionEnded,
    Activity {
        category: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredObservation {
    pub event_id: String,
    pub session_key: String,
    pub observed_at_unix_ms: u64,
    pub kind: ObservationKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationBatch {
    pub request_id: String,
    pub source_id: String,
    pub expected_cursor: Option<String>,
    pub next_cursor: String,
    pub observations: Vec<StoredObservation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    Committed { inserted: usize },
    Replay,
}

#[derive(Debug)]
pub enum StoreError {
    Invalid(&'static str),
    RequestConflict,
    StaleCursor,
    EventConflict { event_id: String },
    UnrecognizedDatabase,
    UnsupportedSchema(i64),
    Sqlite(rusqlite::Error),
    Serialization(serde_json::Error),
    InjectedFailure,
}

impl Display for StoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid observation batch: {message}"),
            Self::RequestConflict => write!(f, "request ID was reused with different content"),
            Self::StaleCursor => write!(f, "source cursor changed since batch preparation"),
            Self::EventConflict { event_id } => {
                write!(f, "event ID was reused with different content: {event_id}")
            }
            Self::UnrecognizedDatabase => {
                write!(f, "database is not a fresh or recognized Agenturm store")
            }
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported Agenturm store schema version: {version}")
            }
            Self::Sqlite(error) => Display::fmt(error, f),
            Self::Serialization(error) => Display::fmt(error, f),
            Self::InjectedFailure => write!(f, "injected transaction failure"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

fn validate_marked_v1_shape(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT type, name FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' AND type IN ('table', 'index', 'view', 'trigger')
         ORDER BY type, name",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let expected = vec![
        (
            "index".to_owned(),
            "agenturm_observations_by_session".to_owned(),
        ),
        ("table".to_owned(), "agenturm_ingest_receipts".to_owned()),
        ("table".to_owned(), "agenturm_observations".to_owned()),
        ("table".to_owned(), "agenturm_source_state".to_owned()),
        ("table".to_owned(), "agenturm_store_meta".to_owned()),
    ];
    if objects != expected {
        return Err(StoreError::UnrecognizedDatabase);
    }
    let marker: Option<String> = connection
        .query_row(
            "SELECT value FROM agenturm_store_meta WHERE key = 'format'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if marker.as_deref() != Some(STORE_MARKER) {
        return Err(StoreError::UnrecognizedDatabase);
    }
    for (table, expected_columns) in [
        (
            "agenturm_store_meta",
            vec![("key", "TEXT", 1, 1), ("value", "TEXT", 1, 0)],
        ),
        (
            "agenturm_ingest_receipts",
            vec![
                ("request_id", "TEXT", 1, 1),
                ("source_id", "TEXT", 1, 0),
                ("batch_digest", "TEXT", 1, 0),
                ("event_count", "INTEGER", 1, 0),
            ],
        ),
        (
            "agenturm_source_state",
            vec![("source_id", "TEXT", 1, 1), ("cursor", "TEXT", 1, 0)],
        ),
        (
            "agenturm_observations",
            vec![
                ("source_id", "TEXT", 1, 1),
                ("event_id", "TEXT", 1, 2),
                ("request_id", "TEXT", 1, 0),
                ("event_digest", "TEXT", 1, 0),
                ("session_key", "TEXT", 1, 0),
                ("observed_at_unix_ms", "INTEGER", 1, 0),
                ("kind_json", "TEXT", 1, 0),
            ],
        ),
    ] {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let expected_columns = expected_columns
            .into_iter()
            .map(|(name, column_type, not_null, primary_key)| {
                (
                    name.to_owned(),
                    column_type.to_owned(),
                    not_null,
                    primary_key,
                )
            })
            .collect::<Vec<_>>();
        if columns != expected_columns {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    let foreign_keys = connection
        .prepare("PRAGMA foreign_key_list(agenturm_observations)")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if foreign_keys
        != [(
            "agenturm_ingest_receipts".into(),
            "request_id".into(),
            "request_id".into(),
        )]
    {
        return Err(StoreError::UnrecognizedDatabase);
    }
    let index_columns = connection
        .prepare("PRAGMA index_info(agenturm_observations_by_session)")?
        .query_map([], |row| row.get::<_, String>(2))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if index_columns != ["session_key", "observed_at_unix_ms", "event_id"] {
        return Err(StoreError::UnrecognizedDatabase);
    }
    Ok(())
}

/// One SQLite connection owned by the caller. Methods requiring mutation take
/// `&mut self`, which serializes writes within this process. This is not a
/// cross-process lease or an untrusted-client boundary.
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens or creates a store at `path`. A new database receives the v1
    /// schema; an unknown or unrelated database is rejected without changing
    /// its journal mode or schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        let schema_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if schema_version == 0 {
            let user_objects: i64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite_%' AND type IN ('table', 'index', 'view', 'trigger')",
                [],
                |row| row.get(0),
            )?;
            if user_objects != 0 {
                return Err(StoreError::UnrecognizedDatabase);
            }
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "CREATE TABLE agenturm_store_meta (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                );
                INSERT INTO agenturm_store_meta (key, value)
                    VALUES ('format', 'agenturm.observation-store.v1');
                CREATE TABLE agenturm_ingest_receipts (
                    request_id TEXT PRIMARY KEY NOT NULL,
                    source_id TEXT NOT NULL,
                    batch_digest TEXT NOT NULL,
                    event_count INTEGER NOT NULL CHECK (event_count >= 0)
                );
                CREATE TABLE agenturm_source_state (
                    source_id TEXT PRIMARY KEY NOT NULL,
                    cursor TEXT NOT NULL
                );
                CREATE TABLE agenturm_observations (
                    source_id TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    event_digest TEXT NOT NULL,
                    session_key TEXT NOT NULL,
                    observed_at_unix_ms INTEGER NOT NULL CHECK (observed_at_unix_ms >= 0),
                    kind_json TEXT NOT NULL,
                    PRIMARY KEY (source_id, event_id),
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                        DEFERRABLE INITIALLY DEFERRED
                );
                CREATE INDEX agenturm_observations_by_session
                    ON agenturm_observations (session_key, observed_at_unix_ms, event_id);
                PRAGMA user_version = 1;",
            )?;
            transaction.commit()?;
        } else if schema_version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(schema_version));
        } else {
            validate_marked_v1_shape(&connection)?;
        }
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { connection })
    }

    /// Atomically records one source batch and its replay receipt. Repeating
    /// the same request returns `Replay`; changing content under a request or
    /// existing source/event identity fails without partial writes.
    pub fn commit_batch(&mut self, batch: &ObservationBatch) -> Result<CommitOutcome> {
        self.commit_batch_inner(batch, None)
    }

    fn commit_batch_inner(
        &mut self,
        batch: &ObservationBatch,
        fail_after_insert: Option<usize>,
    ) -> Result<CommitOutcome> {
        validate_batch(batch)?;
        let digest = batch_digest(batch)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, String)> = transaction
            .query_row(
                "SELECT source_id, batch_digest FROM agenturm_ingest_receipts WHERE request_id = ?1",
                [&batch.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((source_id, prior_digest)) = prior {
            if source_id == batch.source_id && prior_digest == digest {
                return Ok(CommitOutcome::Replay);
            }
            return Err(StoreError::RequestConflict);
        }

        let current_cursor: Option<String> = transaction
            .query_row(
                "SELECT cursor FROM agenturm_source_state WHERE source_id = ?1",
                [&batch.source_id],
                |row| row.get(0),
            )
            .optional()?;
        if current_cursor != batch.expected_cursor {
            return Err(StoreError::StaleCursor);
        }

        let mut inserted = 0;
        for observation in &batch.observations {
            let serialized_kind = serde_json::to_string(&observation.kind)?;
            let event_hash = event_digest(&batch.source_id, observation)?;
            let prior_digest: Option<String> = transaction
                .query_row(
                    "SELECT event_digest FROM agenturm_observations
                     WHERE source_id = ?1 AND event_id = ?2",
                    params![batch.source_id, observation.event_id],
                    |row| row.get(0),
                )
                .optional()?;
            match prior_digest {
                Some(prior_digest) if prior_digest == event_hash => continue,
                Some(_) => {
                    return Err(StoreError::EventConflict {
                        event_id: observation.event_id.clone(),
                    });
                }
                None => {
                    transaction.execute(
                        "INSERT INTO agenturm_observations
                         (source_id, event_id, request_id, event_digest, session_key,
                          observed_at_unix_ms, kind_json)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            batch.source_id,
                            observation.event_id,
                            batch.request_id,
                            event_hash,
                            observation.session_key,
                            i64::try_from(observation.observed_at_unix_ms).map_err(|_| {
                                StoreError::Invalid("timestamp exceeds SQLite INTEGER")
                            })?,
                            serialized_kind,
                        ],
                    )?;
                    inserted += 1;
                    if fail_after_insert == Some(inserted) {
                        return Err(StoreError::InjectedFailure);
                    }
                }
            }
        }
        transaction.execute(
            "INSERT INTO agenturm_ingest_receipts (request_id, source_id, batch_digest, event_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                batch.request_id,
                batch.source_id,
                digest,
                i64::try_from(batch.observations.len())
                    .map_err(|_| StoreError::Invalid("event count exceeds SQLite INTEGER"))?,
            ],
        )?;
        transaction.execute(
            "INSERT INTO agenturm_source_state (source_id, cursor) VALUES (?1, ?2)
             ON CONFLICT(source_id) DO UPDATE SET cursor = excluded.cursor",
            params![batch.source_id, batch.next_cursor],
        )?;
        transaction.commit()?;
        Ok(CommitOutcome::Committed { inserted })
    }

    /// Returns all observations in stable chronological/source/event order.
    pub fn observations(&self) -> Result<Vec<(String, StoredObservation)>> {
        let mut statement = self.connection.prepare(
            "SELECT source_id, event_id, session_key, observed_at_unix_ms, kind_json
             FROM agenturm_observations
             ORDER BY observed_at_unix_ms, source_id, event_id",
        )?;
        let rows = statement.query_map([], |row| {
            let timestamp: i64 = row.get(3)?;
            let kind_json: String = row.get(4)?;
            let kind = serde_json::from_str(&kind_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok((
                row.get::<_, String>(0)?,
                StoredObservation {
                    event_id: row.get(1)?,
                    session_key: row.get(2)?,
                    observed_at_unix_ms: u64::try_from(timestamp).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    kind,
                },
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Returns a source's opaque durable cursor, if any batch has committed.
    pub fn source_cursor(&self, source_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT cursor FROM agenturm_source_state WHERE source_id = ?1",
                [source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
}

fn validate_batch(batch: &ObservationBatch) -> Result<()> {
    validate_identifier(&batch.source_id, MAX_SOURCE_ID_BYTES, "source ID")?;
    validate_identifier(&batch.request_id, MAX_REQUEST_ID_BYTES, "request ID")?;
    validate_cursor(&batch.next_cursor)?;
    if let Some(cursor) = &batch.expected_cursor {
        validate_cursor(cursor)?;
    }
    if batch.observations.len() > MAX_BATCH_EVENTS {
        return Err(StoreError::Invalid("batch exceeds event count limit"));
    }
    let mut event_ids = std::collections::BTreeSet::new();
    for observation in &batch.observations {
        validate_identifier(&observation.event_id, MAX_EVENT_ID_BYTES, "event ID")?;
        if !event_ids.insert(observation.event_id.as_str()) {
            return Err(StoreError::Invalid("duplicate event ID within batch"));
        }
        if observation.session_key.is_empty()
            || observation.session_key.len() > MAX_SESSION_KEY_BYTES
            || observation.session_key.chars().any(char::is_control)
        {
            return Err(StoreError::Invalid(
                "session key is empty, invalid, or too long",
            ));
        }
        if observation.observed_at_unix_ms > i64::MAX as u64 {
            return Err(StoreError::Invalid("timestamp exceeds SQLite INTEGER"));
        }
        if let ObservationKind::Activity { category } = &observation.kind
            && (category.is_empty()
                || category.len() > MAX_CATEGORY_BYTES
                || category.chars().any(char::is_control))
        {
            return Err(StoreError::Invalid(
                "activity category is invalid or too long",
            ));
        }
    }
    Ok(())
}

fn validate_cursor(cursor: &str) -> Result<()> {
    if cursor.len() > MAX_CURSOR_BYTES || cursor.chars().any(char::is_control) {
        return Err(StoreError::Invalid("cursor is invalid or too long"));
    }
    Ok(())
}

fn validate_identifier(value: &str, max_bytes: usize, label: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return Err(StoreError::Invalid(label));
    }
    Ok(())
}

fn batch_digest(batch: &ObservationBatch) -> Result<String> {
    let mut observations = batch.observations.clone();
    observations.sort_by(|left, right| left.event_id.cmp(&right.event_id));
    let encoded = serde_json::to_vec(&DigestInput {
        source_id: &batch.source_id,
        expected_cursor: &batch.expected_cursor,
        next_cursor: &batch.next_cursor,
        observations: &observations,
    })?;
    Ok(hex_digest(&encoded))
}

#[derive(Serialize)]
struct DigestInput<'a> {
    source_id: &'a str,
    expected_cursor: &'a Option<String>,
    next_cursor: &'a str,
    observations: &'a [StoredObservation],
}

fn event_digest(source_id: &str, observation: &StoredObservation) -> Result<String> {
    let encoded = serde_json::to_vec(&EventDigestInput {
        source_id,
        observation,
    })?;
    Ok(hex_digest(&encoded))
}

#[derive(Serialize)]
struct EventDigestInput<'a> {
    source_id: &'a str,
    observation: &'a StoredObservation,
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDb(std::path::PathBuf);

    impl TempDb {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("agenturm-store-{}-{id}.sqlite", std::process::id()));
            Self(path)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    fn event(event_id: &str, session_key: &str, timestamp: u64) -> StoredObservation {
        StoredObservation {
            event_id: event_id.into(),
            session_key: session_key.into(),
            observed_at_unix_ms: timestamp,
            kind: ObservationKind::Activity {
                category: "tool".into(),
            },
        }
    }

    fn batch(request_id: &str, observations: Vec<StoredObservation>) -> ObservationBatch {
        ObservationBatch {
            request_id: request_id.into(),
            source_id: "harness.codex".into(),
            expected_cursor: None,
            next_cursor: "cursor-1".into(),
            observations,
        }
    }

    fn batch_after(
        request_id: &str,
        expected_cursor: &str,
        next_cursor: &str,
        observations: Vec<StoredObservation>,
    ) -> ObservationBatch {
        ObservationBatch {
            request_id: request_id.into(),
            source_id: "harness.codex".into(),
            expected_cursor: Some(expected_cursor.into()),
            next_cursor: next_cursor.into(),
            observations,
        }
    }

    #[test]
    fn commits_once_and_exact_request_replay_is_idempotent() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let input = batch("request-1", vec![event("event-1", "session-1", 10)]);
        assert_eq!(
            store.commit_batch(&input).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(store.commit_batch(&input).unwrap(), CommitOutcome::Replay);
        assert_eq!(store.observations().unwrap().len(), 1);
    }

    #[test]
    fn exact_retry_after_later_cursor_advance_is_still_a_replay() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let first = batch("request-1", vec![event("event-1", "session-1", 10)]);
        store.commit_batch(&first).unwrap();
        let second = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![event("event-2", "session-1", 11)],
        );
        store.commit_batch(&second).unwrap();

        assert_eq!(store.commit_batch(&first).unwrap(), CommitOutcome::Replay);
        assert_eq!(store.observations().unwrap().len(), 2);
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-2")
        );
    }

    #[test]
    fn request_id_reuse_with_different_batch_is_rejected() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let changed = batch("request-1", vec![event("event-2", "session-1", 10)]);
        assert!(matches!(
            store.commit_batch(&changed),
            Err(StoreError::RequestConflict)
        ));
        assert_eq!(store.observations().unwrap().len(), 1);
    }

    #[test]
    fn event_identity_cannot_be_reused_for_changed_content() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let changed = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![event("event-1", "session-1", 11)],
        );
        assert!(matches!(
            store.commit_batch(&changed),
            Err(StoreError::EventConflict { .. })
        ));
        assert_eq!(store.observations().unwrap().len(), 1);
    }

    #[test]
    fn failed_batch_rolls_back_rows_and_receipt() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let input = batch(
            "request-1",
            vec![
                event("event-1", "session-1", 10),
                event("event-2", "session-1", 11),
            ],
        );
        assert!(matches!(
            store.commit_batch_inner(&input, Some(1)),
            Err(StoreError::InjectedFailure)
        ));
        assert!(store.observations().unwrap().is_empty());
        assert_eq!(
            store.commit_batch(&input).unwrap(),
            CommitOutcome::Committed { inserted: 2 }
        );
    }

    #[test]
    fn committed_observations_survive_close_and_reopen() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
                .unwrap();
            assert_eq!(
                store.source_cursor("harness.codex").unwrap().as_deref(),
                Some("cursor-1")
            );
        }
        let store = Store::open(&db.0).unwrap();
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-1")
        );
        assert_eq!(
            store.observations().unwrap(),
            vec![("harness.codex".into(), event("event-1", "session-1", 10))]
        );
    }

    #[test]
    fn events_are_atomic_when_already_known_and_new_events_share_batch() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let mixed = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![
                event("event-1", "session-1", 10),
                event("event-2", "session-1", 11),
            ],
        );
        assert_eq!(
            store.commit_batch(&mixed).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(store.observations().unwrap().len(), 2);
    }

    #[test]
    fn malformed_batch_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let invalid = batch("request-1", vec![event("../bad", "session-1", 10)]);
        assert!(matches!(
            store.commit_batch(&invalid),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn unrelated_existing_database_is_rejected_without_modification() {
        let db = TempDb::new();
        {
            let connection = Connection::open(&db.0).unwrap();
            connection
                .execute_batch("CREATE TABLE unrelated (value TEXT NOT NULL);")
                .unwrap();
            connection
                .execute("INSERT INTO unrelated VALUES ('keep me')", [])
                .unwrap();
        }
        let before = fs::read(&db.0).unwrap();
        assert!(matches!(
            Store::open(&db.0),
            Err(StoreError::UnrecognizedDatabase)
        ));
        let after = fs::read(&db.0).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn stale_cursor_rejects_batch_without_rows_receipt_or_cursor_advance() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let first = batch("request-1", vec![event("event-1", "session-1", 10)]);
        store.commit_batch(&first).unwrap();
        let stale = batch_after(
            "request-2",
            "old-cursor",
            "cursor-2",
            vec![event("event-2", "session-1", 11)],
        );
        assert!(matches!(
            store.commit_batch(&stale),
            Err(StoreError::StaleCursor)
        ));
        assert_eq!(store.observations().unwrap().len(), 1);
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some("cursor-1")
        );
        let retry = batch_after(
            "request-2",
            "cursor-1",
            "cursor-2",
            vec![event("event-2", "session-1", 11)],
        );
        assert_eq!(
            store.commit_batch(&retry).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
    }
}
