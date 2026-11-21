//! Feature-off SQLite admission for host-prepared metadata-only plugin facts.
//!
//! This crate is deliberately absent from the receiver and plugin host. Its
//! CLI requires an explicit marker on a disposable database before opening a
//! read/write connection, then holds a cooperating writer lease across its
//! transaction. It does not validate plugin wire messages or replace Python's
//! admission, retention, discovery, content, or policy code. The generic fact
//! subset is enabled only for an explicitly marked detached real-Store clone;
//! the scratch JSON adapter remains lifecycle-only.

#![forbid(unsafe_code)]

use agenturm_storage_lease::WriterLease;
use agenturm_storage_service_contract::v2;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 48;
const MAX_SOURCE_RECORDS: i64 = 50_000;
const MAX_DISCOVERY_MEMBERS: i64 = v2::MAX_DISCOVERY_MEMBERS as i64;
// Matches observatory.plugin_retention.MAX_INVALIDATED_SESSIONS.
const MAX_SESSION_MEMBER_OWNERS: i64 = 2_000;
const MAX_CURSOR_CHARS: usize = v2::MAX_CURSOR_CHARS;
const SCRATCH_MARKER: &str = "storage_admission_scratch_v1";
const CLONE_MARKER: &str = "storage_activation_clone_v1";
const V2_RECEIPT_TABLES: &[&str] = &[
    "storage_admission_request_v2",
    "storage_source_revoke_v2",
    "storage_source_activate_v2",
    "storage_source_capture_set_v2",
];
const BATCH_BINDING_TABLE: &str = "storage_admission_batch_binding_v1";

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
    value.len() <= MAX_CURSOR_CHARS && !value.chars().any(char::is_control)
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

#[derive(Clone, Debug, Deserialize)]
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
    #[serde(default)]
    member_id: String,
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
    #[serde(default)]
    member_update: Option<v2::MemberCursor>,
}

#[derive(Debug, Serialize)]
struct Receipt {
    cursor: String,
    disposition: &'static str,
    dirty_seq: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceState {
    receiver_id: String,
    plugin_id: String,
    generation: String,
    lease_epoch: i64,
    policy_epoch: i64,
    capture_enabled: i64,
    content_fingerprint: String,
    cursor: String,
    prior_cursor: String,
    dirty_seq: i64,
    last_batch_digest: String,
    updated_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BatchBinding {
    generation: String,
    policy_epoch: i64,
    expected_cursor: String,
    cursor: String,
    batch_digest: String,
    prepared_digest: String,
    dirty_seq: i64,
    prepared_version: i64,
    member_id: String,
    member_cursor: String,
}

fn prepared_rows_digest(records: &[PreparedRecord]) -> Result<String> {
    // This v1 fingerprint hashes a fixed-order tuple representation rather
    // than a JSON object. Changing its fields/order requires a v2 algorithm and
    // binding table so persisted rows retain their original interpretation.
    let rows = records
        .iter()
        .map(|row| {
            (
                row.event_id_hash.as_str(),
                row.revision,
                row.kind.as_str(),
                row.time_ns,
                row.payload_json.as_str(),
                row.record_digest.as_str(),
                row.native_session_id.as_str(),
                row.native_worker_id.as_str(),
                row.native_turn_id.as_str(),
                row.native_operation_id.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&rows)?;
    let mut digest = Sha256::new();
    digest.update(b"agenturm-prepared-rows-v1\0");
    digest.update(encoded);
    Ok(format!("{:x}", digest.finalize()))
}

fn prepared_state_digest(
    records: &[PreparedRecord],
    member_update: Option<&v2::MemberCursor>,
) -> Result<(i64, String)> {
    let Some(member_update) = member_update else {
        return Ok((1, prepared_rows_digest(records)?));
    };
    let rows = records
        .iter()
        .map(|row| {
            (
                row.event_id_hash.as_str(),
                row.revision,
                row.kind.as_str(),
                row.time_ns,
                row.payload_json.as_str(),
                row.record_digest.as_str(),
                row.native_session_id.as_str(),
                row.native_worker_id.as_str(),
                row.native_turn_id.as_str(),
                row.native_operation_id.as_str(),
                row.member_id.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&(2_u8, member_update, rows))?;
    let mut digest = Sha256::new();
    digest.update(b"agenturm-prepared-member-state-v2\0");
    digest.update(encoded);
    Ok((2, format!("{:x}", digest.finalize())))
}

fn batch_binding_columns(db: &Connection) -> Result<Vec<(String, String)>> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' \
         AND name='storage_admission_batch_binding_v1')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = db.prepare("PRAGMA table_info(storage_admission_batch_binding_v1)")?;
    let rows = statement.query_map([], |row| Ok((row.get(1)?, row.get(2)?)))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn ensure_batch_binding_schema(db: &Connection) -> Result<()> {
    const LEGACY: &[(&str, &str)] = &[
        ("source_id", "TEXT"),
        ("generation", "TEXT"),
        ("policy_epoch", "INTEGER"),
        ("expected_cursor", "TEXT"),
        ("cursor", "TEXT"),
        ("batch_digest", "TEXT"),
        ("prepared_digest", "TEXT"),
        ("dirty_seq", "INTEGER"),
    ];
    const CURRENT: &[(&str, &str)] = &[
        ("source_id", "TEXT"),
        ("generation", "TEXT"),
        ("policy_epoch", "INTEGER"),
        ("expected_cursor", "TEXT"),
        ("cursor", "TEXT"),
        ("batch_digest", "TEXT"),
        ("prepared_digest", "TEXT"),
        ("dirty_seq", "INTEGER"),
        ("prepared_version", "INTEGER"),
        ("member_id", "TEXT"),
        ("member_cursor", "TEXT"),
    ];
    let columns = batch_binding_columns(db)?;
    let matches = |expected: &[(&str, &str)]| {
        columns.len() == expected.len()
            && columns
                .iter()
                .zip(expected)
                .all(|((name, kind), (expected_name, expected_kind))| {
                    name == expected_name && kind.eq_ignore_ascii_case(expected_kind)
                })
    };
    if columns.is_empty() {
        db.execute_batch(
            "CREATE TABLE storage_admission_batch_binding_v1(\
             source_id TEXT PRIMARY KEY,generation TEXT NOT NULL,policy_epoch INTEGER NOT NULL,\
             expected_cursor TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
             prepared_digest TEXT NOT NULL,dirty_seq INTEGER NOT NULL,\
             prepared_version INTEGER NOT NULL DEFAULT 1,member_id TEXT NOT NULL DEFAULT '',\
             member_cursor TEXT NOT NULL DEFAULT '')",
        )?;
        return Ok(());
    }
    if matches(LEGACY) {
        db.execute_batch(
            "ALTER TABLE storage_admission_batch_binding_v1 ADD COLUMN \
             prepared_version INTEGER NOT NULL DEFAULT 1;\
             ALTER TABLE storage_admission_batch_binding_v1 ADD COLUMN \
             member_id TEXT NOT NULL DEFAULT '';\
             ALTER TABLE storage_admission_batch_binding_v1 ADD COLUMN \
             member_cursor TEXT NOT NULL DEFAULT '';",
        )?;
        return Ok(());
    }
    if matches(CURRENT) {
        return Ok(());
    }
    Err(reject("unsupported prepared batch binding schema"))
}

fn read_batch_binding(db: &Connection, source_id: &str) -> Result<Option<BatchBinding>> {
    let columns = batch_binding_columns(db)?;
    const LEGACY: &[(&str, &str)] = &[
        ("source_id", "TEXT"),
        ("generation", "TEXT"),
        ("policy_epoch", "INTEGER"),
        ("expected_cursor", "TEXT"),
        ("cursor", "TEXT"),
        ("batch_digest", "TEXT"),
        ("prepared_digest", "TEXT"),
        ("dirty_seq", "INTEGER"),
    ];
    const CURRENT: &[(&str, &str)] = &[
        ("source_id", "TEXT"),
        ("generation", "TEXT"),
        ("policy_epoch", "INTEGER"),
        ("expected_cursor", "TEXT"),
        ("cursor", "TEXT"),
        ("batch_digest", "TEXT"),
        ("prepared_digest", "TEXT"),
        ("dirty_seq", "INTEGER"),
        ("prepared_version", "INTEGER"),
        ("member_id", "TEXT"),
        ("member_cursor", "TEXT"),
    ];
    let matches = |expected: &[(&str, &str)]| {
        columns.len() == expected.len()
            && columns
                .iter()
                .zip(expected)
                .all(|((name, kind), (expected_name, expected_kind))| {
                    name == expected_name && kind.eq_ignore_ascii_case(expected_kind)
                })
    };
    if columns.is_empty() {
        return Ok(None);
    }
    if matches(CURRENT) {
        return Ok(db
            .query_row(
                "SELECT generation,policy_epoch,expected_cursor,cursor,batch_digest,\
                 prepared_digest,dirty_seq,prepared_version,member_id,member_cursor \
                 FROM storage_admission_batch_binding_v1 WHERE source_id=?",
                [source_id],
                |row| {
                    Ok(BatchBinding {
                        generation: row.get(0)?,
                        policy_epoch: row.get(1)?,
                        expected_cursor: row.get(2)?,
                        cursor: row.get(3)?,
                        batch_digest: row.get(4)?,
                        prepared_digest: row.get(5)?,
                        dirty_seq: row.get(6)?,
                        prepared_version: row.get(7)?,
                        member_id: row.get(8)?,
                        member_cursor: row.get(9)?,
                    })
                },
            )
            .optional()?);
    }
    if matches(LEGACY) {
        return Ok(db
            .query_row(
                "SELECT generation,policy_epoch,expected_cursor,cursor,batch_digest,\
                 prepared_digest,dirty_seq FROM storage_admission_batch_binding_v1 \
                 WHERE source_id=?",
                [source_id],
                |row| {
                    Ok(BatchBinding {
                        generation: row.get(0)?,
                        policy_epoch: row.get(1)?,
                        expected_cursor: row.get(2)?,
                        cursor: row.get(3)?,
                        batch_digest: row.get(4)?,
                        prepared_digest: row.get(5)?,
                        dirty_seq: row.get(6)?,
                        prepared_version: 1,
                        member_id: String::new(),
                        member_cursor: String::new(),
                    })
                },
            )
            .optional()?);
    }
    Err(reject("unsupported prepared batch binding schema"))
}

fn binding_for_request(
    request: &Request,
    dirty_seq: i64,
    prepared_version: i64,
    prepared_digest: String,
) -> BatchBinding {
    BatchBinding {
        generation: request.lease.generation.clone(),
        policy_epoch: request.lease.policy_epoch,
        expected_cursor: request.expected_cursor.clone(),
        cursor: request.cursor.clone(),
        batch_digest: request.batch_digest.clone(),
        prepared_digest,
        dirty_seq,
        prepared_version,
        member_id: request
            .member_update
            .as_ref()
            .map_or_else(String::new, |member| member.member_id.clone()),
        member_cursor: request
            .member_update
            .as_ref()
            .map_or_else(String::new, |member| member.cursor.clone()),
    }
}

fn source_state(db: &Connection, source_id: &str) -> Result<SourceState> {
    Ok(db.query_row(
        "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
         content_fingerprint,cursor,prior_cursor,dirty_seq,last_batch_digest,updated_ns \
         FROM plugin_source_cursor_v1 WHERE source_id=?",
        [source_id],
        |row| {
            Ok(SourceState {
                receiver_id: row.get(0)?,
                plugin_id: row.get(1)?,
                generation: row.get(2)?,
                lease_epoch: row.get(3)?,
                policy_epoch: row.get(4)?,
                capture_enabled: row.get(5)?,
                content_fingerprint: row.get(6)?,
                cursor: row.get(7)?,
                prior_cursor: row.get(8)?,
                dirty_seq: row.get(9)?,
                last_batch_digest: row.get(10)?,
                updated_ns: row.get(11)?,
            })
        },
    )?)
}

#[derive(Clone, Debug)]
struct AdmissionSnapshot {
    evidence_count: i64,
    dirty_marker_before: Option<i64>,
    batch_binding_before: Option<BatchBinding>,
    member_cursors_before: Vec<(String, String)>,
    session_members_before: Vec<(String, String)>,
    tables: Vec<(&'static str, &'static str, &'static str, bool)>,
}

type StoredFactRow = (
    String,
    String,
    i64,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
);

struct AdmissionReadback<'a> {
    snapshot: &'a AdmissionSnapshot,
    expected_source: &'a SourceState,
    inserted_hashes: &'a HashSet<String>,
    admitted_ns: Option<i64>,
    effective_records: &'a [PreparedRecord],
    member_cursors_after: &'a [(String, String)],
    session_members_after: &'a [(String, String)],
}

struct MemberAdmissionPlan {
    effective_records: Vec<PreparedRecord>,
    session_member_inserts: Vec<(String, String)>,
    member_cursors_after: Vec<(String, String)>,
    session_members_after: Vec<(String, String)>,
}

impl AdmissionSnapshot {
    fn capture(db: &Connection, source_id: &str) -> Result<Self> {
        let dirty_marker_before = db
            .query_row(
                "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
                [source_id],
                |row| row.get(0),
            )
            .optional()?;
        let specs: [(&str, &str, &str, bool); 9] = [
            (
                "_agenturm_fact_before_evidence",
                "plugin_evidence_v1",
                "source_id=?",
                false,
            ),
            (
                "_agenturm_fact_before_session_projection",
                "plugin_session_projection_v1",
                "source_id=?",
                true,
            ),
            (
                "_agenturm_fact_before_projection_cursor",
                "plugin_projection_cursor_v1",
                "source_id=?",
                true,
            ),
            (
                "_agenturm_fact_before_activity_index",
                "activity_index_v1",
                "kind='plugin' AND owner_key=?",
                true,
            ),
            (
                "_agenturm_fact_before_investigation_dirty",
                "investigation_dirty_v1",
                "kind='plugin' AND owner_key=?",
                true,
            ),
            (
                "_agenturm_fact_before_content_batch",
                "plugin_content_batch_v1",
                "source_id=?",
                false,
            ),
            (
                "_agenturm_fact_before_content_status",
                "plugin_content_status_v1",
                "source_id=?",
                false,
            ),
            (
                "_agenturm_fact_before_member_cursor",
                "plugin_source_member_cursor_v1",
                "source_id=?",
                false,
            ),
            (
                "_agenturm_fact_before_session_member",
                "plugin_session_member_v1",
                "source_id=?",
                false,
            ),
        ];
        let mut tables = Vec::new();
        for (snapshot, table, filter, verify_unchanged) in specs {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
                [table],
                |row| row.get(0),
            )?;
            if !exists {
                continue;
            }
            db.execute_batch(&format!("DROP TABLE IF EXISTS temp.{snapshot}"))?;
            db.execute(
                &format!("CREATE TEMP TABLE {snapshot} AS SELECT * FROM {table} WHERE {filter}"),
                [source_id],
            )?;
            tables.push((snapshot, table, filter, verify_unchanged));
        }
        let evidence_count = db.query_row(
            "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
            [source_id],
            |row| row.get(0),
        )?;
        let batch_binding_before = read_batch_binding(db, source_id)?;
        let member_cursors_before = read_member_cursor_rows(db, source_id)?;
        let session_members_before = read_session_member_rows(db, source_id)?;
        Ok(Self {
            evidence_count,
            dirty_marker_before,
            batch_binding_before,
            member_cursors_before,
            session_members_before,
            tables,
        })
    }

    fn verify_unchanged_table(
        db: &Connection,
        snapshot: &str,
        table: &str,
        filter: &str,
        source_id: &str,
    ) -> Result<()> {
        let differs: bool = db.query_row(
            &format!(
                "SELECT EXISTS(SELECT * FROM temp.{snapshot} EXCEPT \
                 SELECT * FROM {table} WHERE {filter}) OR \
                 EXISTS(SELECT * FROM {table} WHERE {filter} EXCEPT \
                 SELECT * FROM temp.{snapshot})"
            ),
            params![source_id, source_id],
            |row| row.get(0),
        )?;
        if differs {
            return Err(format!("admission trigger changed protected {table} rows").into());
        }
        Ok(())
    }

    fn verify_snapshot_rows_preserved(
        db: &Connection,
        snapshot: &str,
        table: &str,
        filter: &str,
        source_id: &str,
    ) -> Result<()> {
        let missing_or_changed: bool = db.query_row(
            &format!(
                "SELECT EXISTS(SELECT * FROM temp.{snapshot} EXCEPT \
                 SELECT * FROM {table} WHERE {filter})"
            ),
            [source_id],
            |row| row.get(0),
        )?;
        if missing_or_changed {
            return Err(format!("admission trigger changed existing {table} rows").into());
        }
        Ok(())
    }

    fn verify_preserved(&self, db: &Connection, source_id: &str) -> Result<()> {
        for (snapshot, table, filter, verify_unchanged) in &self.tables {
            if *verify_unchanged {
                Self::verify_unchanged_table(db, snapshot, table, filter, source_id)?;
            }
        }
        Self::verify_snapshot_rows_preserved(
            db,
            "_agenturm_fact_before_evidence",
            "plugin_evidence_v1",
            "source_id=?",
            source_id,
        )?;
        Ok(())
    }

    fn verify_exact_evidence_count(
        &self,
        db: &Connection,
        source_id: &str,
        inserted: i64,
    ) -> Result<()> {
        let actual: i64 = db.query_row(
            "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id=?",
            [source_id],
            |row| row.get(0),
        )?;
        if actual != self.evidence_count + inserted {
            return Err(reject(
                "admission changed an unexpected number of evidence rows",
            ));
        }
        Ok(())
    }
}

fn read_member_cursor_rows(db: &Connection, source_id: &str) -> Result<Vec<(String, String)>> {
    let mut statement = db.prepare(
        "SELECT member_id,cursor FROM plugin_source_member_cursor_v1 \
         WHERE source_id=? ORDER BY member_id LIMIT ?",
    )?;
    let rows = statement.query_map(params![source_id, MAX_DISCOVERY_MEMBERS + 1], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    let values = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    if values.len() > MAX_DISCOVERY_MEMBERS as usize {
        return Err(reject("source member cursor capacity exceeded"));
    }
    Ok(values)
}

fn read_session_member_rows(db: &Connection, source_id: &str) -> Result<Vec<(String, String)>> {
    let mut statement = db.prepare(
        "SELECT native_session_id,member_id FROM plugin_session_member_v1 \
         WHERE source_id=? ORDER BY native_session_id LIMIT ?",
    )?;
    let rows = statement.query_map(params![source_id, MAX_SESSION_MEMBER_OWNERS + 1], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    let values = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    if values.len() as i64 > MAX_SESSION_MEMBER_OWNERS {
        return Err(reject("source session-member capacity exceeded"));
    }
    Ok(values)
}

fn plan_member_admission(
    db: &Connection,
    request: &Request,
    snapshot: &AdmissionSnapshot,
) -> Result<MemberAdmissionPlan> {
    let mut effective_records = request.records.clone();
    if request.member_update.is_none() {
        if effective_records
            .iter()
            .any(|row| !row.member_id.is_empty())
        {
            return Err(reject("member evidence requires a member cursor update"));
        }
        let owners: BTreeMap<&str, &str> = snapshot
            .session_members_before
            .iter()
            .map(|(session, member)| (session.as_str(), member.as_str()))
            .collect();
        for row in &effective_records {
            if owners
                .get(row.native_session_id.as_str())
                .is_some_and(|owner| !owner.is_empty())
            {
                return Err(reject("plugin session changed discovery member"));
            }
        }
        return Ok(MemberAdmissionPlan {
            effective_records,
            session_member_inserts: Vec::new(),
            member_cursors_after: snapshot.member_cursors_before.clone(),
            session_members_after: snapshot.session_members_before.clone(),
        });
    }

    let update = request
        .member_update
        .as_ref()
        .expect("member update checked");
    let mut member_cursors: BTreeMap<String, String> =
        snapshot.member_cursors_before.iter().cloned().collect();
    if !member_cursors.contains_key(&update.member_id)
        && member_cursors.len() >= MAX_DISCOVERY_MEMBERS as usize
    {
        return Err(reject("discovery member limit exceeded"));
    }
    member_cursors.insert(update.member_id.clone(), update.cursor.clone());

    let mut session_members: BTreeMap<String, String> =
        snapshot.session_members_before.iter().cloned().collect();
    let mut session_member_inserts = Vec::new();
    let incoming_sessions: BTreeSet<&str> = effective_records
        .iter()
        .filter_map(|row| {
            (!row.native_session_id.is_empty()).then_some(row.native_session_id.as_str())
        })
        .collect();
    let mut ambiguous_sessions = HashSet::new();
    for session in incoming_sessions {
        if let Some(owner) = session_members.get(session) {
            if owner.is_empty() {
                ambiguous_sessions.insert(session.to_owned());
            } else if owner != &update.member_id {
                return Err(reject("plugin session changed discovery member"));
            }
            continue;
        }
        let legacy: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1 \
             AND native_session_id=?2)",
            params![request.lease.source_id, session],
            |row| row.get(0),
        )?;
        if session_members.len() >= MAX_SESSION_MEMBER_OWNERS as usize {
            return Err(reject("plugin session member capacity reached"));
        }
        let owner = if legacy {
            ambiguous_sessions.insert(session.to_owned());
            String::new()
        } else {
            update.member_id.clone()
        };
        session_members.insert(session.to_owned(), owner.clone());
        session_member_inserts.push((session.to_owned(), owner));
    }
    for row in &mut effective_records {
        row.member_id = if ambiguous_sessions.contains(&row.native_session_id) {
            String::new()
        } else {
            update.member_id.clone()
        };
    }
    Ok(MemberAdmissionPlan {
        effective_records,
        session_member_inserts,
        member_cursors_after: member_cursors.into_iter().collect(),
        session_members_after: session_members.into_iter().collect(),
    })
}

fn verify_fact_admission_readback(
    db: &Connection,
    request: &Request,
    receipt: &Receipt,
    readback: &AdmissionReadback<'_>,
) -> Result<()> {
    let actual_source = source_state(db, &request.lease.source_id)?;
    if &actual_source != readback.expected_source {
        return Err(reject("admission source state was not persisted exactly"));
    }

    readback
        .snapshot
        .verify_preserved(db, &request.lease.source_id)?;
    readback.snapshot.verify_exact_evidence_count(
        db,
        &request.lease.source_id,
        readback.inserted_hashes.len() as i64,
    )?;
    let actual_binding = read_batch_binding(db, &request.lease.source_id)?;
    if receipt.disposition == "committed" {
        let (prepared_version, prepared_digest) =
            prepared_state_digest(readback.effective_records, request.member_update.as_ref())?;
        let expected_binding = binding_for_request(
            request,
            receipt.dirty_seq,
            prepared_version,
            prepared_digest,
        );
        if actual_binding.as_ref() != Some(&expected_binding) {
            return Err(reject("prepared batch binding was not persisted exactly"));
        }
    } else if actual_binding != readback.snapshot.batch_binding_before {
        return Err(reject("non-commit changed the prepared batch binding"));
    }
    for (row, effective) in request.records.iter().zip(readback.effective_records) {
        let stored: Option<StoredFactRow> = db
            .query_row(
                "SELECT source_id,event_id_hash,revision,kind,time_ns,payload_json,record_digest,\
                 native_session_id,native_worker_id,native_turn_id,native_operation_id,\
                 member_id,policy_epoch,admitted_ns FROM plugin_evidence_v1 \
                 WHERE source_id=? AND event_id_hash=? AND revision=?",
                params![request.lease.source_id, row.event_id_hash, row.revision],
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
                        found.get(8)?,
                        found.get(9)?,
                        found.get(10)?,
                        found.get(11)?,
                        found.get(12)?,
                        found.get(13)?,
                    ))
                },
            )
            .optional()?;
        let Some(stored) = stored else {
            return Err(reject("admission evidence row was ignored or removed"));
        };
        if stored.0 != request.lease.source_id
            || stored.1 != row.event_id_hash
            || stored.2 != row.revision
            || stored.3 != row.kind
            || stored.4 != row.time_ns
            || stored.5 != row.payload_json
            || stored.6 != row.record_digest
            || stored.7 != row.native_session_id
            || stored.8 != row.native_worker_id
            || stored.9 != row.native_turn_id
            || stored.10 != row.native_operation_id
            || stored.11 != effective.member_id
            || stored.12 != request.lease.policy_epoch
            || (readback.inserted_hashes.contains(&row.event_id_hash)
                && Some(stored.13) != readback.admitted_ns)
        {
            return Err(reject("admission evidence row was changed by a trigger"));
        }
    }

    if read_member_cursor_rows(db, &request.lease.source_id)? != readback.member_cursors_after
        || read_session_member_rows(db, &request.lease.source_id)? != readback.session_members_after
    {
        return Err(reject(
            "member cursor or session ownership readback mismatch",
        ));
    }

    let expected_dirty_marker = if readback.inserted_hashes.is_empty() {
        readback.snapshot.dirty_marker_before
    } else {
        Some(receipt.dirty_seq)
    };
    let actual_dirty_marker: Option<i64> = db
        .query_row(
            "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
            [&request.lease.source_id],
            |row| row.get(0),
        )
        .optional()?;
    if actual_dirty_marker != expected_dirty_marker {
        return Err(reject("admission dirty marker was not persisted exactly"));
    }

    if receipt.disposition == "committed" {
        let empty_refs_digest = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
        let content_batch: Option<(String, String)> = db
            .query_row(
                "SELECT cursor,refs_digest FROM plugin_content_batch_v1 WHERE source_id=?",
                [&request.lease.source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let content_status: Option<(String, String, i64, i64)> = db
            .query_row(
                "SELECT cursor,reason,refs_count,policy_epoch \
                 FROM plugin_content_status_v1 WHERE source_id=?",
                [&request.lease.source_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if content_batch != Some((request.cursor.clone(), empty_refs_digest))
            || content_status
                != Some((
                    request.cursor.clone(),
                    "not_requested".to_owned(),
                    0,
                    request.lease.policy_epoch,
                ))
        {
            return Err(reject(
                "admission content receipts were not persisted exactly",
            ));
        }
    } else {
        for (snapshot_name, table, filter, _) in &readback.snapshot.tables {
            if *table == "plugin_content_batch_v1" || *table == "plugin_content_status_v1" {
                AdmissionSnapshot::verify_unchanged_table(
                    db,
                    snapshot_name,
                    table,
                    filter,
                    &request.lease.source_id,
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct SourceRevocationReceipt {
    cursor: String,
    disposition: &'static str,
    dirty_seq: i64,
    lease_epoch: i64,
    policy_epoch: i64,
    capture_enabled: bool,
}

#[derive(Debug)]
struct SourceActivationReceipt {
    lease: v2::SourceLease,
    cursor: String,
    dirty_seq: i64,
    disposition: &'static str,
}

#[derive(Debug)]
struct SourceCaptureSetReceipt {
    lease: v2::SourceLease,
    enabled: bool,
    cursor: String,
    dirty_seq: i64,
    disposition: &'static str,
}

type SourceMarkerState = (Option<i64>, Option<(i64, i64)>);

fn finish(
    transaction: Transaction<'_>,
    receipt: Receipt,
    request: &Request,
    replay: Option<&v2::ReplayIdentity>,
    existing_v2_receipt: bool,
    real_clone: bool,
    readback: Option<AdmissionReadback<'_>>,
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
    if real_clone {
        verify_real_clone_lifecycle_commit(
            &transaction,
            request,
            &receipt,
            replay,
            existing_v2_receipt,
        )?;
        let readback = readback.ok_or_else(|| reject("real clone admission snapshot missing"))?;
        verify_fact_admission_readback(&transaction, request, &receipt, &readback)?;
    }
    transaction.commit()?;
    Ok(receipt)
}

fn reject_clone_write_triggers(
    db: &Connection,
    targets: &[&str],
    error: &'static str,
) -> Result<()> {
    // A trigger on a write target can mutate another source or receipt table.
    // Per-source readback cannot bound those effects, so reject before writes.
    let placeholders = vec!["?"; targets.len()].join(",");
    for schema in ["sqlite_master", "sqlite_temp_master"] {
        let trigger: Option<String> = db
            .query_row(
                &format!(
                    "SELECT name FROM {schema} WHERE type='trigger' AND tbl_name IN ({placeholders}) LIMIT 1"
                ),
                rusqlite::params_from_iter(targets.iter().copied()),
                |row| row.get(0),
            )
            .optional()?;
        if trigger.is_some() {
            return Err(reject(error));
        }
    }
    Ok(())
}

fn reject_real_clone_admission_triggers(db: &Connection) -> Result<()> {
    reject_clone_write_triggers(
        db,
        &[
            "plugin_evidence_v1",
            "plugin_source_cursor_v1",
            "plugin_evidence_dirty_v1",
            "plugin_session_member_v1",
            "plugin_source_member_cursor_v1",
            "plugin_content_batch_v1",
            "plugin_content_status_v1",
            "storage_admission_request_v2",
            BATCH_BINDING_TABLE,
        ],
        "SQLite triggers on real clone admission write tables are unsupported",
    )
}

fn reject_real_clone_activation_triggers(db: &Connection) -> Result<()> {
    reject_clone_write_triggers(
        db,
        &["plugin_source_cursor_v1", "storage_source_activate_v2"],
        "SQLite triggers on real clone activation write tables are unsupported",
    )
}

fn normalized_trigger_sql(sql: &str) -> String {
    let mut normalized = String::with_capacity(sql.len());
    let mut quote = None;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            normalized.push(ch);
            if ch == delimiter {
                if chars.peek() == Some(&delimiter) {
                    normalized.push(chars.next().expect("peeked SQL quote"));
                } else {
                    quote = None;
                }
            }
        } else if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
            normalized.push(ch);
        } else if !ch.is_whitespace() {
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    normalized
}

fn verify_exact_trigger_set(db: &Connection, table: &str, expected: &[(&str, &str)]) -> Result<()> {
    let mut actual = Vec::<(String, String)>::new();
    for schema in ["sqlite_master", "sqlite_temp_master"] {
        let query = format!(
            "SELECT name,sql FROM {schema} WHERE type='trigger' AND tbl_name=? ORDER BY name"
        );
        let mut statement = db.prepare(&query)?;
        let rows = statement.query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            actual.push(row?);
        }
    }
    actual.sort_by(|left, right| left.0.cmp(&right.0));
    let mut expected = expected
        .iter()
        .map(|(name, sql)| ((*name).to_owned(), normalized_trigger_sql(sql)))
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    if actual.len() != expected.len()
        || actual.iter().zip(&expected).any(|(actual, expected)| {
            actual.0 != expected.0 || normalized_trigger_sql(&actual.1) != expected.1
        })
    {
        return Err(reject(
            "real clone policy encountered unsupported SQLite triggers",
        ));
    }
    Ok(())
}

fn reject_real_clone_policy_triggers(db: &Connection) -> Result<()> {
    const ACTIVITY_TRIGGERS: &[(&str, &str)] = &[
        (
            "investigation_activity_delete_v1",
            "CREATE TRIGGER investigation_activity_delete_v1 AFTER DELETE ON activity_index_v1 BEGIN DELETE FROM investigation_terms_v1 WHERE event_key=OLD.event_key; END",
        ),
        (
            "investigation_dirty_delete_v1",
            "CREATE TRIGGER investigation_dirty_delete_v1 AFTER DELETE ON activity_index_v1 BEGIN INSERT OR IGNORE INTO investigation_dirty_v1 VALUES(OLD.kind,OLD.owner_key); END",
        ),
        (
            "investigation_dirty_insert_v1",
            "CREATE TRIGGER investigation_dirty_insert_v1 AFTER INSERT ON activity_index_v1 BEGIN INSERT OR IGNORE INTO investigation_dirty_v1 VALUES(NEW.kind,NEW.owner_key); END",
        ),
        (
            "investigation_dirty_update_v1",
            "CREATE TRIGGER investigation_dirty_update_v1 AFTER UPDATE ON activity_index_v1 BEGIN INSERT OR IGNORE INTO investigation_dirty_v1 VALUES(NEW.kind,NEW.owner_key);INSERT OR IGNORE INTO investigation_dirty_v1 VALUES(OLD.kind,OLD.owner_key); END",
        ),
        (
            "native_activity_delete_v1",
            "CREATE TRIGGER native_activity_delete_v1 AFTER DELETE ON activity_index_v1 BEGIN INSERT OR IGNORE INTO native_lifecycle_dirty_v1 SELECT OLD.kind,OLD.owner_key WHERE json_type(OLD.payload,'$.native_lifecycle')='object';DELETE FROM native_lifecycle_index_v1 WHERE event_key=OLD.event_key; END",
        ),
        (
            "native_activity_insert_v1",
            "CREATE TRIGGER native_activity_insert_v1 AFTER INSERT ON activity_index_v1 BEGIN INSERT OR IGNORE INTO native_lifecycle_dirty_v1 SELECT NEW.kind,NEW.owner_key WHERE json_type(NEW.payload,'$.native_lifecycle')='object'; END",
        ),
        (
            "native_activity_update_v1",
            "CREATE TRIGGER native_activity_update_v1 AFTER UPDATE ON activity_index_v1 BEGIN INSERT OR IGNORE INTO native_lifecycle_dirty_v1 SELECT NEW.kind,NEW.owner_key WHERE json_type(NEW.payload,'$.native_lifecycle')='object';INSERT OR IGNORE INTO native_lifecycle_dirty_v1 SELECT OLD.kind,OLD.owner_key WHERE json_type(OLD.payload,'$.native_lifecycle')='object'; END",
        ),
    ];
    const INVESTIGATION_TERM_TRIGGERS: &[(&str, &str)] = &[
        (
            "investigation_delete_v1",
            "CREATE TRIGGER investigation_delete_v1 AFTER DELETE ON investigation_terms_v1 BEGIN INSERT INTO investigation_fts_v1(investigation_fts_v1,rowid,text) VALUES('delete',OLD.term_id,OLD.text); END",
        ),
        (
            "investigation_insert_v1",
            "CREATE TRIGGER investigation_insert_v1 AFTER INSERT ON investigation_terms_v1 BEGIN INSERT INTO investigation_fts_v1(rowid,text) VALUES(NEW.term_id,NEW.text); END",
        ),
    ];
    reject_clone_write_triggers(
        db,
        &[
            "plugin_evidence_v1",
            "plugin_source_cursor_v1",
            "plugin_evidence_dirty_v1",
            "plugin_session_projection_v1",
            "plugin_projection_cursor_v1",
            "plugin_session_member_v1",
            "plugin_source_member_cursor_v1",
            "plugin_content_v1",
            "plugin_content_batch_v1",
            "plugin_content_status_v1",
            "plugin_reference_owner_v1",
            "plugin_retention_v1",
            "plugin_retention_invalidated_v1",
            "plugin_source_gap_invalidated_v1",
            "investigation_dirty_v1",
            "native_lifecycle_dirty_v1",
            "native_lifecycle_index_v1",
            "investigation_fts_v1",
            "storage_source_revoke_v2",
            "storage_source_capture_set_v2",
            BATCH_BINDING_TABLE,
        ],
        "SQLite triggers on real clone policy write tables are unsupported",
    )?;
    verify_exact_trigger_set(db, "activity_index_v1", ACTIVITY_TRIGGERS)?;
    verify_exact_trigger_set(db, "investigation_terms_v1", INVESTIGATION_TERM_TRIGGERS)?;
    Ok(())
}

fn preflight_clone_policy_deletes(db: &Connection, source_id: &str) -> Result<()> {
    for table in ["plugin_command_content_v1", "plugin_command_batch_v1"] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            [table],
            |row| row.get(0),
        )?;
        if exists {
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE source_id=?)");
            if db.query_row(&query, [source_id], |row| row.get::<_, bool>(0))? {
                return Err(reject(
                    "real clone source has unsupported provider reference history",
                ));
            }
        }
    }
    let overlap: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM activity_index_v1 a \
         JOIN native_lifecycle_index_v1 n ON n.event_key=a.event_key \
         WHERE a.kind='plugin' AND a.owner_key=?)",
        [source_id],
        |row| row.get(0),
    )?;
    if overlap {
        return Err(reject(
            "real clone activity overlaps native lifecycle index ownership",
        ));
    }
    let mut statement =
        db.prepare("SELECT payload FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?")?;
    let payloads = statement.query_map([source_id], |row| row.get::<_, String>(0))?;
    for payload in payloads {
        let payload: serde_json::Value = serde_json::from_str(&payload?)
            .map_err(|_| reject("real clone activity payload is malformed"))?;
        if payload.get("native_lifecycle").is_some() {
            return Err(reject("real clone activity has native lifecycle metadata"));
        }
    }
    let native_dirty: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_lifecycle_dirty_v1 WHERE kind='plugin' AND owner_key=?)",
        [source_id],
        |row| row.get(0),
    )?;
    if native_dirty {
        return Err(reject(
            "real clone source has unsupported native lifecycle dirty state",
        ));
    }
    Ok(())
}

fn validate_clone_policy_replay_source(
    db: &Connection,
    source_id: &str,
    receiver: &str,
    plugin_id: &str,
    generation: &str,
    receipt_policy_epoch: i64,
    receipt_dirty_seq: i64,
) -> Result<()> {
    let current: Option<(String, String, String, i64, i64)> = db
        .query_row(
            "SELECT receiver_id,plugin_id,generation,policy_epoch,dirty_seq \
             FROM plugin_source_cursor_v1 WHERE source_id=?",
            [source_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((current_receiver, current_plugin, current_generation, policy, dirty_seq)) = current
    else {
        return Err(reject("clone policy replay has no persisted source"));
    };
    if current_receiver != receiver
        || current_plugin != plugin_id
        || current_generation != generation
        || policy < receipt_policy_epoch
        || dirty_seq < receipt_dirty_seq
    {
        return Err(reject(
            "clone policy replay is not fenced by the current source state",
        ));
    }
    Ok(())
}

fn verify_real_clone_lifecycle_commit(
    db: &Connection,
    request: &Request,
    receipt: &Receipt,
    replay: Option<&v2::ReplayIdentity>,
    existing_v2_receipt: bool,
) -> Result<()> {
    require_activation_clone_markers(db)?;
    let replay = replay.ok_or_else(|| reject("real clone admission requires replay identity"))?;
    let receiver: String = db.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if receiver != request.lease.receiver_id {
        return Err(reject(
            "real clone receiver identity changed during admission",
        ));
    }
    type Source = (
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        i64,
        String,
        String,
    );
    let source: Source = db.query_row(
        "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,\
         capture_enabled,content_fingerprint,cursor,dirty_seq,prior_cursor,last_batch_digest \
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
    if source.0 != receiver
        || source.1 != request.lease.plugin_id
        || source.2 != request.lease.generation
        || source.3 != request.lease.epoch
        || source.4 != request.lease.policy_epoch
        || source.5 != 1
        || !source.6.is_empty()
        || source.7 != receipt.cursor
        || source.8 != receipt.dirty_seq
        || (receipt.disposition == "committed"
            && (source.9 != request.expected_cursor || source.10 != request.batch_digest))
    {
        return Err(reject(
            "real clone source changed during lifecycle admission",
        ));
    }
    reject_cross_operation_request_id(db, &replay.request_id, "storage_admission_request_v2")?;
    type Stored = (String, String, String, String, String, i64, String);
    let stored: Stored = db.query_row(
        "SELECT receiver_id,request_digest,source_id,cursor,batch_digest,dirty_seq,disposition \
         FROM storage_admission_request_v2 WHERE request_id=?",
        [&replay.request_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        },
    )?;
    if stored.0 != replay.receiver_id
        || stored.1 != replay.digest
        || stored.2 != request.lease.source_id
        || stored.3 != receipt.cursor
        || stored.4 != request.batch_digest
        || stored.5 != receipt.dirty_seq
        || (if existing_v2_receipt {
            !matches!(stored.6.as_str(), "committed" | "replay" | "idle")
        } else {
            stored.6 != receipt.disposition
        })
    {
        return Err(reject(
            "real clone admission receipt changed during transaction",
        ));
    }
    Ok(())
}

fn validate(request: &Request, allow_plugin_facts: bool) -> Result<()> {
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
        || (request.cursor.is_empty() && !request.records.is_empty())
    {
        return Err(reject("invalid prepared admission request"));
    }
    if request.member_update.as_ref().is_some_and(|member| {
        !allow_plugin_facts
            || !v2::member_id_is_valid(&member.member_id)
            || !v2::member_cursor_is_valid(&member.cursor)
    }) {
        return Err(reject("invalid or unavailable member cursor update"));
    }
    if request.records.iter().any(|row| !row.member_id.is_empty()) {
        return Err(reject(
            "member identity is assigned by the admission fixture",
        ));
    }
    let mut seen = HashSet::new();
    for row in &request.records {
        let fact_kind = match row.kind.as_str() {
            "message" => Some(v2::FactKind::Message),
            "tool" => Some(v2::FactKind::Tool),
            "check" => Some(v2::FactKind::Check),
            "usage" => Some(v2::FactKind::Usage),
            _ => None,
        };
        let payload_valid = if row.kind == "lifecycle" {
            lifecycle_payload(&row.payload_json)
        } else if row.kind == "diagnostic" {
            allow_plugin_facts && v2::diagnostic_payload_is_valid(&row.payload_json)
        } else {
            allow_plugin_facts
                && fact_kind.is_some_and(|kind| v2::fact_payload_is_valid(kind, &row.payload_json))
        };
        let identity_policy_valid = match row.kind.as_str() {
            "lifecycle" => true,
            "message" | "tool" | "check" => allow_plugin_facts && !row.native_session_id.is_empty(),
            "usage" => {
                allow_plugin_facts
                    && ((row.native_session_id.is_empty()
                        && row.native_worker_id.is_empty()
                        && row.native_turn_id.is_empty()
                        && row.native_operation_id.is_empty())
                        || !row.native_session_id.is_empty())
            }
            "diagnostic" => {
                allow_plugin_facts
                    && row.native_session_id.is_empty()
                    && row.native_worker_id.is_empty()
                    && row.native_turn_id.is_empty()
                    && row.native_operation_id.is_empty()
            }
            _ => false,
        } && (row.kind == "tool" || row.native_operation_id.is_empty());
        if !hash(&row.event_id_hash)
            || !seen.insert(&row.event_id_hash)
            || row.revision != 0
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
            || !payload_valid
            || !identity_policy_valid
        {
            return Err(reject(if row.kind == "lifecycle" {
                "unsupported prepared lifecycle record"
            } else {
                "unsupported prepared plugin fact"
            }));
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

fn reject_cross_operation_request_id(
    db: &Connection,
    request_id: &str,
    own_table: &str,
) -> Result<()> {
    if !V2_RECEIPT_TABLES.contains(&own_table) {
        return Err(reject("unknown v2 receipt namespace"));
    }
    for table in V2_RECEIPT_TABLES
        .iter()
        .copied()
        .filter(|table| *table != own_table)
    {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        // `table` is selected only from V2_RECEIPT_TABLES above.
        let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE request_id=?)");
        if db.query_row(&query, [request_id], |row| row.get::<_, bool>(0))? {
            return Err(reject("conflicting v2 request replay identity"));
        }
    }
    Ok(())
}

fn fresh_lease_epoch(avoid: Option<i64>) -> Result<i64> {
    let mut random = fs::File::open("/dev/urandom")?;
    for _ in 0..8 {
        let mut bytes = [0_u8; 8];
        random.read_exact(&mut bytes)?;
        let epoch = i64::from_ne_bytes(bytes) & i64::MAX;
        if epoch > 0 && Some(epoch) != avoid {
            return Ok(epoch);
        }
    }
    Err(reject("could not generate a fresh positive lease epoch"))
}

fn activation_has_orphan_rows(db: &Connection, source_id: &str) -> Result<bool> {
    let rows: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_reference_owner_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_evidence_dirty_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_session_projection_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_projection_cursor_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_content_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_content_batch_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_content_status_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?1) \
         OR EXISTS(SELECT 1 FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=?1)",
        [source_id],
        |row| row.get(0),
    )?;
    if rows {
        return Ok(true);
    }
    for table in V2_RECEIPT_TABLES {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            [table],
            |row| row.get(0),
        )?;
        if exists {
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE source_id=?)");
            if db.query_row(&query, [source_id], |row| row.get::<_, bool>(0))? {
                return Ok(true);
            }
        }
    }
    let binding_table_exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        [BATCH_BINDING_TABLE],
        |row| row.get(0),
    )?;
    Ok(binding_table_exists
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_admission_batch_binding_v1 WHERE source_id=?)",
            [source_id],
            |row| row.get::<_, bool>(0),
        )?)
}

