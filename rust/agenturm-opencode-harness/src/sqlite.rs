//! The source DB is opened read-only. Each poll has one SQLite snapshot and
//! pages complete rows; only coarse facts cross the process boundary.

use agenturm_source_process::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

const MAX_ROW_BYTES: i64 = 64 * 1024;
const MAX_SCAN_ROWS: usize = 256;
const MAX_STRUCTURAL_TOKENS: usize = 8192;
const MAX_SOURCE_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SOURCE_ROWS: i64 = 50_000;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}
fn digest(raw: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(raw))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cursor {
    legacy: bool,
    epoch: String,
    signature: String,
    identities: String,
    revision: u32,
    phase: char,
    rowid: i64,
    message_max: i64,
    part_max: i64,
    session_max: i64,
    message_count: i64,
    part_count: i64,
    session_count: i64,
}

impl Cursor {
    fn parse(raw: &str) -> Result<Option<Self>> {
        if raw.is_empty() {
            return Ok(None);
        }
        let parts: Vec<_> = raw.split(':').collect();
        let legacy = parts.first() == Some(&"oc1");
        if !matches!(parts.first().copied(), Some("oc1" | "oc2"))
            || parts.len() != if legacy { 12 } else { 13 }
            || ![parts[1], parts[2]].iter().all(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
            || (!legacy
                && (parts[3].len() != 64
                    || !parts[3]
                        .bytes()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())))
            || !matches!(parts[if legacy { 4 } else { 5 }], "m" | "p" | "s" | "d")
        {
            return Err(invalid("invalid OpenCode cursor"));
        }
        let offset = usize::from(!legacy);
        let revision: u32 = parts[3 + offset].parse()?;
        let rowid: i64 = parts[5 + offset].parse()?;
        let message_max: i64 = parts[6 + offset].parse()?;
        let part_max: i64 = parts[7 + offset].parse()?;
        let session_max: i64 = parts[8 + offset].parse()?;
        let message_count: i64 = parts[9 + offset].parse()?;
        let part_count: i64 = parts[10 + offset].parse()?;
        let session_count: i64 = parts[11 + offset].parse()?;
        if revision > i32::MAX as u32
            || rowid < 0
            || message_max < 0
            || part_max < 0
            || session_max < 0
            || message_count < 0
            || part_count < 0
            || session_count < 0
        {
            return Err(invalid("invalid OpenCode cursor range"));
        }
        Ok(Some(Self {
            legacy,
            epoch: parts[1].to_owned(),
            signature: parts[2].to_owned(),
            identities: if legacy {
                String::new()
            } else {
                parts[3].to_owned()
            },
            revision,
            phase: parts[4 + offset].chars().next().ok_or("missing phase")?,
            rowid,
            message_max,
            part_max,
            session_max,
            message_count,
            part_count,
            session_count,
        }))
    }

    fn encode(&self) -> String {
        format!(
            "oc2:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.epoch,
            self.signature,
            self.identities,
            self.revision,
            self.phase,
            self.rowid,
            self.message_max,
            self.part_max,
            self.session_max,
            self.message_count,
            self.part_count,
            self.session_count
        )
    }
}

#[cfg(unix)]
fn file_mark(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid("OpenCode source is not a regular file"));
    }
    if metadata.len() > MAX_SOURCE_FILE_BYTES {
        return Err(invalid("OpenCode source file ceiling exceeded"));
    }
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(format!(
        "{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        modified,
        metadata.ctime_nsec() as i128 + metadata.ctime() as i128 * 1_000_000_000
    ))
}

#[cfg(not(unix))]
compile_error!("The OpenCode source adapter currently supports POSIX platforms only");

