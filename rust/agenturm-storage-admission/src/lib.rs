//! Scratch-only SQLite admission experiment for host-prepared lifecycle facts.
//!
//! This crate is deliberately absent from the receiver and plugin host. Its
//! CLI requires an explicit marker on a disposable database before opening a
//! read/write connection, then holds a cooperating writer lease across its
//! transaction. It does not validate plugin wire messages or replace Python's
//! admission, retention, discovery, content, or policy code.

#![forbid(unsafe_code)]

use agenturm_storage_lease::WriterLease;
use agenturm_storage_service_contract::v2;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 48;
const MAX_SOURCE_RECORDS: i64 = 50_000;
const SCRATCH_MARKER: &str = "storage_admission_scratch_v1";

fn reject(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn cursor(value: &str) -> bool {
    value.len() <= 512 && !value.chars().any(char::is_control)
}

fn reason_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().skip(1).all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

fn lifecycle_payload(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let Some(payload) = value.as_object() else {
        return false;
    };
    if !matches!(payload.len(), 1 | 2)
        || payload
            .keys()
            .any(|key| key != "state" && key != "reason_code")
        || !payload
            .get("state")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|state| {
                matches!(
                    state,
                    "started"
                        | "running"
                        | "paused"
                        | "completed"
                        | "failed"
                        | "interrupted"
                        | "cancelled"
                )
            })
        || payload
            .get("reason_code")
            .is_some_and(|reason| !reason.as_str().is_some_and(reason_code))
    {
        return false;
    }
    // Host-prepared payloads are already canonical JSON. This also rejects
    // duplicate-key and alternate-encoding strings that normalize on parse.
    serde_json::to_string(&value).is_ok_and(|canonical| canonical == raw)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lease {
    receiver_id: String,
    plugin_id: String,
    source_id: String,
    generation: String,
    epoch: i64,
    policy_epoch: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedRecord {
    event_id_hash: String,
    revision: i64,
    kind: String,
    time_ns: i64,
    payload_json: String,
    record_digest: String,
    native_session_id: String,
    native_worker_id: String,
    native_turn_id: String,
    native_operation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    sqlite_path: String,
    lease: Lease,
    expected_cursor: String,
    cursor: String,
    batch_digest: String,
    records: Vec<PreparedRecord>,
}

#[derive(Debug, Serialize)]
struct Receipt {
    cursor: String,
    disposition: &'static str,
    dirty_seq: i64,
}

fn finish(
    transaction: Transaction<'_>,
    receipt: Receipt,
    request: &Request,
    replay: Option<&v2::ReplayIdentity>,
    existing_v2_receipt: bool,
) -> Result<Receipt> {
    if let Some(replay) = replay.filter(|_| !existing_v2_receipt) {
        transaction.execute(
            "INSERT INTO storage_admission_request_v2 \
             (request_id,receiver_id,request_digest,source_id,cursor,batch_digest,dirty_seq,disposition) \
             VALUES(?,?,?,?,?,?,?,?)",
            params![
                replay.request_id,
                replay.receiver_id,
                replay.digest,
                request.lease.source_id,
                receipt.cursor,
                request.batch_digest,
                receipt.dirty_seq,
                receipt.disposition,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(receipt)
}

fn validate(request: &Request) -> Result<()> {
    let lease = &request.lease;
    if request.schema_version != 1
        || !identity(&lease.receiver_id)
        || !identity(&lease.plugin_id)
        || !identity(&lease.source_id)
        || !hash(&lease.generation)
        || lease.epoch <= 0
        || lease.policy_epoch <= 0
        || !cursor(&request.expected_cursor)
        || !cursor(&request.cursor)
        || !hash(&request.batch_digest)
        || request.records.len() > MAX_RECORDS
    {
        return Err(reject("invalid prepared admission request"));
    }
    let mut seen = HashSet::new();
    for row in &request.records {
        if !hash(&row.event_id_hash)
            || !seen.insert(&row.event_id_hash)
            || row.revision != 0
            || row.kind != "lifecycle"
            || row.time_ns < 0
            || row.payload_json.len() > 4096
            || !hash(&row.record_digest)
            || ![
                &row.native_session_id,
                &row.native_worker_id,
                &row.native_turn_id,
                &row.native_operation_id,
            ]
            .into_iter()
            .all(|field| field.is_empty() || identity(field))
            || !lifecycle_payload(&row.payload_json)
        {
            return Err(reject("unsupported prepared lifecycle record"));
        }
    }
    Ok(())
}

fn now_ns() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
    )?)
}

fn require_scratch(db: &Connection) -> Result<()> {
    let marker: Option<String> = db
        .query_row(
            "SELECT value FROM schema_meta WHERE key=?",
            [SCRATCH_MARKER],
            |row| row.get(0),
        )
        .optional()?;
    if marker.as_deref() != Some("enabled") {
        return Err(reject("database is not an explicit scratch fixture"));
    }
    Ok(())
}

fn commit_guarded(
    db: &mut Connection,
    request: &Request,
    writer_lease: Option<&WriterLease>,
    replay: Option<&v2::ReplayIdentity>,
) -> Result<Receipt> {
    validate(request)?;
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(lease) = writer_lease {
        lease.verify_admission()?;
    }
    require_scratch(&transaction)?;
    let receiver: String = transaction.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    type Source = (
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        String,
        i64,
        String,
    );
    let source: Source = transaction.query_row(
        "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
         content_fingerprint,cursor,prior_cursor,dirty_seq,last_batch_digest \
         FROM plugin_source_cursor_v1 WHERE source_id=?",
        [&request.lease.source_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        },
    )?;
    let (
        source_receiver,
        plugin,
        generation,
        epoch,
        policy,
        enabled,
        fingerprint,
        current_cursor,
        prior_cursor,
        dirty_seq,
        prior_digest,
    ) = source;
    if receiver != request.lease.receiver_id
        || source_receiver != receiver
        || plugin != request.lease.plugin_id
        || generation != request.lease.generation
        || epoch != request.lease.epoch
        || policy != request.lease.policy_epoch
        || enabled != 1
        || !fingerprint.is_empty()
    {
        return Err(reject("stale or unsupported source lease"));
    }
    // This experiment deliberately covers only an ordinary metadata-only
    // lifecycle source. A scratch copy with discovery, content, retention,
    // gaps, or prior non-lifecycle evidence must take the Python path.
    let unsupported: i64 = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_content_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1 \
                   AND (revision<>0 OR kind<>'lifecycle' OR member_id<>''))",
        [&request.lease.source_id],
        |row| row.get(0),
    )?;
    if unsupported != 0 {
        return Err(reject("scratch source has unsupported history"));
    }
    let mut existing_v2_receipt = false;
    if let Some(replay) = replay {
        // The request identity is committed in the same scratch transaction as
        // the cursor/evidence. A process crash cannot leave a successful v2
        // admission without its conflict fence.
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS storage_admission_request_v2( \
             request_id TEXT PRIMARY KEY, receiver_id TEXT NOT NULL, \
             request_digest TEXT NOT NULL, source_id TEXT NOT NULL, \
             cursor TEXT NOT NULL, batch_digest TEXT NOT NULL, \
             dirty_seq INTEGER NOT NULL, disposition TEXT NOT NULL)",
        )?;
        type StoredReceipt = (String, String, String, String, String, i64, String);
        let stored: Option<StoredReceipt> = transaction
            .query_row(
                "SELECT receiver_id,source_id,request_digest,cursor,batch_digest,dirty_seq,disposition \
                 FROM storage_admission_request_v2 \
                 WHERE request_id=?",
                [&replay.request_id],
                |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                        row.get(4)?, row.get(5)?, row.get(6)?))
                },
            )
            .optional()?;
        if let Some((receiver, source, digest, cursor, batch, recorded_dirty, disposition)) = stored
        {
            if receiver != replay.receiver_id
                || source != request.lease.source_id
                || digest != replay.digest
                || cursor != request.cursor
                || batch != request.batch_digest
                || recorded_dirty != dirty_seq
                || !matches!(disposition.as_str(), "committed" | "replay" | "idle")
            {
                return Err(reject("conflicting v2 request replay identity"));
            }
            existing_v2_receipt = true;
        }
    }
    if request.records.is_empty()
        && request.cursor == current_cursor
        && request.expected_cursor == current_cursor
    {
        return finish(
            transaction,
            Receipt {
                cursor: current_cursor,
                disposition: "idle",
                dirty_seq,
            },
            request,
            replay,
            existing_v2_receipt,
        );
    }
    if request.cursor == current_cursor
        && request.batch_digest == prior_digest
        && (request.expected_cursor == prior_cursor || request.expected_cursor == current_cursor)
    {
        let receipt: Option<(String, String)> = transaction
            .query_row(
                "SELECT cursor,refs_digest FROM plugin_content_batch_v1 WHERE source_id=?",
                [&request.lease.source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let expected_digest = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
        if receipt
            .is_some_and(|(cursor, digest)| cursor != request.cursor || digest != expected_digest)
        {
            return Err(reject("scratch empty-content replay receipt mismatch"));
        }
        return finish(
            transaction,
            Receipt {
                cursor: current_cursor,
                disposition: "replay",
                dirty_seq,
            },
            request,
            replay,
            existing_v2_receipt,
        );
    }
    if request.expected_cursor != current_cursor {
        return Err(reject("stale source cursor"));
    }
    if request.cursor == current_cursor {
        return Err(reject("source cursor did not advance"));
    }
    let mut count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
        [&request.lease.source_id],
        |row| row.get(0),
    )?;
    let admitted_ns = now_ns()?;
    let mut inserted = false;
    for row in &request.records {
        type Existing = (i64, String, String, String, String, String, String, String);
        let latest: Option<Existing> = transaction
            .query_row(
                "SELECT revision,record_digest,kind,native_session_id,native_worker_id,\
                 native_turn_id,native_operation_id,member_id FROM plugin_evidence_v1 \
                 WHERE source_id=? AND event_id_hash=? ORDER BY revision DESC LIMIT 1",
                params![request.lease.source_id, row.event_id_hash],
                |found| {
                    Ok((
                        found.get(0)?,
                        found.get(1)?,
                        found.get(2)?,
                        found.get(3)?,
                        found.get(4)?,
                        found.get(5)?,
                        found.get(6)?,
                        found.get(7)?,
                    ))
                },
            )
            .optional()?;
        if let Some((revision, digest, kind, session, worker, turn, operation, member)) = latest {
            if revision != 0
                || kind != row.kind
                || session != row.native_session_id
                || worker != row.native_worker_id
                || turn != row.native_turn_id
                || operation != row.native_operation_id
                || !member.is_empty()
                || digest != row.record_digest
            {
                return Err(reject("conflicting or unsupported event revision"));
            }
            continue;
        }
        if count >= MAX_SOURCE_RECORDS {
            return Err(reject("scratch evidence capacity reached"));
        }
        transaction.execute(
            "INSERT INTO plugin_evidence_v1 \
             (source_id,event_id_hash,revision,kind,time_ns,payload_json,record_digest,\
             native_session_id,native_worker_id,native_turn_id,native_operation_id,member_id,\
             policy_epoch,admitted_ns) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                request.lease.source_id,
                row.event_id_hash,
                row.revision,
                row.kind,
                row.time_ns,
                row.payload_json,
                row.record_digest,
                row.native_session_id,
                row.native_worker_id,
                row.native_turn_id,
                row.native_operation_id,
                "",
                request.lease.policy_epoch,
                admitted_ns
            ],
        )?;
        count += 1;
        inserted = true;
    }
    let next_dirty = dirty_seq
        .checked_add(i64::from(inserted))
        .ok_or_else(|| reject("dirty sequence overflow"))?;
    transaction.execute(
        "UPDATE plugin_source_cursor_v1 SET cursor=?,prior_cursor=?,last_batch_digest=?,\
         dirty_seq=?,updated_ns=? WHERE source_id=?",
        params![
            request.cursor,
            request.expected_cursor,
            request.batch_digest,
            next_dirty,
            now_ns()?,
            request.lease.source_id
        ],
    )?;
    if inserted {
        transaction.execute(
            "INSERT INTO plugin_evidence_dirty_v1(source_id,dirty_seq) VALUES(?,?) \
             ON CONFLICT(source_id) DO UPDATE SET dirty_seq=excluded.dirty_seq",
            params![request.lease.source_id, next_dirty],
        )?;
    }
    let empty_refs_digest = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
    transaction.execute(
        "INSERT INTO plugin_content_batch_v1(source_id,cursor,refs_digest) VALUES(?,?,?) \
         ON CONFLICT(source_id) DO UPDATE SET cursor=excluded.cursor,refs_digest=excluded.refs_digest",
        params![request.lease.source_id, request.cursor, empty_refs_digest],
    )?;
    transaction.execute(
        "INSERT INTO plugin_content_status_v1(source_id,cursor,reason,refs_count,policy_epoch) \
         VALUES(?,?,?,0,?) ON CONFLICT(source_id) DO UPDATE SET cursor=excluded.cursor,\
         reason=excluded.reason,refs_count=0,policy_epoch=excluded.policy_epoch",
        params![
            request.lease.source_id,
            request.cursor,
            "not_requested",
            request.lease.policy_epoch
        ],
    )?;
    finish(
        transaction,
        Receipt {
            cursor: request.cursor.clone(),
            disposition: "committed",
            dirty_seq: next_dirty,
        },
        request,
        replay,
        existing_v2_receipt,
    )
}