fn validate_member_history(db: &Connection, source_id: &str) -> Result<()> {
    let invalid_cursor: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 \
         WHERE source_id=?1 AND (length(CAST(member_id AS BLOB))<>64 OR \
           length(CAST(cursor AS BLOB))>?2 OR cursor=''))",
        params![source_id, MAX_CURSOR_CHARS],
        |row| row.get(0),
    )?;
    if invalid_cursor {
        return Err(reject("source has malformed member cursor history"));
    }
    let member_cursors = read_member_cursor_rows(db, source_id)?;
    if member_cursors.len() > MAX_DISCOVERY_MEMBERS as usize
        || member_cursors.iter().any(|(member, cursor)| {
            !v2::member_id_is_valid(member)
                || !v2::member_cursor_is_valid(cursor)
                || cursor.len() > MAX_CURSOR_CHARS
        })
    {
        return Err(reject(
            "source member cursor history exceeds supported limits",
        ));
    }
    let member_ids: HashSet<&str> = member_cursors
        .iter()
        .map(|(member, _)| member.as_str())
        .collect();

    let owner_count: i64 = db.query_row(
        "SELECT COUNT(*) FROM plugin_session_member_v1 WHERE source_id=?",
        [source_id],
        |row| row.get(0),
    )?;
    if owner_count > MAX_SESSION_MEMBER_OWNERS {
        return Err(reject(
            "source session-member ownership exceeds supported limits",
        ));
    }
    let session_members = read_session_member_rows(db, source_id)?;
    let mut owners = BTreeMap::new();
    for (native_session, member) in session_members {
        if !identity(&native_session)
            || (!member.is_empty() && !v2::member_id_is_valid(&member))
            || (!member.is_empty() && !member_ids.contains(member.as_str()))
            || owners.insert(native_session, member).is_some()
        {
            return Err(reject("source session-member ownership is malformed"));
        }
    }

    let mut evidence =
        db.prepare("SELECT native_session_id,member_id FROM plugin_evidence_v1 WHERE source_id=?")?;
    let mut rows = evidence.query([source_id])?;
    while let Some(row) = rows.next()? {
        let native_session: String = row.get(0)?;
        let member: String = row.get(1)?;
        if !member.is_empty()
            && (!v2::member_id_is_valid(&member) || !member_ids.contains(member.as_str()))
        {
            return Err(reject("source evidence has an unsupported member identity"));
        }
        if native_session.is_empty() {
            continue;
        }
        if let Some(owner) = owners.get(&native_session) {
            if (!owner.is_empty() && member != *owner) || (owner.is_empty() && !member.is_empty()) {
                return Err(reject(
                    "source evidence conflicts with session-member ownership",
                ));
            }
        } else if !member.is_empty() {
            return Err(reject(
                "source evidence member has no session ownership row",
            ));
        }
    }
    Ok(())
}

fn validate_active_source_history(
    db: &Connection,
    source_id: &str,
    cursor: &str,
    dirty_seq: i64,
    policy_epoch: i64,
    allow_plugin_facts: bool,
) -> Result<()> {
    require_no_provider_reference_rows(db, source_id)?;
    let unsupported: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_reference_owner_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_content_v1 WHERE source_id=?1) \
         OR (?6=0 AND EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?1)) \
         OR EXISTS(SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=?1) \
         OR (?6=0 AND EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1)) \
         OR EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1 \
                   AND (revision<>0 OR policy_epoch<>?2 OR (?6=0 AND member_id<>'') \
                     OR (?6=0 AND kind<>'lifecycle') \
                     OR (?6=1 AND kind NOT IN('lifecycle','message','tool','check','usage','diagnostic')))) \
         OR EXISTS(SELECT 1 FROM plugin_session_projection_v1 p WHERE p.source_id=?1 \
                   AND (p.policy_epoch<>?2 OR p.source_dirty_seq>?3 OR NOT EXISTS(\
                       SELECT 1 FROM plugin_evidence_v1 e WHERE e.source_id=p.source_id \
                         AND e.native_session_id=p.native_session_id \
                         AND e.policy_epoch=?2 AND e.revision=0 AND (?6=1 OR e.member_id='') \
                         AND ((?6=0 AND e.kind='lifecycle') OR (?6=1 AND \
                           e.kind IN('lifecycle','message','tool','check','usage')))))) \
         OR EXISTS(SELECT 1 FROM plugin_content_batch_v1 WHERE source_id=?1 \
                   AND (cursor<>?4 OR refs_digest<>?5)) \
         OR (?4<>'' AND NOT EXISTS(SELECT 1 FROM plugin_content_batch_v1 \
                    WHERE source_id=?1 AND cursor=?4 AND refs_digest=?5)) \
         OR EXISTS(SELECT 1 FROM plugin_content_status_v1 WHERE source_id=?1 \
                   AND (cursor<>?4 OR refs_count<>0 OR policy_epoch<>?2 OR reason<>'not_requested')) \
         OR (?4<>'' AND NOT EXISTS(SELECT 1 FROM plugin_content_status_v1 \
                    WHERE source_id=?1 AND cursor=?4 AND refs_count=0 \
                      AND policy_epoch=?2 AND reason='not_requested')) \
         OR EXISTS(SELECT 1 FROM plugin_projection_cursor_v1 WHERE source_id=?1 \
                   AND (projected_seq>?3 OR policy_epoch<>?2)) \
         OR EXISTS(SELECT 1 FROM plugin_evidence_dirty_v1 WHERE source_id=?1 AND dirty_seq<>?3) \
         OR EXISTS(SELECT 1 FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?1 \
                   AND NOT EXISTS(SELECT 1 FROM plugin_session_projection_v1 p WHERE p.source_id=?1))",
        params![
            source_id,
            policy_epoch,
            dirty_seq,
            cursor,
            format!("{:x}", Sha256::digest(b"{\"refs\":[]}")),
            i64::from(allow_plugin_facts),
        ],
        |row| row.get(0),
    )?;
    if unsupported {
        return Err(reject(
            "scratch source has private, unsupported, or orphan history",
        ));
    }
    if allow_plugin_facts {
        validate_member_history(db, source_id)?;
    }
    let mut diagnostics = db.prepare(
        "SELECT payload_json,native_session_id,native_worker_id,native_turn_id,native_operation_id \
         FROM plugin_evidence_v1 WHERE source_id=?1 AND kind='diagnostic'",
    )?;
    let mut rows = diagnostics.query([source_id])?;
    while let Some(row) = rows.next()? {
        let payload: String = row.get(0)?;
        let identities = [
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ];
        let has_identity = identities.iter().any(|value| !value.is_empty());
        if !allow_plugin_facts || has_identity || !v2::diagnostic_payload_is_valid(&payload) {
            return Err(reject("scratch source has unsupported diagnostic history"));
        }
    }
    let evidence_dirty: Option<i64> = db
        .query_row(
            "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    if evidence_dirty.unwrap_or(0) != dirty_seq {
        return Err(reject("scratch evidence dirty sequence is inconsistent"));
    }
    let projection: Option<(i64, i64)> = db
        .query_row(
            "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 WHERE source_id=?",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if projection
        .is_some_and(|(projected, policy)| projected == dirty_seq && policy == policy_epoch)
    {
        let orphaned: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_evidence_v1 e WHERE e.source_id=?1 \
             AND e.native_session_id<>'' AND NOT EXISTS(SELECT 1 FROM plugin_session_projection_v1 p \
               WHERE p.source_id=e.source_id AND p.native_session_id=e.native_session_id))",
            [source_id],
            |row| row.get(0),
        )?;
        if orphaned {
            return Err(reject(
                "scratch projection omits admitted lifecycle history",
            ));
        }
    }
    Ok(())
}

fn commit_guarded(
    db: &mut Connection,
    request: &Request,
    writer_lease: Option<&WriterLease>,
    replay: Option<&v2::ReplayIdentity>,
) -> Result<Receipt> {
    commit_guarded_mode(db, request, writer_lease, replay, false)
}

fn commit_guarded_mode(
    db: &mut Connection,
    request: &Request,
    writer_lease: Option<&WriterLease>,
    replay: Option<&v2::ReplayIdentity>,
    real_clone: bool,
) -> Result<Receipt> {
    commit_guarded_member_mode(db, request, writer_lease, replay, real_clone, false)
}