fn fingerprint(path: &Path) -> Result<(String, String)> {
    let main = file_mark(path)?;
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name()
            .ok_or("invalid DB filename")?
            .to_string_lossy()
    ));
    let wal_mark = if wal.exists() {
        file_mark(&wal)?
    } else {
        String::new()
    };
    // The epoch changes only when the database file identity changes. The
    // ctime catches ordinary rewrites that restore mtime and length. A
    // same-user actor able to forge all file metadata is outside this source
    // trust boundary; full byte hashing on every idle poll is not performed.
    let epoch = digest(main.split(':').take(2).collect::<Vec<_>>().join(":"));
    Ok((epoch, digest(format!("{main}|{wal_mark}"))))
}

fn preflight_sidecars(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 20];
    file.read_exact(&mut header)?;
    if &header[..16] != b"SQLite format 3\0" {
        return Err(invalid("invalid SQLite source header"));
    }
    if header[18] == 2 {
        let name = path
            .file_name()
            .ok_or("invalid DB filename")?
            .to_string_lossy();
        let wal = path.with_file_name(format!("{name}-wal"));
        let shm = path.with_file_name(format!("{name}-shm"));
        if !wal.is_file() || !shm.is_file() {
            return Err(invalid("WAL source requires existing sidecars"));
        }
    }
    Ok(())
}

fn columns(db: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = db.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(stmt
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?)
}

fn check_schema(db: &Connection) -> Result<()> {
    for (table, required) in [
        ("session_v2", &["id", "time_created", "time_updated"][..]),
        (
            "session_message",
            &[
                "id",
                "session_id",
                "type",
                "seq",
                "time_created",
                "time_updated",
                "data",
            ],
        ),
        (
            "session_part",
            &[
                "message_id",
                "session_id",
                "ordinal",
                "type",
                "call_id",
                "time_created",
                "time_updated",
                "data",
            ],
        ),
    ] {
        let found = columns(db, table)?;
        if !required.iter().all(|column| found.contains(*column)) {
            return Err(invalid("unsupported OpenCode SQLite layout"));
        }
    }
    Ok(())
}

fn bounded_count(db: &Connection, table: &str, remaining: i64) -> Result<i64> {
    let mut stmt = db.prepare(&format!("SELECT 1 FROM {table} LIMIT ?1"))?;
    let mut rows = stmt.query([remaining + 1])?;
    let mut count = 0;
    while rows.next()?.is_some() {
        count += 1;
        if count > remaining {
            return Err(invalid("OpenCode source row ceiling exceeded"));
        }
    }
    Ok(count)
}

