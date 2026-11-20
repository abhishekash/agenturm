//! Read a source revision from an isolated Python Store backup.
//!
//! This is a diagnostic shadow reader. It never opens a database for writes,
//! publishes a query result, or decides whether a live Store is still current.
//! The Python caller must recheck its live lease/policy/projection fence.

#![forbid(unsafe_code)]

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
}

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
    })?)
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
        ] {
            assert!(inspect_json(input).is_err());
        }
        assert!(inspect_json(&" ".repeat(MAX_REQUEST_BYTES + 1)).is_err());
    }
}