fn commit_guarded_member_mode(
    db: &mut Connection,
    request: &Request,
    writer_lease: Option<&WriterLease>,
    replay: Option<&v2::ReplayIdentity>,
    real_clone: bool,
    allow_member_fixture: bool,
) -> Result<Receipt> {
    validate(request, real_clone)?;
    if allow_member_fixture && !real_clone {
        return Err(reject("member cursor fixture requires a marked real clone"));
    }
    if request.member_update.is_some() && !allow_member_fixture {
        return Err(reject(
            "member cursor update requires clone fixture dispatch",
        ));
    }
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(lease) = writer_lease {
        lease.verify_admission()?;
    }
    if real_clone {
        require_activation_clone_markers(&transaction)?;
        reject_real_clone_admission_triggers(&transaction)?;
        if allow_member_fixture {
            ensure_batch_binding_schema(&transaction)?;
        } else {
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS storage_admission_batch_binding_v1(\
                 source_id TEXT PRIMARY KEY,generation TEXT NOT NULL,policy_epoch INTEGER NOT NULL,\
                 expected_cursor TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
                 prepared_digest TEXT NOT NULL,dirty_seq INTEGER NOT NULL)",
            )?;
        }
    } else {
        require_scratch(&transaction)?;
    }
    let receiver: String = transaction.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if let Some(replay) = replay {
        reject_cross_operation_request_id(
            &transaction,
            &replay.request_id,
            "storage_admission_request_v2",
        )?;
    }
    let source = source_state(&transaction, &request.lease.source_id)?;
    let SourceState {
        receiver_id: source_receiver,
        plugin_id: plugin,
        generation,
        lease_epoch: epoch,
        policy_epoch: policy,
        capture_enabled: enabled,
        content_fingerprint: fingerprint,
        cursor: current_cursor,
        prior_cursor,
        dirty_seq,
        last_batch_digest: prior_digest,
        updated_ns: _,
    } = source.clone();
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
    if request.cursor.is_empty() && !current_cursor.is_empty() {
        return Err(reject("source cursor cannot return to empty"));
    }
    if real_clone {
        // A real Store clone must pass the same complete history gate as
        // activation. Stale or private content receipts cannot be overwritten
        // by an empty-reference receipt for the next cursor.
        validate_active_source_history(
            &transaction,
            &request.lease.source_id,
            &current_cursor,
            dirty_seq,
            policy,
            true,
        )?;
    } else {
        // Preserve the scratch-only replay fixture's historical narrow gate.
        let unsupported: i64 = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_content_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_retention_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_reference_owner_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1 \
                       AND (revision<>0 OR kind<>'lifecycle' OR member_id<>''))",
            [&request.lease.source_id],
            |row| row.get(0),
        )?;
        if unsupported != 0 {
            return Err(reject("scratch source has unsupported history"));
        }
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
    let snapshot = if real_clone {
        Some(AdmissionSnapshot::capture(
            &transaction,
            &request.lease.source_id,
        )?)
    } else {
        None
    };
    if real_clone && !allow_member_fixture {
        let has_member_evidence: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1 AND member_id<>'') \
             OR EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1)",
            [&request.lease.source_id],
            |row| row.get(0),
        )?;
        let has_member_binding = snapshot
            .as_ref()
            .and_then(|value| value.batch_binding_before.as_ref())
            .is_some_and(|binding| {
                !binding.member_id.is_empty() || !binding.member_cursor.is_empty()
            });
        if has_member_evidence || has_member_binding {
            return Err(reject(
                "member history is available only through the clone member fixture",
            ));
        }
    }
    let empty_member_plan = MemberAdmissionPlan {
        effective_records: request.records.clone(),
        session_member_inserts: Vec::new(),
        member_cursors_after: Vec::new(),
        session_members_after: Vec::new(),
    };
    let member_plan = if let Some(snapshot) = snapshot.as_ref() {
        plan_member_admission(&transaction, request, snapshot)?
    } else {
        if request.member_update.is_some() {
            return Err(reject("member cursor update requires marked clone fixture"));
        }
        empty_member_plan
    };
    let prepared_state = if real_clone && allow_member_fixture {
        Some(prepared_state_digest(
            &member_plan.effective_records,
            request.member_update.as_ref(),
        )?)
    } else if real_clone {
        Some((1, prepared_rows_digest(&member_plan.effective_records)?))
    } else {
        None
    };
    let empty_inserted_hashes = HashSet::new();
    if request.records.is_empty()
        && request.member_update.is_none()
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
            real_clone,
            snapshot.as_ref().map(|snapshot| AdmissionReadback {
                snapshot,
                expected_source: &source,
                inserted_hashes: &empty_inserted_hashes,
                admitted_ns: None,
                effective_records: &member_plan.effective_records,
                member_cursors_after: &member_plan.member_cursors_after,
                session_members_after: &member_plan.session_members_after,
            }),
        );
    }
    if request.cursor == current_cursor
        && request.batch_digest == prior_digest
        && (request.expected_cursor == prior_cursor || request.expected_cursor == current_cursor)
    {
        if let Some((prepared_version, prepared_digest)) = prepared_state.as_ref() {
            let binding = read_batch_binding(&transaction, &request.lease.source_id)?
                .ok_or_else(|| reject("prepared batch replay binding is missing"))?;
            if binding.generation != request.lease.generation
                || binding.policy_epoch != request.lease.policy_epoch
                || binding.cursor != request.cursor
                || binding.batch_digest != request.batch_digest
                || binding.prepared_version != *prepared_version
                || binding.prepared_digest != *prepared_digest
                || binding.member_id
                    != request
                        .member_update
                        .as_ref()
                        .map_or("", |m| m.member_id.as_str())
                || binding.member_cursor
                    != request
                        .member_update
                        .as_ref()
                        .map_or("", |m| m.cursor.as_str())
                || binding.dirty_seq != dirty_seq
                || (binding.expected_cursor != request.expected_cursor
                    && request.expected_cursor != request.cursor)
            {
                return Err(reject("conflicting prepared batch replay identity"));
            }
            if member_plan.member_cursors_after != snapshot.as_ref().unwrap().member_cursors_before
                && request.member_update.is_some()
            {
                return Err(reject("member cursor replay state mismatch"));
            }
        }
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
            real_clone,
            snapshot.as_ref().map(|snapshot| AdmissionReadback {
                snapshot,
                expected_source: &source,
                inserted_hashes: &empty_inserted_hashes,
                admitted_ns: None,
                effective_records: &member_plan.effective_records,
                member_cursors_after: &member_plan.member_cursors_after,
                session_members_after: &member_plan.session_members_after,
            }),
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
    let mut inserted_hashes = HashSet::new();
    for row in &member_plan.effective_records {
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
                || member != row.member_id
                || digest != row.record_digest
            {
                return Err(reject("conflicting or unsupported event revision"));
            }
            continue;
        }
        if count >= MAX_SOURCE_RECORDS {
            return Err(reject("scratch evidence capacity reached"));
        }
        let changed = transaction.execute(
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
                row.member_id,
                request.lease.policy_epoch,
                admitted_ns
            ],
        )?;
        if changed != 1 {
            return Err(reject(
                "admission evidence insert did not affect exactly one row",
            ));
        }
        count += 1;
        inserted = true;
        inserted_hashes.insert(row.event_id_hash.clone());
    }
    let next_dirty = dirty_seq
        .checked_add(i64::from(inserted))
        .ok_or_else(|| reject("dirty sequence overflow"))?;
    let updated_ns = now_ns()?;
    let source_changed = transaction.execute(
        "UPDATE plugin_source_cursor_v1 SET cursor=?,prior_cursor=?,last_batch_digest=?,\
         dirty_seq=?,updated_ns=? WHERE source_id=?",
        params![
            request.cursor,
            request.expected_cursor,
            request.batch_digest,
            next_dirty,
            updated_ns,
            request.lease.source_id
        ],
    )?;
    if source_changed != 1 {
        return Err(reject(
            "admission source update did not affect exactly one row",
        ));
    }
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
    for (session_id, member_id) in &member_plan.session_member_inserts {
        transaction.execute(
            "INSERT INTO plugin_session_member_v1(source_id,native_session_id,member_id) \
             VALUES(?,?,?)",
            params![request.lease.source_id, session_id, member_id],
        )?;
    }
    if let Some(member_update) = request.member_update.as_ref() {
        transaction.execute(
            "INSERT INTO plugin_source_member_cursor_v1(source_id,member_id,cursor) VALUES(?,?,?) \
             ON CONFLICT(source_id,member_id) DO UPDATE SET cursor=excluded.cursor",
            params![
                request.lease.source_id,
                member_update.member_id,
                member_update.cursor
            ],
        )?;
    }
    if let Some((prepared_version, prepared_digest)) = prepared_state.as_ref() {
        let binding = binding_for_request(
            request,
            next_dirty,
            *prepared_version,
            prepared_digest.clone(),
        );
        let changed = if allow_member_fixture {
            transaction.execute(
                "INSERT INTO storage_admission_batch_binding_v1(source_id,generation,policy_epoch,\
             expected_cursor,cursor,batch_digest,prepared_digest,dirty_seq,prepared_version,\
             member_id,member_cursor) VALUES(?,?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(source_id) DO UPDATE SET generation=excluded.generation,\
             policy_epoch=excluded.policy_epoch,expected_cursor=excluded.expected_cursor,\
             cursor=excluded.cursor,batch_digest=excluded.batch_digest,\
             prepared_digest=excluded.prepared_digest,dirty_seq=excluded.dirty_seq,\
             prepared_version=excluded.prepared_version,member_id=excluded.member_id,\
             member_cursor=excluded.member_cursor",
                params![
                    request.lease.source_id,
                    binding.generation,
                    binding.policy_epoch,
                    binding.expected_cursor,
                    binding.cursor,
                    binding.batch_digest,
                    binding.prepared_digest,
                    binding.dirty_seq,
                    binding.prepared_version,
                    binding.member_id,
                    binding.member_cursor,
                ],
            )?
        } else {
            transaction.execute(
                "INSERT INTO storage_admission_batch_binding_v1(source_id,generation,policy_epoch,\
                 expected_cursor,cursor,batch_digest,prepared_digest,dirty_seq) \
                 VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(source_id) DO UPDATE SET \
                 generation=excluded.generation,policy_epoch=excluded.policy_epoch,\
                 expected_cursor=excluded.expected_cursor,cursor=excluded.cursor,\
                 batch_digest=excluded.batch_digest,prepared_digest=excluded.prepared_digest,\
                 dirty_seq=excluded.dirty_seq",
                params![
                    request.lease.source_id,
                    binding.generation,
                    binding.policy_epoch,
                    binding.expected_cursor,
                    binding.cursor,
                    binding.batch_digest,
                    binding.prepared_digest,
                    binding.dirty_seq,
                ],
            )?
        };
        if changed != 1 {
            return Err(reject(
                "prepared batch binding did not persist exactly one row",
            ));
        }
    }
    let mut expected_source = source.clone();
    expected_source.cursor.clone_from(&request.cursor);
    expected_source
        .prior_cursor
        .clone_from(&request.expected_cursor);
    expected_source
        .last_batch_digest
        .clone_from(&request.batch_digest);
    expected_source.dirty_seq = next_dirty;
    expected_source.updated_ns = updated_ns;
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
        real_clone,
        snapshot.as_ref().map(|snapshot| AdmissionReadback {
            snapshot,
            expected_source: &expected_source,
            inserted_hashes: &inserted_hashes,
            admitted_ns: Some(admitted_ns),
            effective_records: &member_plan.effective_records,
            member_cursors_after: &member_plan.member_cursors_after,
            session_members_after: &member_plan.session_members_after,
        }),
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
    validate(&request, false)?;
    let path = Path::new(&request.sqlite_path);
    let receipt = with_marked_scratch(path, |db, lease| {
        commit_guarded(db, &request, Some(lease), None)
    })?;
    Ok(serde_json::to_string(&receipt)?)
}

/// Read-only preflight for the fixed disposable path before a service socket
/// is published. The transaction rechecks the marker before any mutation.
pub fn require_marked_scratch_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(reject("scratch fixture must be an existing regular file"));
    }
    let read = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    require_scratch(&read)
}

/// Only a detached, explicitly marked real Store clone may enter this route.
/// The service supplies the path; untrusted v2 frames contain no path.
fn require_activation_clone_markers(db: &Connection) -> Result<()> {
    let marker: Option<String> = db
        .query_row(
            "SELECT value FROM schema_meta WHERE key=?",
            [CLONE_MARKER],
            |row| row.get(0),
        )
        .optional()?;
    let authority: Option<String> = db
        .query_row(
            "SELECT value FROM schema_meta WHERE key='storage_authority_v1'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let schema: Option<String> = db
        .query_row(
            "SELECT value FROM schema_meta WHERE key='plugin_sessions_read_schema'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if marker.as_deref() != Some("enabled")
        || authority.as_deref() != Some("python-v1")
        || schema.as_deref() != Some("1")
    {
        return Err(reject(
            "activation clone marker or real Store schema is unavailable",
        ));
    }
    Ok(())
}

pub fn require_marked_activation_clone_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(reject("activation clone must be an existing regular file"));
    }
    let read = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    require_activation_clone_markers(&read)
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
    require_marked_scratch_path(path)?;
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

fn with_marked_activation_clone<T>(
    path: &Path,
    apply: impl FnOnce(&mut Connection, &WriterLease) -> Result<T>,
) -> Result<T> {
    require_marked_activation_clone_path(path)?;
    let lease = WriterLease::acquire(path)?;
    lease.verify_admission()?;
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    require_marked_activation_clone_path(path)?;
    let result = apply(&mut db, &lease)?;
    drop(db);
    drop(lease);
    Ok(result)
}

/// Mutate only source activation and bounded metadata-only admissions on an
/// explicitly marked detached real-schema clone. No production caller.
pub fn apply_activation_clone_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    apply_activation_clone_v2_json_inner(path, input, false)
}

/// Fixture-only mixed-batch entry point. The experimental storage service
/// deliberately uses `apply_activation_clone_v2_json`, which rejects this
/// operation; only the separately invoked admission fixture binary opts in.
pub fn apply_activation_clone_mixed_batch_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    if !matches!(request.operation, v2::Operation::AdmitBatch { .. }) {
        return Err(reject("fixture entry point accepts mixed admission only"));
    }
    apply_activation_clone_v2_json_inner(path, input, true)
}

/// Fixture-only capture and revoke transitions for an explicitly marked
/// detached real-Store clone. The storage service dispatcher does not call it.
pub fn apply_activation_clone_policy_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    let replay = request.replay_identity()?;
    let result = match &request.operation {
        v2::Operation::RevokeSource {
            expected_dirty_seq, ..
        } => {
            let receipt = with_marked_activation_clone(path, |db, writer_lease| {
                revoke_source_guarded_mode(
                    db,
                    &request,
                    *expected_dirty_seq,
                    Some(writer_lease),
                    &replay,
                    true,
                )
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                _ => return Err(reject("unexpected clone revocation disposition")),
            };
            v2::OperationResult::SourceRevoked(v2::SourceRevokedResult {
                disposition,
                cursor: receipt.cursor,
                dirty_seq: receipt.dirty_seq,
                lease_epoch: receipt.lease_epoch,
                policy_epoch: receipt.policy_epoch,
                capture_enabled: receipt.capture_enabled,
            })
        }
        v2::Operation::SetSourceCapture { .. } => {
            let receipt = with_marked_activation_clone(path, |db, writer_lease| {
                set_source_capture_guarded_mode(db, &request, Some(writer_lease), &replay, true)
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                "idle" => v2::Disposition::Idle,
                _ => return Err(reject("unexpected clone capture disposition")),
            };
            v2::OperationResult::SourceCaptureSet(v2::SourceCaptureSetResult {
                disposition,
                lease: receipt.lease,
                enabled: receipt.enabled,
                cursor: receipt.cursor,
                dirty_seq: receipt.dirty_seq,
            })
        }
        _ => {
            return Err(reject(
                "clone policy fixture accepts capture or revoke only",
            ));
        }
    };
    v2::Reply {
        version: v2::VERSION,
        request_id: request.request_id.clone(),
        receiver_id: request.receiver_id.clone(),
        request_digest: replay.digest,
        result,
    }
    .to_json(&request)
    .map_err(Into::into)
}

/// Read-only, fixture-only access to source-scoped diagnostics on a marked
/// detached Store clone. This route creates no schema objects and never opens
/// a writable SQLite connection.
pub fn read_activation_clone_diagnostic_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    let v2::Operation::ReadLastSourceDiagnostic { lease } = &request.operation else {
        return Err(reject(
            "diagnostic fixture accepts source diagnostic reads only",
        ));
    };
    require_marked_activation_clone_path(path)?;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    require_activation_clone_markers(&db)?;
    let diagnostic = read_last_source_diagnostic(&db, lease)?;
    let replay = request.replay_identity()?;
    v2::Reply {
        version: v2::VERSION,
        request_id: request.request_id.clone(),
        receiver_id: request.receiver_id.clone(),
        request_digest: replay.digest,
        result: v2::OperationResult::SourceDiagnosticRead(v2::SourceDiagnosticReadResult {
            diagnostic,
        }),
    }
    .to_json(&request)
    .map_err(Into::into)
}

/// Read the bounded member cursor map from a marked detached Store clone.
/// This is fixture-only and opens SQLite read-only; it has no service caller.
pub fn read_activation_clone_member_cursors_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    let v2::Operation::ReadSourceMemberCursors { lease } = &request.operation else {
        return Err(reject(
            "member cursor fixture accepts member cursor reads only",
        ));
    };
    require_marked_activation_clone_path(path)?;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    require_activation_clone_markers(&db)?;
    let current = source_state(&db, &lease.source_id)?;
    if current.receiver_id != lease.receiver_id
        || current.plugin_id != lease.plugin_id
        || current.generation != lease.generation
        || current.lease_epoch != lease.epoch
        || current.policy_epoch != lease.policy_epoch
        || current.capture_enabled != 1
    {
        return Err(reject(
            "stale or disabled source lease for member cursor read",
        ));
    }
    validate_active_source_history(
        &db,
        &lease.source_id,
        &current.cursor,
        current.dirty_seq,
        current.policy_epoch,
        true,
    )?;
    let cursors = read_member_cursor_rows(&db, &lease.source_id)?
        .into_iter()
        .map(|(member_id, cursor)| v2::MemberCursor { member_id, cursor })
        .collect();
    let replay = request.replay_identity()?;
    v2::Reply {
        version: v2::VERSION,
        request_id: request.request_id.clone(),
        receiver_id: request.receiver_id.clone(),
        request_digest: replay.digest,
        result: v2::OperationResult::SourceMemberCursorsRead(v2::SourceMemberCursorsReadResult {
            cursors,
        }),
    }
    .to_json(&request)
    .map_err(Into::into)
}

fn read_last_source_diagnostic(
    db: &Connection,
    lease: &v2::SourceLease,
) -> Result<Option<v2::SourceDiagnostic>> {
    let source_matches: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_source_cursor_v1 \
         WHERE source_id=?1 AND receiver_id=?2 AND plugin_id=?3 AND generation=?4 \
           AND policy_epoch=?5 AND capture_enabled=1)",
        params![
            lease.source_id,
            lease.receiver_id,
            lease.plugin_id,
            lease.generation,
            lease.policy_epoch,
        ],
        |row| row.get(0),
    )?;
    if !source_matches {
        return Ok(None);
    }

    let mut statement = db.prepare(
        "SELECT payload_json,time_ns,revision,member_id,native_session_id, \
                native_worker_id,native_turn_id,native_operation_id \
         FROM plugin_evidence_v1 WHERE source_id=?1 AND kind='diagnostic' \
           AND policy_epoch=?2 ORDER BY admitted_ns DESC,rowid DESC",
    )?;
    let mut rows = statement.query(params![lease.source_id, lease.policy_epoch])?;
    let mut latest = None;
    while let Some(row) = rows.next()? {
        let payload_json: String = row.get(0)?;
        let time_ns: i64 = row.get(1)?;
        let revision: i64 = row.get(2)?;
        let member_id: String = row.get(3)?;
        let identities = [
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
        ];
        if time_ns < 0
            || revision != 0
            || (!member_id.is_empty()
                && (!v2::member_id_is_valid(&member_id)
                    || !db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 \
                         WHERE source_id=?1 AND member_id=?2 AND length(CAST(cursor AS BLOB))<=800 \
                           AND cursor<>'')",
                        params![lease.source_id, member_id],
                        |found| found.get::<_, bool>(0),
                    )?))
            || identities.iter().any(|value| !value.is_empty())
            || !v2::diagnostic_payload_is_valid(&payload_json)
        {
            return Err(reject("stored source diagnostic is invalid"));
        }
        if latest.is_none() {
            let payload: serde_json::Value = serde_json::from_str(&payload_json)?;
            let code = payload
                .get("code")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| reject("stored source diagnostic is invalid"))?;
            let severity = payload
                .get("severity")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| reject("stored source diagnostic is invalid"))?;
            latest = Some(v2::SourceDiagnostic {
                code: code.to_owned(),
                severity: severity.to_owned(),
                time_ns: time_ns.to_string(),
            });
        }
    }
    Ok(latest)
}