fn identity_digest(db: &Connection, maxima: (i64, i64, i64)) -> Result<String> {
    // A changed file signature starts a sweep. Counts alone cannot prove that
    // its rows are the same: DELETE plus INSERT may preserve every count.
    // SQLite evaluates the length guard before returning variable-length IDs,
    // so an oversized native identity never crosses this boundary.
    const MESSAGE: &str = "SELECT rowid,length(CAST(session_id AS BLOB)),CASE WHEN length(CAST(session_id AS BLOB)) BETWEEN 1 AND 256 THEN session_id END,length(CAST(id AS BLOB)),CASE WHEN length(CAST(id AS BLOB)) BETWEEN 1 AND 512 THEN id END,length(CAST(type AS BLOB)),CASE WHEN length(CAST(type AS BLOB)) BETWEEN 1 AND 32 THEN type END,0 FROM session_message WHERE rowid<=?1 ORDER BY rowid LIMIT 50001";
    const PART: &str = "SELECT rowid,length(CAST(session_id AS BLOB)),CASE WHEN length(CAST(session_id AS BLOB)) BETWEEN 1 AND 256 THEN session_id END,length(CAST(message_id AS BLOB)),CASE WHEN length(CAST(message_id AS BLOB)) BETWEEN 1 AND 512 THEN message_id END,length(CAST(type AS BLOB)),CASE WHEN length(CAST(type AS BLOB)) BETWEEN 1 AND 32 THEN type END,ordinal FROM session_part WHERE rowid<=?1 ORDER BY rowid LIMIT 50001";
    const SESSION: &str = "SELECT rowid,length(CAST(id AS BLOB)),CASE WHEN length(CAST(id AS BLOB)) BETWEEN 1 AND 256 THEN id END,0,NULL,0,NULL,0 FROM session_v2 WHERE rowid<=?1 ORDER BY rowid LIMIT 50001";
    let mut hasher = Sha256::new();
    let mut total = 0usize;
    for (tag, sql, maximum) in [
        (b'm', MESSAGE, maxima.0),
        (b'p', PART, maxima.1),
        (b's', SESSION, maxima.2),
    ] {
        let mut stmt = db.prepare(sql)?;
        let mut rows = stmt.query([maximum])?;
        while let Some(row) = rows.next()? {
            total += 1;
            if total > MAX_SOURCE_ROWS as usize {
                return Err(invalid("OpenCode source row ceiling exceeded"));
            }
            let rowid: i64 = row.get(0)?;
            let order: i64 = row.get(7)?;
            hasher.update([tag]);
            hasher.update(rowid.to_be_bytes());
            hasher.update(order.to_be_bytes());
            for offset in [1, 3, 5] {
                let length: i64 = row.get(offset)?;
                let value: Option<String> = row.get(offset + 1)?;
                hasher.update(length.to_be_bytes());
                if let Some(value) = value {
                    hasher.update(value.as_bytes());
                }
            }
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn structural_tokens(raw: &[u8]) -> usize {
    let (mut quoted, mut escaped, mut count) = (false, false, 0);
    for byte in raw {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else if *byte == b'"' {
            quoted = true;
            count += 1;
        } else if matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':') {
            count += 1;
        }
    }
    count
}

fn batch(source_id: &str, cursor: &str, records: &[Value]) -> Value {
    json!({"type":"batch","protocol":1,"source_id":source_id,"cursor":cursor,"records":records})
}

fn stamp(ms: i64) -> Result<String> {
    let ns = ms
        .checked_mul(1_000_000)
        .ok_or("OpenCode timestamp overflow")?;
    if ns < 0 {
        return Err(invalid("OpenCode timestamp before epoch"));
    }
    Ok(ns.to_string())
}

fn opaque(parts: &[&str]) -> String {
    digest(parts.join("\0"))
}

fn wire_id(epoch: &str, table: &str, session: &str, row: &str, kind: &str) -> String {
    format!("oc:{}", opaque(&[epoch, table, session, row, kind]))
}

fn identity(session: &str, operation: Option<&str>) -> Value {
    let mut id = json!({"session_id":session});
    if let Some(operation) = operation {
        id["operation_id"] = json!(operation);
    }
    id
}

struct FactScope<'a> {
    epoch: &'a str,
    table: &'a str,
    session: &'a str,
    row: &'a str,
    revision: u32,
    time_ns: &'a str,
    operation: Option<&'a str>,
}

impl FactScope<'_> {
    fn record(&self, kind: &str, payload: Value) -> Value {
        json!({"event_id":wire_id(self.epoch,self.table,self.session,self.row,kind),
            "revision":self.revision,"kind":kind,"time_ns":self.time_ns,
            "identity":identity(self.session,self.operation),"payload":payload})
    }

    fn diagnostic(&self, code: &str) -> Value {
        json!({"event_id":wire_id(self.epoch,self.table,self.session,self.row,"diagnostic"),
            "revision":self.revision,"kind":"diagnostic","time_ns":self.time_ns,
            "payload":{"code":code,"severity":"warning"}})
    }
}

struct SourceRow<'a> {
    session: &'a str,
    id: &'a str,
    kind: &'a str,
    order: i64,
    updated: i64,
    data: &'a str,
}