#[cfg(test)]
fn commit(db: &mut Connection, request: &Request) -> Result<Receipt> {
    // In-memory unit fixtures have no filesystem path; the public scratch
    // entrypoint always supplies a cooperating lease.
    commit_guarded(db, request, None, None)
}

/// Apply one host-prepared lifecycle batch to an explicitly marked disposable
/// SQLite fixture. This API is feature-off and has no production caller.
pub fn apply_json(input: &str) -> Result<String> {
    if input.len() > MAX_REQUEST_BYTES {
        return Err(reject("scratch request exceeds byte limit"));
    }
    let request: Request = serde_json::from_str(input)?;
    validate(&request)?;
    let path = Path::new(&request.sqlite_path);
    let receipt = with_marked_scratch(path, |db, lease| {
        commit_guarded(db, &request, Some(lease), None)
    })?;
    Ok(serde_json::to_string(&receipt)?)
}

fn with_marked_scratch<T>(
    path: &Path,
    apply: impl FnOnce(&mut Connection, &WriterLease) -> Result<T>,
) -> Result<T> {
    if path.as_os_str().as_encoded_bytes().len() > 4096
        || !path.is_absolute()
        || !fs::symlink_metadata(path)?.file_type().is_file()
    {
        return Err(reject("scratch fixture must be an existing regular file"));
    }
    let read = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    require_scratch(&read)?;
    drop(read);
    // Only an explicitly marked scratch fixture creates a sibling lease file.
    // The lease binds the lexical and resolved database path and stays alive
    // until the write connection has closed.
    let writer_lease = WriterLease::acquire(path)?;
    writer_lease.verify_admission()?;
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let result = apply(&mut db, &writer_lease)?;
    drop(db);
    drop(writer_lease);
    Ok(result)
}