fn apply_activation_clone_v2_json_inner(
    path: &Path,
    input: &[u8],
    allow_mixed_batch_fixture: bool,
) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    let replay = request.replay_identity()?;
    let result = match &request.operation {
        v2::Operation::ActivateSource { .. } => {
            let receipt = with_marked_activation_clone(path, |db, lease| {
                activate_source_guarded(db, &request, Some(lease), &replay, true)
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                _ => return Err(reject("unexpected activation clone disposition")),
            };
            v2::OperationResult::SourceActivated(v2::SourceActivatedResult {
                disposition,
                lease: receipt.lease,
                cursor: receipt.cursor,
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::AdmitLifecycle {
            lease,
            expected_cursor,
            cursor,
            batch_digest,
            records,
        } => {
            let path_string = path
                .to_str()
                .ok_or_else(|| reject("activation clone path must be UTF-8"))?
                .to_owned();
            let prepared = Request {
                schema_version: 1,
                sqlite_path: path_string,
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
                        member_id: String::new(),
                    })
                    .collect(),
                member_update: None,
            };
            let receipt = with_marked_activation_clone(path, |db, lease| {
                commit_guarded_mode(db, &prepared, Some(lease), Some(&replay), true)
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                "idle" => v2::Disposition::Idle,
                _ => return Err(reject("unexpected activation-clone admission disposition")),
            };
            v2::OperationResult::LifecycleAdmission(v2::AdmissionResult {
                disposition,
                cursor: receipt.cursor,
                batch_digest: batch_digest.clone(),
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::AdmitFacts {
            lease,
            expected_cursor,
            cursor,
            batch_digest,
            records,
        } => {
            let prepared = Request {
                schema_version: 1,
                sqlite_path: path
                    .to_str()
                    .ok_or_else(|| reject("activation clone path must be UTF-8"))?
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
                        kind: row.kind.as_str().to_owned(),
                        time_ns: row.time_ns,
                        payload_json: row.payload_json.clone(),
                        record_digest: row.record_digest.clone(),
                        native_session_id: row.native_session_id.clone(),
                        native_worker_id: row.native_worker_id.clone(),
                        native_turn_id: row.native_turn_id.clone(),
                        native_operation_id: row.native_operation_id.clone(),
                        member_id: String::new(),
                    })
                    .collect(),
                member_update: None,
            };
            let receipt = with_marked_activation_clone(path, |db, lease| {
                commit_guarded_mode(db, &prepared, Some(lease), Some(&replay), true)
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                "idle" => v2::Disposition::Idle,
                _ => return Err(reject("unexpected activation-clone fact disposition")),
            };
            v2::OperationResult::FactAdmission(v2::AdmissionResult {
                disposition,
                cursor: receipt.cursor,
                batch_digest: batch_digest.clone(),
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::AdmitBatch {
            lease,
            expected_cursor,
            cursor,
            batch_digest,
            records,
            member_update,
        } => {
            if !allow_mixed_batch_fixture {
                return Err(reject(
                    "mixed admission is unavailable through the storage service path",
                ));
            }
            let prepared = Request {
                schema_version: 1,
                sqlite_path: path
                    .to_str()
                    .ok_or_else(|| reject("activation clone path must be UTF-8"))?
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
                        kind: row.kind.as_str().to_owned(),
                        time_ns: row.time_ns,
                        payload_json: row.payload_json.clone(),
                        record_digest: row.record_digest.clone(),
                        native_session_id: row.native_session_id.clone(),
                        native_worker_id: row.native_worker_id.clone(),
                        native_turn_id: row.native_turn_id.clone(),
                        native_operation_id: row.native_operation_id.clone(),
                        member_id: String::new(),
                    })
                    .collect(),
                member_update: member_update.clone(),
            };
            let receipt = with_marked_activation_clone(path, |db, writer_lease| {
                commit_guarded_member_mode(
                    db,
                    &prepared,
                    Some(writer_lease),
                    Some(&replay),
                    true,
                    true,
                )
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                "idle" => v2::Disposition::Idle,
                _ => {
                    return Err(reject(
                        "unexpected activation-clone mixed batch disposition",
                    ));
                }
            };
            v2::OperationResult::BatchAdmission(v2::AdmissionResult {
                disposition,
                cursor: receipt.cursor,
                batch_digest: batch_digest.clone(),
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::RevokeSource { .. } => {
            return Err(reject(
                "clone revocation is unavailable through the storage service path",
            ));
        }
        _ => {
            return Err(reject(
                "real-schema clone accepts only activation or admission",
            ));
        }
    };
    v2::Reply {
        version: v2::VERSION,
        request_id: request.request_id.clone(),
        receiver_id: request.receiver_id.clone(),
        request_digest: replay.digest,
        result,
    }
    .to_json(&request)
    .map_err(Into::into)
}

/// Apply a strict v2 lifecycle request to an explicit disposable scratch path.
/// The path is a CLI/library argument, never accepted from the untrusted v2
/// frame. No service listener, receiver, or production Store calls this API.
pub fn apply_v2_json(path: &Path, input: &[u8]) -> Result<Vec<u8>> {
    let request = v2::Request::from_json(input)?;
    let replay = request.replay_identity()?;
    let result = match &request.operation {
        v2::Operation::ActivateSource { .. } => {
            let receipt = with_marked_scratch(path, |db, writer_lease| {
                activate_source_guarded(db, &request, Some(writer_lease), &replay, false)
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                _ => return Err(reject("unexpected scratch activation disposition")),
            };
            v2::OperationResult::SourceActivated(v2::SourceActivatedResult {
                disposition,
                lease: receipt.lease,
                cursor: receipt.cursor,
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::AdmitLifecycle {
            lease,
            expected_cursor,
            cursor,
            batch_digest,
            records,
        } => {
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
                        member_id: String::new(),
                    })
                    .collect(),
                member_update: None,
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
            v2::OperationResult::LifecycleAdmission(v2::AdmissionResult {
                disposition,
                cursor: receipt.cursor,
                batch_digest: batch_digest.clone(),
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::AdmitFacts { .. } | v2::Operation::AdmitBatch { .. } => {
            return Err(reject("scratch mode accepts lifecycle admission only"));
        }
        v2::Operation::RevokeSource {
            cursor,
            expected_dirty_seq,
            ..
        } => {
            let receipt = with_marked_scratch(path, |db, writer_lease| {
                revoke_source_guarded(
                    db,
                    &request,
                    *expected_dirty_seq,
                    Some(writer_lease),
                    &replay,
                )
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                _ => return Err(reject("unexpected scratch revocation disposition")),
            };
            if receipt.cursor != *cursor {
                return Err(reject("scratch revocation cursor changed"));
            }
            v2::OperationResult::SourceRevoked(v2::SourceRevokedResult {
                disposition,
                cursor: receipt.cursor,
                dirty_seq: receipt.dirty_seq,
                lease_epoch: receipt.lease_epoch,
                policy_epoch: receipt.policy_epoch,
                capture_enabled: receipt.capture_enabled,
            })
        }
        v2::Operation::SetSourceCapture { .. } => {
            let receipt = with_marked_scratch(path, |db, writer_lease| {
                set_source_capture_guarded(db, &request, Some(writer_lease), &replay)
            })?;
            let disposition = match receipt.disposition {
                "committed" => v2::Disposition::Committed,
                "replay" => v2::Disposition::Replay,
                "idle" => v2::Disposition::Idle,
                _ => return Err(reject("unexpected scratch source capture disposition")),
            };
            v2::OperationResult::SourceCaptureSet(v2::SourceCaptureSetResult {
                disposition,
                lease: receipt.lease,
                enabled: receipt.enabled,
                cursor: receipt.cursor,
                dirty_seq: receipt.dirty_seq,
            })
        }
        v2::Operation::ReadLastSourceDiagnostic { .. } => {
            return Err(reject("scratch mode does not expose source diagnostics"));
        }
        v2::Operation::ReadSourceMemberCursors { .. } => {
            return Err(reject("scratch mode does not expose member cursors"));
        }
    };
    v2::Reply {
        version: v2::VERSION,
        request_id: request.request_id.clone(),
        receiver_id: request.receiver_id.clone(),
        request_digest: replay.digest,
        result,
    }
    .to_json(&request)
    .map_err(Into::into)
}

fn activate_source_guarded(
    db: &mut Connection,
    request: &v2::Request,
    writer_lease: Option<&WriterLease>,
    replay: &v2::ReplayIdentity,
    real_clone: bool,
) -> Result<SourceActivationReceipt> {
    let v2::Operation::ActivateSource {
        plugin_id,
        source_id,
        generation,
    } = &request.operation
    else {
        return Err(reject("source activation operation required"));
    };
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(writer_lease) = writer_lease {
        writer_lease.verify_admission()?;
    }
    if real_clone {
        require_activation_clone_markers(&transaction)?;
        reject_real_clone_activation_triggers(&transaction)?;
    } else {
        require_scratch(&transaction)?;
    }
    let receiver: String = transaction.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if receiver != request.receiver_id {
        return Err(reject("scratch receiver identity mismatch"));
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS storage_source_activate_v2(\
         request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
         source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
         lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
         dirty_seq INTEGER NOT NULL)",
    )?;
    reject_cross_operation_request_id(
        &transaction,
        &replay.request_id,
        "storage_source_activate_v2",
    )?;
    type Stored = (
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
        String,
        i64,
    );
    let stored: Option<Stored> = transaction
        .query_row(
            "SELECT receiver_id,request_digest,source_id,plugin_id,generation,lease_epoch,\
             policy_epoch,cursor,dirty_seq FROM storage_source_activate_v2 WHERE request_id=?",
            [&replay.request_id],
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
                ))
            },
        )
        .optional()?;
    if let Some((
        stored_receiver,
        digest,
        stored_source,
        stored_plugin,
        stored_generation,
        epoch,
        policy,
        cursor,
        dirty_seq,
    )) = stored
    {
        if stored_receiver != receiver
            || digest != replay.digest
            || stored_source != *source_id
            || stored_plugin != *plugin_id
            || stored_generation != *generation
            || epoch <= 0
            || policy <= 0
            || !cursor_value(&cursor)
            || dirty_seq < 0
        {
            return Err(reject("conflicting v2 source activation request"));
        }
        transaction.commit()?;
        return Ok(SourceActivationReceipt {
            lease: v2::SourceLease {
                receiver_id: receiver,
                plugin_id: stored_plugin,
                source_id: stored_source,
                generation: stored_generation,
                epoch,
                policy_epoch: policy,
            },
            cursor,
            dirty_seq,
            disposition: "replay",
        });
    }
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
        String,
        i64,
    );
    let source: Option<Source> = transaction
        .query_row(
            "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
             content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
             FROM plugin_source_cursor_v1 WHERE source_id=?",
            [source_id],
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
        )
        .optional()?;
    let now = now_ns()?;
    let (epoch, policy, cursor, dirty_seq, prior_cursor, last_digest) = if let Some((
        source_receiver,
        stored_plugin,
        stored_generation,
        old_epoch,
        policy,
        enabled,
        fingerprint,
        cursor,
        prior_cursor,
        last_digest,
        dirty_seq,
    )) = source
    {
        if source_receiver != receiver
            || stored_plugin != *plugin_id
            || stored_generation != *generation
        {
            return Err(reject("source ownership, receiver, or generation mismatch"));
        }
        if enabled != 1 || !fingerprint.is_empty() {
            return Err(reject(
                "scratch source capture is revoked or has private state",
            ));
        }
        if old_epoch <= 0
            || policy <= 0
            || dirty_seq < 0
            || !cursor_value(&cursor)
            || !cursor_value(&prior_cursor)
            || (!last_digest.is_empty() && !hash(&last_digest))
        {
            return Err(reject("scratch source state is malformed"));
        }
        validate_active_source_history(
            &transaction,
            source_id,
            &cursor,
            dirty_seq,
            policy,
            real_clone,
        )?;
        let epoch = fresh_lease_epoch(Some(old_epoch))?;
        let changed = transaction.execute(
            "UPDATE plugin_source_cursor_v1 SET lease_epoch=?,updated_ns=? WHERE source_id=?",
            params![epoch, now, source_id],
        )?;
        if changed != 1 {
            return Err(reject(
                "source activation update did not affect exactly one row",
            ));
        }
        (epoch, policy, cursor, dirty_seq, prior_cursor, last_digest)
    } else {
        if activation_has_orphan_rows(&transaction, source_id)? {
            return Err(reject("orphan scratch source history prevents activation"));
        }
        let epoch = fresh_lease_epoch(None)?;
        let changed = transaction.execute(
            "INSERT INTO plugin_source_cursor_v1 \
             (source_id,receiver_id,plugin_id,generation,lease_epoch,cursor,prior_cursor,\
              last_batch_digest,dirty_seq,policy_epoch,capture_enabled,content_fingerprint,updated_ns) \
             VALUES(?,?,?,?,?,'','','',0,1,1,'',?)",
            params![source_id, receiver, plugin_id, generation, epoch, now],
        )?;
        if changed != 1 {
            return Err(reject(
                "source activation insert did not affect exactly one row",
            ));
        }
        (epoch, 1, String::new(), 0, String::new(), String::new())
    };
    let expected_source: Source = (
        receiver.clone(),
        plugin_id.clone(),
        generation.clone(),
        epoch,
        policy,
        1,
        String::new(),
        cursor.clone(),
        prior_cursor,
        last_digest,
        dirty_seq,
    );
    let persisted_source: Option<Source> = transaction
        .query_row(
            "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
             content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
             FROM plugin_source_cursor_v1 WHERE source_id=?",
            [source_id],
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
        )
        .optional()?;
    if persisted_source.as_ref() != Some(&expected_source) {
        return Err(reject("source activation state was not persisted exactly"));
    }
    let lease = v2::SourceLease {
        receiver_id: receiver.clone(),
        plugin_id: plugin_id.clone(),
        source_id: source_id.clone(),
        generation: generation.clone(),
        epoch,
        policy_epoch: policy,
    };
    let receipt_changed = transaction.execute(
        "INSERT INTO storage_source_activate_v2(request_id,receiver_id,request_digest,\
         source_id,plugin_id,generation,lease_epoch,policy_epoch,cursor,dirty_seq) \
         VALUES(?,?,?,?,?,?,?,?,?,?)",
        params![
            replay.request_id,
            receiver,
            replay.digest,
            source_id,
            plugin_id,
            generation,
            epoch,
            policy,
            cursor,
            dirty_seq
        ],
    )?;
    if receipt_changed != 1 {
        return Err(reject(
            "source activation receipt insert did not affect exactly one row",
        ));
    }
    let persisted_receipt: Option<Stored> = transaction
        .query_row(
            "SELECT receiver_id,request_digest,source_id,plugin_id,generation,lease_epoch,\
             policy_epoch,cursor,dirty_seq FROM storage_source_activate_v2 WHERE request_id=?",
            [&replay.request_id],
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
                ))
            },
        )
        .optional()?;
    if persisted_receipt.as_ref()
        != Some(&(
            receiver.clone(),
            replay.digest.clone(),
            source_id.clone(),
            plugin_id.clone(),
            generation.clone(),
            epoch,
            policy,
            cursor.clone(),
            dirty_seq,
        ))
    {
        return Err(reject(
            "source activation receipt was not persisted exactly",
        ));
    }
    let source_after_receipt: Option<Source> = transaction
        .query_row(
            "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
             content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
             FROM plugin_source_cursor_v1 WHERE source_id=?",
            [source_id],
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
        )
        .optional()?;
    if source_after_receipt.as_ref() != Some(&expected_source) {
        return Err(reject(
            "source activation state changed while persisting its receipt",
        ));
    }
    reject_cross_operation_request_id(
        &transaction,
        &replay.request_id,
        "storage_source_activate_v2",
    )?;
    if real_clone {
        // Triggers may alter authorization metadata while the source or replay
        // receipt is written. Recheck under the same transaction so marker
        // removal rolls back both writes instead of authorizing this commit.
        require_activation_clone_markers(&transaction)?;
    }
    transaction.commit()?;
    Ok(SourceActivationReceipt {
        lease,
        cursor,
        dirty_seq,
        disposition: "committed",
    })
}

fn cursor_value(value: &str) -> bool {
    value.len() <= MAX_CURSOR_CHARS && !value.chars().any(char::is_control)
}

fn revoke_source_guarded(
    db: &mut Connection,
    request: &v2::Request,
    expected_dirty_seq: i64,
    writer_lease: Option<&WriterLease>,
    replay: &v2::ReplayIdentity,
) -> Result<SourceRevocationReceipt> {
    revoke_source_guarded_mode(db, request, expected_dirty_seq, writer_lease, replay, false)
}

fn revoke_source_guarded_mode(
    db: &mut Connection,
    request: &v2::Request,
    expected_dirty_seq: i64,
    writer_lease: Option<&WriterLease>,
    replay: &v2::ReplayIdentity,
    real_clone: bool,
) -> Result<SourceRevocationReceipt> {
    let v2::Operation::RevokeSource {
        lease,
        cursor,
        expected_dirty_seq: request_dirty_seq,
    } = &request.operation
    else {
        return Err(reject("source revocation operation required"));
    };
    if *request_dirty_seq != expected_dirty_seq {
        return Err(reject("source revocation dirty sequence changed"));
    }
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(writer_lease) = writer_lease {
        writer_lease.verify_admission()?;
    }
    if real_clone {
        require_activation_clone_markers(&transaction)?;
    } else {
        require_scratch(&transaction)?;
    }
    let receiver: String = transaction.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if receiver != request.receiver_id || lease.receiver_id != receiver {
        return Err(reject("scratch receiver identity mismatch"));
    }

    reject_cross_operation_request_id(
        &transaction,
        &replay.request_id,
        "storage_source_revoke_v2",
    )?;

    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS storage_source_revoke_v2(\
         request_id TEXT PRIMARY KEY, receiver_id TEXT NOT NULL, request_digest TEXT NOT NULL,\
         source_id TEXT NOT NULL, cursor TEXT NOT NULL, dirty_seq INTEGER NOT NULL,\
         lease_epoch INTEGER NOT NULL, policy_epoch INTEGER NOT NULL, capture_enabled INTEGER NOT NULL)",
    )?;
    if real_clone {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS storage_admission_batch_binding_v1(\
             source_id TEXT PRIMARY KEY,generation TEXT NOT NULL,policy_epoch INTEGER NOT NULL,\
             expected_cursor TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
             prepared_digest TEXT NOT NULL,dirty_seq INTEGER NOT NULL)",
        )?;
        reject_real_clone_policy_triggers(&transaction)?;
    }
    type Stored = (String, String, String, String, i64, i64, i64, i64);
    let stored: Option<Stored> = transaction
        .query_row(
            "SELECT receiver_id,request_digest,source_id,cursor,dirty_seq,lease_epoch,policy_epoch,capture_enabled \
             FROM storage_source_revoke_v2 WHERE request_id=?",
            [&replay.request_id],
            |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?))
            },
        )
        .optional()?;
    if let Some((stored_receiver, digest, source, stored_cursor, dirty, epoch, policy, enabled)) =
        stored
    {
        if stored_receiver != receiver || digest != replay.digest || source != lease.source_id {
            return Err(reject("conflicting v2 source revocation request"));
        }
        if real_clone {
            let expected_dirty = request_dirty_seq
                .checked_add(1)
                .ok_or_else(|| reject("source revoke dirty sequence overflow"))?;
            let expected_policy = lease
                .policy_epoch
                .checked_add(1)
                .ok_or_else(|| reject("source revoke policy epoch overflow"))?;
            if stored_cursor != *cursor
                || dirty != expected_dirty
                || epoch == lease.epoch
                || policy != expected_policy
                || enabled != 0
            {
                return Err(reject("conflicting v2 source revocation receipt"));
            }
            validate_clone_policy_replay_source(
                &transaction,
                &lease.source_id,
                &receiver,
                &lease.plugin_id,
                &lease.generation,
                policy,
                dirty,
            )?;
        }
        transaction.commit()?;
        return Ok(SourceRevocationReceipt {
            cursor: stored_cursor,
            disposition: "replay",
            dirty_seq: dirty,
            lease_epoch: epoch,
            policy_epoch: policy,
            capture_enabled: enabled != 0,
        });
    }

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
        String,
        i64,
    );
    let source: Source = transaction.query_row(
        "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
         content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
         FROM plugin_source_cursor_v1 WHERE source_id=?",
        [&lease.source_id],
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
        source_cursor,
        prior_cursor,
        last_digest,
        dirty_seq,
    ) = source;
    if source_receiver != receiver
        || plugin != lease.plugin_id
        || generation != lease.generation
        || epoch != lease.epoch
        || policy != lease.policy_epoch
        || enabled != 1
        || !fingerprint.is_empty()
        || source_cursor != *cursor
        || dirty_seq != expected_dirty_seq
    {
        return Err(reject("stale or unsupported source revocation lease"));
    }
    if real_clone {
        validate_active_source_history(
            &transaction,
            &lease.source_id,
            &source_cursor,
            dirty_seq,
            policy,
            true,
        )?;
        preflight_clone_policy_deletes(&transaction, &lease.source_id)?;
    } else {
        let unsupported: i64 = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_content_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_retention_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_reference_owner_v1 WHERE source_id=?1) \
             OR EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1 \
                       AND (revision<>0 OR kind<>'lifecycle' OR member_id<>''))",
            [&lease.source_id],
            |row| row.get(0),
        )?;
        if unsupported != 0 {
            return Err(reject(
                "scratch source has unsupported or privately owned history",
            ));
        }
    }
    let next_epoch = epoch
        .checked_add(1)
        .ok_or_else(|| reject("lease epoch overflow"))?;
    let next_policy = policy
        .checked_add(1)
        .ok_or_else(|| reject("policy epoch overflow"))?;
    let next_dirty = dirty_seq
        .checked_add(1)
        .ok_or_else(|| reject("dirty sequence overflow"))?;
    let updated = now_ns()?;
    transaction.execute(
        "DELETE FROM plugin_evidence_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM plugin_session_projection_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM plugin_session_member_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM plugin_content_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    if !real_clone {
        transaction.execute(
            "DELETE FROM plugin_content_batch_v1 WHERE source_id=?",
            [&lease.source_id],
        )?;
        transaction.execute(
            "DELETE FROM plugin_content_status_v1 WHERE source_id=?",
            [&lease.source_id],
        )?;
    }
    transaction.execute(
        "DELETE FROM plugin_retention_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM plugin_retention_invalidated_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM plugin_source_gap_invalidated_v1 WHERE source_id=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
        [&lease.source_id],
    )?;
    transaction.execute(
        "DELETE FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=?",
        [&lease.source_id],
    )?;
    let source_changed = transaction.execute(
        "UPDATE plugin_source_cursor_v1 SET lease_epoch=?,policy_epoch=?,capture_enabled=0,\
         content_fingerprint='',dirty_seq=?,updated_ns=? WHERE source_id=?",
        params![
            next_epoch,
            next_policy,
            next_dirty,
            updated,
            lease.source_id
        ],
    )?;
    if source_changed != 1 {
        return Err(reject(
            "source revoke update did not affect exactly one row",
        ));
    }
    if real_clone {
        let deleted = transaction.execute(
            "DELETE FROM storage_admission_batch_binding_v1 WHERE source_id=?",
            [&lease.source_id],
        )?;
        if deleted > 1 {
            return Err(reject("prepared batch binding cleanup was not unique"));
        }
        set_metadata_content_receipts_mode(
            &transaction,
            &lease.source_id,
            cursor,
            next_policy,
            true,
        )?;
    }
    transaction.execute(
        "INSERT INTO plugin_evidence_dirty_v1(source_id,dirty_seq) VALUES(?,?) \
         ON CONFLICT(source_id) DO UPDATE SET dirty_seq=excluded.dirty_seq",
        params![lease.source_id, next_dirty],
    )?;
    transaction.execute(
        "INSERT INTO plugin_projection_cursor_v1(source_id,projected_seq,policy_epoch) VALUES(?,?,?) \
         ON CONFLICT(source_id) DO UPDATE SET projected_seq=excluded.projected_seq,policy_epoch=excluded.policy_epoch",
        params![lease.source_id, next_dirty, next_policy],
    )?;
    transaction.execute(
        "INSERT INTO storage_source_revoke_v2(request_id,receiver_id,request_digest,source_id,cursor,\
         dirty_seq,lease_epoch,policy_epoch,capture_enabled) VALUES(?,?,?,?,?,?,?,?,0)",
        params![replay.request_id, receiver, replay.digest, lease.source_id, cursor,
                next_dirty, next_epoch, next_policy],
    )?;
    if real_clone {
        validate_disabled_capture_state(
            &transaction,
            &lease.source_id,
            cursor,
            next_dirty,
            next_policy,
            real_clone,
        )?;
        if read_batch_binding(&transaction, &lease.source_id)?.is_some() {
            return Err(reject("source revoke retained prepared batch binding"));
        }
        let persisted: Option<Source> = transaction
            .query_row(
                "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
                 content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
                 FROM plugin_source_cursor_v1 WHERE source_id=?",
                [&lease.source_id],
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
            )
            .optional()?;
        let expected: Source = (
            receiver.clone(),
            plugin.clone(),
            generation.clone(),
            next_epoch,
            next_policy,
            0,
            String::new(),
            cursor.clone(),
            prior_cursor,
            last_digest,
            next_dirty,
        );
        if persisted.as_ref() != Some(&expected) {
            return Err(reject("source revoke state was not persisted exactly"));
        }
        let stored_receipt: Option<Stored> = transaction
            .query_row(
                "SELECT receiver_id,request_digest,source_id,cursor,dirty_seq,lease_epoch,policy_epoch,capture_enabled \
                 FROM storage_source_revoke_v2 WHERE request_id=?",
                [&replay.request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?)),
            )
            .optional()?;
        let expected_receipt: Stored = (
            receiver.clone(),
            replay.digest.clone(),
            lease.source_id.clone(),
            cursor.clone(),
            next_dirty,
            next_epoch,
            next_policy,
            0,
        );
        if stored_receipt.as_ref() != Some(&expected_receipt) {
            return Err(reject("source revoke receipt was not persisted exactly"));
        }
        require_activation_clone_markers(&transaction)?;
    }
    transaction.commit()?;
    Ok(SourceRevocationReceipt {
        cursor: cursor.clone(),
        disposition: "committed",
        dirty_seq: next_dirty,
        lease_epoch: next_epoch,
        policy_epoch: next_policy,
        capture_enabled: false,
    })
}

fn validate_disabled_capture_state(
    db: &Connection,
    source_id: &str,
    cursor: &str,
    dirty_seq: i64,
    policy_epoch: i64,
    real_clone: bool,
) -> Result<()> {
    require_no_provider_reference_rows(db, source_id)?;
    let unsupported: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_reference_owner_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_content_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_source_gap_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_retention_invalidated_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_session_member_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM plugin_session_projection_v1 WHERE source_id=?1) \
         OR EXISTS(SELECT 1 FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?1) \
         OR EXISTS(SELECT 1 FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=?1)",
        [source_id],
        |row| row.get(0),
    )?;
    if unsupported {
        return Err(reject(
            "disabled scratch source has private, unsupported, or residual history",
        ));
    }
    if real_clone {
        validate_member_history(db, source_id)?;
    } else {
        let has_member_rows: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_source_member_cursor_v1 WHERE source_id=?)",
            [source_id],
            |row| row.get(0),
        )?;
        if has_member_rows {
            return Err(reject(
                "scratch source has unsupported member cursor history",
            ));
        }
    }
    let evidence_dirty: Option<i64> = db
        .query_row(
            "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    if evidence_dirty != Some(dirty_seq) {
        return Err(reject("disabled source dirty marker is inconsistent"));
    }
    let projection: Option<(i64, i64)> = db
        .query_row(
            "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 WHERE source_id=?",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if projection != Some((dirty_seq, policy_epoch)) {
        return Err(reject("disabled source projection marker is inconsistent"));
    }
    validate_metadata_content_receipts(db, source_id, cursor, policy_epoch, true)
}

fn require_no_provider_reference_rows(db: &Connection, source_id: &str) -> Result<()> {
    for table in ["plugin_command_content_v1", "plugin_command_batch_v1"] {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            [table],
            |row| row.get(0),
        )?;
        if exists {
            let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE source_id=?)");
            if db.query_row(&query, [source_id], |row| row.get::<_, bool>(0))? {
                return Err(reject(
                    "source has unsupported provider-owned reference history",
                ));
            }
        }
    }
    Ok(())
}

fn validate_metadata_content_receipts(
    db: &Connection,
    source_id: &str,
    cursor: &str,
    policy_epoch: i64,
    allow_older_policy: bool,
) -> Result<()> {
    let batch: Option<(String, String)> = db
        .query_row(
            "SELECT cursor,refs_digest FROM plugin_content_batch_v1 WHERE source_id=?",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let status: Option<(String, String, i64, i64)> = db
        .query_row(
            "SELECT cursor,reason,refs_count,policy_epoch FROM plugin_content_status_v1 \
             WHERE source_id=?",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    match (batch, status) {
        (None, None) => Ok(()),
        (
            Some((batch_cursor, refs_digest)),
            Some((status_cursor, reason, refs_count, status_policy)),
        ) => {
            let expected_refs = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
            let policy_valid = if allow_older_policy {
                status_policy > 0 && status_policy <= policy_epoch
            } else {
                status_policy == policy_epoch
            };
            if batch_cursor != cursor
                || status_cursor != cursor
                || refs_digest != expected_refs
                || reason != "not_requested"
                || refs_count != 0
                || !policy_valid
            {
                return Err(reject(
                    "scratch source has nonempty or inconsistent content receipts",
                ));
            }
            Ok(())
        }
        _ => Err(reject(
            "scratch source has an incomplete content receipt pair",
        )),
    }
}

fn set_metadata_content_receipts_mode(
    transaction: &Transaction<'_>,
    source_id: &str,
    cursor: &str,
    policy_epoch: i64,
    record_empty_initial: bool,
) -> Result<()> {
    let batch_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_content_batch_v1 WHERE source_id=?)",
        [source_id],
        |row| row.get(0),
    )?;
    let status_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_content_status_v1 WHERE source_id=?)",
        [source_id],
        |row| row.get(0),
    )?;
    if batch_exists != status_exists {
        return Err(reject(
            "scratch source has an incomplete content receipt pair",
        ));
    }
    if !batch_exists && cursor.is_empty() && !record_empty_initial {
        return Ok(());
    }
    if batch_exists {
        let changed = transaction.execute(
            "UPDATE plugin_content_status_v1 SET policy_epoch=? WHERE source_id=?",
            params![policy_epoch, source_id],
        )?;
        if changed != 1 {
            return Err(reject(
                "content status update did not affect exactly one row",
            ));
        }
    } else {
        let refs_digest = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
        let batch_changed = transaction.execute(
            "INSERT INTO plugin_content_batch_v1(source_id,cursor,refs_digest) VALUES(?,?,?)",
            params![source_id, cursor, refs_digest],
        )?;
        if batch_changed != 1 {
            return Err(reject(
                "content batch insert did not affect exactly one row",
            ));
        }
        let status_changed = transaction.execute(
            "INSERT INTO plugin_content_status_v1(source_id,cursor,reason,refs_count,policy_epoch) \
             VALUES(?,?,'not_requested',0,?)",
            params![source_id, cursor, policy_epoch],
        )?;
        if status_changed != 1 {
            return Err(reject(
                "content status insert did not affect exactly one row",
            ));
        }
    }
    validate_metadata_content_receipts(transaction, source_id, cursor, policy_epoch, false)
}