fn diagnostic(
    epoch: &str,
    table: &str,
    session: &str,
    row: &str,
    revision: u32,
    time_ns: &str,
    code: &str,
) -> Value {
    json!({"event_id":wire_id(epoch,table,session,row,"diagnostic"),"revision":revision,
        "kind":"diagnostic","time_ns":time_ns,"payload":{"code":code,"severity":"warning"}})
}

fn usage(raw: &Value) -> Result<Option<Value>> {
    let Some(tokens) = raw.get("tokens").and_then(Value::as_object) else {
        return Ok(None);
    };
    let cache = tokens.get("cache").and_then(Value::as_object);
    let number = |value: Option<&Value>| -> Result<Option<u64>> {
        match value {
            None => Ok(None),
            Some(v) => {
                let n = v.as_u64().ok_or("invalid OpenCode usage counter")?;
                if n > i64::MAX as u64 {
                    return Err(invalid("OpenCode usage counter overflow"));
                }
                Ok(Some(n))
            }
        }
    };
    let input = number(tokens.get("input"))?;
    let output = number(tokens.get("output"))?;
    let reasoning = number(tokens.get("reasoning"))?;
    let read = number(cache.and_then(|c| c.get("read")))?;
    let write = number(cache.and_then(|c| c.get("write")))?;
    let mut result = serde_json::Map::new();
    if let Some(value) = read {
        result.insert("cache_read_tokens".into(), json!(value));
    }
    if let Some(value) = write {
        result.insert("cache_write_tokens".into(), json!(value));
    }
    if let (Some(i), Some(r), Some(w)) = (input, read, write) {
        result.insert(
            "input_tokens".into(),
            json!(
                i.checked_add(r)
                    .and_then(|v| v.checked_add(w))
                    .filter(|v| *v <= i64::MAX as u64)
                    .ok_or("OpenCode input usage overflow")?
            ),
        );
    }
    if let (Some(o), Some(r)) = (output, reasoning) {
        result.insert(
            "output_tokens".into(),
            json!(
                o.checked_add(r)
                    .filter(|v| *v <= i64::MAX as u64)
                    .ok_or("OpenCode output usage overflow")?
            ),
        );
    }
    if let (Some(i), Some(o)) = (
        result.get("input_tokens").and_then(Value::as_u64),
        result.get("output_tokens").and_then(Value::as_u64),
    ) && let Some(total) = i.checked_add(o).filter(|v| *v <= i64::MAX as u64)
    {
        result.insert("total_tokens".into(), json!(total));
    }
    Ok((!result.is_empty()).then_some(Value::Object(result)))
}

fn message_facts(cursor: &Cursor, row: SourceRow<'_>) -> Result<Vec<Value>> {
    if row.session.is_empty()
        || row.session.len() > 256
        || row.id.is_empty()
        || row.id.len() > 512
        || row.order < 0
    {
        return Err(invalid("invalid OpenCode message identity"));
    }
    let raw: Value = serde_json::from_str(row.data)?;
    let time_ns = stamp(row.updated)?;
    let scope = FactScope {
        epoch: &cursor.epoch,
        table: "message",
        session: row.session,
        row: row.id,
        revision: cursor.revision,
        time_ns: &time_ns,
        operation: None,
    };
    let facts = match row.kind {
        "user" => vec![scope.record("message", json!({"role":"user"}))],
        "assistant" => {
            let mut facts = vec![scope.record("message", json!({"role":"assistant"}))];
            if let Some(counters) = usage(&raw)? {
                facts.push(scope.record("usage", counters));
            }
            facts
        }
        "idle" => Vec::new(),
        _ => vec![scope.diagnostic("unsupported_opencode_message")],
    };
    Ok(facts)
}

fn category(name: &str) -> &'static str {
    if name.starts_with("mcp__") {
        "mcp"
    } else if matches!(
        name.to_ascii_lowercase().as_str(),
        "shell" | "bash" | "exec" | "run"
    ) {
        "shell"
    } else if matches!(
        name.to_ascii_lowercase().as_str(),
        "read" | "write" | "edit" | "patch"
    ) {
        "file"
    } else {
        "other"
    }
}

