//! The source DB is opened read-only. Each poll has one SQLite snapshot and
//! pages complete rows; only coarse facts cross the process boundary.

use agenturm_protocol::observation::{McpCall, McpMethod, mcp_tool_labels};
use agenturm_source_process::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

const MAX_ROW_BYTES: i64 = 64 * 1024;
const MAX_SCAN_ROWS: usize = 256;
const MAX_STRUCTURAL_TOKENS: usize = 8192;
// SQLite reads only the admitted rows and bounded fields. The database
// file can include content from fields that the adapter never projects.
const MAX_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_WAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SOURCE_ROWS: i64 = 50_000;
// Up to six row proofs fit in the 800-byte host cursor even at the largest
// supported row IDs. This only limits a page, never the retained history.
const MAX_PROMPT_PROOFS: usize = 6;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}
fn digest(raw: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(raw))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cursor {
    legacy: bool,
    turns: bool,
    proofs: bool,
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
        let has_proofs = parts.first() == Some(&"oc4");
        if !matches!(parts.first().copied(), Some("oc1" | "oc2" | "oc3" | "oc4"))
            || (if has_proofs {
                !(13..=13 + MAX_PROMPT_PROOFS).contains(&parts.len())
                    || parts[13..].iter().any(|value| !valid_digest(value))
            } else {
                parts.len() != if legacy { 12 } else { 13 }
            })
            || ![parts[1], parts[2]].iter().all(|s| valid_digest(s))
            || (!legacy && !valid_digest(parts[3]))
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
            turns: matches!(parts[0], "oc3" | "oc4"),
            proofs: has_proofs,
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

    fn encode(&self, prompt_proofs: &[String]) -> String {
        let mut cursor = format!(
            "oc4:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
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
        );
        for proof in prompt_proofs {
            cursor.push(':');
            cursor.push_str(proof);
        }
        cursor
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn prompt_digest(event: &str, turn: &str, rowid: i64, seq: i64, updated: i64, raw: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"agenturm.opencode-user-row/1");
    for value in [rowid, seq, updated] {
        hash.update(value.to_be_bytes());
    }
    for value in [event, turn, raw] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[cfg(unix)]
fn file_mark(path: &Path, max_bytes: u64) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid("OpenCode source is not a regular file"));
    }
    if metadata.len() > max_bytes {
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
    let main = file_mark(path, MAX_DATABASE_BYTES)?;
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name()
            .ok_or("invalid DB filename")?
            .to_string_lossy()
    ));
    let wal_mark = if wal.exists() {
        file_mark(&wal, MAX_WAL_BYTES)?
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

struct SessionRelationshipColumns {
    parent: bool,
    fork: bool,
}

fn check_schema(db: &Connection) -> Result<SessionRelationshipColumns> {
    let session_columns = columns(db, "session_v2")?;
    if !["id", "time_created", "time_updated"]
        .iter()
        .all(|column| session_columns.contains(*column))
    {
        return Err(invalid("unsupported OpenCode SQLite layout"));
    }
    for (table, required) in [
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
            ][..],
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
            ][..],
        ),
    ] {
        let found = columns(db, table)?;
        if !required.iter().all(|column| found.contains(*column)) {
            return Err(invalid("unsupported OpenCode SQLite layout"));
        }
    }
    Ok(SessionRelationshipColumns {
        parent: session_columns.contains("parent_id"),
        fork: session_columns.contains("fork_session_id"),
    })
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

fn identity(session: &str, turn: Option<&str>, operation: Option<&str>) -> Value {
    let mut id = json!({"session_id":session});
    if let Some(turn) = turn {
        id["turn_id"] = json!(turn);
    }
    if let Some(operation) = operation {
        id["operation_id"] = json!(operation);
    }
    id
}

#[derive(Clone, Copy)]
struct FactScope<'a> {
    epoch: &'a str,
    table: &'a str,
    session: &'a str,
    row: &'a str,
    revision: u32,
    time_ns: &'a str,
    operation: Option<&'a str>,
    turn: Option<&'a str>,
}