fn source_marker_state(db: &Connection, source_id: &str) -> Result<SourceMarkerState> {
    let dirty = db
        .query_row(
            "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id=?",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    let projection = db
        .query_row(
            "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 WHERE source_id=?",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok((dirty, projection))
}

fn set_source_capture_guarded(
    db: &mut Connection,
    request: &v2::Request,
    writer_lease: Option<&WriterLease>,
    replay: &v2::ReplayIdentity,
) -> Result<SourceCaptureSetReceipt> {
    set_source_capture_guarded_mode(db, request, writer_lease, replay, false)
}

fn set_source_capture_guarded_mode(
    db: &mut Connection,
    request: &v2::Request,
    writer_lease: Option<&WriterLease>,
    replay: &v2::ReplayIdentity,
    real_clone: bool,
) -> Result<SourceCaptureSetReceipt> {
    let v2::Operation::SetSourceCapture {
        lease,
        enabled,
        expected_cursor,
        expected_dirty_seq,
        expected_capture_enabled,
    } = &request.operation
    else {
        return Err(reject("source capture update operation required"));
    };
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(writer_lease) = writer_lease {
        writer_lease.verify_admission()?;
    }
    if real_clone {
        require_activation_clone_markers(&transaction)?;
    } else {
        require_scratch(&transaction)?;
    }
    let receiver: String = transaction.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if receiver != request.receiver_id || lease.receiver_id != receiver {
        return Err(reject("scratch receiver identity mismatch"));
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS storage_source_capture_set_v2(\
         request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
         source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
         expected_capture_enabled INTEGER NOT NULL,enabled INTEGER NOT NULL,disposition TEXT NOT NULL,\
         lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
         dirty_seq INTEGER NOT NULL)",
    )?;
    if real_clone {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS storage_admission_batch_binding_v1(\
             source_id TEXT PRIMARY KEY,generation TEXT NOT NULL,policy_epoch INTEGER NOT NULL,\
             expected_cursor TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
             prepared_digest TEXT NOT NULL,dirty_seq INTEGER NOT NULL)",
        )?;
        reject_real_clone_policy_triggers(&transaction)?;
    } else {
        let has_receipt_trigger: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' \
             AND tbl_name='storage_source_capture_set_v2')",
            [],
            |row| row.get(0),
        )?;
        if has_receipt_trigger {
            return Err(reject("source capture receipt triggers are unsupported"));
        }
    }
    reject_cross_operation_request_id(
        &transaction,
        &replay.request_id,
        "storage_source_capture_set_v2",
    )?;
    type Stored = (
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
        String,
        i64,
        i64,
        String,
        i64,
    );
    let stored: Option<Stored> = transaction
        .query_row(
            "SELECT receiver_id,request_digest,source_id,plugin_id,generation,\
             expected_capture_enabled,enabled,disposition,lease_epoch,policy_epoch,cursor,dirty_seq \
             FROM storage_source_capture_set_v2 WHERE request_id=?",
            [&replay.request_id],
            |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?,
                ))
            },
        )
        .optional()?;
    if let Some((
        stored_receiver,
        digest,
        source_id,
        plugin_id,
        generation,
        stored_expected,
        stored_enabled,
        stored_disposition,
        epoch,
        policy,
        cursor,
        dirty_seq,
    )) = stored
    {
        let expected_disposition = if enabled == expected_capture_enabled {
            "idle"
        } else {
            "committed"
        };
        let expected_policy = if enabled == expected_capture_enabled {
            lease.policy_epoch
        } else {
            lease.policy_epoch.checked_add(1).unwrap_or_default()
        };
        let expected_dirty = if enabled == expected_capture_enabled {
            *expected_dirty_seq
        } else {
            expected_dirty_seq.checked_add(1).unwrap_or_default()
        };
        if stored_receiver != receiver
            || digest != replay.digest
            || source_id != lease.source_id
            || plugin_id != lease.plugin_id
            || generation != lease.generation
            || stored_expected != i64::from(*expected_capture_enabled)
            || stored_enabled != i64::from(*enabled)
            || stored_disposition != expected_disposition
            || epoch <= 0
            || epoch == lease.epoch && expected_disposition == "committed"
            || epoch != lease.epoch && expected_disposition == "idle"
            || policy != expected_policy
            || cursor != *expected_cursor
            || dirty_seq != expected_dirty
        {
            return Err(reject("conflicting v2 source capture request"));
        }
        if real_clone {
            validate_clone_policy_replay_source(
                &transaction,
                &lease.source_id,
                &receiver,
                &lease.plugin_id,
                &lease.generation,
                policy,
                dirty_seq,
            )?;
        }
        transaction.commit()?;
        return Ok(SourceCaptureSetReceipt {
            lease: v2::SourceLease {
                receiver_id: receiver,
                plugin_id: lease.plugin_id.clone(),
                source_id: lease.source_id.clone(),
                generation: lease.generation.clone(),
                epoch,
                policy_epoch: policy,
            },
            enabled: *enabled,
            cursor,
            dirty_seq,
            disposition: "replay",
        });
    }

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
        String,
        i64,
    );
    let source: Source = transaction.query_row(
        "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
         content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
         FROM plugin_source_cursor_v1 WHERE source_id=?",
        [&lease.source_id],
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
        current_epoch,
        current_policy,
        current_enabled,
        fingerprint,
        cursor,
        prior_cursor,
        last_digest,
        dirty_seq,
    ) = source;
    if !matches!(current_enabled, 0 | 1) {
        return Err(reject("scratch source capture state is malformed"));
    }
    let current_enabled = current_enabled == 1;
    if source_receiver != receiver
        || plugin != lease.plugin_id
        || generation != lease.generation
        || current_epoch != lease.epoch
        || current_policy != lease.policy_epoch
        || current_enabled != *expected_capture_enabled
        || cursor != *expected_cursor
        || dirty_seq != *expected_dirty_seq
        || !fingerprint.is_empty()
    {
        return Err(reject("stale or unsupported source capture lease"));
    }
    if current_epoch <= 0
        || current_policy <= 0
        || dirty_seq < 0
        || !cursor_value(&cursor)
        || !cursor_value(&prior_cursor)
        || (!last_digest.is_empty() && !hash(&last_digest))
    {
        return Err(reject("scratch source state is malformed"));
    }
    if current_enabled {
        validate_active_source_history(
            &transaction,
            &lease.source_id,
            &cursor,
            dirty_seq,
            current_policy,
            real_clone,
        )?;
    } else {
        validate_disabled_capture_state(
            &transaction,
            &lease.source_id,
            &cursor,
            dirty_seq,
            current_policy,
            real_clone,
        )?;
    }
    let previous_markers = source_marker_state(&transaction, &lease.source_id)?;
    let binding_before = if real_clone {
        read_batch_binding(&transaction, &lease.source_id)?
    } else {
        None
    };

    let is_noop = *enabled == current_enabled;
    let (next_epoch, next_policy, next_dirty, disposition) = if is_noop {
        (current_epoch, current_policy, dirty_seq, "idle")
    } else {
        let next_policy = current_policy
            .checked_add(1)
            .ok_or_else(|| reject("policy epoch overflow"))?;
        let next_dirty = dirty_seq
            .checked_add(1)
            .ok_or_else(|| reject("dirty sequence overflow"))?;
        let next_epoch = fresh_lease_epoch(Some(current_epoch))?;
        (next_epoch, next_policy, next_dirty, "committed")
    };
    let expected_markers = if is_noop {
        previous_markers
    } else {
        (Some(next_dirty), Some((next_dirty, next_policy)))
    };
    if !is_noop {
        if !enabled {
            if real_clone {
                preflight_clone_policy_deletes(&transaction, &lease.source_id)?;
            }
            transaction.execute(
                "DELETE FROM plugin_evidence_v1 WHERE source_id=?",
                [&lease.source_id],
            )?;
            transaction.execute(
                "DELETE FROM plugin_session_projection_v1 WHERE source_id=?",
                [&lease.source_id],
            )?;
            transaction.execute(
                "DELETE FROM plugin_session_member_v1 WHERE source_id=?",
                [&lease.source_id],
            )?;
            transaction.execute(
                "DELETE FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?",
                [&lease.source_id],
            )?;
            transaction.execute(
                "DELETE FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=?",
                [&lease.source_id],
            )?;
        }
        set_metadata_content_receipts_mode(
            &transaction,
            &lease.source_id,
            &cursor,
            next_policy,
            real_clone,
        )?;
        if real_clone {
            let deleted = transaction.execute(
                "DELETE FROM storage_admission_batch_binding_v1 WHERE source_id=?",
                [&lease.source_id],
            )?;
            if deleted > 1 {
                return Err(reject("prepared batch binding cleanup was not unique"));
            }
        }
        let changed = transaction.execute(
            "UPDATE plugin_source_cursor_v1 SET lease_epoch=?,policy_epoch=?,capture_enabled=?,\
             dirty_seq=?,content_fingerprint='',updated_ns=? WHERE source_id=?",
            params![
                next_epoch,
                next_policy,
                i64::from(*enabled),
                next_dirty,
                now_ns()?,
                lease.source_id
            ],
        )?;
        if changed != 1 {
            return Err(reject(
                "source capture update did not affect exactly one row",
            ));
        }
        let dirty_changed = transaction.execute(
            "INSERT INTO plugin_evidence_dirty_v1(source_id,dirty_seq) VALUES(?,?) \
             ON CONFLICT(source_id) DO UPDATE SET dirty_seq=excluded.dirty_seq",
            params![lease.source_id, next_dirty],
        )?;
        if dirty_changed != 1 {
            return Err(reject(
                "source dirty marker update did not affect exactly one row",
            ));
        }
        let projection_changed = transaction.execute(
            "INSERT INTO plugin_projection_cursor_v1(source_id,projected_seq,policy_epoch) \
             VALUES(?,?,?) ON CONFLICT(source_id) DO UPDATE SET \
             projected_seq=excluded.projected_seq,policy_epoch=excluded.policy_epoch",
            params![lease.source_id, next_dirty, next_policy],
        )?;
        if projection_changed != 1 {
            return Err(reject(
                "source projection marker update did not affect exactly one row",
            ));
        }
    }
    let resulting_lease = v2::SourceLease {
        receiver_id: receiver.clone(),
        plugin_id: plugin.clone(),
        source_id: lease.source_id.clone(),
        generation: generation.clone(),
        epoch: next_epoch,
        policy_epoch: next_policy,
    };
    let expected_source: Source = (
        receiver.clone(),
        plugin,
        generation,
        next_epoch,
        next_policy,
        i64::from(*enabled),
        String::new(),
        cursor.clone(),
        prior_cursor,
        last_digest,
        next_dirty,
    );
    let validation_mode = CaptureValidationMode {
        require_empty_lifecycle: !*expected_capture_enabled || !*enabled,
        allow_plugin_facts: real_clone,
    };
    validate_capture_set_persisted_state(
        &transaction,
        &lease.source_id,
        &expected_source,
        &expected_markers,
        &cursor,
        *enabled,
        validation_mode,
    )?;
    if real_clone {
        let expected_binding = if is_noop {
            binding_before.clone()
        } else {
            None
        };
        if read_batch_binding(&transaction, &lease.source_id)? != expected_binding {
            return Err(reject(
                "source capture changed the prepared batch binding unexpectedly",
            ));
        }
    }
    let receipt_changed = transaction.execute(
        "INSERT INTO storage_source_capture_set_v2(request_id,receiver_id,request_digest,source_id,\
         plugin_id,generation,expected_capture_enabled,enabled,disposition,lease_epoch,policy_epoch,\
         cursor,dirty_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            replay.request_id,
            receiver,
            replay.digest,
            lease.source_id,
            lease.plugin_id,
            lease.generation,
            i64::from(*expected_capture_enabled),
            i64::from(*enabled),
            disposition,
            next_epoch,
            next_policy,
            cursor,
            next_dirty
        ],
    )?;
    if receipt_changed != 1 {
        return Err(reject(
            "source capture receipt insert did not affect exactly one row",
        ));
    }
    let persisted_receipt: Option<Stored> = transaction
        .query_row(
            "SELECT receiver_id,request_digest,source_id,plugin_id,generation,\
             expected_capture_enabled,enabled,disposition,lease_epoch,policy_epoch,cursor,dirty_seq \
             FROM storage_source_capture_set_v2 WHERE request_id=?",
            [&replay.request_id],
            |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?,
                ))
            },
        )
        .optional()?;
    let expected_receipt: Stored = (
        receiver.clone(),
        replay.digest.clone(),
        lease.source_id.clone(),
        lease.plugin_id.clone(),
        lease.generation.clone(),
        i64::from(*expected_capture_enabled),
        i64::from(*enabled),
        disposition.to_owned(),
        next_epoch,
        next_policy,
        cursor.clone(),
        next_dirty,
    );
    if persisted_receipt.as_ref() != Some(&expected_receipt) {
        return Err(reject("source capture receipt was not persisted exactly"));
    }
    let validation_mode = CaptureValidationMode {
        require_empty_lifecycle: !*expected_capture_enabled || !*enabled,
        allow_plugin_facts: real_clone,
    };
    validate_capture_set_persisted_state(
        &transaction,
        &lease.source_id,
        &expected_source,
        &expected_markers,
        &cursor,
        *enabled,
        validation_mode,
    )?;
    if real_clone {
        let expected_binding = if is_noop { binding_before } else { None };
        if read_batch_binding(&transaction, &lease.source_id)? != expected_binding {
            return Err(reject(
                "source capture changed the prepared batch binding unexpectedly",
            ));
        }
    }
    reject_cross_operation_request_id(
        &transaction,
        &replay.request_id,
        "storage_source_capture_set_v2",
    )?;
    if real_clone {
        require_activation_clone_markers(&transaction)?;
    } else {
        require_scratch(&transaction)?;
    }
    let receiver_after_mutation: String = transaction.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if receiver_after_mutation != receiver {
        return Err(reject(
            "scratch receiver identity changed during capture update",
        ));
    }
    transaction.commit()?;
    Ok(SourceCaptureSetReceipt {
        lease: resulting_lease,
        enabled: *enabled,
        cursor,
        dirty_seq: next_dirty,
        disposition,
    })
}

struct CaptureValidationMode {
    require_empty_lifecycle: bool,
    allow_plugin_facts: bool,
}