fn part_facts(cursor: &Cursor, row: SourceRow<'_>) -> Result<Vec<Value>> {
    if row.order < 0
        || row.session.is_empty()
        || row.session.len() > 256
        || row.id.is_empty()
        || row.id.len() > 512
    {
        return Err(invalid("invalid OpenCode part identity"));
    }
    if row.kind != "tool" {
        return Ok(Vec::new());
    }
    let raw: Value = serde_json::from_str(row.data)?;
    let time_ns = stamp(row.updated)?;
    let part_id = format!("{}:{}", row.id, row.order);
    let operation = opaque(&[&cursor.epoch, row.session, row.id, &row.order.to_string()]);
    let scope = FactScope {
        epoch: &cursor.epoch,
        table: "part",
        session: row.session,
        row: &part_id,
        revision: cursor.revision,
        time_ns: &time_ns,
        operation: Some(&operation),
    };
    let state = raw.get("state").and_then(Value::as_object);
    let status = state
        .and_then(|v| v.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let metadata = state.and_then(|v| v.get("metadata"));
    let exit = metadata.and_then(|v| v.get("exit")).and_then(Value::as_i64);
    let name = raw.get("name").and_then(Value::as_str).unwrap_or("");
    let shell = matches!(name.to_ascii_lowercase().as_str(), "shell" | "bash");
    let background = shell
        && metadata
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
            == Some("running");
    let failed = shell
        && (exit.is_some_and(|v| v != 0)
            || metadata
                .and_then(|v| v.get("timeout"))
                .and_then(Value::as_bool)
                == Some(true)
            || metadata
                .and_then(|v| v.get("signal"))
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty()));
    let result = match status {
        "pending" | "running" => "started",
        "error" | "failed" => "failed",
        "denied" => "denied",
        "completed" if background => "started",
        "completed" if failed => "failed",
        "completed" => "succeeded",
        _ => return Ok(vec![scope.diagnostic("opencode_tool_result_unknown")]),
    };
    Ok(vec![scope.record(
        "tool",
        json!({"category":category(name),"result":result}),
    )])
}

