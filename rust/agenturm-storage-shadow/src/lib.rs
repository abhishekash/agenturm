//! Read a source revision from an isolated Python Store backup.
//!
//! This is a diagnostic shadow reader. It never opens a database for writes,
//! publishes a query result, or decides whether a live Store is still current.
//! The Python caller must recheck its live lease/policy/projection fence.

#![forbid(unsafe_code)]

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, value::RawValue};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    sqlite_path: String,
    source_id: String,
    #[serde(default)]
    include_projection: bool,
}

#[derive(Serialize)]
struct Revision {
    lease_epoch: String,
    policy_epoch: String,
    dirty_seq: String,
    capture_enabled: bool,
    projected_seq: Option<String>,
    projected_policy: Option<String>,
}

#[derive(Serialize)]
struct Certificate<'a> {
    schema_version: u32,
    receiver_id: &'a str,
    source_id: &'a str,
    state: &'static str,
    revision: Option<Revision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions: Option<Vec<Session>>,
}

#[derive(Serialize)]
struct Session {
    native_session_id: String,
    session_key: String,
    reported_state: String,
    run_state: String,
    reason_code: String,
    state_time_ns: i64,
    last_record_ns: i64,
    metadata_usage: BTreeMap<String, Value>,
    parent_native_session_id: String,
    relationship_kind: String,
    relationship_state: String,
}

const MAX_PROJECTION_SESSIONS: i64 = 2_000;
const MAX_PROJECTION_BYTES: i64 = 4 * 1024 * 1024;
const MAX_USAGE_BYTES: i64 = 4_096;
const MAX_JSON_SAFE_INTEGER: u128 = (1_u128 << 53) - 1;
// At most 50,000 admitted records can contribute one i64 usage observation
// each to a source; Python's reducer sums them with arbitrary-size integers.
const MAX_USAGE_SUM: u128 = 50_000_u128 * (i64::MAX as u128);

fn source_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn positive(value: i64, field: &'static str) -> Result<i64> {
    if value <= 0 {
        return Err(invalid(field));
    }
    Ok(value)
}

fn nonnegative(value: i64, field: &'static str) -> Result<i64> {
    if value < 0 {
        return Err(invalid(field));
    }
    Ok(value)
}