fn validate_capture_set_persisted_state(
    db: &Connection,
    source_id: &str,
    expected_source: &(
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        String,
        String,
        i64,
    ),
    expected_markers: &SourceMarkerState,
    cursor: &str,
    enabled: bool,
    mode: CaptureValidationMode,
) -> Result<()> {
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
        String,
        i64,
    );
    let actual: Option<Source> = db
        .query_row(
            "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
             content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
             FROM plugin_source_cursor_v1 WHERE source_id=?",
            [source_id],
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
        )
        .optional()?;
    if actual.as_ref() != Some(expected_source) {
        return Err(reject("source capture state was not persisted exactly"));
    }
    let markers = source_marker_state(db, source_id)?;
    if &markers != expected_markers {
        return Err(reject(
            "source capture projection markers changed unexpectedly",
        ));
    }
    if enabled {
        validate_active_source_history(
            db,
            source_id,
            cursor,
            expected_source.10,
            expected_source.4,
            mode.allow_plugin_facts,
        )?;
        if mode.require_empty_lifecycle {
            let residual: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM plugin_evidence_v1 WHERE source_id=?1) \
                 OR EXISTS(SELECT 1 FROM plugin_session_projection_v1 WHERE source_id=?1) \
                 OR EXISTS(SELECT 1 FROM activity_index_v1 WHERE kind='plugin' AND owner_key=?1) \
                 OR EXISTS(SELECT 1 FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key=?1)",
                [source_id],
                |row| row.get(0),
            )?;
            if residual {
                return Err(reject(
                    "capture enable unexpectedly restored lifecycle evidence",
                ));
            }
        }
    } else {
        validate_disabled_capture_state(
            db,
            source_id,
            cursor,
            expected_source.10,
            expected_source.4,
            mode.allow_plugin_facts,
        )?;
    }
    if enabled != (expected_source.5 == 1) {
        return Err(reject(
            "source capture enabled state disagrees with persisted row",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    type TestSourceState = (
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        String,
        String,
        i64,
    );

    fn fixture() -> Connection {
        fixture_at(Path::new(":memory:"))
    }

    fn fixture_at(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
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
             CREATE TABLE plugin_reference_owner_v1(source_id TEXT,provider_id TEXT); \
             CREATE TABLE plugin_evidence_v1( \
               source_id TEXT,event_id_hash TEXT,revision INTEGER,kind TEXT,time_ns INTEGER, \
               payload_json TEXT,record_digest TEXT,native_session_id TEXT,native_worker_id TEXT, \
               native_turn_id TEXT,native_operation_id TEXT,member_id TEXT,policy_epoch INTEGER, \
               admitted_ns INTEGER,PRIMARY KEY(source_id,event_id_hash,revision)); \
             CREATE TABLE plugin_evidence_dirty_v1(source_id TEXT PRIMARY KEY,dirty_seq INTEGER); \
             CREATE TABLE plugin_session_projection_v1(source_id TEXT,native_session_id TEXT, \
               policy_epoch INTEGER,source_dirty_seq INTEGER); \
             CREATE TABLE plugin_projection_cursor_v1(source_id TEXT PRIMARY KEY,projected_seq INTEGER,policy_epoch INTEGER); \
             CREATE TABLE activity_index_v1(event_key TEXT,kind TEXT,owner_key TEXT); \
             CREATE TABLE investigation_dirty_v1(kind TEXT,owner_key TEXT); \
             CREATE TABLE plugin_source_member_cursor_v1( \
               source_id TEXT,member_id TEXT,cursor TEXT,PRIMARY KEY(source_id,member_id)); \
             CREATE TABLE plugin_content_v1(source_id TEXT); \
             CREATE TABLE plugin_source_gap_invalidated_v1(source_id TEXT); \
             CREATE TABLE plugin_retention_v1(source_id TEXT); \
             CREATE TABLE plugin_retention_invalidated_v1(source_id TEXT); \
             CREATE TABLE plugin_session_member_v1( \
               source_id TEXT,native_session_id TEXT,member_id TEXT, \
               PRIMARY KEY(source_id,native_session_id)); \
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

    #[test]
    fn prepared_rows_digest_is_stable_and_binds_order_and_fields() {
        let row = PreparedRecord {
            event_id_hash: "1".repeat(64),
            revision: 0,
            kind: "message".to_owned(),
            time_ns: 10,
            payload_json: "{\"role\":\"assistant\"}".to_owned(),
            record_digest: "2".repeat(64),
            native_session_id: "session-one".to_owned(),
            native_worker_id: String::new(),
            native_turn_id: String::new(),
            native_operation_id: String::new(),
            member_id: String::new(),
        };
        let second = PreparedRecord {
            event_id_hash: "3".repeat(64),
            native_session_id: "session-two".to_owned(),
            ..row.clone()
        };
        let baseline = prepared_rows_digest(&[row.clone(), second.clone()]).unwrap();
        assert_eq!(
            baseline,
            prepared_rows_digest(&[row.clone(), second.clone()]).unwrap()
        );
        assert_ne!(
            baseline,
            prepared_rows_digest(&[second.clone(), row.clone()]).unwrap()
        );
        let mut changed = row.clone();
        changed.native_session_id = "different-session".to_owned();
        assert_ne!(baseline, prepared_rows_digest(&[changed, second]).unwrap());
    }

    #[test]
    fn prepared_state_digest_v2_binds_effective_member_and_empty_updates() {
        let row = PreparedRecord {
            event_id_hash: "1".repeat(64),
            revision: 0,
            kind: "usage".to_owned(),
            time_ns: 10,
            payload_json: "{\"input_tokens\":1}".to_owned(),
            record_digest: "2".repeat(64),
            native_session_id: String::new(),
            native_worker_id: String::new(),
            native_turn_id: String::new(),
            native_operation_id: String::new(),
            member_id: "a".repeat(64),
        };
        let update = v2::MemberCursor {
            member_id: "a".repeat(64),
            cursor: "member-one".to_owned(),
        };
        let first = prepared_state_digest(std::slice::from_ref(&row), Some(&update)).unwrap();
        assert_eq!(
            first,
            prepared_state_digest(std::slice::from_ref(&row), Some(&update)).unwrap()
        );
        let mut changed_row = row.clone();
        changed_row.member_id = "b".repeat(64);
        assert_ne!(
            first,
            prepared_state_digest(&[changed_row], Some(&update)).unwrap()
        );
        let changed_cursor = v2::MemberCursor {
            cursor: "member-two".to_owned(),
            ..update.clone()
        };
        assert_ne!(
            first,
            prepared_state_digest(std::slice::from_ref(&row), Some(&changed_cursor)).unwrap()
        );
        assert_ne!(first, prepared_state_digest(&[], Some(&update)).unwrap());
        assert_ne!(
            prepared_state_digest(&[], Some(&update)).unwrap(),
            prepared_state_digest(&[], Some(&changed_cursor)).unwrap()
        );
        assert_eq!(
            prepared_state_digest(&[], None).unwrap(),
            (1, prepared_rows_digest(&[]).unwrap())
        );
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
    fn storage_cursor_accepts_exact_shared_limit_and_rejects_overflow() {
        assert!(cursor_value(&"c".repeat(MAX_CURSOR_CHARS)));
        assert!(!cursor_value(&"c".repeat(MAX_CURSOR_CHARS + 1)));
        assert!(!cursor_value(&format!(
            "{}\n",
            "c".repeat(MAX_CURSOR_CHARS - 1)
        )));
    }

    fn activation_source_state(db: &Connection, source_id: &str) -> Option<TestSourceState> {
        db.query_row(
            "SELECT receiver_id,plugin_id,generation,lease_epoch,policy_epoch,capture_enabled,\
             content_fingerprint,cursor,prior_cursor,last_batch_digest,dirty_seq \
             FROM plugin_source_cursor_v1 WHERE source_id=?",
            [source_id],
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
        )
        .optional()
        .unwrap()
    }

    fn activation_receipt_count(db: &Connection, request_id: &str) -> Option<i64> {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' \
                 AND name='storage_source_activate_v2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        exists.then(|| {
            db.query_row(
                "SELECT COUNT(*) FROM storage_source_activate_v2 WHERE request_id=?",
                [request_id],
                |row| row.get(0),
            )
            .unwrap()
        })
    }

    fn admission_receipt_count(db: &Connection, request_id: &str) -> Option<i64> {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' \
                 AND name='storage_admission_request_v2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        exists.then(|| {
            db.query_row(
                "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                [request_id],
                |row| row.get(0),
            )
            .unwrap()
        })
    }

    fn revoke_request(expected_dirty_seq: i64) -> v2::Request {
        v2::Request {
            version: v2::VERSION,
            request_id: "1123456789abcdef0123456789abcdef".to_owned(),
            receiver_id: "receiver-one".to_owned(),
            operation: v2::Operation::RevokeSource {
                lease: v2::SourceLease {
                    receiver_id: "receiver-one".to_owned(),
                    plugin_id: "fixture.plugin".to_owned(),
                    source_id: "source-one".to_owned(),
                    generation: "a".repeat(64),
                    epoch: 7,
                    policy_epoch: 1,
                },
                cursor: "next".to_owned(),
                expected_dirty_seq,
            },
        }
    }

    fn activation_request(request_id: &str, source_id: &str) -> v2::Request {
        v2::Request {
            version: v2::VERSION,
            request_id: request_id.to_owned(),
            receiver_id: "receiver-one".to_owned(),
            operation: v2::Operation::ActivateSource {
                plugin_id: "fixture.plugin".to_owned(),
                source_id: source_id.to_owned(),
                generation: "a".repeat(64),
            },
        }
    }

    fn lifecycle_v2_request(
        request_id: &str,
        lease: &v2::SourceLease,
        expected_cursor: &str,
        cursor: &str,
    ) -> v2::Request {
        v2::Request {
            version: v2::VERSION,
            request_id: request_id.to_owned(),
            receiver_id: lease.receiver_id.clone(),
            operation: v2::Operation::AdmitLifecycle {
                lease: lease.clone(),
                expected_cursor: expected_cursor.to_owned(),
                cursor: cursor.to_owned(),
                batch_digest: "d".repeat(64),
                records: vec![],
            },
        }
    }

    fn activate(db: &mut Connection, request: &v2::Request) -> Result<SourceActivationReceipt> {
        let replay = request.replay_identity()?;
        activate_source_guarded(db, request, None, &replay, false)
    }

    fn capture_request(
        request_id: &str,
        lease: &v2::SourceLease,
        enabled: bool,
        expected_capture_enabled: bool,
        expected_cursor: &str,
        expected_dirty_seq: i64,
    ) -> v2::Request {
        v2::Request {
            version: v2::VERSION,
            request_id: request_id.to_owned(),
            receiver_id: lease.receiver_id.clone(),
            operation: v2::Operation::SetSourceCapture {
                lease: lease.clone(),
                enabled,
                expected_cursor: expected_cursor.to_owned(),
                expected_dirty_seq,
                expected_capture_enabled,
            },
        }
    }

    fn set_capture(db: &mut Connection, request: &v2::Request) -> Result<SourceCaptureSetReceipt> {
        let replay = request.replay_identity()?;
        set_source_capture_guarded(db, request, None, &replay)
    }

    fn source_lease() -> v2::SourceLease {
        v2::SourceLease {
            receiver_id: "receiver-one".into(),
            plugin_id: "fixture.plugin".into(),
            source_id: "source-one".into(),
            generation: "a".repeat(64),
            epoch: 7,
            policy_epoch: 1,
        }
    }

    fn revoke(db: &mut Connection, request: &v2::Request) -> Result<SourceRevocationReceipt> {
        let replay = request.replay_identity()?;
        let v2::Operation::RevokeSource {
            expected_dirty_seq, ..
        } = &request.operation
        else {
            return Err(reject("source revocation operation required"));
        };
        revoke_source_guarded(db, request, *expected_dirty_seq, None, &replay)
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
    fn revoke_atomically_clears_core_projections_and_fences_old_admission() {
        let mut db = fixture();
        let event_hash = "1".repeat(64);
        let event_digest = "2".repeat(64);
        let first = request("next", "", &[(&event_hash, &event_digest)]);
        commit(&mut db, &first).unwrap();
        db.execute(
            "INSERT INTO plugin_session_projection_v1 VALUES('source-one','native-one',1,1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO activity_index_v1 VALUES('event-one','plugin','source-one')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO investigation_dirty_v1 VALUES('plugin','source-one')",
            [],
        )
        .unwrap();

        let revoke_req = revoke_request(1);
        let result = revoke(&mut db, &revoke_req).unwrap();
        assert_eq!(result.disposition, "committed");
        assert_eq!(
            (
                result.cursor.as_str(),
                result.dirty_seq,
                result.lease_epoch,
                result.policy_epoch,
                result.capture_enabled
            ),
            ("next", 2, 8, 2, false)
        );
        assert_eq!(state(&db), ("next".into(), 2, 0));
        let source: (i64, i64, i64, String) = db
            .query_row(
                "SELECT lease_epoch,policy_epoch,capture_enabled,content_fingerprint \
             FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(source, (8, 2, 0, String::new()));
        let dirty: i64 = db
            .query_row(
                "SELECT dirty_seq FROM plugin_evidence_dirty_v1 WHERE source_id='source-one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dirty, 2);
        let projection_cursor: (i64, i64) = db.query_row(
            "SELECT projected_seq,policy_epoch FROM plugin_projection_cursor_v1 WHERE source_id='source-one'",
            [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(projection_cursor, (2, 2));
        for (table, query) in [
            (
                "session projection",
                "SELECT COUNT(*) FROM plugin_session_projection_v1",
            ),
            (
                "Activity",
                "SELECT COUNT(*) FROM activity_index_v1 WHERE kind='plugin' AND owner_key='source-one'",
            ),
            (
                "investigation dirty",
                "SELECT COUNT(*) FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key='source-one'",
            ),
            (
                "content batch",
                "SELECT COUNT(*) FROM plugin_content_batch_v1 WHERE source_id='source-one'",
            ),
            (
                "content status",
                "SELECT COUNT(*) FROM plugin_content_status_v1 WHERE source_id='source-one'",
            ),
        ] {
            let count: i64 = db.query_row(query, [], |row| row.get(0)).unwrap();
            assert_eq!(count, 0, "{table} cleanup");
        }
        let stale = request("later", "next", &[]);
        assert!(
            commit(&mut db, &stale)
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        assert_eq!(state(&db), ("next".into(), 2, 0));
    }

    #[test]
    fn revoke_exact_retry_replays_once_and_same_id_conflicts() {
        let mut db = fixture();
        commit(&mut db, &request("next", "", &[])).unwrap();
        let mut request = revoke_request(0);
        assert_eq!(revoke(&mut db, &request).unwrap().disposition, "committed");
        let retry = revoke(&mut db, &request).unwrap();
        assert_eq!(retry.disposition, "replay");
        assert_eq!(
            (retry.dirty_seq, retry.lease_epoch, retry.policy_epoch),
            (1, 8, 2)
        );
        if let v2::Operation::RevokeSource { cursor, .. } = &mut request.operation {
            *cursor = "different".to_owned();
        }
        assert!(
            revoke(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        assert_eq!(state(&db), ("next".into(), 1, 0));
    }

    #[test]
    fn revoke_request_id_is_shared_with_lifecycle_admission_namespace() {
        let request_id = "2123456789abcdef0123456789abcdef".to_owned();
        let mut db = fixture();
        let admission = v2::Request {
            version: v2::VERSION,
            request_id: request_id.clone(),
            receiver_id: "receiver-one".into(),
            operation: v2::Operation::AdmitLifecycle {
                lease: v2::SourceLease {
                    receiver_id: "receiver-one".into(),
                    plugin_id: "fixture.plugin".into(),
                    source_id: "source-one".into(),
                    generation: "a".repeat(64),
                    epoch: 7,
                    policy_epoch: 1,
                },
                expected_cursor: "".into(),
                cursor: "next".into(),
                batch_digest: "d".repeat(64),
                records: vec![],
            },
        };
        let replay = admission.replay_identity().unwrap();
        let prepared = request("next", "", &[]);
        commit_guarded(&mut db, &prepared, None, Some(&replay)).unwrap();
        let mut revoke = revoke_request(0);
        revoke.request_id = request_id.clone();
        assert!(
            self::revoke(&mut db, &revoke)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        assert_eq!(state(&db), ("next".into(), 0, 0));

        let mut db = fixture();
        commit(&mut db, &request("next", "", &[])).unwrap();
        let mut revoke = revoke_request(0);
        revoke.request_id = request_id.clone();
        self::revoke(&mut db, &revoke).unwrap();
        let admission = v2::Request {
            version: v2::VERSION,
            request_id,
            receiver_id: "receiver-one".into(),
            operation: v2::Operation::AdmitLifecycle {
                lease: v2::SourceLease {
                    receiver_id: "receiver-one".into(),
                    plugin_id: "fixture.plugin".into(),
                    source_id: "source-one".into(),
                    generation: "a".repeat(64),
                    epoch: 7,
                    policy_epoch: 1,
                },
                expected_cursor: "next".into(),
                cursor: "later".into(),
                batch_digest: "e".repeat(64),
                records: vec![],
            },
        };
        let prepared = request("later", "next", &[]);
        let replay = admission.replay_identity().unwrap();
        let error = commit_guarded(&mut db, &prepared, None, Some(&replay)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("conflicting v2 request replay identity")
        );
        assert_eq!(state(&db), ("next".into(), 1, 0));
    }

    #[test]
    fn revoke_refuses_private_owner_and_rolls_back_everything() {
        let mut db = fixture();
        commit(&mut db, &request("next", "", &[])).unwrap();
        db.execute(
            "INSERT INTO plugin_reference_owner_v1 VALUES('source-one','private-provider')",
            [],
        )
        .unwrap();
        let before = state(&db);
        let error = revoke(&mut db, &revoke_request(0)).unwrap_err();
        assert!(error.to_string().contains("privately owned"));
        assert_eq!(state(&db), before);
        let enabled: i64 = db
            .query_row(
                "SELECT capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 1);
        let receipts: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='storage_source_revoke_v2'",
                                         [], |row| row.get(0)).unwrap();
        assert_eq!(
            receipts, 0,
            "rejected revoke must not persist a request receipt"
        );
    }

    #[test]
    fn revoke_late_failure_rolls_back_deletes_epochs_and_receipt() {
        let mut db = fixture();
        commit(&mut db, &request("next", "", &[])).unwrap();
        db.execute(
            "INSERT INTO plugin_session_projection_v1 VALUES('source-one','native-one',1,0)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO activity_index_v1 VALUES('event-one','plugin','source-one')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO investigation_dirty_v1 VALUES('plugin','source-one')",
            [],
        )
        .unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_revoke_projection BEFORE INSERT ON plugin_projection_cursor_v1 \
             BEGIN SELECT RAISE(ABORT,'injected revoke failure'); END;",
        )
        .unwrap();
        let error = revoke(&mut db, &revoke_request(0)).unwrap_err();
        assert!(error.to_string().contains("injected revoke failure"));
        assert_eq!(state(&db), ("next".into(), 0, 0));
        let source: (i64, i64, i64) = db.query_row(
            "SELECT lease_epoch,policy_epoch,capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(source, (7, 1, 1));
        for query in [
            "SELECT COUNT(*) FROM plugin_session_projection_v1",
            "SELECT COUNT(*) FROM activity_index_v1 WHERE kind='plugin' AND owner_key='source-one'",
            "SELECT COUNT(*) FROM investigation_dirty_v1 WHERE kind='plugin' AND owner_key='source-one'",
        ] {
            assert_eq!(
                db.query_row::<i64, _, _>(query, [], |row| row.get(0))
                    .unwrap(),
                1
            );
        }
        let receipt_table: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='storage_source_revoke_v2'",
            [], |row| row.get(0)).unwrap();
        assert_eq!(receipt_table, 0);
    }

    #[test]
    fn revoke_retry_after_database_reopen_returns_committed_receipt() {
        let path: PathBuf = std::env::temp_dir().join(format!(
            "agenturm-revoke-replay-{}-{}.db",
            std::process::id(),
            now_ns().unwrap()
        ));
        let mut db = fixture_at(&path);
        commit(&mut db, &request("next", "", &[])).unwrap();
        let request = revoke_request(0);
        assert_eq!(revoke(&mut db, &request).unwrap().disposition, "committed");
        drop(db);

        let mut reopened = Connection::open(&path).unwrap();
        let replay = revoke(&mut reopened, &request).unwrap();
        assert_eq!(replay.disposition, "replay");
        assert_eq!(
            (
                replay.cursor.as_str(),
                replay.dirty_seq,
                replay.lease_epoch,
                replay.policy_epoch,
                replay.capture_enabled
            ),
            ("next", 1, 8, 2, false)
        );
        assert_eq!(state(&reopened), ("next".into(), 1, 0));
        drop(reopened);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
    }

    #[test]
    fn concurrent_exact_revocations_commit_once_across_connections() {
        let path = std::env::temp_dir().join(format!(
            "agenturm-revoke-race-{}-{}.db",
            std::process::id(),
            now_ns().unwrap()
        ));
        let mut db = fixture_at(&path);
        commit(&mut db, &request("next", "", &[])).unwrap();
        drop(db);

        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            let request = revoke_request(0);
            workers.push(thread::spawn(move || {
                let mut db = Connection::open(&path).unwrap();
                db.busy_timeout(Duration::from_secs(5)).unwrap();
                barrier.wait();
                revoke(&mut db, &request).unwrap().disposition.to_owned()
            }));
        }
        barrier.wait();
        let mut dispositions: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        dispositions.sort();
        assert_eq!(dispositions, ["committed", "replay"]);
        let reopened = Connection::open(&path).unwrap();
        assert_eq!(state(&reopened), ("next".into(), 1, 0));
        let source: (i64, i64, i64) = reopened.query_row(
            "SELECT lease_epoch,policy_epoch,capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(source, (8, 2, 0));
        drop(reopened);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
    }

    #[test]
    fn activate_new_source_sets_explicit_defaults_and_exact_retry_is_stable() {
        let mut db = fixture();
        let activation = activation_request("3123456789abcdef0123456789abcdef", "new-source");
        let first = activate(&mut db, &activation).unwrap();
        assert_eq!(first.disposition, "committed");
        assert!(first.lease.epoch > 0);
        assert_eq!(
            (
                first.lease.policy_epoch,
                first.cursor.as_str(),
                first.dirty_seq
            ),
            (1, "", 0)
        );
        let row: (
            String,
            String,
            String,
            i64,
            String,
            String,
            String,
            i64,
            i64,
            i64,
            String,
        ) = db
            .query_row(
                "SELECT receiver_id,plugin_id,generation,lease_epoch,cursor,prior_cursor,\
                 last_batch_digest,dirty_seq,policy_epoch,capture_enabled,content_fingerprint \
                 FROM plugin_source_cursor_v1 WHERE source_id='new-source'",
                [],
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
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "receiver-one".into(),
                "fixture.plugin".into(),
                "a".repeat(64),
                first.lease.epoch,
                "".into(),
                "".into(),
                "".into(),
                0,
                1,
                1,
                "".into()
            )
        );
        let retry = activate(&mut db, &activation).unwrap();
        assert_eq!(retry.disposition, "replay");
        assert_eq!(retry.lease, first.lease);
        assert_eq!((retry.cursor.as_str(), retry.dirty_seq), ("", 0));

        let mut changed = activation.clone();
        if let v2::Operation::ActivateSource { generation, .. } = &mut changed.operation {
            *generation = "b".repeat(64);
        }
        assert!(
            activate(&mut db, &changed)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        let stored_epoch: i64 = db
            .query_row(
                "SELECT lease_epoch FROM plugin_source_cursor_v1 WHERE source_id='new-source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_epoch, first.lease.epoch);
    }

    #[test]
    fn activate_reissues_lease_without_changing_cursor_policy_or_dirty_state() {
        let mut db = fixture();
        let first_request = activation_request("4123456789abcdef0123456789abcdef", "source-one");
        let first = activate(&mut db, &first_request).unwrap();
        let event_hash = "1".repeat(64);
        let event_digest = "2".repeat(64);
        db.execute(
            "INSERT INTO plugin_evidence_v1(source_id,event_id_hash,revision,kind,time_ns,\
             payload_json,record_digest,native_session_id,native_worker_id,native_turn_id,\
             native_operation_id,member_id,policy_epoch,admitted_ns) \
             VALUES('source-one',?,0,'lifecycle',100,'{\"state\":\"started\"}',?,\
                    'native-one','','','', '',1,100)",
            params![event_hash, event_digest],
        )
        .unwrap();
        db.execute(
            "INSERT INTO plugin_evidence_dirty_v1 VALUES('source-one',1)",
            [],
        )
        .unwrap();
        let empty_refs = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
        db.execute(
            "INSERT INTO plugin_content_batch_v1 VALUES('source-one','next',?)",
            [&empty_refs],
        )
        .unwrap();
        db.execute(
            "INSERT INTO plugin_content_status_v1 VALUES('source-one','next',\
                    'not_requested',0,1)",
            [],
        )
        .unwrap();
        db.execute(
            "UPDATE plugin_source_cursor_v1 SET cursor='next',prior_cursor='',\
                    last_batch_digest=?,dirty_seq=1 WHERE source_id='source-one'",
            [&"c".repeat(64)],
        )
        .unwrap();

        let mut existing = activation_request("5123456789abcdef0123456789abcdef", "source-one");
        if let v2::Operation::ActivateSource { generation, .. } = &mut existing.operation {
            *generation = first.lease.generation.clone();
        }
        let renewed = activate(&mut db, &existing).unwrap();
        assert_eq!(renewed.disposition, "committed");
        assert_ne!(renewed.lease.epoch, first.lease.epoch);
        assert_eq!(renewed.lease.policy_epoch, first.lease.policy_epoch);
        assert_eq!((renewed.cursor.as_str(), renewed.dirty_seq), ("next", 1));
        let state: (i64, i64, String, i64, String) = db
            .query_row(
                "SELECT lease_epoch,policy_epoch,cursor,dirty_seq,content_fingerprint \
             FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            state,
            (renewed.lease.epoch, 1, "next".into(), 1, String::new())
        );
        let mut stale = request("later", "next", &[]);
        stale.lease.epoch = first.lease.epoch;
        assert!(
            commit(&mut db, &stale)
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        let before_replay: (i64, i64, String, i64) = db
            .query_row(
                "SELECT lease_epoch,policy_epoch,cursor,dirty_seq FROM plugin_source_cursor_v1 \
             WHERE source_id='source-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let replay = activate(&mut db, &first_request).unwrap();
        assert_eq!(replay.disposition, "replay");
        assert_eq!(replay.lease, first.lease);
        assert_eq!(before_replay, (renewed.lease.epoch, 1, "next".into(), 1));
        let after_replay: (i64, i64, String, i64) = db
            .query_row(
                "SELECT lease_epoch,policy_epoch,cursor,dirty_seq FROM plugin_source_cursor_v1 \
             WHERE source_id='source-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(after_replay, before_replay);
    }

    #[test]
    fn activation_replay_after_revoke_does_not_reenable_source() {
        let mut db = fixture();
        let activation = activation_request("6123456789abcdef0123456789abcdef", "source-one");
        let original = activate(&mut db, &activation).unwrap();
        let mut revoke_request = revoke_request(0);
        revoke_request.request_id = "7123456789abcdef0123456789abcdef".into();
        if let v2::Operation::RevokeSource { lease, cursor, .. } = &mut revoke_request.operation {
            *lease = original.lease.clone();
            cursor.clear();
        }
        let revoked = revoke(&mut db, &revoke_request).unwrap();
        assert_eq!(revoked.disposition, "committed");
        let before: (i64, i64, i64, String) = db.query_row(
            "SELECT lease_epoch,policy_epoch,capture_enabled,cursor FROM plugin_source_cursor_v1 \
             WHERE source_id='source-one'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(before.2, 0);
        let replay = activate(&mut db, &activation).unwrap();
        assert_eq!(replay.disposition, "replay");
        assert_eq!(replay.lease, original.lease);
        let after: (i64, i64, i64, String) = db.query_row(
            "SELECT lease_epoch,policy_epoch,capture_enabled,cursor FROM plugin_source_cursor_v1 \
             WHERE source_id='source-one'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(after, before);
        let refusal = activate(
            &mut db,
            &activation_request("8123456789abcdef0123456789abcdef", "source-one"),
        );
        assert!(refusal.unwrap_err().to_string().contains("revoked"));
    }

    #[test]
    fn activate_refuses_identity_mismatch_revoked_and_private_or_unsupported_state() {
        for (table, insert, message) in [
            (
                "plugin_reference_owner_v1",
                "INSERT INTO plugin_reference_owner_v1 VALUES('source-one','private')",
                "private",
            ),
            (
                "plugin_content_v1",
                "INSERT INTO plugin_content_v1 VALUES('source-one')",
                "private",
            ),
            (
                "plugin_source_member_cursor_v1",
                "INSERT INTO plugin_source_member_cursor_v1 VALUES('source-one','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','cursor-one')",
                "unsupported",
            ),
            (
                "plugin_source_gap_invalidated_v1",
                "INSERT INTO plugin_source_gap_invalidated_v1 VALUES('source-one')",
                "unsupported",
            ),
            (
                "plugin_retention_v1",
                "INSERT INTO plugin_retention_v1 VALUES('source-one')",
                "unsupported",
            ),
            (
                "plugin_session_member_v1",
                "INSERT INTO plugin_session_member_v1 VALUES('source-one','native-one','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')",
                "unsupported",
            ),
        ] {
            let mut db = fixture();
            let first = activate(
                &mut db,
                &activation_request("6123456789abcdef0123456789abcdef", "source-one"),
            )
            .unwrap();
            db.execute(insert, []).unwrap();
            let changed = activation_request("7123456789abcdef0123456789abcdef", "source-one");
            let error = activate(&mut db, &changed).unwrap_err();
            assert!(error.to_string().contains(message), "{table}: {error}");
            let state: (i64,i64) = db.query_row(
                "SELECT lease_epoch,capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
            assert_eq!(state, (first.lease.epoch, 1), "{table}");
        }

        let mut db = fixture();
        let first = activate(
            &mut db,
            &activation_request("8123456789abcdef0123456789abcdef", "source-one"),
        )
        .unwrap();
        let mut wrong_plugin = activation_request("9123456789abcdef0123456789abcdef", "source-one");
        if let v2::Operation::ActivateSource { plugin_id, .. } = &mut wrong_plugin.operation {
            *plugin_id = "different.plugin".into();
        }
        assert!(
            activate(&mut db, &wrong_plugin)
                .unwrap_err()
                .to_string()
                .contains("mismatch")
        );
        let mut wrong_generation =
            activation_request("a123456789abcdef0123456789abcdef", "source-one");
        if let v2::Operation::ActivateSource { generation, .. } = &mut wrong_generation.operation {
            *generation = "b".repeat(64);
        }
        assert!(
            activate(&mut db, &wrong_generation)
                .unwrap_err()
                .to_string()
                .contains("mismatch")
        );
        db.execute(
            "UPDATE plugin_source_cursor_v1 SET capture_enabled=0 WHERE source_id='source-one'",
            [],
        )
        .unwrap();
        let revoked = activation_request("b123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &revoked)
                .unwrap_err()
                .to_string()
                .contains("revoked")
        );
        let enabled: i64 = db
            .query_row(
                "SELECT capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0);
        assert!(first.lease.epoch > 0);
    }

    #[test]
    fn activation_refuses_missing_current_content_receipts_for_nonempty_cursor() {
        for missing_table in ["plugin_content_batch_v1", "plugin_content_status_v1"] {
            let mut db = fixture();
            let first = activate(
                &mut db,
                &activation_request("9123456789abcdef0123456789abcdef", "source-one"),
            )
            .unwrap();
            let empty_refs = format!("{:x}", Sha256::digest(b"{\"refs\":[]}"));
            db.execute(
                "INSERT INTO plugin_content_batch_v1 VALUES('source-one','cursor-one',?)",
                [&empty_refs],
            )
            .unwrap();
            db.execute(
                "INSERT INTO plugin_content_status_v1 VALUES(\
                 'source-one','cursor-one','not_requested',0,1)",
                [],
            )
            .unwrap();
            db.execute(
                "UPDATE plugin_source_cursor_v1 SET cursor='cursor-one',last_batch_digest=? \
                 WHERE source_id='source-one'",
                [&"a".repeat(64)],
            )
            .unwrap();
            db.execute(
                &format!("DELETE FROM {missing_table} WHERE source_id='source-one'"),
                [],
            )
            .unwrap();
            let retry = activation_request("a123456789abcdef0123456789abcdef", "source-one");
            let error = activate(&mut db, &retry).unwrap_err();
            assert!(
                error.to_string().contains("orphan history"),
                "{missing_table}: {error}"
            );
            let state: (i64, String) = db
                .query_row(
                    "SELECT lease_epoch,cursor FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                state,
                (first.lease.epoch, "cursor-one".into()),
                "{missing_table}"
            );
        }
    }

    #[test]
    fn activation_rejects_receiver_and_unmarked_scratch_before_mutation() {
        let mut db = fixture();
        let before = state(&db);
        let mut wrong_receiver =
            activation_request("b123456789abcdef0123456789abcdef", "source-one");
        wrong_receiver.receiver_id = "receiver-two".into();
        assert!(
            activate(&mut db, &wrong_receiver)
                .unwrap_err()
                .to_string()
                .contains("receiver identity mismatch")
        );
        assert_eq!(state(&db), before);

        db.execute(
            "DELETE FROM schema_meta WHERE key='storage_admission_scratch_v1'",
            [],
        )
        .unwrap();
        let unmarked = activation_request("c123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &unmarked)
                .unwrap_err()
                .to_string()
                .contains("explicit scratch")
        );
        assert_eq!(state(&db), before);
        let receipt_table: Option<String> = db
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='storage_source_activate_v2'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(receipt_table.is_none());
    }

    #[test]
    fn activation_ignored_or_mutated_writes_roll_back_without_a_reply() {
        let mut db = fixture();
        let original = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER ignore_activation_update BEFORE UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN OLD.source_id='source-one' \
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let request = activation_request("d123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("update did not affect exactly one row")
        );
        assert_eq!(
            activation_source_state(&db, "source-one"),
            Some(original.clone())
        );
        assert_eq!(activation_receipt_count(&db, &request.request_id), None);

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER ignore_activation_insert BEFORE INSERT ON plugin_source_cursor_v1 \
             WHEN NEW.source_id='fresh-source' BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let request = activation_request("e123456789abcdef0123456789abcdef", "fresh-source");
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("insert did not affect exactly one row")
        );
        assert_eq!(activation_source_state(&db, "fresh-source"), None);
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(activation_receipt_count(&db, &request.request_id), None);

        let mut db = fixture();
        let original = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TABLE storage_source_activate_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
             lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL);\
             CREATE TRIGGER ignore_activation_receipt BEFORE INSERT ON storage_source_activate_v2 \
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let request = activation_request("f123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("receipt insert did not affect exactly one row")
        );
        assert_eq!(
            activation_source_state(&db, "source-one"),
            Some(original.clone())
        );
        assert_eq!(activation_receipt_count(&db, &request.request_id), Some(0));

        let mut db = fixture();
        let original = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER mutate_activation_update AFTER UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN NEW.source_id='source-one' \
             BEGIN UPDATE plugin_source_cursor_v1 SET policy_epoch=99 \
                   WHERE source_id='source-one'; END;",
        )
        .unwrap();
        let request = activation_request("0123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("state was not persisted exactly")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(original));
        assert_eq!(activation_receipt_count(&db, &request.request_id), None);

        let mut db = fixture();
        let original = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TABLE storage_source_activate_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
             lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL);\
             CREATE TRIGGER mutate_activation_receipt AFTER INSERT ON storage_source_activate_v2 \
             BEGIN UPDATE plugin_source_cursor_v1 SET policy_epoch=99 \
                   WHERE source_id=NEW.source_id; END;",
        )
        .unwrap();
        let request = activation_request("1123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("state changed while persisting its receipt")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(original));
        assert_eq!(activation_receipt_count(&db, &request.request_id), Some(0));

        let mut db = fixture();
        let original = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TABLE storage_source_activate_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
             lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL);\
             CREATE TABLE storage_admission_request_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL,disposition TEXT NOT NULL);\
             CREATE TRIGGER collide_activation_receipt AFTER INSERT ON storage_source_activate_v2 \
             BEGIN INSERT INTO storage_admission_request_v2 \
               VALUES(NEW.request_id,NEW.receiver_id,NEW.request_digest,NEW.source_id,\
                      NEW.cursor,'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',\
                      NEW.dirty_seq,'committed'); END;",
        )
        .unwrap();
        let request = activation_request("2123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 request replay identity")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(original));
        assert_eq!(activation_receipt_count(&db, &request.request_id), Some(0));
        let admission_rows: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM storage_admission_request_v2 WHERE request_id=?",
                [&request.request_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(admission_rows, 0);
    }

    #[test]
    fn activation_rejects_orphan_history_and_receipts_after_cursor_removal() {
        let mut db = fixture();
        let first = activate(
            &mut db,
            &activation_request("c123456789abcdef0123456789abcdef", "source-one"),
        )
        .unwrap();
        db.execute(
            "DELETE FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
            [],
        )
        .unwrap();
        let later = activation_request("d123456789abcdef0123456789abcdef", "source-one");
        assert!(
            activate(&mut db, &later)
                .unwrap_err()
                .to_string()
                .contains("orphan")
        );
        assert_eq!(
            activate(
                &mut db,
                &activation_request("c123456789abcdef0123456789abcdef", "source-one")
            )
            .unwrap()
            .lease,
            first.lease
        );
        assert!(
            db.query_row(
                "SELECT 1 FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |_| Ok(())
            )
            .optional()
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn activation_rejects_orphan_prepared_binding_after_cursor_removal() {
        let mut db = fixture();
        activate(
            &mut db,
            &activation_request("e123456789abcdef0123456789abcdef", "source-one"),
        )
        .unwrap();
        db.execute_batch(
            "CREATE TABLE storage_admission_batch_binding_v1(\
             source_id TEXT PRIMARY KEY,generation TEXT,policy_epoch INTEGER,\
             expected_cursor TEXT,cursor TEXT,batch_digest TEXT,prepared_digest TEXT,dirty_seq INTEGER); \
             INSERT INTO storage_admission_batch_binding_v1 VALUES(\
             'source-one','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
             1,'','cursor-one','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',\
             'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',1); \
             DELETE FROM storage_source_activate_v2 WHERE source_id='source-one'; \
             DELETE FROM plugin_source_cursor_v1 WHERE source_id='source-one';",
        )
        .unwrap();
        let error = activate(
            &mut db,
            &activation_request("f123456789abcdef0123456789abcdef", "source-one"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("orphan"));
    }

    #[test]
    fn activation_and_admission_and_revoke_share_request_id_namespace() {
        let id = "e123456789abcdef0123456789abcdef";
        let mut db = fixture();
        let activated = activate(&mut db, &activation_request(id, "source-one")).unwrap();
        let mut admission = request("next", "", &[]);
        admission.lease.epoch = activated.lease.epoch;
        let typed_admission = lifecycle_v2_request(id, &activated.lease, "", "next");
        let replay = typed_admission.replay_identity().unwrap();
        assert!(
            commit_guarded(&mut db, &admission, None, Some(&replay))
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        let mut revoke_req = revoke_request(0);
        revoke_req.request_id = id.into();
        if let v2::Operation::RevokeSource { lease, .. } = &mut revoke_req.operation {
            *lease = activated.lease.clone();
        }
        if let v2::Operation::RevokeSource { cursor, .. } = &mut revoke_req.operation {
            cursor.clear();
        }
        assert!(
            revoke(&mut db, &revoke_req)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );

        let mut db = fixture();
        let activated = activate(
            &mut db,
            &activation_request("f123456789abcdef0123456789abcdef", "source-one"),
        )
        .unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let typed_admission = lifecycle_v2_request(id, &activated.lease, "", "next");
        let replay = typed_admission.replay_identity().unwrap();
        let mut admission = request("next", "", &[]);
        admission.lease.epoch = activated.lease.epoch;
        commit_guarded(&mut db, &admission, None, Some(&replay)).unwrap();
        let mut reused_activation = activation_request(id, "source-one");
        if let v2::Operation::ActivateSource { generation, .. } = &mut reused_activation.operation {
            *generation = activated.lease.generation.clone();
        }
        assert!(
            activate(&mut db, &reused_activation)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );

        let mut db = fixture();
        let activated = activate(
            &mut db,
            &activation_request("1123456789abcdef0123456789abcdef", "source-one"),
        )
        .unwrap();
        let id = "2123456789abcdef0123456789abcdef";
        let mut revoke_req = revoke_request(0);
        revoke_req.request_id = id.into();
        if let v2::Operation::RevokeSource { lease, .. } = &mut revoke_req.operation {
            *lease = activated.lease.clone();
        }
        if let v2::Operation::RevokeSource { cursor, .. } = &mut revoke_req.operation {
            cursor.clear();
        }
        revoke(&mut db, &revoke_req).unwrap();
        let mut reused_activation = activation_request(id, "source-one");
        if let v2::Operation::ActivateSource { generation, .. } = &mut reused_activation.operation {
            *generation = activated.lease.generation.clone();
        }
        assert!(
            activate(&mut db, &reused_activation)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
    }

    #[test]
    fn activation_retry_rejects_cross_operation_request_id_collision() {
        let mut db = fixture();
        let request = activation_request("3123456789abcdef0123456789abcdef", "source-one");
        let activated = activate(&mut db, &request).unwrap();
        let state_before = activation_source_state(&db, "source-one");
        db.execute_batch(
            "CREATE TABLE storage_admission_request_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL,disposition TEXT NOT NULL);",
        )
        .unwrap();
        db.execute(
            "INSERT INTO storage_admission_request_v2 VALUES(\
             ?,'receiver-one','dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',\
             'source-one','','eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',0,'committed')",
            [&request.request_id],
        )
        .unwrap();
        assert!(
            activate(&mut db, &request)
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 request replay identity")
        );
        assert_eq!(activation_source_state(&db, "source-one"), state_before);
        assert_eq!(activation_receipt_count(&db, &request.request_id), Some(1));
        assert!(activated.lease.epoch > 0);
    }

    #[test]
    fn real_clone_activation_rejects_marker_trigger_before_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TRIGGER tamper_activation_clone_marker \
             AFTER UPDATE OF lease_epoch ON plugin_source_cursor_v1 \
             BEGIN DELETE FROM schema_meta WHERE key='storage_activation_clone_v1'; END;",
        )
        .unwrap();
        let request = activation_request("4123456789abcdef0123456789abcdef", "source-one");
        let before = activation_source_state(&db, "source-one");
        let replay = request.replay_identity().unwrap();
        let error = activate_source_guarded(&mut db, &request, None, &replay, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("triggers on real clone activation")
        );
        assert_eq!(activation_source_state(&db, "source-one"), before);
        assert!(matches!(
            activation_receipt_count(&db, &request.request_id),
            None | Some(0)
        ));
        let marker: String = db
            .query_row(
                "SELECT value FROM schema_meta WHERE key='storage_activation_clone_v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, "enabled");
    }

    #[test]
    fn real_clone_activation_rejects_sibling_capture_trigger() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TRIGGER mutate_activation_victim AFTER INSERT ON plugin_source_cursor_v1 \
             BEGIN UPDATE plugin_source_cursor_v1 SET capture_enabled=0 \
             WHERE source_id='source-one'; END;",
        )
        .unwrap();
        let request = activation_request("5123456789abcdef0123456789abcdef", "source-two");
        let replay = request.replay_identity().unwrap();
        let victim_before = activation_source_state(&db, "source-one");
        let error = activate_source_guarded(&mut db, &request, None, &replay, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("triggers on real clone activation")
        );
        assert_eq!(activation_source_state(&db, "source-one"), victim_before);
        assert_eq!(activation_source_state(&db, "source-two"), None);
        assert!(matches!(
            activation_receipt_count(&db, &request.request_id),
            None | Some(0)
        ));
    }

    #[test]
    fn real_clone_activation_rejects_temp_receiver_trigger() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TEMP TRIGGER mutate_activation_receiver \
             AFTER UPDATE ON plugin_source_cursor_v1 \
             BEGIN UPDATE schema_meta SET value='other-receiver' WHERE key='receiver_id'; END;",
        )
        .unwrap();
        let request = activation_request("6123456789abcdef0123456789abcdef", "source-one");
        let replay = request.replay_identity().unwrap();
        let source_before = activation_source_state(&db, "source-one");
        let error = activate_source_guarded(&mut db, &request, None, &replay, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("triggers on real clone activation")
        );
        assert_eq!(activation_source_state(&db, "source-one"), source_before);
        let receiver: String = db
            .query_row(
                "SELECT value FROM schema_meta WHERE key='receiver_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receiver, "receiver-one");
        assert!(matches!(
            activation_receipt_count(&db, &request.request_id),
            None | Some(0)
        ));
    }

    #[test]
    fn real_clone_activation_rejects_receipt_namespace_trigger() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TABLE storage_source_activate_v2( \
             request_id TEXT PRIMARY KEY,receiver_id TEXT,request_digest TEXT,source_id TEXT, \
             plugin_id TEXT,generation TEXT,lease_epoch INTEGER,policy_epoch INTEGER, \
             cursor TEXT,dirty_seq INTEGER); \
             CREATE TABLE storage_admission_request_v2(request_id TEXT PRIMARY KEY); \
             CREATE TEMP TRIGGER collide_activation_receipt AFTER INSERT ON storage_source_activate_v2 \
             BEGIN INSERT INTO storage_admission_request_v2 VALUES(NEW.request_id); END;",
        )
        .unwrap();
        let request = activation_request("7123456789abcdef0123456789abcdef", "source-two");
        let replay = request.replay_identity().unwrap();
        let error = activate_source_guarded(&mut db, &request, None, &replay, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("triggers on real clone activation")
        );
        assert_eq!(activation_source_state(&db, "source-two"), None);
        assert_eq!(activation_receipt_count(&db, &request.request_id), Some(0));
        assert_eq!(admission_receipt_count(&db, &request.request_id), Some(0));
    }

    fn clone_admission_request(request_id: &str, cursor: &str) -> v2::Request {
        v2::Request {
            version: v2::VERSION,
            request_id: request_id.to_owned(),
            receiver_id: "receiver-one".to_owned(),
            operation: v2::Operation::AdmitLifecycle {
                lease: v2::SourceLease {
                    receiver_id: "receiver-one".to_owned(),
                    plugin_id: "fixture.plugin".to_owned(),
                    source_id: "source-one".to_owned(),
                    generation: "a".repeat(64),
                    epoch: 7,
                    policy_epoch: 1,
                },
                expected_cursor: String::new(),
                cursor: cursor.to_owned(),
                batch_digest: "d".repeat(64),
                records: vec![v2::PreparedLifecycle {
                    event_id_hash: "e".repeat(64),
                    revision: 0,
                    kind: v2::LifecycleKind::Lifecycle,
                    time_ns: 100,
                    payload_json: "{\"state\":\"started\"}".to_owned(),
                    record_digest: "f".repeat(64),
                    native_session_id: "native-one".to_owned(),
                    native_worker_id: String::new(),
                    native_turn_id: String::new(),
                    native_operation_id: String::new(),
                }],
            },
        }
    }

    fn clone_fact_admission_request(request_id: &str, cursor: &str) -> (v2::Request, Request) {
        let lease = v2::SourceLease {
            receiver_id: "receiver-one".to_owned(),
            plugin_id: "fixture.plugin".to_owned(),
            source_id: "source-one".to_owned(),
            generation: "a".repeat(64),
            epoch: 7,
            policy_epoch: 1,
        };
        let records = vec![
            v2::PreparedFact {
                event_id_hash: "a".repeat(64),
                revision: 0,
                kind: v2::FactKind::Message,
                time_ns: 100,
                payload_json: "{\"role\":\"assistant\"}".to_owned(),
                record_digest: "1".repeat(64),
                native_session_id: "native-one".to_owned(),
                native_worker_id: String::new(),
                native_turn_id: String::new(),
                native_operation_id: String::new(),
            },
            v2::PreparedFact {
                event_id_hash: "b".repeat(64),
                revision: 0,
                kind: v2::FactKind::Tool,
                time_ns: 110,
                payload_json: "{\"category\":\"shell\",\"result\":\"started\"}".to_owned(),
                record_digest: "2".repeat(64),
                native_session_id: "native-one".to_owned(),
                native_worker_id: String::new(),
                native_turn_id: "turn-one".to_owned(),
                native_operation_id: "op-one".to_owned(),
            },
            v2::PreparedFact {
                event_id_hash: "c".repeat(64),
                revision: 0,
                kind: v2::FactKind::Check,
                time_ns: 120,
                payload_json: "{\"category\":\"test\",\"result\":\"passed\"}".to_owned(),
                record_digest: "3".repeat(64),
                native_session_id: "native-one".to_owned(),
                native_worker_id: String::new(),
                native_turn_id: String::new(),
                native_operation_id: String::new(),
            },
            v2::PreparedFact {
                event_id_hash: "d".repeat(64),
                revision: 0,
                kind: v2::FactKind::Usage,
                time_ns: 130,
                payload_json: "{\"input_tokens\":5,\"total_tokens\":5}".to_owned(),
                record_digest: "4".repeat(64),
                native_session_id: String::new(),
                native_worker_id: String::new(),
                native_turn_id: String::new(),
                native_operation_id: String::new(),
            },
        ];
        let wire = v2::Request {
            version: v2::VERSION,
            request_id: request_id.to_owned(),
            receiver_id: lease.receiver_id.clone(),
            operation: v2::Operation::AdmitFacts {
                lease: lease.clone(),
                expected_cursor: String::new(),
                cursor: cursor.to_owned(),
                batch_digest: "d".repeat(64),
                records: records.clone(),
            },
        };
        let prepared = Request {
            schema_version: 1,
            sqlite_path: "/tmp/real-clone.db".to_owned(),
            lease: Lease {
                receiver_id: lease.receiver_id,
                plugin_id: lease.plugin_id,
                source_id: lease.source_id,
                generation: lease.generation,
                epoch: lease.epoch,
                policy_epoch: lease.policy_epoch,
            },
            expected_cursor: String::new(),
            cursor: cursor.to_owned(),
            batch_digest: "d".repeat(64),
            records: records
                .into_iter()
                .map(|row| PreparedRecord {
                    event_id_hash: row.event_id_hash,
                    revision: row.revision,
                    kind: row.kind.as_str().to_owned(),
                    time_ns: row.time_ns,
                    payload_json: row.payload_json,
                    record_digest: row.record_digest,
                    native_session_id: row.native_session_id,
                    native_worker_id: row.native_worker_id,
                    native_turn_id: row.native_turn_id,
                    native_operation_id: row.native_operation_id,
                    member_id: String::new(),
                })
                .collect(),
            member_update: None,
        };
        (wire, prepared)
    }

    fn add_real_clone_markers(db: &Connection) {
        db.execute_batch(
            "INSERT INTO schema_meta VALUES('storage_activation_clone_v1','enabled'); \
             INSERT INTO schema_meta VALUES('storage_authority_v1','python-v1'); \
             INSERT INTO schema_meta VALUES('plugin_sessions_read_schema','1');",
        )
        .unwrap();
    }

    fn reject_clone_lifecycle_trigger(db: &mut Connection, request_id: &str) -> String {
        let wire = clone_admission_request(request_id, "cursor-one");
        let replay = wire.replay_identity().unwrap();
        let event_hash = "e".repeat(64);
        let record_digest = "f".repeat(64);
        let prepared = request(
            "cursor-one",
            "",
            &[(event_hash.as_str(), record_digest.as_str())],
        );
        let before = state(db);
        let source_before = activation_source_state(db, "source-one");
        let error = commit_guarded_mode(db, &prepared, None, Some(&replay), true).unwrap_err();
        assert_eq!(state(db), before);
        assert_eq!(activation_source_state(db, "source-one"), source_before);
        assert!(matches!(
            admission_receipt_count(db, request_id),
            None | Some(0)
        ));
        error.to_string()
    }

    #[test]
    fn real_clone_admission_commits_replays_and_observes_shared_cursor_limit() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        let cursor = "k".repeat(v2::MAX_CURSOR_CHARS);
        let wire = clone_admission_request("5123456789abcdef0123456789abcdef", &cursor);
        let replay = wire.replay_identity().unwrap();
        let prepared = Request {
            schema_version: 1,
            sqlite_path: "/tmp/real-clone.db".to_owned(),
            lease: Lease {
                receiver_id: "receiver-one".to_owned(),
                plugin_id: "fixture.plugin".to_owned(),
                source_id: "source-one".to_owned(),
                generation: "a".repeat(64),
                epoch: 7,
                policy_epoch: 1,
            },
            expected_cursor: String::new(),
            cursor: cursor.clone(),
            batch_digest: "d".repeat(64),
            records: vec![PreparedRecord {
                event_id_hash: "e".repeat(64),
                revision: 0,
                kind: "lifecycle".to_owned(),
                time_ns: 100,
                payload_json: "{\"state\":\"started\"}".to_owned(),
                record_digest: "f".repeat(64),
                native_session_id: "native-one".to_owned(),
                native_worker_id: String::new(),
                native_turn_id: String::new(),
                native_operation_id: String::new(),
                member_id: String::new(),
            }],
            member_update: None,
        };
        let committed = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true).unwrap();
        assert_eq!(committed.disposition, "committed");
        assert_eq!(committed.cursor, cursor);
        let after_commit = state(&db);
        let replayed = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true).unwrap();
        assert_eq!(replayed.disposition, "replay");
        assert_eq!(state(&db), after_commit);
        assert_eq!(admission_receipt_count(&db, &wire.request_id), Some(1));
        assert_eq!(after_commit, ("k".repeat(v2::MAX_CURSOR_CHARS), 1, 1));
    }

    #[test]
    fn real_clone_admission_rejects_stale_or_nonempty_content_receipts() {
        for (idx, malformed) in [
            "INSERT INTO plugin_content_batch_v1 VALUES('source-one','','nonempty-digest')",
            "INSERT INTO plugin_content_batch_v1 VALUES('source-one','stale-cursor','nonempty-digest')",
            "INSERT INTO plugin_content_status_v1 VALUES('source-one','','not_requested',1,1)",
        ]
        .iter()
        .enumerate()
        {
            let mut db = fixture();
            add_real_clone_markers(&db);
            db.execute(malformed, []).unwrap();
            let request_id = format!("{:032x}", idx + 1);
            let wire = clone_admission_request(&request_id, "cursor-one");
            let replay = wire.replay_identity().unwrap();
            let prepared = request("cursor-one", "", &[("e".repeat(64).as_str(), "f".repeat(64).as_str())]);
            let source_before = activation_source_state(&db, "source-one");
            let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true).unwrap_err();
            assert!(error.to_string().contains("unsupported, or orphan history"), "{malformed}: {error}");
            assert_eq!(activation_source_state(&db, "source-one"), source_before);
            assert_eq!(state(&db), (String::new(), 0, 0));
            assert!(matches!(admission_receipt_count(&db, &request_id), None | Some(0)));
            let retained: i64 = db.query_row(
                "SELECT (SELECT COUNT(*) FROM plugin_content_batch_v1 WHERE source_id='source-one') \
                 + (SELECT COUNT(*) FROM plugin_content_status_v1 WHERE source_id='source-one')",
                [], |row| row.get(0),
            ).unwrap();
            assert_eq!(retained, 1, "{malformed}");
        }
    }

    #[test]
    fn real_clone_admission_rejects_marker_trigger_before_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TRIGGER tamper_admission_clone_marker \
             AFTER INSERT ON plugin_evidence_v1 \
             BEGIN DELETE FROM schema_meta WHERE key='storage_activation_clone_v1'; END;",
        )
        .unwrap();
        let error = reject_clone_lifecycle_trigger(&mut db, "6123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
        let marker: String = db
            .query_row(
                "SELECT value FROM schema_meta WHERE key='storage_activation_clone_v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, "enabled");
    }

    #[test]
    fn real_clone_admission_rejects_prepared_binding_trigger_before_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TABLE storage_admission_batch_binding_v1(source_id TEXT); \
             CREATE TRIGGER tamper_prepared_binding BEFORE INSERT \
             ON storage_admission_batch_binding_v1 BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let error = reject_clone_lifecycle_trigger(&mut db, "7123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM storage_admission_batch_binding_v1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    fn assert_fact_trigger_rolls_back(trigger: &str, request_id: &str) -> String {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(trigger).unwrap();
        let (wire, prepared) = clone_fact_admission_request(request_id, "facts-one");
        assert!(wire.validate().is_ok());
        let replay = wire.replay_identity().unwrap();
        let before = state(&db);
        let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect_err("trigger must reject the fact transaction");
        assert_eq!(state(&db), before);
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM plugin_evidence_v1 WHERE source_id='source-one'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert!(matches!(
            admission_receipt_count(&db, request_id),
            None | Some(0)
        ));
        error.to_string()
    }

    fn assert_fact_receipt_trigger_rolls_back(trigger: &str, request_id: &str) -> String {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TABLE storage_admission_request_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,\
             request_digest TEXT NOT NULL,source_id TEXT NOT NULL,cursor TEXT NOT NULL,\
             batch_digest TEXT NOT NULL,dirty_seq INTEGER NOT NULL,disposition TEXT NOT NULL);",
        )
        .unwrap();
        db.execute_batch(trigger).unwrap();
        let (wire, prepared) = clone_fact_admission_request(request_id, "facts-one");
        let replay = wire.replay_identity().unwrap();
        let before = state(&db);
        let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect_err("receipt trigger must reject the fact transaction");
        assert_eq!(state(&db), before);
        assert_eq!(admission_receipt_count(&db, request_id), Some(0));
        error.to_string()
    }

    #[test]
    fn real_clone_fact_admission_rolls_back_first_insert_on_late_fact_conflict() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        let conflicting_hash = "b".repeat(64);
        db.execute(
            "INSERT INTO plugin_evidence_v1 \
             (source_id,event_id_hash,revision,kind,time_ns,payload_json,record_digest,\
              native_session_id,native_worker_id,native_turn_id,native_operation_id,member_id,\
              policy_epoch,admitted_ns) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                "source-one",
                conflicting_hash,
                0,
                "tool",
                110,
                r#"{"category":"shell","result":"started"}"#,
                "9".repeat(64),
                "native-one",
                "",
                "turn-one",
                "op-one",
                "",
                1,
                1,
            ],
        )
        .unwrap();
        let request_id = "7123456789abcdef0123456789abcdef";
        let (wire, prepared) = clone_fact_admission_request(request_id, "facts-one");
        let replay = wire.replay_identity().unwrap();
        let before = state(&db);

        let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect_err("conflicting second fact must abort the transaction");
        assert!(
            error
                .to_string()
                .contains("conflicting or unsupported event revision")
        );
        assert_eq!(state(&db), before, "first fact or cursor survived rollback");
        assert_eq!(admission_receipt_count(&db, request_id), None);
    }

    #[test]
    fn real_clone_fact_admission_rejects_ignored_source_update_atomically() {
        let error = assert_fact_trigger_rolls_back(
            "CREATE TRIGGER ignore_fact_cursor BEFORE UPDATE OF cursor ON \
             plugin_source_cursor_v1 WHEN NEW.source_id='source-one' \
             BEGIN SELECT RAISE(IGNORE); END;",
            "8123456789abcdef0123456789abcdef",
        );
        assert!(error.contains("triggers on real clone admission"));
    }

    #[test]
    fn real_clone_fact_admission_rejects_after_insert_evidence_mutation() {
        let trigger = format!(
            "CREATE TRIGGER mutate_fact_after_insert AFTER INSERT ON plugin_evidence_v1 \
             WHEN NEW.event_id_hash='{}' BEGIN UPDATE plugin_evidence_v1 SET payload_json='{{}}' \
             WHERE source_id=NEW.source_id AND event_id_hash=NEW.event_id_hash; END;",
            "a".repeat(64)
        );
        let error = assert_fact_trigger_rolls_back(&trigger, "9123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
    }

    #[test]
    fn real_clone_fact_admission_rejects_after_receipt_source_mutation() {
        let trigger = "CREATE TRIGGER mutate_fact_after_receipt \
             AFTER INSERT ON storage_admission_request_v2 \
             BEGIN UPDATE plugin_source_cursor_v1 SET prior_cursor='triggered' \
             WHERE source_id=NEW.source_id; END;";
        let error =
            assert_fact_receipt_trigger_rolls_back(trigger, "a123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
    }

    #[test]
    fn real_clone_fact_admission_rejects_after_receipt_mutation() {
        let trigger = "CREATE TRIGGER mutate_fact_receipt AFTER INSERT \
             ON storage_admission_request_v2 BEGIN UPDATE storage_admission_request_v2 \
             SET batch_digest='tampered' WHERE request_id=NEW.request_id; END;";
        let error =
            assert_fact_receipt_trigger_rolls_back(trigger, "d123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
    }

    #[test]
    fn real_clone_fact_admission_rejects_ignored_receipt_insert() {
        let trigger = "CREATE TRIGGER ignore_fact_receipt BEFORE INSERT \
             ON storage_admission_request_v2 BEGIN SELECT RAISE(IGNORE); END;";
        let error =
            assert_fact_receipt_trigger_rolls_back(trigger, "e123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
    }

    #[test]
    fn real_clone_fact_admission_rejects_triggered_competing_receipt_before_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TABLE storage_source_activate_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
             lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL);\
             CREATE TRIGGER collide_fact_with_activation AFTER INSERT ON plugin_evidence_v1 \
             BEGIN INSERT INTO storage_source_activate_v2 VALUES(\
               'f123456789abcdef0123456789abcdef','receiver-one',\
               'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
               NEW.source_id,'fixture.plugin',\
               'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
               7,1,'facts-one',1); END;",
        )
        .unwrap();
        let request_id = "f123456789abcdef0123456789abcdef";
        let (wire, prepared) = clone_fact_admission_request(request_id, "facts-one");
        let replay = wire.replay_identity().unwrap();
        let before = state(&db);
        let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect_err("triggered competing receipt must fail closed");
        assert!(
            error
                .to_string()
                .contains("triggers on real clone admission")
        );
        assert_eq!(state(&db), before);
        assert_eq!(admission_receipt_count(&db, request_id), None);
        assert_eq!(activation_receipt_count(&db, request_id), Some(0));
    }

    #[test]
    fn real_clone_fact_admission_rejects_sibling_source_trigger_before_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "INSERT INTO plugin_source_cursor_v1 SELECT 'source-two',receiver_id,plugin_id,\
             generation,lease_epoch,policy_epoch,capture_enabled,content_fingerprint,cursor,\
             prior_cursor,dirty_seq,last_batch_digest,updated_ns \
             FROM plugin_source_cursor_v1 WHERE source_id='source-one';\
             INSERT INTO plugin_projection_cursor_v1 VALUES('source-two',4,2);\
             CREATE TRIGGER mutate_sibling_after_fact AFTER INSERT ON plugin_evidence_v1 \
             BEGIN UPDATE plugin_source_cursor_v1 SET cursor='tampered' \
               WHERE source_id='source-two';\
               UPDATE plugin_projection_cursor_v1 SET projected_seq=99 \
               WHERE source_id='source-two'; END;",
        )
        .unwrap();
        let sibling_before: (String, i64, i64) = db
            .query_row(
                "SELECT s.cursor,s.dirty_seq,p.projected_seq \
                 FROM plugin_source_cursor_v1 s JOIN plugin_projection_cursor_v1 p \
                   USING(source_id) WHERE s.source_id='source-two'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let (wire, prepared) =
            clone_fact_admission_request("0123456789abcdef0123456789abcdef", "facts-one");
        let replay = wire.replay_identity().unwrap();
        let before = state(&db);
        let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect_err("sibling-source trigger must fail closed");
        assert!(
            error
                .to_string()
                .contains("triggers on real clone admission")
        );
        assert_eq!(state(&db), before);
        let sibling_after: (String, i64, i64) = db
            .query_row(
                "SELECT s.cursor,s.dirty_seq,p.projected_seq \
                 FROM plugin_source_cursor_v1 s JOIN plugin_projection_cursor_v1 p \
                   USING(source_id) WHERE s.source_id='source-two'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(sibling_after, sibling_before);
        assert!(matches!(
            admission_receipt_count(&db, &wire.request_id),
            None | Some(0)
        ));
    }

    #[test]
    fn real_clone_fact_admission_rejects_receiver_mutation_trigger_before_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TEMP TRIGGER change_receiver_after_fact AFTER INSERT ON plugin_evidence_v1 \
             BEGIN UPDATE schema_meta SET value='other-receiver' \
               WHERE key='receiver_id'; END;",
        )
        .unwrap();
        let (wire, prepared) =
            clone_fact_admission_request("1123456789abcdef0123456789abcdef", "facts-one");
        let replay = wire.replay_identity().unwrap();
        let before = state(&db);
        let error = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect_err("receiver mutation trigger must fail closed");
        assert!(
            error
                .to_string()
                .contains("triggers on real clone admission")
        );
        assert_eq!(state(&db), before);
        let receiver: String = db
            .query_row(
                "SELECT value FROM schema_meta WHERE key='receiver_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receiver, "receiver-one");
        assert!(matches!(
            admission_receipt_count(&db, &wire.request_id),
            None | Some(0)
        ));
    }

    #[test]
    fn real_clone_fact_request_ids_share_lifecycle_namespace_and_bind_source() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        let request_id = "b123456789abcdef0123456789abcdef";
        let (wire, prepared) = clone_fact_admission_request(request_id, "facts-one");
        let replay = wire.replay_identity().unwrap();
        let committed = commit_guarded_mode(&mut db, &prepared, None, Some(&replay), true)
            .expect("valid metadata fact batch commits");
        assert_eq!(committed.disposition, "committed");
        let before_conflict = state(&db);

        let lifecycle = clone_admission_request(request_id, "facts-two");
        let lifecycle_replay = lifecycle.replay_identity().unwrap();
        let lifecycle_prepared = Request {
            schema_version: 1,
            sqlite_path: "/tmp/real-clone.db".to_owned(),
            lease: Lease {
                receiver_id: prepared.lease.receiver_id.clone(),
                plugin_id: prepared.lease.plugin_id.clone(),
                source_id: prepared.lease.source_id.clone(),
                generation: prepared.lease.generation.clone(),
                epoch: prepared.lease.epoch,
                policy_epoch: prepared.lease.policy_epoch,
            },
            expected_cursor: "facts-one".to_owned(),
            cursor: "facts-two".to_owned(),
            batch_digest: "e".repeat(64),
            records: vec![PreparedRecord {
                event_id_hash: "e".repeat(64),
                revision: 0,
                kind: "lifecycle".to_owned(),
                time_ns: 200,
                payload_json: "{\"state\":\"completed\"}".to_owned(),
                record_digest: "f".repeat(64),
                native_session_id: "native-two".to_owned(),
                native_worker_id: String::new(),
                native_turn_id: String::new(),
                native_operation_id: String::new(),
                member_id: String::new(),
            }],
            member_update: None,
        };
        let error = commit_guarded_mode(
            &mut db,
            &lifecycle_prepared,
            None,
            Some(&lifecycle_replay),
            true,
        )
        .expect_err("request ID cannot be reused for lifecycle admission");
        assert!(
            error
                .to_string()
                .contains("conflicting v2 request replay identity")
        );
        assert_eq!(state(&db), before_conflict);

        let (mut wrong_wire, mut wrong_prepared) =
            clone_fact_admission_request("c123456789abcdef0123456789abcdef", "facts-two");
        let v2::Operation::AdmitFacts { lease, .. } = &mut wrong_wire.operation else {
            unreachable!()
        };
        lease.source_id = "missing-source".to_owned();
        wrong_prepared.lease.source_id = "missing-source".to_owned();
        assert!(wrong_wire.validate().is_ok());
        let wrong_replay = wrong_wire.replay_identity().unwrap();
        let error = commit_guarded_mode(&mut db, &wrong_prepared, None, Some(&wrong_replay), true)
            .expect_err("source ID must resolve to the leased source");
        assert!(error.to_string().contains("Query returned no rows"));
        assert_eq!(state(&db), before_conflict);
    }

    #[test]
    fn real_clone_lifecycle_rejects_receiver_and_capture_trigger_mutation() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "CREATE TEMP TRIGGER mutate_lifecycle_receiver AFTER INSERT ON plugin_evidence_v1 \
             BEGIN UPDATE schema_meta SET value='foreign-receiver' WHERE key='receiver_id'; END;",
        )
        .unwrap();
        let error = reject_clone_lifecycle_trigger(&mut db, "7123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
        let receiver: String = db
            .query_row(
                "SELECT value FROM schema_meta WHERE key='receiver_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receiver, "receiver-one");

        db.execute_batch(
            "DROP TRIGGER mutate_lifecycle_receiver; \
             CREATE TRIGGER mutate_lifecycle_capture AFTER UPDATE OF cursor ON plugin_source_cursor_v1 \
             BEGIN UPDATE plugin_source_cursor_v1 SET lease_epoch=8,capture_enabled=0 \
             WHERE source_id=NEW.source_id; END;",
        )
        .unwrap();
        let error = reject_clone_lifecycle_trigger(&mut db, "8123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
    }

    #[test]
    fn real_clone_lifecycle_rejects_sibling_and_receipt_namespace_triggers() {
        let mut db = fixture();
        add_real_clone_markers(&db);
        db.execute_batch(
            "INSERT INTO plugin_source_cursor_v1 SELECT 'source-two',receiver_id,plugin_id,\
             generation,lease_epoch,policy_epoch,capture_enabled,content_fingerprint,cursor,\
             prior_cursor,dirty_seq,last_batch_digest,updated_ns \
             FROM plugin_source_cursor_v1 WHERE source_id='source-one'; \
             CREATE TRIGGER mutate_lifecycle_sibling AFTER INSERT ON plugin_evidence_v1 \
             BEGIN UPDATE plugin_source_cursor_v1 SET cursor='tampered' \
             WHERE source_id='source-two'; END;",
        )
        .unwrap();
        let sibling_before = activation_source_state(&db, "source-two");
        let error = reject_clone_lifecycle_trigger(&mut db, "9123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
        assert_eq!(activation_source_state(&db, "source-two"), sibling_before);

        db.execute_batch(
            "DROP TRIGGER mutate_lifecycle_sibling; \
             CREATE TABLE storage_source_activate_v2(\
               request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
               source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
               lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
               dirty_seq INTEGER NOT NULL); \
             CREATE TRIGGER collide_lifecycle_namespace AFTER INSERT ON plugin_evidence_v1 \
             BEGIN INSERT INTO storage_source_activate_v2 VALUES(\
               'a123456789abcdef0123456789abcdef','receiver-one',\
               'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
               NEW.source_id,'fixture.plugin',\
               'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
               7,1,'cursor-one',1); END;",
        )
        .unwrap();
        let error = reject_clone_lifecycle_trigger(&mut db, "a123456789abcdef0123456789abcdef");
        assert!(error.contains("triggers on real clone admission"));
        assert_eq!(
            activation_receipt_count(&db, "a123456789abcdef0123456789abcdef"),
            Some(0)
        );
    }

    #[test]
    fn activation_exact_retry_survives_restart_and_duplicate_race() {
        let path = std::env::temp_dir().join(format!(
            "agenturm-activate-replay-{}-{}.db",
            std::process::id(),
            now_ns().unwrap()
        ));
        let request = activation_request("3123456789abcdef0123456789abcdef", "source-one");
        let mut db = fixture_at(&path);
        let first = activate(&mut db, &request).unwrap();
        drop(db);
        let mut reopened = Connection::open(&path).unwrap();
        let replay = activate(&mut reopened, &request).unwrap();
        assert_eq!(replay.disposition, "replay");
        assert_eq!(replay.lease, first.lease);
        assert_eq!((replay.cursor.as_str(), replay.dirty_seq), ("", 0));
        drop(reopened);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));

        let path = std::env::temp_dir().join(format!(
            "agenturm-activate-race-{}-{}.db",
            std::process::id(),
            now_ns().unwrap()
        ));
        let db = fixture_at(&path);
        drop(db);
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            let request = request.clone();
            workers.push(thread::spawn(move || {
                let mut db = Connection::open(&path).unwrap();
                db.busy_timeout(Duration::from_secs(5)).unwrap();
                barrier.wait();
                activate(&mut db, &request).unwrap()
            }));
        }
        barrier.wait();
        let first = workers.remove(0).join().unwrap();
        let second = workers.remove(0).join().unwrap();
        assert_ne!(first.disposition, second.disposition);
        assert_eq!(first.lease, second.lease);
        let reopened = Connection::open(&path).unwrap();
        let state:(i64,i64,i64,String)=reopened.query_row(
            "SELECT lease_epoch,policy_epoch,dirty_seq,cursor FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
            [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(state, (first.lease.epoch, 1, 0, String::new()));
        drop(reopened);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
    }

    #[test]
    fn revoke_rejects_receiver_or_scratch_mismatch_without_changes() {
        let mut db = fixture();
        commit(&mut db, &request("next", "", &[])).unwrap();
        let before = state(&db);
        let mut wrong_receiver = revoke_request(0);
        wrong_receiver.receiver_id = "receiver-two".into();
        if let v2::Operation::RevokeSource { lease, .. } = &mut wrong_receiver.operation {
            lease.receiver_id = "receiver-two".into();
        }
        assert!(
            revoke(&mut db, &wrong_receiver)
                .unwrap_err()
                .to_string()
                .contains("receiver")
        );
        db.execute("DELETE FROM schema_meta WHERE key=?", [SCRATCH_MARKER])
            .unwrap();
        assert!(revoke(&mut db, &revoke_request(0)).is_err());
        assert_eq!(state(&db), before);
        let enabled: i64 = db
            .query_row(
                "SELECT capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 1);
    }

    #[test]
    fn initial_same_state_capture_noop_preserves_absent_markers_and_replays() {
        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        let req = capture_request(
            "8123456789abcdef0123456789abcdef",
            &source_lease(),
            true,
            true,
            "",
            0,
        );
        let first = set_capture(&mut db, &req).unwrap();
        assert_eq!(first.disposition, "idle");
        assert_eq!(first.lease, source_lease());
        assert!(first.enabled);
        assert_eq!(first.cursor, "");
        assert_eq!(first.dirty_seq, 0);
        assert_eq!(
            activation_source_state(&db, "source-one"),
            Some(before.clone())
        );
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let replay = set_capture(&mut db, &req).unwrap();
        assert_eq!(replay.disposition, "replay");
        assert_eq!(replay.lease, first.lease);
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );
        db.execute(
            "UPDATE storage_source_capture_set_v2 SET lease_epoch=lease_epoch+1 WHERE request_id=?",
            [&req.request_id],
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 source capture request")
        );
    }

    #[test]
    fn same_state_capture_noop_rejects_receipt_triggers_before_source_mutation() {
        let mut db = fixture();
        db.execute_batch(
            "CREATE TABLE storage_source_capture_set_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,plugin_id TEXT NOT NULL,generation TEXT NOT NULL,\
             expected_capture_enabled INTEGER NOT NULL,enabled INTEGER NOT NULL,disposition TEXT NOT NULL,\
             lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,cursor TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL);\
             CREATE TRIGGER inject_evidence_after_capture_receipt \
             AFTER INSERT ON storage_source_capture_set_v2 BEGIN \
               INSERT INTO plugin_evidence_v1(source_id,event_id_hash,revision,kind,time_ns,\
                 payload_json,record_digest,native_session_id,native_worker_id,native_turn_id,\
                 native_operation_id,member_id,policy_epoch,admitted_ns) \
               VALUES(NEW.source_id,lower(hex(randomblob(32))),0,'lifecycle',1,\
                 '{\"state\":\"started\"}',lower(hex(randomblob(32))),'session','','','',NULL,\
                 NEW.policy_epoch,1); END;",
        )
        .unwrap();
        let before = activation_source_state(&db, "source-one").unwrap();
        let req = capture_request(
            "9123456789abcdef0123456789abcdef",
            &source_lease(),
            true,
            true,
            "",
            0,
        );
        let error = set_capture(&mut db, &req).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("receipt triggers are unsupported")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );
        let evidence_count: i64 = db
            .query_row("SELECT COUNT(*) FROM plugin_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let receipt_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM storage_source_capture_set_v2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(evidence_count, 0);
        assert_eq!(receipt_count, 0);
    }

    #[test]
    fn capture_disable_and_enable_fence_old_leases_without_restoring_evidence() {
        let mut db = fixture();
        let old_lease = source_lease();
        let disabled_request = capture_request(
            "a123456789abcdef0123456789abcdef",
            &old_lease,
            false,
            true,
            "",
            0,
        );
        let disabled = set_capture(&mut db, &disabled_request).unwrap();
        assert_eq!(disabled.disposition, "committed");
        assert!(!disabled.enabled);
        assert_eq!(disabled.cursor, "");
        assert_eq!(disabled.dirty_seq, 1);
        assert!(disabled.lease.epoch > old_lease.epoch);
        assert_eq!(disabled.lease.policy_epoch, 2);
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (Some(1), Some((1, 2)))
        );
        assert!(commit(&mut db, &request("stale", "", &[])).is_err());

        let enabled_request = capture_request(
            "c123456789abcdef0123456789abcdef",
            &disabled.lease,
            true,
            false,
            "",
            1,
        );
        let enabled = set_capture(&mut db, &enabled_request).unwrap();
        assert_eq!(enabled.disposition, "committed");
        assert!(enabled.enabled);
        assert_eq!(enabled.cursor, "");
        assert_eq!(enabled.dirty_seq, 2);
        assert_ne!(enabled.lease.epoch, disabled.lease.epoch);
        assert_eq!(enabled.lease.policy_epoch, 3);
        let evidence_count: i64 = db
            .query_row("SELECT COUNT(*) FROM plugin_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(evidence_count, 0);
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (Some(2), Some((2, 3)))
        );
        assert!(commit(&mut db, &request("stale-again", "", &[])).is_err());
    }

    #[test]
    fn capture_disable_cleans_supported_lifecycle_atomically_and_preserves_cursor() {
        let mut db = fixture();
        let event_hash = "1".repeat(64);
        let digest = "2".repeat(64);
        commit(&mut db, &request("next", "", &[(&event_hash, &digest)])).unwrap();
        db.execute(
            "INSERT INTO plugin_session_projection_v1 VALUES('source-one','native-one',1,1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO activity_index_v1(event_key,kind,owner_key) VALUES('activity','plugin','source-one')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO investigation_dirty_v1(kind,owner_key) VALUES('plugin','source-one')",
            [],
        )
        .unwrap();
        let lease = source_lease();
        let req = capture_request(
            "e123456789abcdef0123456789abcdef",
            &lease,
            false,
            true,
            "next",
            1,
        );
        let result = set_capture(&mut db, &req).unwrap();
        assert_eq!(result.cursor, "next");
        assert_eq!(result.dirty_seq, 2);
        let source: (String, i64, i64, i64) = db
            .query_row(
                "SELECT cursor,dirty_seq,policy_epoch,capture_enabled FROM plugin_source_cursor_v1 \
                 WHERE source_id='source-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(source, ("next".into(), 2, 2, 0));
        for table in [
            "plugin_evidence_v1",
            "plugin_session_projection_v1",
            "activity_index_v1",
            "investigation_dirty_v1",
        ] {
            let count: i64 = db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (Some(2), Some((2, 2)))
        );
    }

    #[test]
    fn capture_disable_ignored_cleanup_delete_rolls_back_all_transition_state() {
        let mut db = fixture();
        let event_hash = "1".repeat(64);
        let digest = "2".repeat(64);
        commit(&mut db, &request("next", "", &[(&event_hash, &digest)])).unwrap();
        let before = activation_source_state(&db, "source-one").unwrap();
        let evidence_before: i64 = db
            .query_row("SELECT COUNT(*) FROM plugin_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        db.execute_batch(
            "CREATE TRIGGER ignore_capture_evidence_delete BEFORE DELETE ON plugin_evidence_v1 \
             WHEN OLD.source_id='source-one' BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        let req = capture_request(
            "f123456789abcdef0123456789abcdef",
            &source_lease(),
            false,
            true,
            "next",
            1,
        );
        assert!(set_capture(&mut db, &req).is_err());
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (Some(1), None)
        );
        let evidence_after: i64 = db
            .query_row("SELECT COUNT(*) FROM plugin_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(evidence_after, evidence_before);
        assert_eq!(evidence_after, 1);
        let receipt_table_exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' \
                 AND name='storage_source_capture_set_v2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let receipts = if receipt_table_exists {
            db.query_row(
                "SELECT COUNT(*) FROM storage_source_capture_set_v2 WHERE request_id=?",
                [&req.request_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
        } else {
            0
        };
        assert_eq!(receipts, 0);
    }

    #[test]
    fn capture_exact_retry_after_later_transition_returns_original_without_mutation() {
        let mut db = fixture();
        let first_request = capture_request(
            "0123456789abcdef0123456789abcdef",
            &source_lease(),
            false,
            true,
            "",
            0,
        );
        let first = set_capture(&mut db, &first_request).unwrap();
        let later_request = capture_request(
            "1123456789abcdef0123456789abcdef",
            &first.lease,
            true,
            false,
            "",
            1,
        );
        set_capture(&mut db, &later_request).unwrap();
        let current_source = activation_source_state(&db, "source-one").unwrap();
        let current_markers = source_marker_state(&db, "source-one").unwrap();
        let replay = set_capture(&mut db, &first_request).unwrap();
        assert_eq!(replay.disposition, "replay");
        assert_eq!(replay.lease, first.lease);
        assert!(!replay.enabled);
        assert_eq!(replay.dirty_seq, 1);
        assert_eq!(
            activation_source_state(&db, "source-one"),
            Some(current_source)
        );
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            current_markers
        );

        let changed = capture_request(
            &first_request.request_id,
            &source_lease(),
            true,
            true,
            "",
            0,
        );
        assert!(set_capture(&mut db, &changed).is_err());
    }

    #[test]
    fn capture_request_ids_conflict_bidirectionally_with_all_existing_operations() {
        let id = "2123456789abcdef0123456789abcdef";

        let mut db = fixture();
        activate(&mut db, &activation_request(id, "source-one")).unwrap();
        assert!(
            set_capture(
                &mut db,
                &capture_request(id, &source_lease(), true, true, "", 0)
            )
            .unwrap_err()
            .to_string()
            .contains("conflicting v2 request replay identity")
        );

        let mut db = fixture();
        let typed = lifecycle_v2_request(id, &source_lease(), "", "next");
        let admission = request("next", "", &[]);
        let replay = typed.replay_identity().unwrap();
        commit_guarded(&mut db, &admission, None, Some(&replay)).unwrap();
        assert!(
            set_capture(
                &mut db,
                &capture_request(id, &source_lease(), true, true, "", 0)
            )
            .unwrap_err()
            .to_string()
            .contains("conflicting v2 request replay identity")
        );

        let mut db = fixture();
        let mut revocation = revoke_request(0);
        revocation.request_id = id.into();
        if let v2::Operation::RevokeSource { lease, cursor, .. } = &mut revocation.operation {
            *lease = source_lease();
            cursor.clear();
        }
        revoke(&mut db, &revocation).unwrap();
        assert!(
            set_capture(
                &mut db,
                &capture_request(id, &source_lease(), true, true, "", 0)
            )
            .unwrap_err()
            .to_string()
            .contains("conflicting v2 request replay identity")
        );

        let mut db = fixture();
        set_capture(
            &mut db,
            &capture_request(id, &source_lease(), true, true, "", 0),
        )
        .unwrap();
        assert!(
            activate(&mut db, &activation_request(id, "source-one"))
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 request replay identity")
        );

        let mut db = fixture();
        set_capture(
            &mut db,
            &capture_request(id, &source_lease(), true, true, "", 0),
        )
        .unwrap();
        let typed = lifecycle_v2_request(id, &source_lease(), "", "next");
        let admission = request("next", "", &[]);
        let replay = typed.replay_identity().unwrap();
        assert!(
            commit_guarded(&mut db, &admission, None, Some(&replay))
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 request replay identity")
        );

        let mut db = fixture();
        set_capture(
            &mut db,
            &capture_request(id, &source_lease(), true, true, "", 0),
        )
        .unwrap();
        let mut revocation = revoke_request(0);
        revocation.request_id = id.into();
        if let v2::Operation::RevokeSource { lease, cursor, .. } = &mut revocation.operation {
            *lease = source_lease();
            cursor.clear();
        }
        assert!(
            revoke(&mut db, &revocation)
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 request replay identity")
        );
    }

    #[test]
    fn capture_rejects_malformed_enabled_flag_without_repairing_source() {
        let mut db = fixture();
        db.execute(
            "UPDATE plugin_source_cursor_v1 SET capture_enabled=2 WHERE source_id='source-one'",
            [],
        )
        .unwrap();
        let before = activation_source_state(&db, "source-one").unwrap();
        let req = capture_request(
            "3123456789abcdef0123456789abcdef",
            &source_lease(),
            false,
            true,
            "",
            0,
        );
        let error = set_capture(&mut db, &req).unwrap_err();
        assert!(error.to_string().contains("state is malformed"));
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        let receipt_exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' \
                 AND name='storage_source_capture_set_v2')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!receipt_exists);
    }

    #[test]
    fn capture_explicitly_reenables_revoked_source_without_restoring_evidence() {
        let mut db = fixture();
        let mut revoke_req = revoke_request(0);
        if let v2::Operation::RevokeSource { cursor, .. } = &mut revoke_req.operation {
            cursor.clear();
        }
        let revoked = revoke(&mut db, &revoke_req).unwrap();
        let revoked_lease = v2::SourceLease {
            epoch: revoked.lease_epoch,
            policy_epoch: revoked.policy_epoch,
            ..source_lease()
        };
        let req = capture_request(
            "4123456789abcdef0123456789abcdef",
            &revoked_lease,
            true,
            false,
            "",
            revoked.dirty_seq,
        );
        let enabled_result = set_capture(&mut db, &req).unwrap();
        assert_eq!(enabled_result.disposition, "committed");
        assert!(enabled_result.enabled);
        assert_ne!(enabled_result.lease.epoch, revoked_lease.epoch);
        assert_eq!(enabled_result.lease.policy_epoch, revoked.policy_epoch + 1);
        assert_eq!(enabled_result.dirty_seq, revoked.dirty_seq + 1);
        let enabled: i64 = db
            .query_row(
                "SELECT capture_enabled FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 1);
        let evidence: i64 = db
            .query_row("SELECT COUNT(*) FROM plugin_evidence_v1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(evidence, 0);
        assert!(commit(&mut db, &request("stale-after-reenable", "", &[])).is_err());
    }

    #[test]
    fn capture_rejects_stale_fences_private_state_and_epoch_overflow() {
        for (name, mutate_request) in [
            ("lease", 0usize),
            ("policy", 1),
            ("cursor", 2),
            ("dirty", 3),
            ("capture", 4),
        ] {
            let mut db = fixture();
            let mut lease = source_lease();
            let (enabled, expected_enabled, cursor, dirty_seq) = match mutate_request {
                0 => {
                    lease.epoch += 1;
                    (false, true, "", 0)
                }
                1 => {
                    lease.policy_epoch += 1;
                    (false, true, "", 0)
                }
                2 => (false, true, "stale-cursor", 0),
                3 => (false, true, "", 1),
                _ => (false, false, "", 0),
            };
            let req = capture_request(
                &format!("6{}23456789abcdef0123456789abcdef", name.len()),
                &lease,
                enabled,
                expected_enabled,
                cursor,
                dirty_seq,
            );
            assert!(set_capture(&mut db, &req).is_err(), "{name}");
            let source = activation_source_state(&db, "source-one").unwrap();
            assert_eq!(source.3, 7, "{name}");
            assert_eq!(source.4, 1, "{name}");
            assert_eq!(source.5, 1, "{name}");
            assert_eq!(source.10, 0, "{name}");
        }

        for unsupported in [
            "owner",
            "content",
            "member",
            "gap",
            "retention",
            "fingerprint",
        ] {
            let mut db = fixture();
            match unsupported {
                "owner" => db
                    .execute(
                        "INSERT INTO plugin_reference_owner_v1 VALUES('source-one','provider')",
                        [],
                    )
                    .unwrap(),
                "content" => db
                    .execute("INSERT INTO plugin_content_v1 VALUES('source-one')", [])
                    .unwrap(),
                "member" => db
                    .execute(
                        "INSERT INTO plugin_source_member_cursor_v1 VALUES('source-one','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','cursor-one')",
                        [],
                    )
                    .unwrap(),
                "gap" => db
                    .execute(
                        "INSERT INTO plugin_source_gap_invalidated_v1 VALUES('source-one')",
                        [],
                    )
                    .unwrap(),
                "retention" => db
                    .execute("INSERT INTO plugin_retention_v1 VALUES('source-one')", [])
                    .unwrap(),
                _ => db
                    .execute(
                        "UPDATE plugin_source_cursor_v1 SET content_fingerprint='private' \
                     WHERE source_id='source-one'",
                        [],
                    )
                    .unwrap(),
            };
            let req = capture_request(
                "7123456789abcdef0123456789abcdef",
                &source_lease(),
                false,
                true,
                "",
                0,
            );
            assert!(set_capture(&mut db, &req).is_err(), "{unsupported}");
        }

        for column in ["policy_epoch", "dirty_seq"] {
            let mut db = fixture();
            db.execute(
                &format!(
                    "UPDATE plugin_source_cursor_v1 SET {column}=? WHERE source_id='source-one'"
                ),
                [i64::MAX],
            )
            .unwrap();
            let mut lease = source_lease();
            let dirty_seq = if column == "policy_epoch" {
                lease.policy_epoch = i64::MAX;
                0
            } else {
                i64::MAX
            };
            let req = capture_request(
                "8123456789abcdef0123456789abcdef",
                &lease,
                false,
                true,
                "",
                dirty_seq,
            );
            assert!(set_capture(&mut db, &req).is_err(), "{column}");
        }
    }

    #[test]
    fn capture_ignored_mutated_and_namespace_trigger_writes_roll_back() {
        let request_id = "9123456789abcdef0123456789abcdef";
        let req = capture_request(request_id, &source_lease(), false, true, "", 0);

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER ignore_capture_source_update BEFORE UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN OLD.source_id='source-one' \
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("exactly one row")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER ignore_capture_dirty_insert BEFORE INSERT ON plugin_evidence_dirty_v1 \
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("dirty marker")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER ignore_capture_projection_insert BEFORE INSERT ON plugin_projection_cursor_v1 \
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("projection marker")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER mutate_capture_source_after_update AFTER UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN NEW.source_id='source-one' \
             BEGIN UPDATE plugin_source_cursor_v1 SET policy_epoch=99 WHERE source_id='source-one'; END;",
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("persisted exactly")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER mutate_capture_scratch_identity AFTER UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN NEW.source_id='source-one' \
             BEGIN UPDATE schema_meta SET value='disabled' \
                   WHERE key='storage_admission_scratch_v1'; END;",
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("explicit scratch")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let mut db = fixture();
        let before = activation_source_state(&db, "source-one").unwrap();
        db.execute_batch(
            "CREATE TRIGGER mutate_capture_receiver AFTER UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN NEW.source_id='source-one' \
             BEGIN UPDATE schema_meta SET value='receiver-two' WHERE key='receiver_id'; END;",
        )
        .unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("receiver identity changed")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        assert_eq!(
            source_marker_state(&db, "source-one").unwrap(),
            (None, None)
        );

        let mut db = fixture();
        db.execute_batch(
            "CREATE TABLE storage_admission_request_v2(\
             request_id TEXT PRIMARY KEY,receiver_id TEXT NOT NULL,request_digest TEXT NOT NULL,\
             source_id TEXT NOT NULL,cursor TEXT NOT NULL,batch_digest TEXT NOT NULL,\
             dirty_seq INTEGER NOT NULL,disposition TEXT NOT NULL);\
             CREATE TRIGGER collide_capture_request_id AFTER UPDATE OF lease_epoch \
             ON plugin_source_cursor_v1 WHEN NEW.source_id='source-one' BEGIN \
               INSERT INTO storage_admission_request_v2 VALUES('9123456789abcdef0123456789abcdef','receiver-one',\
                 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',\
                 'source-one','','eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',0,'committed'); END;",
        )
        .unwrap();
        let before = activation_source_state(&db, "source-one").unwrap();
        assert!(
            set_capture(&mut db, &req)
                .unwrap_err()
                .to_string()
                .contains("conflicting v2 request replay identity")
        );
        assert_eq!(activation_source_state(&db, "source-one"), Some(before));
        let collision_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM storage_admission_request_v2 \
                 WHERE request_id='9123456789abcdef0123456789abcdef'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(collision_count, 0);
    }

    #[test]
    fn concurrent_exact_capture_requests_commit_once_across_connections() {
        let path = std::env::temp_dir().join(format!(
            "agenturm-capture-race-{}-{}.db",
            std::process::id(),
            now_ns().unwrap()
        ));
        drop(fixture_at(&path));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            let req = capture_request(
                "5123456789abcdef0123456789abcdef",
                &source_lease(),
                false,
                true,
                "",
                0,
            );
            workers.push(thread::spawn(move || {
                let mut db = Connection::open(&path).unwrap();
                db.busy_timeout(Duration::from_secs(5)).unwrap();
                barrier.wait();
                set_capture(&mut db, &req).unwrap().disposition.to_owned()
            }));
        }
        barrier.wait();
        let mut dispositions: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        dispositions.sort();
        assert_eq!(dispositions, ["committed", "replay"]);
        let reopened = Connection::open(&path).unwrap();
        let source: (i64, i64, i64, i64) = reopened
            .query_row(
                "SELECT lease_epoch,policy_epoch,dirty_seq,capture_enabled \
                 FROM plugin_source_cursor_v1 WHERE source_id='source-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_ne!(source.0, 7);
        assert_eq!((source.1, source.2, source.3), (2, 1, 0));
        drop(reopened);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", path.display())));
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

    #[test]
    fn shared_fact_validator_preserves_lifecycle_rejection_reason() {
        let hash = "e".repeat(64);
        let digest = "f".repeat(64);
        let mut prepared = request("cursor-one", "", &[(&hash, &digest)]);
        prepared.records[0].payload_json = "{\"state\":\"started\",\"text\":\"prompt\"}".to_owned();
        let error = validate(&prepared, true).unwrap_err();
        assert_eq!(error.to_string(), "unsupported prepared lifecycle record");
    }
}