fn session_fact(db: &Connection, cursor: &Cursor, session: &str) -> Result<Vec<Value>> {
    if session.is_empty() || session.len() > 256 {
        return Err(invalid("invalid OpenCode session identity"));
    }
    let latest: Option<(String,i64,Option<String>)> = db.query_row(
        "SELECT substr(type,1,33),time_updated,CASE WHEN length(CAST(data AS BLOB))<=65536 AND json_valid(data) THEN substr(json_extract(data,'$.outcome'),1,65) END FROM session_message WHERE session_id=?1 AND rowid<=?2 AND type IN ('user','assistant','idle') ORDER BY seq DESC,id DESC LIMIT 1",
        params![session,cursor.message_max],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((role, updated, outcome)) = latest else {
        return Ok(Vec::new());
    };
    let time_ns = stamp(updated)?;
    let scope = FactScope {
        epoch: &cursor.epoch,
        table: "session",
        session,
        row: session,
        revision: cursor.revision,
        time_ns: &time_ns,
        operation: None,
    };
    let (state, reason) = match role.as_str() {
        "user" => ("running", "opencode_latest_user"),
        "assistant" => ("running", "opencode_latest_assistant"),
        "idle" => match outcome.as_deref() {
            Some("succeeded") => ("completed", "opencode_idle_succeeded"),
            Some("failed") => ("failed", "opencode_idle_failed"),
            Some("interrupted") => ("interrupted", "opencode_idle_interrupted"),
            _ => ("paused", "opencode_idle_outcome_unknown"),
        },
        _ => return Ok(vec![scope.diagnostic("opencode_latest_role_unknown")]),
    };
    Ok(vec![scope.record(
        "lifecycle",
        json!({"state":state,"reason_code":reason}),
    )])
}

pub fn poll(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    let previous = Cursor::parse(raw_cursor)?;
    let (epoch, signature) = match fingerprint(path) {
        Ok(value) => value,
        Err(error)
            if previous.is_none()
                && error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(batch(source_id, raw_cursor, &[]));
        }
        Err(_) => return Err(invalid("OpenCode source unavailable")),
    };
    let replacement = previous.as_ref().is_some_and(|c| c.epoch != epoch);
    let sweep_finished = previous.as_ref().is_some_and(|c| c.phase == 'd');
    if sweep_finished
        && previous
            .as_ref()
            .is_some_and(|c| c.signature == signature && !replacement && !c.legacy)
    {
        return Ok(batch(source_id, raw_cursor, &[]));
    }
    preflight_sidecars(path)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_millis(250))?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN")?;
    check_schema(&db)?;
    let start_sweep = previous.is_none()
        || replacement
        || sweep_finished
        || previous.as_ref().is_some_and(|c| c.legacy);
    let prior_counts = previous
        .as_ref()
        .map(|c| (c.message_count, c.part_count, c.session_count));
    let lost_prior_identity = if start_sweep {
        if let Some(prior) = previous.as_ref().filter(|c| !c.legacy && !replacement) {
            identity_digest(&db, (prior.message_max, prior.part_max, prior.session_max))?
                != prior.identities
        } else {
            false
        }
    } else {
        false
    };
    let revision = if let Some(previous) = previous.as_ref().filter(|_| start_sweep) {
        previous
            .revision
            .checked_add(1)
            .filter(|v| *v <= i32::MAX as u32)
            .ok_or("OpenCode revision exhausted")?
    } else {
        previous.as_ref().map_or(0, |c| c.revision)
    };
    let mut cursor = if start_sweep {
        let message_max = db.query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM session_message",
            [],
            |r| r.get(0),
        )?;
        let part_max =
            db.query_row("SELECT COALESCE(MAX(rowid),0) FROM session_part", [], |r| {
                r.get(0)
            })?;
        let session_max =
            db.query_row("SELECT COALESCE(MAX(rowid),0) FROM session_v2", [], |r| {
                r.get(0)
            })?;
        let message_count = bounded_count(&db, "session_message", MAX_SOURCE_ROWS)?;
        let part_count = bounded_count(&db, "session_part", MAX_SOURCE_ROWS - message_count)?;
        let session_count = bounded_count(
            &db,
            "session_v2",
            MAX_SOURCE_ROWS - message_count - part_count,
        )?;
        let identities = identity_digest(&db, (message_max, part_max, session_max))?;
        Cursor {
            legacy: false,
            epoch,
            signature,
            identities,
            revision,
            phase: 'm',
            rowid: 0,
            message_max,
            part_max,
            session_max,
            message_count,
            part_count,
            session_count,
        }
    } else {
        previous.clone().ok_or("missing OpenCode cursor")?
    };
    let mut records = Vec::new();
    let mut scanned = 0;
    let loss = start_sweep
        && (lost_prior_identity
            || previous.as_ref().is_some_and(|c| c.legacy)
            || prior_counts.is_some_and(|(m, p, s)| {
                cursor.message_count < m || cursor.part_count < p || cursor.session_count < s
            }));
    if replacement || loss {
        records.push(diagnostic(
            &cursor.epoch,
            "source",
            "",
            &cursor.signature,
            0,
            "0",
            "possible_source_gap",
        ));
    }
    while scanned < MAX_SCAN_ROWS && records.len() < max_records {
        let phase = cursor.phase;
        // Measure every variable-length source field before transferring it.
        let next = match phase {
            'm' => db.query_row("SELECT rowid,seq,time_updated,length(CAST(data AS BLOB)),length(CAST(session_id AS BLOB)),length(CAST(id AS BLOB)),length(CAST(type AS BLOB)) FROM session_message WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 1",
                params![cursor.rowid,cursor.message_max], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,
                    r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?))).optional()?,
            'p' => db.query_row("SELECT rowid,ordinal,time_updated,length(CAST(data AS BLOB)),length(CAST(session_id AS BLOB)),length(CAST(message_id AS BLOB)),length(CAST(type AS BLOB)) FROM session_part WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 1",
                params![cursor.rowid,cursor.part_max], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,
                    r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?))).optional()?,
            's' => db.query_row("SELECT rowid,0,time_updated,0,length(CAST(id AS BLOB)),length(CAST(id AS BLOB)),1 FROM session_v2 WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 1",
                params![cursor.rowid,cursor.session_max], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,
                    r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?))).optional()?,
            _ => return Err(invalid("invalid sweep phase")),
        };
        let Some((rowid, seq_or_ordinal, updated, size, session_size, id_size, kind_size)) = next
        else {
            cursor.phase = match phase {
                'm' => 'p',
                'p' => 's',
                _ => 'd',
            };
            cursor.rowid = 0;
            if cursor.phase == 'd' {
                break;
            }
            continue;
        };
        let time_ns = stamp(updated)?;
        let malformed_identity = !(1..=256).contains(&session_size)
            || !(1..=512).contains(&id_size)
            || !(1..=32).contains(&kind_size);
        let candidates = if malformed_identity || !(0..=MAX_ROW_BYTES).contains(&size) {
            vec![diagnostic(
                &cursor.epoch,
                if phase == 'm' {
                    "message"
                } else if phase == 'p' {
                    "part"
                } else {
                    "session"
                },
                "",
                &format!("rowid-{rowid}"),
                cursor.revision,
                &time_ns,
                if malformed_identity {
                    "oversized_opencode_identity"
                } else {
                    "oversized_opencode_row"
                },
            )]
        } else if phase == 's' {
            let session: String =
                db.query_row("SELECT id FROM session_v2 WHERE rowid=?1", [rowid], |r| {
                    r.get(0)
                })?;
            session_fact(&db, &cursor, &session)?
        } else {
            let sql = if phase == 'm' {
                "SELECT session_id,id,type,data FROM session_message WHERE rowid=?1"
            } else {
                "SELECT session_id,message_id,type,data FROM session_part WHERE rowid=?1"
            };
            let (session, id, kind, data): (String, String, String, String) =
                db.query_row(sql, [rowid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
            let row = SourceRow {
                session: &session,
                id: &id,
                kind: &kind,
                order: seq_or_ordinal,
                updated,
                data: &data,
            };
            if phase == 'm' {
                message_facts(&cursor, row)?
            } else {
                part_facts(&cursor, row)?
            }
        };
        if candidates.len() > max_records - records.len() {
            if records.is_empty() {
                return Err(invalid("OpenCode row exceeds record budget"));
            }
            break;
        }
        let next_cursor = Cursor {
            rowid,
            ..cursor.clone()
        };
        let before = records.len();
        records.extend(candidates);
        let encoded = serde_json::to_vec(&batch(source_id, &next_cursor.encode(), &records))?;
        if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
            records.truncate(before);
            if before == 0 {
                return Err(invalid("OpenCode row exceeds batch budget"));
            }
            break;
        }
        cursor = next_cursor;
        scanned += 1;
    }
    db.execute_batch("COMMIT")?;
    // The cursor pins a finite sweep. A concurrent writer changes the next
    // sweep's signature; it never rewinds this one or starves later rows.
    if fingerprint(path)?.0 != cursor.epoch {
        return Err(invalid("OpenCode database path changed during poll"));
    }
    let cursor = cursor.encode();
    let response = batch(source_id, &cursor, &records);
    let encoded = serde_json::to_vec(&response)?;
    if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
        return Err(invalid("OpenCode batch budget exceeded"));
    }
    Ok(response)
}