/// Apply a strict v2 lifecycle request to an explicit disposable scratch path.
/// The path is a CLI/library argument, never accepted from the untrusted v2
/// frame. No service listener, receiver, or production Store calls this API.
pub fn apply_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    let replay = request.replay_identity()?;
    let v2::Operation::AdmitLifecycle {
        lease,
        expected_cursor,
        cursor,
        batch_digest,
        records,
    } = &request.operation;
    let prepared = Request {
        schema_version: 1,
        sqlite_path: path
            .to_str()
            .ok_or_else(|| reject("scratch path must be UTF-8"))?
            .to_owned(),
        lease: Lease {
            receiver_id: lease.receiver_id.clone(),
            plugin_id: lease.plugin_id.clone(),
            source_id: lease.source_id.clone(),
            generation: lease.generation.clone(),
            epoch: lease.epoch,
            policy_epoch: lease.policy_epoch,
        },
        expected_cursor: expected_cursor.clone(),
        cursor: cursor.clone(),
        batch_digest: batch_digest.clone(),
        records: records
            .iter()
            .map(|row| PreparedRecord {
                event_id_hash: row.event_id_hash.clone(),
                revision: row.revision,
                kind: "lifecycle".to_owned(),
                time_ns: row.time_ns,
                payload_json: row.payload_json.clone(),
                record_digest: row.record_digest.clone(),
                native_session_id: row.native_session_id.clone(),
                native_worker_id: row.native_worker_id.clone(),
                native_turn_id: row.native_turn_id.clone(),
                native_operation_id: row.native_operation_id.clone(),
            })
            .collect(),
    };
    let receipt = with_marked_scratch(path, |db, writer_lease| {
        commit_guarded(db, &prepared, Some(writer_lease), Some(&replay))
    })?;
    let disposition = match receipt.disposition {
        "committed" => v2::Disposition::Committed,
        "replay" => v2::Disposition::Replay,
        "idle" => v2::Disposition::Idle,
        _ => return Err(reject("unexpected scratch disposition")),
    };
    v2::Reply {
        version: v2::VERSION,
        request_id: request.request_id.clone(),
        receiver_id: request.receiver_id.clone(),
        request_digest: replay.digest,
        result: v2::AdmissionResult {
            disposition,
            cursor: receipt.cursor,
            batch_digest: batch_digest.clone(),
            dirty_seq: receipt.dirty_seq,
        },
    }
    .to_json(&request)
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); \
             INSERT INTO schema_meta VALUES('receiver_id','receiver-one'); \
             INSERT INTO schema_meta VALUES('storage_admission_scratch_v1','enabled'); \
             CREATE TABLE plugin_source_cursor_v1( \
               source_id TEXT PRIMARY KEY,receiver_id TEXT,plugin_id TEXT,generation TEXT, \
               lease_epoch INTEGER,policy_epoch INTEGER,capture_enabled INTEGER, \
               content_fingerprint TEXT,cursor TEXT,prior_cursor TEXT,dirty_seq INTEGER, \
               last_batch_digest TEXT,updated_ns INTEGER); \
             INSERT INTO plugin_source_cursor_v1 VALUES( \
               'source-one','receiver-one','fixture.plugin','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', \
               7,1,1,'','','',0,'',0); \
             CREATE TABLE plugin_evidence_v1( \
               source_id TEXT,event_id_hash TEXT,revision INTEGER,kind TEXT,time_ns INTEGER, \
               payload_json TEXT,record_digest TEXT,native_session_id TEXT,native_worker_id TEXT, \
               native_turn_id TEXT,native_operation_id TEXT,member_id TEXT,policy_epoch INTEGER, \
               admitted_ns INTEGER,PRIMARY KEY(source_id,event_id_hash,revision)); \
             CREATE TABLE plugin_evidence_dirty_v1(source_id TEXT PRIMARY KEY,dirty_seq INTEGER); \
             CREATE TABLE plugin_source_member_cursor_v1(source_id TEXT); \
             CREATE TABLE plugin_content_v1(source_id TEXT); \
             CREATE TABLE plugin_source_gap_invalidated_v1(source_id TEXT); \
             CREATE TABLE plugin_retention_v1(source_id TEXT); \
             CREATE TABLE plugin_retention_invalidated_v1(source_id TEXT); \
             CREATE TABLE plugin_session_member_v1(source_id TEXT); \
             CREATE TABLE plugin_content_batch_v1(source_id TEXT PRIMARY KEY,cursor TEXT,refs_digest TEXT); \
             CREATE TABLE plugin_content_status_v1( \
               source_id TEXT PRIMARY KEY,cursor TEXT,reason TEXT,refs_count INTEGER,policy_epoch INTEGER);",
        )
        .unwrap();
        db
    }

    fn request(cursor: &str, expected: &str, events: &[(&str, &str)]) -> Request {
        let records: Vec<_> = events
            .iter()
            .map(|(hash, digest)| {
                json!({"event_id_hash":hash,"revision":0,"kind":"lifecycle",
                       "time_ns":100,"payload_json":"{\"state\":\"started\"}",
                       "record_digest":digest,"native_session_id":"native-one",
                       "native_worker_id":"","native_turn_id":"",
                       "native_operation_id":""})
            })
            .collect();
        serde_json::from_value(json!({
            "schema_version":1,"sqlite_path":"/tmp/scratch-only.db",
            "lease":{"receiver_id":"receiver-one","plugin_id":"fixture.plugin",
                     "source_id":"source-one","generation":"a".repeat(64),
                     "epoch":7,"policy_epoch":1},
            "expected_cursor":expected,"cursor":cursor,
            "batch_digest":"d".repeat(64),"records":records
        }))
        .unwrap()
    }

    fn state(db: &Connection) -> (String, i64, i64) {
        let (cursor, dirty) = db
            .query_row(
                "SELECT cursor,dirty_seq FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let count = db
            .query_row("SELECT COUNT(*) FROM plugin_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        (cursor, dirty, count)
    }

    #[test]
    fn commit_replay_and_idle_preserve_atomic_state() {
        let mut db = fixture();
        let first = request("one", "", &[(&"1".repeat(64), &"2".repeat(64))]);
        assert_eq!(commit(&mut db, &first).unwrap().disposition, "committed");
        assert_eq!(state(&db), ("one".into(), 1, 1));
        assert_eq!(commit(&mut db, &first).unwrap().disposition, "replay");
        assert_eq!(state(&db), ("one".into(), 1, 1));
        assert_eq!(
            commit(&mut db, &request("one", "one", &[]))
                .unwrap()
                .disposition,
            "idle"
        );
        assert_eq!(state(&db), ("one".into(), 1, 1));
        let marker: i64 = db
            .query_row(
                "SELECT dirty_seq FROM plugin_evidence_dirty_v1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, 1);
    }

    #[test]
    fn second_row_conflict_rolls_back_first_insert() {
        let mut db = fixture();
        let hash = "1".repeat(64);
        let digest = "2".repeat(64);
        commit(&mut db, &request("one", "", &[(&hash, &digest)])).unwrap();
        let new_hash = "3".repeat(64);
        let changed = "4".repeat(64);
        let error = commit(
            &mut db,
            &request("two", "one", &[(&new_hash, &digest), (&hash, &changed)]),
        )
        .unwrap_err();
        assert!(error.to_string().contains("conflicting"));
        assert_eq!(state(&db), ("one".into(), 1, 1));
    }

    #[test]
    fn stale_lease_capture_and_cursor_are_fenced() {
        let mut db = fixture();
        let mut stale = request("one", "", &[]);
        stale.lease.epoch = 8;
        assert!(commit(&mut db, &stale).is_err());
        let valid = request("one", "", &[]);
        commit(&mut db, &valid).unwrap();
        assert!(commit(&mut db, &request("two", "old", &[])).is_err());
        db.execute("UPDATE plugin_source_cursor_v1 SET capture_enabled=0", [])
            .unwrap();
        assert!(commit(&mut db, &request("two", "one", &[])).is_err());
        assert_eq!(state(&db), ("one".into(), 0, 0));
    }

    #[test]
    fn missing_scratch_marker_and_corrections_are_rejected() {
        let mut db = fixture();
        let mut unsupported = request("one", "", &[(&"1".repeat(64), &"2".repeat(64))]);
        unsupported.records[0].revision = 1;
        assert!(commit(&mut db, &unsupported).is_err());
        assert_eq!(state(&db), ("".into(), 0, 0));
        db.execute("DELETE FROM schema_meta WHERE key=?", [SCRATCH_MARKER])
            .unwrap();
        assert!(commit(&mut db, &request("one", "", &[])).is_err());
    }

    #[test]
    fn existing_discovery_or_other_evidence_is_outside_subset() {
        let mut db = fixture();
        db.execute(
            "INSERT INTO plugin_source_member_cursor_v1(source_id) VALUES('source-one')",
            [],
        )
        .unwrap();
        let error = commit(&mut db, &request("one", "", &[])).unwrap_err();
        assert!(error.to_string().contains("unsupported history"));
        assert_eq!(state(&db), ("".into(), 0, 0));
    }

    #[test]
    fn retention_watermark_and_session_member_are_outside_subset() {
        for table in ["plugin_retention_v1", "plugin_session_member_v1"] {
            let mut db = fixture();
            db.execute(
                &format!("INSERT INTO {table}(source_id) VALUES('source-one')"),
                [],
            )
            .unwrap();
            let error = commit(&mut db, &request("one", "", &[])).unwrap_err();
            assert!(error.to_string().contains("unsupported history"));
            assert_eq!(state(&db), ("".into(), 0, 0));
        }
    }

    #[test]
    fn replay_checks_present_empty_content_receipt() {
        let mut db = fixture();
        let first = request("one", "", &[(&"1".repeat(64), &"2".repeat(64))]);
        commit(&mut db, &first).unwrap();
        db.execute("DELETE FROM plugin_content_batch_v1", [])
            .unwrap();
        assert_eq!(commit(&mut db, &first).unwrap().disposition, "replay");
        db.execute(
            "INSERT INTO plugin_content_batch_v1 VALUES('source-one','one',?)",
            [&"f".repeat(64)],
        )
        .unwrap();
        let error = commit(&mut db, &first).unwrap_err();
        assert!(error.to_string().contains("receipt mismatch"));
        assert_eq!(state(&db), ("one".into(), 1, 1));
    }

    #[test]
    fn lifecycle_payload_must_be_canonical_and_coarse() {
        assert!(lifecycle_payload("{\"state\":\"started\"}"));
        assert!(lifecycle_payload(
            "{\"reason_code\":\"source_gap\",\"state\":\"failed\"}"
        ));
        for payload in [
            "{\"state\":\"started\",\"text\":\"prompt\"}",
            "{\"state\":\"unknown\"}",
            "{\"reason_code\":\"secret text\",\"state\":\"failed\"}",
            "{\"state\":\"started\",\"state\":\"failed\"}",
            "{\"state\": \"started\"}",
        ] {
            assert!(!lifecycle_payload(payload));
        }
    }
}