impl FactScope<'_> {
    fn record(&self, kind: &str, payload: Value) -> Value {
        json!({"event_id":wire_id(self.epoch,self.table,self.session,self.row,kind),
            "revision":self.revision,"kind":kind,"time_ns":self.time_ns,
            "identity":identity(self.session,self.turn,self.operation),"payload":payload})
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

fn known_idle_outcome(outcome: Option<&str>) -> Option<(&'static str, &'static str)> {
    match outcome? {
        "succeeded" => Some(("completed", "opencode_idle_succeeded")),
        "failed" => Some(("failed", "opencode_idle_failed")),
        "interrupted" => Some(("interrupted", "opencode_idle_interrupted")),
        _ => None,
    }
}

fn verified_turn(
    db: &Connection,
    cursor: &Cursor,
    session: &str,
    seq: i64,
) -> Result<Option<String>> {
    let mut statement = db.prepare_cached(
        "SELECT seq, CASE WHEN length(CAST(id AS BLOB)) BETWEEN 1 AND 512 THEN id END
         FROM session_message WHERE session_id=?1 AND rowid<=?2 AND type='idle' AND seq<?3
         ORDER BY seq DESC LIMIT 2",
    )?;
    let mut rows = statement.query(params![session, cursor.message_max, seq])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let boundary: i64 = row.get(0)?;
    let id: Option<String> = row.get(1)?;
    if boundary < 0 {
        return Ok(None);
    }
    if let Some(next) = rows.next()?
        && next.get::<_, i64>(0)? == boundary
    {
        return Ok(None);
    }
    Ok(id
        .filter(|id| id.starts_with("msg_"))
        .map(|id| opaque(&["opencode-turn", &cursor.epoch, session, &id])))
}

fn part_turn(
    db: &Connection,
    cursor: &Cursor,
    session: &str,
    message_id: &str,
) -> Result<Option<String>> {
    let mut statement = db.prepare_cached(
        "SELECT seq FROM session_message
         WHERE session_id=?1 AND rowid<=?2 AND id=?3 AND type='assistant' LIMIT 2",
    )?;
    let mut rows = statement.query(params![session, cursor.message_max, message_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let seq: i64 = row.get(0)?;
    if seq < 0 || rows.next()?.is_some() {
        return Ok(None);
    }
    verified_turn(db, cursor, session, seq)
}

fn message_facts(db: &Connection, cursor: &Cursor, row: SourceRow<'_>) -> Result<Vec<Value>> {
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
    let turn = verified_turn(db, cursor, row.session, row.order)?;
    let scope = FactScope {
        epoch: &cursor.epoch,
        table: "message",
        session: row.session,
        row: row.id,
        revision: cursor.revision,
        time_ns: &time_ns,
        operation: None,
        turn: turn.as_deref(),
    };
    let facts = match row.kind {
        "user" => vec![scope.record("message", json!({"role":"user"}))],
        "assistant" => {
            let mut facts = vec![scope.record("message", json!({"role":"assistant"}))];
            if let Some(counters) = usage(&raw)? {
                facts.push(scope.record("usage", counters));
            }
            facts.extend(inline_tool_facts(db, cursor, &scope, &raw)?);
            facts
        }
        "idle" => {
            match (
                turn.as_deref(),
                known_idle_outcome(raw.get("outcome").and_then(Value::as_str)),
            ) {
                (Some(_), Some((state, reason))) => {
                    vec![scope.record("lifecycle", json!({"state":state,"reason_code":reason}))]
                }
                _ => Vec::new(),
            }
        }
        _ => vec![scope.diagnostic("unsupported_opencode_message")],
    };
    Ok(facts)
}

fn inline_tool_facts(
    db: &Connection,
    cursor: &Cursor,
    message: &FactScope<'_>,
    raw: &Value,
) -> Result<Vec<Value>> {
    const TOOL_LIMIT: usize = 16;
    let Some(content) = raw.get("content").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let tools: Vec<_> = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool"))
        .take(TOOL_LIMIT + 1)
        .collect();
    if tools.is_empty() {
        return Ok(Vec::new());
    }
    let separate_parts: bool = db
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM session_part
         WHERE session_id=?1 AND message_id=?2 AND rowid<=?3 AND type='tool')",
        )?
        .query_row(
            params![message.session, message.row, cursor.part_max],
            |row| row.get(0),
        )?;
    if separate_parts {
        return Ok(Vec::new());
    }
    let mut facts = Vec::new();
    let mut seen = HashSet::new();
    let mut incomplete = tools.len() > TOOL_LIMIT;
    for tool in tools.into_iter().take(TOOL_LIMIT) {
        let Some(id) = tool
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
        else {
            incomplete = true;
            continue;
        };
        if !seen.insert(id) {
            incomplete = true;
            continue;
        }
        let row = opaque(&[message.row, id]);
        let operation = opaque(&[message.epoch, message.session, "inline-tool", &row]);
        let scope = FactScope {
            table: "inline-tool",
            row: &row,
            operation: Some(&operation),
            ..*message
        };
        facts.extend(tool_facts(&scope, tool)?);
    }
    if incomplete {
        facts.push(message.diagnostic("oversized_opencode_row"));
    }
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

fn check_category(command: &str) -> Option<&'static str> {
    // Recognize a bounded direct invocation. Shell quoting, expansion,
    // redirection, pipelines and chained commands can change what ran.
    if command.len() > 160
        || !command
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" \t._/:-=,@+".contains(&byte))
    {
        return None;
    }
    let mut words = command.split_ascii_whitespace();
    let category = match (words.next()?, words.next()) {
        ("pytest", _) => "test",
        ("cargo" | "go", Some("test")) => "test",
        ("cargo" | "go", Some("build")) => "build",
        ("cargo", Some("clippy")) | ("ruff", Some("check")) => "lint",
        ("python" | "python3", Some("-m"))
            if matches!(words.next(), Some("pytest" | "unittest")) =>
        {
            "test"
        }
        _ => return None,
    };
    if command.split_ascii_whitespace().any(|word| {
        matches!(
            word,
            "-h" | "--help"
                | "--version"
                | "--dry-run"
                | "--list"
                | "--no-run"
                | "--collect-only"
                | "--co"
        )
    }) {
        return None;
    }
    Some(category)
}

