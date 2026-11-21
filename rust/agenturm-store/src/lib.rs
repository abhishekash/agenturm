//! A small Rust-owned SQLite store for kernel observations.
//!
//! This crate proves typed atomic persistence, request replay, event identity,
//! and close/reopen reads. It is not yet a production storage authority: it
//! has no service principal, cross-process writer fence, product policy,
//! retention, backup protocol, or integration with the Agenturm app.
//!
//! Kernel observations carry adapter-native event ID/revision, nanosecond
//! timestamps, and typed session/worker/turn/operation identity. The store
//! implements the kernel's atomic admission port; IDs remain source-scoped and
//! must never be synthesized from timestamps or session keys.

#![forbid(unsafe_code)]

use agenturm_kernel::{
    MAX_DURABLE_SNAPSHOT_OBSERVATIONS, Observation, ObservationKind as KernelObservationKind,
    ObservationPage, ObservationQuery, ObservationStore, StoreCommit,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::Path;

const SCHEMA_VERSION: i64 = 7;
const MAX_BATCH_EVENTS: usize = 2_048;
const MAX_SOURCE_ID_BYTES: usize = 96;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_EVENT_ID_BYTES: usize = 256;
const MAX_SESSION_KEY_BYTES: usize = 512;
const MAX_CATEGORY_BYTES: usize = 64;
const MAX_CURSOR_BYTES: usize = 800;
const STORE_MARKER: &str = "agenturm.observation-store.v7";
const STORE_MARKER_V6: &str = "agenturm.observation-store.v6";
const STORE_MARKER_V5: &str = "agenturm.observation-store.v5";
const STORE_MARKER_V4: &str = "agenturm.observation-store.v4";
const STORE_MARKER_V3: &str = "agenturm.observation-store.v3";
const STORE_MARKER_V2: &str = "agenturm.observation-store.v2";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationKind {
    SessionStarted,
    SessionEnded,
    Activity {
        category: String,
    },
    Tool {
        category: String,
        result: String,
    },
    Lifecycle {
        state: String,
        reason_code: Option<String>,
    },
    Check {
        category: String,
        result: String,
    },
    Message {
        role: String,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredObservation {
    pub event_id: String,
    pub revision: u32,
    pub identity: ObservationIdentity,
    pub observed_at_unix_ns: u64,
    pub kind: ObservationKind,
}

/// Source-native session hierarchy associated with one stored fact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationIdentity {
    pub session_id: String,
    pub worker_id: Option<String>,
    pub turn_id: Option<String>,
    pub operation_id: Option<String>,
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

impl ObservationBatch {
    /// Builds a batch with a repeatable request ID derived from its complete
    /// typed contents and opaque cursor transition. Event input order does not
    /// affect the ID.
    pub fn new_deterministic(
        source_id: impl Into<String>,
        expected_cursor: Option<String>,
        next_cursor: impl Into<String>,
        observations: Vec<StoredObservation>,
    ) -> Result<Self> {
        let mut batch = Self {
            // The request ID is excluded from the digest input, so validate
            // every caller-controlled field and collection bound before
            // cloning or serializing the observations below.
            request_id: "pending".into(),
            source_id: source_id.into(),
            expected_cursor,
            next_cursor: next_cursor.into(),
            observations,
        };
        validate_batch(&batch)?;
        batch.request_id = batch_digest(&batch)?;
        validate_batch(&batch)?;
        Ok(batch)
    }

    /// Returns the canonical content digest used as this batch's request ID.
    pub fn deterministic_request_id(&self) -> Result<String> {
        batch_digest(self)
    }
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

fn validate_marked_shape(connection: &Connection, version: i64) -> Result<()> {
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
    let mut expected = vec![
        (
            "index".to_owned(),
            "agenturm_observations_by_session".to_owned(),
        ),
        ("table".to_owned(), "agenturm_ingest_receipts".to_owned()),
        ("table".to_owned(), "agenturm_observations".to_owned()),
        ("table".to_owned(), "agenturm_source_state".to_owned()),
        ("table".to_owned(), "agenturm_store_meta".to_owned()),
    ];
    if version >= 3 {
        expected.push(("table".to_owned(), "agenturm_history_batches".to_owned()));
        expected.sort();
    }
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
    let expected_marker = match version {
        2 => STORE_MARKER_V2,
        3 => STORE_MARKER_V3,
        4 => STORE_MARKER_V4,
        5 => STORE_MARKER_V5,
        6 => STORE_MARKER_V6,
        7 => STORE_MARKER,
        _ => return Err(StoreError::UnrecognizedDatabase),
    };
    if marker.as_deref() != Some(expected_marker) {
        return Err(StoreError::UnrecognizedDatabase);
    }
    let mut expected_tables = vec![
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
                ("revision", "INTEGER", 1, 3),
                ("request_id", "TEXT", 1, 0),
                ("event_digest", "TEXT", 1, 0),
                ("session_id", "TEXT", 1, 0),
                ("worker_id", "TEXT", 0, 0),
                ("turn_id", "TEXT", 0, 0),
                ("operation_id", "TEXT", 0, 0),
                ("observed_at_unix_ns", "INTEGER", 1, 0),
                ("kind_json", "TEXT", 1, 0),
            ],
        ),
    ];
    if version >= 3 {
        expected_tables.push((
            "agenturm_history_batches",
            vec![("sequence", "INTEGER", 0, 1), ("request_id", "TEXT", 1, 0)],
        ));
    }
    for (table, expected_columns) in expected_tables {
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
    if version >= 3 {
        let history_sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='agenturm_history_batches'",
            [],
            |row| row.get(0),
        )?;
        if !history_sql.contains("AUTOINCREMENT") {
            return Err(StoreError::UnrecognizedDatabase);
        }
        let history_foreign_keys = connection
            .prepare("PRAGMA foreign_key_list(agenturm_history_batches)")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if history_foreign_keys
            != [(
                "agenturm_ingest_receipts".into(),
                "request_id".into(),
                "request_id".into(),
            )]
        {
            return Err(StoreError::UnrecognizedDatabase);
        }
        let unique_request_id = connection
            .prepare("PRAGMA index_list(agenturm_history_batches)")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, unique)| *unique == 1)
            .any(|(index, _)| {
                connection
                    .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                    .and_then(|mut statement| {
                        statement
                            .query_map([index], |row| row.get::<_, String>(0))
                            .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
                    })
                    .is_ok_and(|columns| columns == ["request_id"])
            });
        if !unique_request_id {
            return Err(StoreError::UnrecognizedDatabase);
        }
    }
    let index_columns = connection
        .prepare("PRAGMA index_info(agenturm_observations_by_session)")?
        .query_map([], |row| row.get::<_, String>(2))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if index_columns != ["session_id", "observed_at_unix_ns", "event_id", "revision"] {
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
    /// Opens or creates a store at `path`. A new database receives the v7
    /// schema; an unknown or unrelated database is rejected without changing
    /// its journal mode or schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
    }

    /// Opens an existing v7 store through a read-only connection. This path
    /// never initializes, migrates, changes journal mode, or creates a DB.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let schema_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if schema_version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(schema_version));
        }
        validate_marked_shape(&connection, SCHEMA_VERSION)?;
        Ok(Self { connection })
    }

    fn open_with_flags(path: impl AsRef<Path>, flags: OpenFlags) -> Result<Self> {
        let mut connection = Connection::open_with_flags(path, flags)?;
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
                    VALUES ('format', 'agenturm.observation-store.v7');
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
                CREATE TABLE agenturm_history_batches (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    request_id TEXT NOT NULL UNIQUE,
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                );
                CREATE TABLE agenturm_observations (
                    source_id TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision >= 0),
                    request_id TEXT NOT NULL,
                    event_digest TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    worker_id TEXT,
                    turn_id TEXT,
                    operation_id TEXT,
                    observed_at_unix_ns INTEGER NOT NULL CHECK (observed_at_unix_ns >= 0),
                    kind_json TEXT NOT NULL,
                    PRIMARY KEY (source_id, event_id, revision),
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                        DEFERRABLE INITIALLY DEFERRED
                );
                CREATE INDEX agenturm_observations_by_session
                    ON agenturm_observations
                        (session_id, observed_at_unix_ns, event_id, revision);
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
        } else if schema_version == 2 {
            validate_marked_shape(&connection, 2)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "CREATE TABLE agenturm_history_batches (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                    request_id TEXT NOT NULL UNIQUE,
                    FOREIGN KEY (request_id) REFERENCES agenturm_ingest_receipts(request_id)
                );
                INSERT INTO agenturm_history_batches(request_id)
                    SELECT request_id FROM agenturm_ingest_receipts ORDER BY rowid;
                UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v3' WHERE key='format';
                PRAGMA user_version = 3;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 3)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v4' WHERE key='format';
                PRAGMA user_version = 4;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 4)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v5' WHERE key='format';
                PRAGMA user_version = 5;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 3 {
            validate_marked_shape(&connection, 3)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v4' WHERE key='format';
                PRAGMA user_version = 4;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 4)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v5' WHERE key='format';
                PRAGMA user_version = 5;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 4 {
            validate_marked_shape(&connection, 4)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v5' WHERE key='format';
                PRAGMA user_version = 5;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 5 {
            validate_marked_shape(&connection, 5)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v6' WHERE key='format';
                PRAGMA user_version = 6;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version == 6 {
            validate_marked_shape(&connection, 6)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "UPDATE agenturm_store_meta
                    SET value='agenturm.observation-store.v7' WHERE key='format';
                PRAGMA user_version = 7;",
            )?;
            transaction.commit()?;
            validate_marked_shape(&connection, 7)?;
        } else if schema_version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(schema_version));
        } else {
            validate_marked_shape(&connection, SCHEMA_VERSION)?;
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
                     WHERE source_id = ?1 AND event_id = ?2 AND revision = ?3",
                    params![batch.source_id, observation.event_id, observation.revision],
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
                        (source_id, event_id, revision, request_id, event_digest, session_id,
                          worker_id, turn_id, operation_id,
                          observed_at_unix_ns, kind_json)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            batch.source_id,
                            observation.event_id,
                            i64::from(observation.revision),
                            batch.request_id,
                            event_hash,
                            observation.identity.session_id,
                            observation.identity.worker_id,
                            observation.identity.turn_id,
                            observation.identity.operation_id,
                            i64::try_from(observation.observed_at_unix_ns).map_err(|_| {
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
            "INSERT INTO agenturm_history_batches(request_id) VALUES (?1)",
            [&batch.request_id],
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
            "SELECT source_id, event_id, revision, session_id, worker_id, turn_id,
                    operation_id, observed_at_unix_ns, kind_json
             FROM agenturm_observations
             ORDER BY observed_at_unix_ns, source_id, event_id, revision",
        )?;
        let rows = statement.query_map([], |row| {
            let revision: i64 = row.get(2)?;
            let timestamp: i64 = row.get(7)?;
            let kind_json: String = row.get(8)?;
            let kind = serde_json::from_str(&kind_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok((
                row.get::<_, String>(0)?,
                StoredObservation {
                    event_id: row.get(1)?,
                    revision: u32::try_from(revision).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    identity: ObservationIdentity {
                        session_id: row.get(3)?,
                        worker_id: row.get(4)?,
                        turn_id: row.get(5)?,
                        operation_id: row.get(6)?,
                    },
                    observed_at_unix_ns: u64::try_from(timestamp).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
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

    /// Reads the newest bounded durable page, returned in chronological order.
    /// `has_more` tells callers that older history is outside this snapshot.
    pub fn observation_page(&self, limit: usize) -> Result<ObservationPage> {
        self.query_observations(&ObservationQuery::latest(limit))
    }

    /// Reads one filtered, keyset-paginated page in chronological order.
    /// `before` is exclusive and uses the full stable sort key.
    pub fn query_observations(&self, query: &ObservationQuery) -> Result<ObservationPage> {
        if query.limit == 0 || query.limit > MAX_DURABLE_SNAPSHOT_OBSERVATIONS {
            return Err(StoreError::Invalid("observation page limit is invalid"));
        }
        if let Some(source_id) = &query.harness_id {
            validate_identifier(source_id, MAX_SOURCE_ID_BYTES, "harness ID")?;
            agenturm_kernel::ExtensionId::parse(source_id.clone())
                .map_err(|_| StoreError::Invalid("harness ID"))?;
        }
        if let Some(session_id) = &query.session_id {
            validate_identifier(session_id, MAX_SESSION_KEY_BYTES, "session ID")?;
        }
        if let Some(before) = &query.before {
            validate_identifier(&before.harness_id, MAX_SOURCE_ID_BYTES, "harness ID")?;
            agenturm_kernel::ExtensionId::parse(before.harness_id.clone())
                .map_err(|_| StoreError::Invalid("harness ID"))?;
            validate_identifier(&before.event_id, MAX_EVENT_ID_BYTES, "event ID")?;
            if before.observed_at_unix_ns > i64::MAX as u64 {
                return Err(StoreError::Invalid(
                    "query timestamp exceeds SQLite INTEGER",
                ));
            }
            if query.snapshot_id != Some(before.snapshot_id) {
                return Err(StoreError::Invalid("query cursor snapshot does not match"));
            }
        }
        let newest_snapshot_id: u64 = self.connection.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM agenturm_history_batches",
            [],
            |row| row.get::<_, i64>(0),
        )? as u64;
        let snapshot_id = query.snapshot_id.unwrap_or(newest_snapshot_id);
        if snapshot_id > i64::MAX as u64 {
            return Err(StoreError::Invalid("query snapshot ID is invalid"));
        }
        if snapshot_id > newest_snapshot_id {
            return Err(StoreError::Invalid("query snapshot ID is from the future"));
        }
        let mut statement = self.connection.prepare(
            "SELECT observation.source_id, observation.event_id, observation.revision,
                    observation.session_id, observation.worker_id, observation.turn_id,
                    observation.operation_id, observation.observed_at_unix_ns,
                    observation.kind_json
             FROM agenturm_observations AS observation
             JOIN agenturm_ingest_receipts AS receipt
               ON receipt.request_id=observation.request_id
             JOIN agenturm_history_batches AS history_batch
               ON history_batch.request_id=receipt.request_id
             WHERE history_batch.sequence <= ?1
               AND (?2 IS NULL OR observation.source_id=?2)
               AND (?3 IS NULL OR observation.session_id=?3)
               AND (?4 IS NULL OR
                    (observation.observed_at_unix_ns,observation.source_id,
                     observation.event_id,observation.revision) <
                    (?4,?5,?6,?7))
             ORDER BY observation.observed_at_unix_ns DESC, observation.source_id DESC,
                      observation.event_id DESC, observation.revision DESC
             LIMIT ?8",
        )?;
        let read_limit = i64::try_from(query.limit + 1)
            .map_err(|_| StoreError::Invalid("observation page limit is invalid"))?;
        let before = query.before.as_ref();
        let rows = statement.query_map(
            params![
                snapshot_id as i64,
                query.harness_id,
                query.session_id,
                before.map(|position| position.observed_at_unix_ns as i64),
                before.map(|position| position.harness_id.as_str()),
                before.map(|position| position.event_id.as_str()),
                before.map(|position| i64::from(position.revision)),
                read_limit,
            ],
            |row| {
                let revision: i64 = row.get(2)?;
                let timestamp: i64 = row.get(7)?;
                let kind_json: String = row.get(8)?;
                let kind = serde_json::from_str(&kind_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    u32::try_from(revision).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    ObservationIdentity {
                        session_id: row.get(3)?,
                        worker_id: row.get(4)?,
                        turn_id: row.get(5)?,
                        operation_id: row.get(6)?,
                    },
                    u64::try_from(timestamp).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    kind,
                ))
            },
        )?;
        let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = rows.len() > query.limit;
        rows.truncate(query.limit);
        rows.reverse();
        let mut observations = Vec::with_capacity(rows.len());
        for (source_id, event_id, revision, identity, observed_at_unix_ns, kind) in rows {
            let harness_id = agenturm_kernel::ExtensionId::parse(source_id)
                .map_err(|_| StoreError::Invalid("stored source ID is invalid"))?;
            observations.push(Observation {
                harness_id,
                event_id,
                revision,
                identity: agenturm_kernel::ObservationIdentity {
                    session_id: identity.session_id,
                    worker_id: identity.worker_id,
                    turn_id: identity.turn_id,
                    operation_id: identity.operation_id,
                },
                observed_at_unix_ns,
                kind: match kind {
                    ObservationKind::SessionStarted => KernelObservationKind::SessionStarted,
                    ObservationKind::SessionEnded => KernelObservationKind::SessionEnded,
                    ObservationKind::Activity { category } => {
                        KernelObservationKind::Activity { category }
                    }
                    ObservationKind::Tool { category, result } => {
                        KernelObservationKind::Tool { category, result }
                    }
                    ObservationKind::Lifecycle { state, reason_code } => {
                        KernelObservationKind::Lifecycle { state, reason_code }
                    }
                    ObservationKind::Check { category, result } => {
                        KernelObservationKind::Check { category, result }
                    }
                    ObservationKind::Message { role } => KernelObservationKind::Message { role },
                    ObservationKind::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        total_tokens,
                    } => KernelObservationKind::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        total_tokens,
                    },
                },
            });
        }
        Ok(ObservationPage {
            observations,
            has_more,
            snapshot_id,
        })
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

impl ObservationStore for Store {
    fn source_cursor(&self, source_id: &str) -> std::result::Result<Option<String>, String> {
        Store::source_cursor(self, source_id).map_err(|error| error.to_string())
    }

    fn load_observations(&self, limit: usize) -> std::result::Result<ObservationPage, String> {
        self.observation_page(limit)
            .map_err(|error| error.to_string())
    }

    fn query_observations(
        &self,
        query: &ObservationQuery,
    ) -> std::result::Result<ObservationPage, String> {
        Store::query_observations(self, query).map_err(|error| error.to_string())
    }

    fn commit(
        &mut self,
        source_id: &str,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        observations: &[Observation],
    ) -> std::result::Result<StoreCommit, String> {
        let mut stored = Vec::with_capacity(observations.len());
        for observation in observations {
            if observation.harness_id.as_str() != source_id {
                return Err("observation source identity mismatch".into());
            }
            stored.push(StoredObservation {
                event_id: observation.event_id.clone(),
                revision: observation.revision,
                identity: ObservationIdentity {
                    session_id: observation.identity.session_id.clone(),
                    worker_id: observation.identity.worker_id.clone(),
                    turn_id: observation.identity.turn_id.clone(),
                    operation_id: observation.identity.operation_id.clone(),
                },
                observed_at_unix_ns: observation.observed_at_unix_ns,
                kind: match &observation.kind {
                    KernelObservationKind::SessionStarted => ObservationKind::SessionStarted,
                    KernelObservationKind::SessionEnded => ObservationKind::SessionEnded,
                    KernelObservationKind::Activity { category } => ObservationKind::Activity {
                        category: category.clone(),
                    },
                    KernelObservationKind::Tool { category, result } => ObservationKind::Tool {
                        category: category.clone(),
                        result: result.clone(),
                    },
                    KernelObservationKind::Lifecycle { state, reason_code } => {
                        ObservationKind::Lifecycle {
                            state: state.clone(),
                            reason_code: reason_code.clone(),
                        }
                    }
                    KernelObservationKind::Check { category, result } => ObservationKind::Check {
                        category: category.clone(),
                        result: result.clone(),
                    },
                    KernelObservationKind::Message { role } => {
                        ObservationKind::Message { role: role.clone() }
                    }
                    KernelObservationKind::Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                        total_tokens,
                    } => ObservationKind::Usage {
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        cache_read_tokens: *cache_read_tokens,
                        cache_write_tokens: *cache_write_tokens,
                        total_tokens: *total_tokens,
                    },
                },
            });
        }
        let batch = ObservationBatch::new_deterministic(
            source_id,
            expected_cursor.map(str::to_owned),
            next_cursor,
            stored,
        )
        .map_err(|error| error.to_string())?;
        match self
            .commit_batch(&batch)
            .map_err(|error| error.to_string())?
        {
            CommitOutcome::Committed { inserted } => Ok(StoreCommit {
                inserted,
                replay: false,
            }),
            CommitOutcome::Replay => Ok(StoreCommit {
                inserted: 0,
                replay: true,
            }),
        }
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
        if !event_ids.insert((observation.event_id.as_str(), observation.revision)) {
            return Err(StoreError::Invalid(
                "duplicate event ID and revision within batch",
            ));
        }
        validate_identifier(
            &observation.identity.session_id,
            MAX_SESSION_KEY_BYTES,
            "session ID",
        )?;
        for (value, label) in [
            (observation.identity.worker_id.as_deref(), "worker ID"),
            (observation.identity.turn_id.as_deref(), "turn ID"),
            (observation.identity.operation_id.as_deref(), "operation ID"),
        ] {
            if let Some(value) = value {
                validate_identifier(value, MAX_SESSION_KEY_BYTES, label)?;
            }
        }
        if observation.observed_at_unix_ns > i64::MAX as u64 {
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
        if let ObservationKind::Tool { category, result } = &observation.kind
            && (!(matches!(category.as_str(), "shell" | "file" | "mcp" | "other"))
                || !(matches!(
                    result.as_str(),
                    "started" | "succeeded" | "failed" | "denied"
                )))
        {
            return Err(StoreError::Invalid("tool category or result is invalid"));
        }
        if let ObservationKind::Lifecycle { state, reason_code } = &observation.kind
            && (!matches!(
                state.as_str(),
                "running" | "completed" | "interrupted" | "paused"
            ) || reason_code.as_ref().is_some_and(|reason| {
                reason.is_empty()
                    || reason.len() > 128
                    || !reason.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
                    })
            }))
        {
            return Err(StoreError::Invalid("lifecycle state or reason is invalid"));
        }
        if let ObservationKind::Check { category, result } = &observation.kind
            && (!matches!(category.as_str(), "test" | "lint" | "build")
                || !matches!(result.as_str(), "passed" | "failed" | "unknown"))
        {
            return Err(StoreError::Invalid("check category or result is invalid"));
        }
        if let ObservationKind::Message { role } = &observation.kind
            && !matches!(role.as_str(), "user" | "assistant")
        {
            return Err(StoreError::Invalid("message role is invalid"));
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
    observations.sort_by(|left, right| {
        left.event_id
            .cmp(&right.event_id)
            .then(left.revision.cmp(&right.revision))
    });
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
    use agenturm_kernel::ObservationPosition;
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

    fn event(event_id: &str, session_id: &str, timestamp: u64) -> StoredObservation {
        StoredObservation {
            event_id: event_id.into(),
            revision: 0,
            identity: ObservationIdentity {
                session_id: session_id.into(),
                worker_id: Some("worker-1".into()),
                turn_id: Some("turn-1".into()),
                operation_id: Some("operation-1".into()),
            },
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::Activity {
                category: "tool".into(),
            },
        }
    }

    fn tool_event(event_id: &str, session_id: &str, timestamp: u64) -> StoredObservation {
        let mut observation = event(event_id, session_id, timestamp);
        observation.kind = ObservationKind::Tool {
            category: "shell".into(),
            result: "succeeded".into(),
        };
        observation
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
    fn typed_tool_lifecycle_check_and_message_observations_survive_store_reopen() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            let mut observation = tool_event("tool-event-1", "session-1", 10);
            observation.identity.operation_id = Some("opaque-operation-1".into());
            let mut lifecycle = event("lifecycle-event-1", "session-1", 11);
            lifecycle.kind = ObservationKind::Lifecycle {
                state: "interrupted".into(),
                reason_code: Some("codex_task_aborted".into()),
            };
            let mut check = event("check-event-1", "session-1", 12);
            check.kind = ObservationKind::Check {
                category: "lint".into(),
                result: "passed".into(),
            };
            let mut message = event("message-event-1", "session-1", 13);
            message.kind = ObservationKind::Message {
                role: "user".into(),
            };
            store
                .commit_batch(&batch(
                    "tool-request-1",
                    vec![observation, lifecycle, check, message],
                ))
                .unwrap();
        }
        let store = Store::open_read_only(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.observations.len(), 4);
        let tool = page
            .observations
            .iter()
            .find(|row| matches!(row.kind, KernelObservationKind::Tool { .. }))
            .unwrap();
        assert_eq!(
            tool.kind,
            KernelObservationKind::Tool {
                category: "shell".into(),
                result: "succeeded".into(),
            }
        );
        assert_eq!(
            tool.identity.operation_id.as_deref(),
            Some("opaque-operation-1")
        );
        assert!(page.observations.iter().any(|row| {
            row.kind
                == KernelObservationKind::Lifecycle {
                    state: "interrupted".into(),
                    reason_code: Some("codex_task_aborted".into()),
                }
        }));
        assert!(page.observations.iter().any(|row| {
            row.kind
                == KernelObservationKind::Message {
                    role: "user".into(),
                }
        }));
        assert!(page.observations.iter().any(|row| {
            row.kind
                == KernelObservationKind::Check {
                    category: "lint".into(),
                    result: "passed".into(),
                }
        }));
    }

    #[test]
    fn bounded_observation_page_survives_reopen_and_reports_older_rows() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "request-page-1",
                    vec![event("event-1", "session-1", 10)],
                ))
                .unwrap();
            store
                .commit_batch(&batch_after(
                    "request-page-2",
                    "cursor-1",
                    "cursor-2",
                    vec![event("event-2", "session-2", 20)],
                ))
                .unwrap();
        }

        let store = Store::open(&db.0).unwrap();
        let newest = store.observation_page(1).unwrap();
        assert!(newest.has_more);
        assert_eq!(newest.observations.len(), 1);
        assert_eq!(newest.observations[0].event_id, "event-2");
        assert_eq!(newest.observations[0].harness_id.as_str(), "harness.codex");

        let complete = store.observation_page(2).unwrap();
        assert!(!complete.has_more);
        assert_eq!(
            complete
                .observations
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-1", "event-2"]
        );
        assert!(store.observation_page(0).is_err());
    }

    #[test]
    fn v2_store_upgrades_atomically_to_explicit_history_sequences() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "upgrade-request",
                    vec![event("event-1", "session-1", 1)],
                ))
                .unwrap();
            store
                .connection
                .execute_batch(
                    "DROP TABLE agenturm_history_batches;
                 UPDATE agenturm_store_meta SET value='agenturm.observation-store.v2'
                   WHERE key='format';
                 PRAGMA user_version=2;",
                )
                .unwrap();
        }

        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1);
        assert_eq!(page.observations.len(), 1);
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
    }

    #[test]
    fn v3_store_upgrades_through_v4_v5_and_v6_to_v7_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v3-request", vec![event("v3-event", "s1", 1)]))
                .unwrap();
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v3'
                       WHERE key='format';
                     PRAGMA user_version=3;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1);
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
    }

    #[test]
    fn v4_store_upgrades_through_v5_and_v6_to_v7_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v4-request", vec![event("v4-event", "s1", 1)]))
                .unwrap();
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v4'
                       WHERE key='format';
                     PRAGMA user_version=4;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1);
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
    }

    #[test]
    fn v5_store_upgrades_through_v6_to_v7_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v5-request", vec![event("v5-event", "s1", 1)]))
                .unwrap();
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v5'
                       WHERE key='format';
                     PRAGMA user_version=5;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1);
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
    }

    #[test]
    fn v6_store_upgrades_to_v7_without_changing_history() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("v6-request", vec![event("v6-event", "s1", 1)]))
                .unwrap();
            store
                .connection
                .execute_batch(
                    "UPDATE agenturm_store_meta SET value='agenturm.observation-store.v6'
                       WHERE key='format';
                     PRAGMA user_version=6;",
                )
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let page = store.observation_page(10).unwrap();
        assert_eq!(page.snapshot_id, 1);
        assert_eq!(page.observations.len(), 1);
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
    }

    #[test]
    fn read_only_store_can_query_live_wal_without_admitting_writes() {
        let db = TempDb::new();
        let mut writer = Store::open(&db.0).unwrap();
        writer
            .commit_batch(&batch(
                "readonly-request",
                vec![event("event-1", "session-1", 1)],
            ))
            .unwrap();

        let reader = Store::open_read_only(&db.0).unwrap();
        let page = reader.observation_page(10).unwrap();
        assert_eq!(page.observations.len(), 1);
        assert!(matches!(
            reader.connection.execute(
                "DELETE FROM agenturm_observations WHERE event_id='event-1'",
                [],
            ),
            Err(rusqlite::Error::SqliteFailure(_, _))
        ));
        writer
            .commit_batch(&batch_after(
                "readonly-request-2",
                "cursor-1",
                "cursor-2",
                vec![event("event-2", "session-1", 2)],
            ))
            .unwrap();
        let refreshed = reader.observation_page(10).unwrap();
        assert_eq!(refreshed.observations.len(), 2);
    }

    #[test]
    fn filtered_keyset_query_pages_equal_timestamps_without_skips_or_duplicates() {
        let db = TempDb::new();
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "query-codex",
                    vec![
                        event("event-1", "session-a", 10),
                        event("event-2", "session-a", 20),
                        event("event-3", "session-a", 20),
                        event("event-4", "session-b", 30),
                    ],
                ))
                .unwrap();
            let mut pi = batch("query-pi", vec![event("event-5", "session-a", 20)]);
            pi.source_id = "harness.pi".into();
            store.commit_batch(&pi).unwrap();
        }

        let store = Store::open(&db.0).unwrap();
        let first = store
            .query_observations(&ObservationQuery {
                harness_id: None,
                session_id: Some("session-a".into()),
                before: None,
                snapshot_id: None,
                limit: 2,
            })
            .unwrap();
        assert!(first.has_more);
        assert_eq!(
            first
                .observations
                .iter()
                .map(|row| (row.harness_id.as_str(), row.event_id.as_str()))
                .collect::<Vec<_>>(),
            [("harness.codex", "event-3"), ("harness.pi", "event-5")]
        );

        let mut late = batch_after(
            "query-late-write",
            "cursor-1",
            "cursor-2",
            vec![event("event-0", "session-a", 5)],
        );
        late.source_id = "harness.codex".into();
        let mut store = store;
        store.commit_batch(&late).unwrap();

        let oldest = first.observations.first().unwrap();
        let second = store
            .query_observations(&ObservationQuery {
                harness_id: None,
                session_id: Some("session-a".into()),
                before: Some(ObservationPosition {
                    observed_at_unix_ns: oldest.observed_at_unix_ns,
                    harness_id: oldest.harness_id.as_str().into(),
                    event_id: oldest.event_id.clone(),
                    revision: oldest.revision,
                    snapshot_id: first.snapshot_id,
                }),
                snapshot_id: Some(first.snapshot_id),
                limit: 2,
            })
            .unwrap();
        assert!(!second.has_more);
        assert_eq!(
            second
                .observations
                .iter()
                .map(|row| row.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event-1", "event-2"]
        );
        let latest = store
            .query_observations(&ObservationQuery {
                harness_id: None,
                session_id: Some("session-a".into()),
                before: None,
                snapshot_id: None,
                limit: 8,
            })
            .unwrap();
        assert!(
            latest
                .observations
                .iter()
                .any(|observation| observation.event_id == "event-0")
        );
    }

    #[test]
    fn keyset_query_validates_filters_and_cursor_before_sql() {
        let db = TempDb::new();
        let store = Store::open(&db.0).unwrap();
        let invalid = [
            ObservationQuery {
                harness_id: Some("Invalid ID".into()),
                session_id: None,
                before: None,
                snapshot_id: None,
                limit: 1,
            },
            ObservationQuery {
                harness_id: None,
                session_id: Some("bad session".into()),
                before: None,
                snapshot_id: None,
                limit: 1,
            },
            ObservationQuery {
                harness_id: None,
                session_id: None,
                before: Some(ObservationPosition {
                    observed_at_unix_ns: 1,
                    harness_id: "harness.codex".into(),
                    event_id: "bad event".into(),
                    revision: 0,
                    snapshot_id: 1,
                }),
                snapshot_id: Some(1),
                limit: 1,
            },
        ];
        for query in invalid {
            assert!(matches!(
                store.query_observations(&query),
                Err(StoreError::Invalid(_))
            ));
        }
    }

    #[test]
    fn deterministic_request_id_is_stable_across_event_input_order() {
        let first = event("event-1", "session-1", 10);
        let second = event("event-2", "session-1", 11);
        let forward = ObservationBatch::new_deterministic(
            "harness.codex",
            Some("cursor-0".into()),
            "cursor-1",
            vec![first.clone(), second.clone()],
        )
        .unwrap();
        let reversed = ObservationBatch::new_deterministic(
            "harness.codex",
            Some("cursor-0".into()),
            "cursor-1",
            vec![second, first],
        )
        .unwrap();

        assert_eq!(forward.request_id, reversed.request_id);
        assert_eq!(
            forward.deterministic_request_id().unwrap(),
            forward.request_id
        );
    }

    #[test]
    fn deterministic_batch_constructor_rejects_bounds_before_digest() {
        let observations = (0..=MAX_BATCH_EVENTS)
            .map(|index| event(&format!("event-{index}"), "session-1", index as u64))
            .collect();
        assert!(matches!(
            ObservationBatch::new_deterministic("harness.codex", None, "cursor-1", observations),
            Err(StoreError::Invalid("batch exceeds event count limit"))
        ));
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
    fn higher_revision_is_stored_as_a_correction_without_overwriting_history() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        store
            .commit_batch(&batch("request-1", vec![event("event-1", "session-1", 10)]))
            .unwrap();
        let mut correction = event("event-1", "session-1", 11);
        correction.revision = 1;
        store
            .commit_batch(&batch_after(
                "request-2",
                "cursor-1",
                "cursor-2",
                vec![correction],
            ))
            .unwrap();

        let observations = store.observations().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].1.revision, 0);
        assert_eq!(observations[1].1.revision, 1);
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
    fn complete_native_identity_survives_close_and_reopen() {
        let db = TempDb::new();
        let expected = event("event-1", "session-1", 10);
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch("request-1", vec![expected.clone()]))
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        let actual = store.observations().unwrap();
        assert_eq!(actual[0].1.identity, expected.identity);
    }

    #[test]
    fn nanosecond_timestamps_above_javascript_integer_precision_survive_reopen() {
        let db = TempDb::new();
        let timestamp_ns = 1_700_000_000_123_456_789;
        {
            let mut store = Store::open(&db.0).unwrap();
            store
                .commit_batch(&batch(
                    "request-1",
                    vec![event("event-1", "session-1", timestamp_ns)],
                ))
                .unwrap();
        }
        let store = Store::open(&db.0).unwrap();
        assert_eq!(
            store.observations().unwrap()[0].1.observed_at_unix_ns,
            timestamp_ns
        );
    }

    #[test]
    fn source_cursor_accepts_protocol_limit_and_rejects_over_limit() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let max_cursor = "c".repeat(MAX_CURSOR_BYTES);
        let first = ObservationBatch {
            request_id: "request-1".into(),
            source_id: "harness.codex".into(),
            expected_cursor: None,
            next_cursor: max_cursor.clone(),
            observations: vec![],
        };
        store.commit_batch(&first).unwrap();
        let over_limit = batch_after(
            "request-2",
            &max_cursor,
            &"n".repeat(MAX_CURSOR_BYTES + 1),
            vec![],
        );
        assert!(matches!(
            store.commit_batch(&over_limit),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(
            store.source_cursor("harness.codex").unwrap().as_deref(),
            Some(max_cursor.as_str())
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
    fn invalid_check_category_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid = event("event-1", "session-1", 10);
        invalid.kind = ObservationKind::Check {
            category: "coverage".into(),
            result: "passed".into(),
        };
        assert!(matches!(
            store.commit_batch(&batch("request-1", vec![invalid])),
            Err(StoreError::Invalid("check category or result is invalid"))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn invalid_message_role_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid = event("event-1", "session-1", 10);
        invalid.kind = ObservationKind::Message {
            role: "system".into(),
        };
        assert!(matches!(
            store.commit_batch(&batch("request-1", vec![invalid])),
            Err(StoreError::Invalid("message role is invalid"))
        ));
        assert!(store.observations().unwrap().is_empty());
    }

    #[test]
    fn invalid_optional_native_identity_is_rejected_before_database_writes() {
        let db = TempDb::new();
        let mut store = Store::open(&db.0).unwrap();
        let mut invalid = event("event-1", "session-1", 10);
        invalid.identity.operation_id = Some("operation id with spaces".into());
        assert!(matches!(
            store.commit_batch(&batch("request-1", vec![invalid])),
            Err(StoreError::Invalid("operation ID"))
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