fn immutable_uri(path: &str) -> String {
    let mut uri = String::from("file:");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// Inspect one consistent, caller-supplied SQLite backup, never the live
/// writer. A ready certificate is still only historical until Python checks
/// the source revision against its current Store.
pub fn inspect_json(input: &str) -> Result<Value> {
    if input.len() > MAX_REQUEST_BYTES {
        return Err(invalid("shadow request exceeds byte limit"));
    }
    let request: Request = serde_json::from_str(input)?;
    if request.schema_version != 1
        || !source_id(&request.source_id)
        || request.sqlite_path.is_empty()
        || request.sqlite_path.len() > 4096
        || !Path::new(&request.sqlite_path).is_absolute()
    {
        return Err(invalid("invalid shadow request"));
    }
    let path = Path::new(&request.sqlite_path);
    let file = path.symlink_metadata()?;
    if !file.file_type().is_file() {
        return Err(invalid("shadow backup must be a regular file"));
    }
    // Immutable mode ignores WAL contents. Require a self-contained backup,
    // never a database with uncheckpointed sidecars.
    if ["-wal", "-shm", "-journal"]
        .into_iter()
        .any(|suffix| Path::new(&format!("{}{suffix}", request.sqlite_path)).exists())
    {
        return Err(invalid("shadow backup has SQLite sidecars"));
    }
    // SQLite can create -wal/-shm files even on a read-only WAL database.
    // An isolated, immutable backup avoids those side effects.
    let db = Connection::open_with_flags(
        immutable_uri(&request.sqlite_path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
    let result = inspect_snapshot(&db, &request);
    // Release the read snapshot promptly, including on malformed schema.
    let close = db.execute_batch("ROLLBACK;");
    close?;
    result
}

fn inspect_snapshot(db: &Connection, request: &Request) -> Result<Value> {
    let receiver: String = db.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |row| row.get(0),
    )?;
    if receiver.is_empty() || receiver.len() > 256 || receiver.chars().any(char::is_control) {
        return Err(invalid("invalid receiver identity"));
    }
    type SourceRow = (String, i64, i64, i64, i64, Option<i64>, Option<i64>);
    let row: Option<SourceRow> = db
        .query_row(
            "SELECT c.receiver_id, c.lease_epoch, c.policy_epoch, c.dirty_seq, \
             c.capture_enabled, p.projected_seq, p.policy_epoch \
             FROM plugin_source_cursor_v1 c LEFT JOIN plugin_projection_cursor_v1 p \
             USING(source_id) WHERE c.source_id=?",
            params![request.source_id],
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
        )
        .optional()?;
    let Some((owner, lease, policy, dirty, capture, projected, projected_policy)) = row else {
        return Ok(serde_json::to_value(Certificate {
            schema_version: 1,
            receiver_id: &receiver,
            source_id: &request.source_id,
            state: "unknown",
            revision: None,
            sessions: None,
        })?);
    };
    if owner != receiver {
        return Err(invalid("source receiver differs from Store"));
    }
    let lease = positive(lease, "invalid source lease epoch")?;
    let policy = positive(policy, "invalid source policy epoch")?;
    let dirty = nonnegative(dirty, "invalid source dirty sequence")?;
    if !matches!(capture, 0 | 1) {
        return Err(invalid("invalid source capture flag"));
    }
    let projected = projected
        .map(|value| nonnegative(value, "invalid projected sequence"))
        .transpose()?;
    let projected_policy = projected_policy
        .map(|value| positive(value, "invalid projected policy epoch"))
        .transpose()?;
    if projected.is_some() != projected_policy.is_some() {
        return Err(invalid("incomplete source projection revision"));
    }
    let ready =
        capture == 1 && dirty > 0 && projected == Some(dirty) && projected_policy == Some(policy);
    let state = if capture == 0 {
        "disabled"
    } else if ready {
        "ready"
    } else {
        "pending"
    };
    let sessions = if request.include_projection && ready {
        Some(read_projection(
            db,
            &receiver,
            &request.source_id,
            policy,
            dirty,
        )?)
    } else {
        None
    };
    Ok(serde_json::to_value(Certificate {
        schema_version: 1,
        receiver_id: &receiver,
        source_id: &request.source_id,
        state,
        revision: Some(Revision {
            lease_epoch: lease.to_string(),
            policy_epoch: policy.to_string(),
            dirty_seq: dirty.to_string(),
            capture_enabled: capture == 1,
            projected_seq: projected.map(|value| value.to_string()),
            projected_policy: projected_policy.map(|value| value.to_string()),
        }),
        sessions,
    })?)
}

fn code(value: &str) -> bool {
    value.len() <= 64
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

struct Usage(BTreeMap<String, Value>);

impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct UsageVisitor;

        impl<'de> Visitor<'de> for UsageVisitor {
            type Value = Usage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object of exact nonnegative token counters")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut access: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut counters = BTreeMap::new();
                while let Some((name, raw)) = access.next_entry::<String, Box<RawValue>>()? {
                    if !matches!(
                        name.as_str(),
                        "input_tokens"
                            | "output_tokens"
                            | "cache_read_tokens"
                            | "cache_write_tokens"
                            | "total_tokens"
                    ) {
                        return Err(de::Error::custom("unknown stored usage counter"));
                    }
                    let token = raw.get();
                    if token.is_empty()
                        || token.len() > 26
                        || token.starts_with('0') && token != "0"
                        || !token.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return Err(de::Error::custom("invalid stored usage integer"));
                    }
                    let value = token
                        .parse::<u128>()
                        .map_err(|_| de::Error::custom("stored usage integer exceeds u128"))?;
                    if value > MAX_USAGE_SUM {
                        return Err(de::Error::custom("stored usage exceeds protocol ceiling"));
                    }
                    let exact = if value <= MAX_JSON_SAFE_INTEGER {
                        Value::from(value as u64)
                    } else {
                        Value::from(value.to_string())
                    };
                    if counters.insert(name, exact).is_some() {
                        return Err(de::Error::custom("duplicate stored usage counter"));
                    }
                }
                Ok(Usage(counters))
            }
        }

        deserializer.deserialize_map(UsageVisitor)
    }
}

fn session_key(receiver: &str, source: &str, native: &str) -> String {
    let identity = format!("{receiver}\0{source}\0{native}");
    format!("plugin:{:x}", Sha256::digest(identity.as_bytes()))
}