fn part_facts(cursor: &Cursor, row: SourceRow<'_>, turn: Option<&str>) -> Result<Vec<Value>> {
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
        turn,
    };
    tool_facts(&scope, &raw)
}

fn tool_facts(scope: &FactScope<'_>, raw: &Value) -> Result<Vec<Value>> {
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
    let mut facts = vec![scope.record("tool", json!({"category":category(name),"result":result}))];
    if let Some((server, tool)) = mcp_tool_labels(name) {
        let duration_ns = raw.get("time").and_then(|time| {
            let ran = time.get("ran")?.as_u64()?;
            let completed = time.get("completed")?.as_u64()?;
            completed
                .checked_sub(ran)?
                .checked_mul(1_000_000)
                .filter(|duration| *duration <= i64::MAX as u64)
        });
        facts.push(scope.record(
            "mcp_call",
            serde_json::to_value(McpCall {
                server: Some(server.into()),
                tool: Some(tool.into()),
                method: McpMethod::ToolsCall,
                duration_ns,
                result: result.into(),
            })?,
        ));
    }

    // Tool completion alone only establishes that the tool returned. A
    // native shell exit code establishes the check outcome.
    if shell
        && status == "completed"
        && let Some(metadata) = metadata
        && metadata.is_object()
        && metadata
            .get("status")
            .is_none_or(|value| value.as_str() == Some("completed"))
        && metadata
            .get("timeout")
            .is_none_or(|value| value.as_bool() == Some(false))
        && metadata
            .get("signal")
            .is_none_or(|value| value.is_null() || value.as_str() == Some(""))
        && let Some(exit) = exit.filter(|&code| (0..=255).contains(&code))
        && let Some(category) = raw
            .pointer("/state/input/command")
            .and_then(Value::as_str)
            .and_then(check_category)
    {
        facts.push(scope.record(
            "check",
            json!({"category":category,"result":if exit == 0 {"passed"} else {"failed"}}),
        ));
    }
    Ok(facts)
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
        turn: None,
    };
    let (state, reason) = match role.as_str() {
        "user" => ("running", "opencode_latest_user"),
        "assistant" => ("running", "opencode_latest_assistant"),
        "idle" => known_idle_outcome(outcome.as_deref())
            .unwrap_or(("paused", "opencode_idle_outcome_unknown")),
        _ => return Ok(vec![scope.diagnostic("opencode_latest_role_unknown")]),
    };
    Ok(vec![scope.record(
        "lifecycle",
        json!({"state":state,"reason_code":reason}),
    )])
}

fn session_relationships(
    db: &Connection,
    cursor: &Cursor,
    rowid: i64,
    session: &str,
    columns: &SessionRelationshipColumns,
) -> Result<Vec<Value>> {
    let mut facts = Vec::new();
    for (available, edge, sql) in [
        (
            columns.parent,
            "delegated",
            "SELECT time_created, CAST(parent_id AS BLOB) FROM session_v2 WHERE rowid=?1
               AND typeof(time_created)='integer' AND typeof(parent_id)='text'
               AND length(CAST(parent_id AS BLOB)) BETWEEN 1 AND 256",
        ),
        (
            columns.fork,
            "forked",
            "SELECT time_created, CAST(fork_session_id AS BLOB) FROM session_v2 WHERE rowid=?1
               AND typeof(time_created)='integer' AND typeof(fork_session_id)='text'
               AND length(CAST(fork_session_id AS BLOB)) BETWEEN 1 AND 256",
        ),
    ] {
        if !available {
            continue;
        }
        let parent: Option<(i64, Vec<u8>)> = db
            .prepare_cached(sql)?
            .query_row([rowid], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((created, parent)) = parent else {
            continue;
        };
        if parent == session.as_bytes()
            || !parent[0].is_ascii_alphanumeric()
            || !parent
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            continue;
        }
        let (Ok(parent), Ok(time_ns)) = (String::from_utf8(parent), stamp(created)) else {
            continue;
        };
        let scope = FactScope {
            epoch: &cursor.epoch,
            table: edge,
            session,
            row: session,
            revision: cursor.revision,
            time_ns: &time_ns,
            operation: None,
            turn: None,
        };
        facts.push(scope.record(
            "relationship",
            json!({"relationship_kind":edge,"parent_session_id":parent}),
        ));
    }
    Ok(facts)
}

/// Exact admitted coordinates for a user row. No text is requested by ID alone.
pub struct UserPromptEvidence<'a> {
    pub event_id: &'a str,
    pub turn_id: &'a str,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
    pub admitted_proof: Option<&'a str>,
}

pub fn verify_prompt_checkpoint(path: &Path, raw_cursor: &str) -> Result<()> {
    const SOURCE_CHANGED: &str =
        "Prompt source changed or no longer matches the committed checkpoint";
    let (epoch, signature) = fingerprint(path).map_err(|_| invalid(SOURCE_CHANGED))?;
    if !Cursor::parse(raw_cursor)?.is_some_and(|cursor| {
        cursor.turns && cursor.epoch == epoch && cursor.signature == signature
    }) {
        return Err(invalid(SOURCE_CHANGED));
    }
    Ok(())
}