fn read_projection(
    db: &Connection,
    receiver: &str,
    source: &str,
    policy: i64,
    dirty: i64,
) -> Result<Vec<Session>> {
    // Check lengths in SQLite before bringing any stored text into Rust. The
    // LIMIT detects oversize sources without reading their entire projection.
    let (
        count,
        bytes,
        usage_max,
        native_max,
        key_max,
        state_max,
        reason_max,
        parent_max,
        relation_kind_max,
        relation_state_max,
    ): (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = db.query_row(
        r#"SELECT COUNT(*), COALESCE(SUM(
                COALESCE(length(CAST(native_session_id AS BLOB)),0)+
                COALESCE(length(CAST(session_key AS BLOB)),0)+
                COALESCE(length(CAST(state AS BLOB)),0)+
                COALESCE(length(CAST(reason_code AS BLOB)),0)+
                COALESCE(length(CAST(usage_json AS BLOB)),0)+
                COALESCE(length(CAST(parent_native_session_id AS BLOB)),0)+
                COALESCE(length(CAST(relationship_kind AS BLOB)),0)+
                COALESCE(length(CAST(relationship_state AS BLOB)),0)),0),
                COALESCE(MAX(length(CAST(usage_json AS BLOB))),0),
                COALESCE(MAX(length(CAST(native_session_id AS BLOB))),0),
                COALESCE(MAX(length(CAST(session_key AS BLOB))),0),
                COALESCE(MAX(length(CAST(state AS BLOB))),0),
                COALESCE(MAX(length(CAST(reason_code AS BLOB))),0),
                COALESCE(MAX(length(CAST(parent_native_session_id AS BLOB))),0),
                COALESCE(MAX(length(CAST(relationship_kind AS BLOB))),0),
                COALESCE(MAX(length(CAST(relationship_state AS BLOB))),0)
             FROM (SELECT native_session_id,session_key,state,reason_code,usage_json,
                          parent_native_session_id,relationship_kind,relationship_state
                   FROM plugin_session_projection_v1 WHERE source_id=? LIMIT 2001)"#,
        params![source],
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
            ))
        },
    )?;
    if count > MAX_PROJECTION_SESSIONS
        || bytes > MAX_PROJECTION_BYTES
        || usage_max > MAX_USAGE_BYTES
        || native_max > 256
        || key_max > 71
        || state_max > 16
        || reason_max > 64
        || parent_max > 256
        || relation_kind_max > 16
        || relation_state_max > 16
    {
        return Err(invalid("stored projection exceeds bounded read budget"));
    }
    let mut statement = db.prepare(
        r#"SELECT native_session_id,session_key,state,reason_code,state_time_ns,
                  last_record_ns,policy_epoch,source_dirty_seq,usage_json,
                  parent_native_session_id,relationship_kind,relationship_state
           FROM plugin_session_projection_v1 WHERE source_id=?
           ORDER BY native_session_id LIMIT 2001"#,
    )?;
    let mut rows = statement.query(params![source])?;
    let mut result = Vec::with_capacity(count as usize);
    while let Some(row) = rows.next()? {
        let native: String = row.get(0)?;
        let key: String = row.get(1)?;
        let state: String = row.get(2)?;
        let reason: String = row.get(3)?;
        let state_time: i64 = row.get(4)?;
        let last_record: i64 = row.get(5)?;
        let row_policy: i64 = row.get(6)?;
        let row_dirty: i64 = row.get(7)?;
        let usage_json: String = row.get(8)?;
        let parent: String = row.get(9)?;
        let relation_kind: String = row.get(10)?;
        let relation_state: String = row.get(11)?;
        if !source_id(&native)
            || key != session_key(receiver, source, &native)
            || !matches!(
                state.as_str(),
                "started"
                    | "running"
                    | "paused"
                    | "completed"
                    | "failed"
                    | "interrupted"
                    | "cancelled"
                    | "unknown"
            )
            || !reason.is_empty() && !code(&reason)
            || state_time < 0
            || last_record < 0
            || row_policy != policy
            || row_dirty != dirty
            || !parent.is_empty() && !source_id(&parent)
            || !matches!(relation_kind.as_str(), "delegated" | "review" | "unknown")
            || !matches!(
                relation_state.as_str(),
                "unresolved" | "confirmed" | "invalid" | "conflicted"
            )
        {
            return Err(invalid("invalid or stale stored projection row"));
        }
        let Usage(metadata_usage) = serde_json::from_str(&usage_json)?;
        let run_state = match state.as_str() {
            "started" => "running",
            "paused" => "waiting",
            "cancelled" => "interrupted",
            other => other,
        }
        .to_string();
        result.push(Session {
            native_session_id: native,
            session_key: key,
            reported_state: state,
            run_state,
            reason_code: reason,
            state_time_ns: state_time,
            last_record_ns: last_record,
            metadata_usage,
            parent_native_session_id: parent,
            relationship_kind: relation_kind,
            relationship_state: relation_state,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_requests_fail_before_open() {
        for input in [
            r#"{"schema_version":2,"sqlite_path":"/tmp/missing","source_id":"source"}"#,
            r#"{"schema_version":1,"sqlite_path":"relative.db","source_id":"source"}"#,
            r#"{"schema_version":1,"sqlite_path":"/tmp/missing","source_id":"bad source"}"#,
            r#"{"schema_version":1,"sqlite_path":"/tmp/missing","source_id":"source","extra":1}"#,
            r#"{"schema_version":1,"sqlite_path":"/tmp/missing","source_id":"source","include_projection":"yes"}"#,
        ] {
            assert!(inspect_json(input).is_err());
        }
        assert!(inspect_json(&" ".repeat(MAX_REQUEST_BYTES + 1)).is_err());
    }
}