/// Reconstructs exact user rows using the current native SQLite snapshot and
/// each row's admission proof. The caller rechecks grants before disclosure.
pub fn prove_user_prompts(
    path: &Path,
    raw_cursor: &str,
    session: &str,
    evidence: &[UserPromptEvidence<'_>],
) -> Result<BTreeMap<String, String>> {
    const SOURCE_CHANGED: &str =
        "Prompt source changed or no longer matches the committed checkpoint";
    if evidence.len() > 256 || session.is_empty() || session.len() > 256 {
        return Err(invalid("OpenCode prompt selection exceeds its bound"));
    }
    let (epoch, signature) = fingerprint(path).map_err(|_| invalid(SOURCE_CHANGED))?;
    let cursor = Cursor::parse(raw_cursor)?
        .filter(|cursor| cursor.turns && cursor.epoch == epoch && cursor.signature == signature)
        .ok_or_else(|| invalid(SOURCE_CHANGED))?;
    preflight_sidecars(path)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_millis(250))?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN")?;
    check_schema(&db)?;
    let wanted: BTreeMap<_, _> = evidence
        .iter()
        .filter(|row| row.admitted_proof.is_some() || row.revision == cursor.revision)
        .map(|row| (row.event_id, row))
        .collect();
    let mut statement = db.prepare(
        "SELECT rowid, id, seq, time_updated FROM session_message
         WHERE session_id=?1 AND rowid<=?2 AND type='user'
           AND length(CAST(id AS BLOB)) BETWEEN 1 AND 512
         ORDER BY rowid LIMIT ?3",
    )?;
    let mut rows = statement.query(params![session, cursor.message_max, MAX_SOURCE_ROWS + 1])?;
    let mut count = 0;
    let mut seen = HashSet::new();
    let mut found = BTreeMap::new();
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_SOURCE_ROWS {
            return Err(invalid("OpenCode prompt source row limit exceeded"));
        }
        let native_id: String = row.get(1)?;
        let event_id = wire_id(&cursor.epoch, "message", session, &native_id, "message");
        let Some(expected) = wanted.get(event_id.as_str()) else {
            continue;
        };
        if !seen.insert(expected.event_id) {
            return Err(invalid("OpenCode prompt identity is ambiguous"));
        }
        let seq: i64 = row.get(2)?;
        let updated: i64 = row.get(3)?;
        let native_stamp = stamp(updated)?.parse::<u64>()?;
        if native_stamp != expected.observed_at_unix_ns
            || verified_turn(&db, &cursor, session, seq)?.as_deref() != Some(expected.turn_id)
        {
            continue;
        }
        let rowid: i64 = row.get(0)?;
        let raw: Option<String> = db.query_row(
            "SELECT CASE WHEN length(CAST(data AS BLOB))<=?2 THEN data END
             FROM session_message WHERE rowid=?1",
            params![rowid, MAX_ROW_BYTES],
            |row| row.get(0),
        )?;
        let Some(raw) = raw else { continue };
        if let Some(proof) = expected.admitted_proof {
            let hash = prompt_digest(&event_id, expected.turn_id, rowid, seq, updated, &raw);
            if !proof
                .strip_prefix("oc4:")
                .is_some_and(|proofs| proofs.split(':').any(|value| value == hash))
            {
                continue;
            }
        }
        let payload: Value = serde_json::from_str(&raw)?;
        let Some(text) = payload
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| (1..=2048).contains(&text.len()))
        else {
            continue;
        };
        found.insert(event_id, text.to_owned());
    }
    if fingerprint(path).map_err(|_| invalid(SOURCE_CHANGED))? != (epoch, signature) {
        return Err(invalid(SOURCE_CHANGED));
    }
    Ok(found)
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
            .is_some_and(|c| c.signature == signature && !replacement && c.proofs)
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
    let relationship_columns = check_schema(&db)?;
    let start_sweep = previous.is_none()
        || replacement
        || sweep_finished
        || previous.as_ref().is_some_and(|c| !c.proofs);
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
            turns: true,
            proofs: true,
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
    let mut prompt_proofs = Vec::new();
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
    while scanned < MAX_SCAN_ROWS
        && records.len() < max_records
        && prompt_proofs.len() < MAX_PROMPT_PROOFS
    {
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
        let mut prompt_proof = None;
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
            let mut facts = session_fact(&db, &cursor, &session)?;
            facts.extend(session_relationships(
                &db,
                &cursor,
                rowid,
                &session,
                &relationship_columns,
            )?);
            facts
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
                let facts = message_facts(&db, &cursor, row)?;
                if kind == "user"
                    && let Some(fact) = facts.first()
                    && let Some(event) = fact["event_id"].as_str()
                    && let Some(turn) = fact["identity"]["turn_id"].as_str()
                {
                    prompt_proof = Some(prompt_digest(
                        event,
                        turn,
                        rowid,
                        seq_or_ordinal,
                        updated,
                        &data,
                    ));
                }
                facts
            } else {
                let turn = if row.kind == "tool" {
                    part_turn(&db, &cursor, row.session, row.id)?
                } else {
                    None
                };
                part_facts(&cursor, row, turn.as_deref())?
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
        let adds_proof = prompt_proof.is_some();
        prompt_proofs.extend(prompt_proof);
        let encoded = serde_json::to_vec(&batch(
            source_id,
            &next_cursor.encode(&prompt_proofs),
            &records,
        ))?;
        if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
            records.truncate(before);
            if adds_proof {
                prompt_proofs.pop();
            }
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
    let cursor = cursor.encode(&prompt_proofs);
    let response = batch(source_id, &cursor, &records);
    let encoded = serde_json::to_vec(&response)?;
    if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
        return Err(invalid("OpenCode batch budget exceeded"));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_checks_keep_only_typed_outcomes_and_operation_identity() {
        let native = json!({
            "name": "shell",
            "state": {
                "status": "completed",
                "input": {
                    "command": "pytest tests/private_test.py -q",
                    "workdir": "/private/project",
                },
                "metadata": {"exit": 0, "status": "completed"},
                "content": [{"type": "text", "text": "private check output"}],
            },
        });
        let mut failed = native.clone();
        failed["state"]["input"]["command"] = json!("cargo clippy --workspace");
        failed["state"]["metadata"]["exit"] = json!(1);

        for (raw, category, result, tool_result) in [
            (&native, "test", "passed", "succeeded"),
            (&failed, "lint", "failed", "failed"),
        ] {
            let facts = fixture_part(raw);
            assert_eq!(facts.len(), 2);
            assert_eq!(
                facts[0]["payload"],
                json!({"category":"shell","result":tool_result})
            );
            assert_eq!(facts[1]["kind"], "check");
            assert_eq!(
                facts[1]["payload"],
                json!({"category":category,"result":result})
            );
            assert_eq!(facts[1]["identity"], facts[0]["identity"]);
            assert_ne!(facts[1]["event_id"], facts[0]["event_id"]);
            assert_eq!(facts[1]["identity"]["session_id"], "ses_fixture");
            assert!(facts[1]["identity"]["operation_id"].is_string());
            assert_eq!(fixture_part(raw), facts);
            let wire = serde_json::to_string(&facts).unwrap();
            for private in [
                "private_test.py",
                "/private/project",
                "private check output",
            ] {
                assert!(!wire.contains(private));
            }
        }
    }

    #[test]
    fn ambiguous_commands_and_nonterminal_shell_records_remain_tools() {
        for command in [
            "pytest && cargo build",
            "pytest | tee result.log",
            "pytest --help",
            "pytest --collect-only",
            "cargo test --no-run",
            "pytest tests/$(cat case)",
            "echo pytest",
            "go test\nwhoami",
        ] {
            assert_eq!(check_category(command), None);
        }
        assert_eq!(check_category("cargo build --locked"), Some("build"));

        for metadata in [
            json!({}),
            json!({"exit":"0"}),
            json!({"exit":-1}),
            json!({"exit":256}),
            json!({"exit":0,"status":"running"}),
            json!({"exit":0,"timeout":true}),
            json!({"exit":0,"signal":"SIGTERM"}),
        ] {
            let facts = fixture_part(&json!({
                "name": "shell",
                "state": {
                    "status": "completed",
                    "input": {"command": "pytest tests"},
                    "metadata": metadata,
                },
            }));
            assert_eq!(facts.len(), 1);
            assert_eq!(facts[0]["kind"], "tool");
        }

        let facts = fixture_part(&json!({
            "name": "shell",
            "state": {
                "status": "running",
                "input": {"command": "pytest tests"},
                "metadata": {"exit":0},
            },
        }));
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["payload"]["result"], "started");
    }

    #[test]
    fn turn_membership_requires_a_retained_idle_boundary_and_exact_assistant_part() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE session_message(id TEXT, session_id TEXT, type TEXT, seq INTEGER);
             INSERT INTO session_message VALUES
             ('msg_first','s','assistant',1),('msg_previous','s','idle',2),
             ('msg_user','s','user',3),('msg_assistant','s','assistant',5),('msg_end','s','idle',8),
             ('msg_wrong_other_session','other','idle',4);",
        )
        .unwrap();
        let cursor = Cursor::parse(&format!(
            "oc3:{}:{}:{}:0:m:0:6:1:1:6:1:1",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64)
        ))
        .unwrap()
        .unwrap();
        assert!(verified_turn(&db, &cursor, "s", 1).unwrap().is_none());
        let turn = verified_turn(&db, &cursor, "s", 3).unwrap().unwrap();
        assert_eq!(
            part_turn(&db, &cursor, "s", "msg_assistant")
                .unwrap()
                .as_deref(),
            Some(turn.as_str())
        );
        assert!(
            part_turn(&db, &cursor, "other", "msg_assistant")
                .unwrap()
                .is_none()
        );
        let facts = message_facts(
            &db,
            &cursor,
            SourceRow {
                session: "s",
                id: "msg_end",
                kind: "idle",
                order: 8,
                updated: 15,
                data: r#"{"outcome":"failed"}"#,
            },
        )
        .unwrap();
        assert_eq!(facts[0]["identity"]["turn_id"], turn);
        assert_eq!(facts[0]["payload"]["reason_code"], "opencode_idle_failed");
        let later = verified_turn(&db, &cursor, "s", 9).unwrap().unwrap();
        assert_ne!(later, turn);
    }

    #[test]
    fn inline_shell_evidence_uses_the_idle_boundary_without_inventing_a_check() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE session_message(id TEXT, session_id TEXT, type TEXT, seq INTEGER);
             CREATE TABLE session_part(message_id TEXT, session_id TEXT, type TEXT);
             INSERT INTO session_message VALUES
                ('msg_first','s','assistant',1),('msg_idle','s','idle',2),
                ('msg_later','s','assistant',3),('msg_end','s','idle',4);",
        )
        .unwrap();
        let cursor = Cursor::parse(&format!(
            "oc3:{}:{}:{}:0:m:0:4:1:1:4:1:1",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64)
        ))
        .unwrap()
        .unwrap();
        let content = json!({"content":[
            {"type":"tool","id":"explicit","name":"shell","state":{
                "status":"completed","input":{"command":"python3 -m unittest tests.test_example"},"metadata":{"exit":1}}},
            {"type":"tool","id":"unknown","name":"shell","state":{
                "status":"completed","input":{"command":"python3 -m unittest tests.test_example"}}}
        ]})
        .to_string();
        let first = message_facts(
            &db,
            &cursor,
            SourceRow {
                session: "s",
                id: "msg_first",
                kind: "assistant",
                order: 1,
                updated: 15,
                data: &content,
            },
        )
        .unwrap();
        assert!(
            first
                .iter()
                .all(|fact| fact["identity"].get("turn_id").is_none())
        );
        let later = message_facts(
            &db,
            &cursor,
            SourceRow {
                session: "s",
                id: "msg_later",
                kind: "assistant",
                order: 3,
                updated: 18,
                data: &content,
            },
        )
        .unwrap();
        let turn = &later[0]["identity"]["turn_id"];
        assert!(turn.is_string());
        assert!(
            later
                .iter()
                .all(|fact| fact["identity"]["turn_id"] == *turn)
        );
        assert_eq!(
            later.iter().filter(|fact| fact["kind"] == "tool").count(),
            2
        );
        let checks = later
            .iter()
            .filter(|fact| fact["kind"] == "check")
            .collect::<Vec<_>>();
        assert_eq!(checks.len(), 1);
        assert_eq!(
            checks[0]["payload"],
            json!({"category":"test","result":"failed"})
        );
        let ambiguous = message_facts(
            &db,
            &cursor,
            SourceRow {
                session: "s",
                id: "msg_end",
                kind: "idle",
                order: 4,
                updated: 20,
                data: r#"{"outcome":"unknown"}"#,
            },
        )
        .unwrap();
        assert!(ambiguous.is_empty());
    }

    fn fixture_part(raw: &Value) -> Vec<Value> {
        let cursor = Cursor::parse(&format!(
            "oc2:{}:{}:{}:0:p:0:1:1:1:1:1:1",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64)
        ))
        .unwrap()
        .unwrap();
        part_facts(
            &cursor,
            SourceRow {
                session: "ses_fixture",
                id: "msg_part",
                kind: "tool",
                order: 0,
                updated: 1,
                data: &raw.to_string(),
            },
            None,
        )
        .unwrap()
    }
}
