//! The source DB is opened read-only. Each poll has one SQLite snapshot and
//! pages complete rows; only coarse facts cross the process boundary.

use agenturm_protocol::observation::{
    McpCall, McpMethod, NativeCostBasis, ReportedCost, mcp_tool_labels, opencode_patch_change,
};
use agenturm_source_process::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
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
    patches: bool,
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
        let has_proofs = matches!(parts.first().copied(), Some("oc4" | "oc5"));
        if !matches!(
            parts.first().copied(),
            Some("oc1" | "oc2" | "oc3" | "oc4" | "oc5")
        ) || (if has_proofs {
            !(13..=13 + MAX_PROMPT_PROOFS).contains(&parts.len())
                || parts[13..].iter().any(|value| !valid_digest(value))
        } else {
            parts.len() != if legacy { 12 } else { 13 }
        }) || ![parts[1], parts[2]].iter().all(|s| valid_digest(s))
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
            turns: matches!(parts[0], "oc3" | "oc4" | "oc5"),
            proofs: has_proofs,
            patches: parts[0] == "oc5",
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
            "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            if self.patches { "oc5" } else { "oc4" },
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
    /// The newer native `part` table carrying patch evidence exists with its
    /// required columns. Older databases without it simply have no patches.
    native_parts: bool,
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
    let native_parts = columns(db, "part")
        .map(|found| {
            [
                "id",
                "message_id",
                "session_id",
                "time_created",
                "time_updated",
                "data",
            ]
            .iter()
            .all(|column| found.contains(*column))
        })
        .unwrap_or(false);
    Ok(SessionRelationshipColumns {
        parent: session_columns.contains("parent_id"),
        fork: session_columns.contains("fork_session_id"),
        native_parts,
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

fn message_facts(
    db: &Connection,
    cursor: &Cursor,
    row: SourceRow<'_>,
    native_costs: bool,
    native_parts: bool,
) -> Result<Vec<Value>> {
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
            // A patch summary outside an active turn cannot satisfy the host
            // admission rule, so it is withheld without failing the line.
            if native_costs && native_parts && turn.is_some() {
                facts.extend(patch_part_facts(db, cursor, &scope)?);
            }
            if native_costs
                && raw["time"]["completed"]
                    .as_i64()
                    .is_some_and(|ms| stamp(ms).is_ok())
                && raw["cost"].is_number()
                && let Some(cost) = assistant_cost(row.data)
            {
                let message_event = wire_id(
                    scope.epoch,
                    scope.table,
                    scope.session,
                    scope.row,
                    "message",
                );
                facts.push(json!({
                    "event_id": format!("{message_event}:cost"),
                    "kind": "reported_cost",
                    "revision": scope.revision,
                    "time_ns": scope.time_ns,
                    "identity": identity(scope.session, scope.turn, Some(&message_event)),
                    "payload": cost,
                }));
            }
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

fn assistant_cost(raw: &str) -> Option<ReportedCost> {
    #[derive(Deserialize)]
    struct Assistant<'a> {
        #[serde(borrow)]
        cost: &'a RawValue,
    }

    let message: Assistant<'_> = serde_json::from_str(raw).ok()?;
    ReportedCost::native_amount("USD", NativeCostBasis::Unknown, message.cost)
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

/// Emits one bounded `file_change` fact per native patch part attached to the
/// assistant message in the caller's scope. The newer native `part` table
/// records per-message patch evidence as an edited-path list plus a producer
/// hash; only the bounded file count and a canonical record digest cross this
/// boundary, and malformed patches are withheld without failing the line.
fn patch_part_facts(
    db: &Connection,
    cursor: &Cursor,
    message: &FactScope<'_>,
) -> Result<Vec<Value>> {
    const MAX_PATCH_PARTS: usize = 4;
    let mut statement = db.prepare_cached(
        "SELECT CASE WHEN length(CAST(id AS BLOB)) BETWEEN 1 AND 512 THEN id END,
                time_updated, CASE WHEN length(CAST(data AS BLOB))<=?4 THEN data END
         FROM part WHERE message_id=?1 AND session_id=?2
           AND (length(CAST(data AS BLOB))>?4 OR CASE WHEN length(CAST(data AS BLOB))<=?4 THEN
                 CASE WHEN json_valid(data) THEN json_extract(data,'$.type') END END='patch')
         ORDER BY id LIMIT ?3",
    )?;
    let mut rows = statement.query(params![
        message.row,
        message.session,
        MAX_PATCH_PARTS + 1,
        MAX_ROW_BYTES
    ])?;
    let mut facts = Vec::new();
    let mut incomplete = false;
    while let Some(row) = rows.next()? {
        let part: Option<String> = row.get(0)?;
        let updated: i64 = row.get(1)?;
        let data: Option<String> = row.get(2)?;
        let (Some(part), Some(data)) = (part, data) else {
            incomplete = true;
            continue;
        };
        if part.is_empty() || part.len() > 512 {
            return Err(invalid("invalid OpenCode part identity"));
        }
        if facts.len() >= MAX_PATCH_PARTS {
            incomplete = true;
            break;
        }
        let raw: Value = match serde_json::from_str(&data) {
            Ok(raw) => raw,
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        let Some(change) = opencode_patch_change(&raw) else {
            continue;
        };
        let Ok(time_ns) = stamp(updated) else {
            incomplete = true;
            continue;
        };
        let operation = opaque(&[&cursor.epoch, message.session, message.row, &part]);
        let scope = FactScope {
            epoch: message.epoch,
            table: "native-part",
            session: message.session,
            row: &part,
            revision: message.revision,
            time_ns: &time_ns,
            operation: Some(&operation),
            turn: message.turn,
        };
        let payload = serde_json::to_value(change)?;
        facts.push(scope.record("file_change", payload));
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

const PROMPT_SOURCE_CHANGED: &str = "Prompt source changed during verification";

/// The native database and WAL observed while verifying exact prompt bytes.
pub struct PromptSourceStamp((String, String));

impl PromptSourceStamp {
    pub fn verify(&self, path: &Path) -> Result<()> {
        if fingerprint(path).map_err(|_| invalid(PROMPT_SOURCE_CHANGED))? != self.0 {
            return Err(invalid(PROMPT_SOURCE_CHANGED));
        }
        Ok(())
    }
}

pub struct ProvenUserPrompts {
    pub texts: BTreeMap<String, String>,
    pub stamp: PromptSourceStamp,
}

/// Reconstructs exact user rows from a native SQLite read transaction. An
/// admission digest permits proof after unrelated writes. Legacy rows still
/// require the exact committed sweep. The caller rechecks the stamp and grant
/// at disclosure.
pub fn prove_user_prompts(
    path: &Path,
    raw_cursor: &str,
    session: &str,
    evidence: &[UserPromptEvidence<'_>],
) -> Result<ProvenUserPrompts> {
    const SOURCE_CHANGED: &str =
        "Prompt source changed or no longer matches the committed checkpoint";
    if evidence.len() > 256 || session.is_empty() || session.len() > 256 {
        return Err(invalid("OpenCode prompt selection exceeds its bound"));
    }
    let checkpoint = PromptSourceStamp(fingerprint(path).map_err(|_| invalid(SOURCE_CHANGED))?);
    let (epoch, signature) = &checkpoint.0;
    let cursor = Cursor::parse(raw_cursor)?
        .filter(|cursor| cursor.turns && cursor.epoch == *epoch)
        .ok_or_else(|| invalid(SOURCE_CHANGED))?;
    let has_digest = evidence.iter().any(|row| row.admitted_proof.is_some());
    let current_sweep = cursor.signature == *signature;
    if !has_digest && !current_sweep {
        return Err(invalid(SOURCE_CHANGED));
    }
    preflight_sidecars(path)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_millis(250))?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN")?;
    check_schema(&db)?;
    let mut live_cursor = cursor.clone();
    if has_digest {
        bounded_count(&db, "session_message", MAX_SOURCE_ROWS)?;
        live_cursor.message_max = db.query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM session_message",
            [],
            |row| row.get(0),
        )?;
    }
    let wanted: BTreeMap<_, _> = evidence
        .iter()
        .filter(|row| {
            row.admitted_proof.is_some() || (current_sweep && row.revision == cursor.revision)
        })
        .map(|row| (row.event_id, row))
        .collect();
    let mut statement = db.prepare(
        "SELECT rowid, id, seq, time_updated, COALESCE(type='user', 0) FROM session_message
         WHERE session_id=?1 AND rowid BETWEEN 1 AND ?2
           AND length(CAST(id AS BLOB)) BETWEEN 1 AND 512
         ORDER BY rowid LIMIT ?3",
    )?;
    let mut rows = statement.query(params![
        session,
        live_cursor.message_max,
        MAX_SOURCE_ROWS + 1
    ])?;
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
        let rowid: i64 = row.get(0)?;
        let is_user: bool = row.get(4)?;
        let turn_cursor = if expected.admitted_proof.is_some() {
            &live_cursor
        } else {
            &cursor
        };
        if !is_user || rowid > turn_cursor.message_max {
            continue;
        }
        let seq: i64 = row.get(2)?;
        let updated: i64 = row.get(3)?;
        let native_stamp = stamp(updated)?.parse::<u64>()?;
        if native_stamp != expected.observed_at_unix_ns
            || verified_turn(&db, turn_cursor, session, seq)?.as_deref() != Some(expected.turn_id)
        {
            continue;
        }
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
                .or_else(|| proof.strip_prefix("oc5:"))
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
    checkpoint.verify(path)?;
    Ok(ProvenUserPrompts {
        texts: found,
        stamp: checkpoint,
    })
}

/// Exact retained coordinates for one native patch part. The digest and file
/// count come from the admitted `file_change` observation; nothing here is
/// client-supplied. No path text is requested by identity alone.
#[derive(Clone, Copy)]
pub struct PatchPartEvidence<'a> {
    pub event_id: &'a str,
    pub operation_id: &'a str,
    pub turn_id: &'a str,
    pub session_id: &'a str,
    pub observed_at_unix_ns: u64,
    pub digest: &'a str,
    pub files: u16,
}

pub struct ProvenPatchPart {
    /// The native edited-path list of the one matched patch part, untruncated;
    /// the caller applies its own disclosure bounds before any output.
    pub files: Vec<String>,
    pub stamp: PromptSourceStamp,
}

const PATCH_SOURCE_CHANGED: &str =
    "OpenCode edit source changed or no longer matches the committed checkpoint";
const PATCH_UNAVAILABLE: &str = "OpenCode edit detail is unavailable in the current source";
/// One session's native part rows stay small on supported databases; a larger
/// session fails closed instead of scanning unbounded history.
const MAX_PART_SCAN_ROWS: i64 = 8_192;

/// Reconstructs one exact native patch part from a read-only SQLite
/// transaction. Every retained claim — the part and message identity encoded
/// in the event and operation IDs, the observed timestamp, the canonical record
/// digest, and the file count — is recomputed from the live row and must match
/// exactly and uniquely before any path is disclosed. The native Git
/// hash stays opaque and is not used as a content proof. The retained SHA-256
/// commitment binds the actual patch record, including its path list. The
/// caller rechecks the database/WAL stamp and grant at disclosure.
pub fn prove_patch_part(
    path: &Path,
    raw_cursor: &str,
    evidence: &PatchPartEvidence<'_>,
) -> Result<ProvenPatchPart> {
    if evidence.session_id.is_empty()
        || evidence.session_id.len() > 256
        || evidence.event_id.len() > 256
        || evidence.operation_id.len() > 200
        || evidence.turn_id.len() > 200
        || evidence.files == 0
        || usize::from(evidence.files) > agenturm_protocol::observation::FileChange::MAX_FILES
        || !valid_digest(evidence.digest)
    {
        return Err(invalid(PATCH_UNAVAILABLE));
    }
    let checkpoint =
        PromptSourceStamp(fingerprint(path).map_err(|_| invalid(PATCH_SOURCE_CHANGED))?);
    let (epoch, _signature) = &checkpoint.0;
    let cursor = Cursor::parse(raw_cursor)?
        .filter(|cursor| cursor.patches && cursor.epoch == *epoch)
        .ok_or_else(|| invalid(PATCH_SOURCE_CHANGED))?;
    preflight_sidecars(path)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_millis(250))?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN")?;
    let schema = check_schema(&db)?;
    if !schema.native_parts {
        return Err(invalid(PATCH_UNAVAILABLE));
    }
    let session_parts: i64 = db.query_row(
        "SELECT COUNT(*) FROM (SELECT 1 FROM part WHERE session_id=?1 LIMIT ?2)",
        params![evidence.session_id, MAX_PART_SCAN_ROWS + 1],
        |row| row.get(0),
    )?;
    if session_parts > MAX_PART_SCAN_ROWS {
        return Err(invalid(PATCH_UNAVAILABLE));
    }
    let mut statement = db.prepare(
        "SELECT rowid,
                CASE WHEN length(CAST(id AS BLOB)) BETWEEN 1 AND 512 THEN id END,
                CASE WHEN length(CAST(message_id AS BLOB)) BETWEEN 1 AND 512 THEN message_id END, time_updated,
                CASE WHEN length(CAST(data AS BLOB))<=?2 THEN data END
         FROM part WHERE session_id=?1
           AND CASE WHEN length(CAST(data AS BLOB))<=?2 THEN
                 CASE WHEN json_valid(data) THEN json_extract(data,'$.type') END END='patch'
         ORDER BY rowid LIMIT ?3",
    )?;
    let mut rows = statement.query(params![
        evidence.session_id,
        MAX_ROW_BYTES,
        MAX_PART_SCAN_ROWS
    ])?;
    let mut found: Option<Vec<String>> = None;
    while let Some(row) = rows.next()? {
        let part: Option<String> = row.get(1)?;
        let message: Option<String> = row.get(2)?;
        let updated: i64 = row.get(3)?;
        let data: Option<String> = row.get(4)?;
        let (Some(part), Some(message), Some(data)) = (part, message, data) else {
            continue;
        };
        if part.is_empty() || part.len() > 512 || message.len() > 512 {
            continue;
        }
        let operation = opaque(&[&cursor.epoch, evidence.session_id, &message, &part]);
        let event = wire_id(
            &cursor.epoch,
            "native-part",
            evidence.session_id,
            &part,
            "file_change",
        );
        if event != evidence.event_id || operation != evidence.operation_id {
            continue;
        }
        if found.is_some() {
            return Err(invalid(PATCH_UNAVAILABLE));
        }
        let time_ns = stamp(updated)?.parse::<u64>()?;
        if time_ns != evidence.observed_at_unix_ns
            || part_turn(&db, &cursor, evidence.session_id, &message)?.as_deref()
                != Some(evidence.turn_id)
        {
            return Err(invalid(PATCH_UNAVAILABLE));
        }
        let raw: Value = serde_json::from_str(&data)?;
        let Some(change) = opencode_patch_change(&raw) else {
            return Err(invalid(PATCH_UNAVAILABLE));
        };
        if change.digest != evidence.digest || change.files != evidence.files {
            return Err(invalid(PATCH_UNAVAILABLE));
        }
        let files = raw
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(PATCH_UNAVAILABLE))?
            .iter()
            .map(|file| {
                file.as_str()
                    .filter(|path| !path.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(PATCH_UNAVAILABLE))
            })
            .collect::<Result<Vec<_>>>()?;
        found = Some(files);
    }
    checkpoint.verify(path)?;
    let files = found.ok_or_else(|| invalid(PATCH_UNAVAILABLE))?;
    Ok(ProvenPatchPart {
        files,
        stamp: checkpoint,
    })
}

pub fn poll(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    poll_with_native_costs(path, source_id, raw_cursor, max_records, max_bytes, true)
}

pub fn poll_legacy(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    poll_with_native_costs(path, source_id, raw_cursor, max_records, max_bytes, false)
}

fn poll_with_native_costs(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
    native_costs: bool,
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
        && previous.as_ref().is_some_and(|c| {
            c.signature == signature && !replacement && c.proofs && (!native_costs || c.patches)
        })
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
        || previous
            .as_ref()
            .is_some_and(|c| !c.proofs || (native_costs && !c.patches));
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
            patches: native_costs,
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
                let facts = message_facts(
                    &db,
                    &cursor,
                    row,
                    native_costs,
                    relationship_columns.native_parts,
                )?;
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
            true,
            false,
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
            true,
            false,
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
            true,
            false,
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
            true,
            false,
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

    fn fixture_database(path: &Path) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE session_v2(id TEXT PRIMARY KEY, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE session_message(id TEXT, session_id TEXT, type TEXT, seq INTEGER,
                time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE session_part(message_id TEXT, session_id TEXT, ordinal INTEGER, type TEXT,
                call_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE part(id TEXT, message_id TEXT, session_id TEXT,
                time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session_v2 VALUES('ses_1',1,1);
             INSERT INTO session_message VALUES
                ('msg_user','ses_1','user',1,10,10,'{\"text\":\"private fixture prompt\"}'),
                ('msg_previous','ses_1','idle',2,15,15,'{\"outcome\":\"succeeded\"}'),
                ('msg_asst','ses_1','assistant',3,20,20,'{}'),
                ('msg_end','ses_1','idle',4,30,30,'{\"outcome\":\"succeeded\"}');
             INSERT INTO part VALUES('prt_patch','msg_asst','ses_1',20,25,
                '{\"type\":\"patch\",\"files\":[\"/fixture/private-a.rs\",\"/fixture/private-b.rs\"],
                  \"hash\":\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\"}');",
        )
        .unwrap();
    }

    #[test]
    fn patch_parts_emit_bounded_change_facts_and_prove_their_exact_rows() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-patch-proof-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("opencode.db");
        fixture_database(&path);
        let batch = poll(&path, "harness.opencode", "", 256, 1024 * 1024).unwrap();
        let legacy = poll_legacy(&path, "harness.opencode", "", 256, 1024 * 1024).unwrap();
        assert!(
            legacy["records"]
                .as_array()
                .unwrap()
                .iter()
                .all(|record| record["kind"] != "file_change")
        );
        assert!(legacy["cursor"].as_str().unwrap().starts_with("oc4:"));
        // Upgrading a settled old cursor must sweep even with no source write.
        let upgraded = poll(
            &path,
            "harness.opencode",
            legacy["cursor"].as_str().unwrap(),
            256,
            1024 * 1024,
        )
        .unwrap();
        assert!(upgraded["cursor"].as_str().unwrap().starts_with("oc5:"));
        assert!(
            upgraded["records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["kind"] == "file_change")
        );
        let records = batch["records"].as_array().unwrap();
        let change = records
            .iter()
            .find(|record| record["kind"] == "file_change")
            .expect("the patch part records a file change fact");
        assert_eq!(change["payload"]["files"], 2);
        assert!(valid_digest(change["payload"]["digest"].as_str().unwrap()));
        assert_eq!(change["identity"]["session_id"], "ses_1");
        assert!(change["identity"]["turn_id"].is_string());
        assert!(change["identity"]["operation_id"].is_string());
        // The edited paths stay in the source database.
        let wire = serde_json::to_string(&batch).unwrap();
        assert!(!wire.contains("/fixture/private-a.rs"));
        assert!(!wire.contains("private fixture prompt"));
        let evidence = PatchPartEvidence {
            event_id: change["event_id"].as_str().unwrap(),
            operation_id: change["identity"]["operation_id"].as_str().unwrap(),
            turn_id: change["identity"]["turn_id"].as_str().unwrap(),
            session_id: "ses_1",
            observed_at_unix_ns: 25_000_000,
            digest: change["payload"]["digest"].as_str().unwrap(),
            files: 2,
        };
        let proven = prove_patch_part(&path, batch["cursor"].as_str().unwrap(), &evidence).unwrap();
        assert_eq!(
            proven.files,
            vec![
                "/fixture/private-a.rs".to_owned(),
                "/fixture/private-b.rs".to_owned()
            ]
        );

        // A turnless assistant message withholds the summary without a proof.
        let mut turnless = evidence;
        turnless.turn_id = "oc:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(prove_patch_part(&path, batch["cursor"].as_str().unwrap(), &turnless).is_err());
        // A different digest never discloses the retained row.
        let mut foreign = evidence;
        foreign.digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        assert!(prove_patch_part(&path, batch["cursor"].as_str().unwrap(), &foreign).is_err());
        let mut missing = evidence;
        missing.event_id = "oc:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert!(prove_patch_part(&path, batch["cursor"].as_str().unwrap(), &missing).is_err());
        // A same-count path rewrite with the SAME producer hash and time
        // must invalidate the retained content commitment.
        let db = Connection::open(&path).unwrap();
        db.execute(
            "UPDATE part SET data=?1 WHERE id='prt_patch'",
            [r#"{"type":"patch","files":["/fixture/rewritten.rs","/fixture/private-b.rs"],"hash":"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"}"#],
        )
        .unwrap();
        drop(db);
        assert!(prove_patch_part(&path, batch["cursor"].as_str().unwrap(), &evidence).is_err());
        let corrected = poll(
            &path,
            "harness.opencode",
            batch["cursor"].as_str().unwrap(),
            256,
            1024 * 1024,
        )
        .unwrap();
        let corrected_change = corrected["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["kind"] == "file_change")
            .unwrap();
        assert_eq!(corrected_change["event_id"], change["event_id"]);
        assert_ne!(
            corrected_change["payload"]["digest"],
            change["payload"]["digest"]
        );
        assert_eq!(corrected_change["revision"], 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_patch_parts_withhold_the_change_fact_without_failing_the_line() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE session_message(id TEXT, session_id TEXT, type TEXT, seq INTEGER);
             CREATE TABLE part(id TEXT, message_id TEXT, session_id TEXT,
                time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session_message VALUES
                ('msg_idle','s','idle',1),('msg_asst','s','assistant',2),('msg_end','s','idle',3);
             INSERT INTO part VALUES
                ('prt_empty','msg_asst','s',0,0,'{\"type\":\"patch\",\"files\":[],\"hash\":\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\"}'),
                ('prt_badhash','msg_asst','s',0,0,'{\"type\":\"patch\",\"files\":[\"/a\"],\"hash\":\"not-a-digest\"}'),
                ('prt_nofile','msg_asst','s',0,0,'{\"type\":\"patch\",\"files\":[\"/a\",123],\"hash\":\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\"}'),
                ('prt_ok','msg_asst','s',0,0,'{\"type\":\"patch\",\"files\":[\"/a\"],\"hash\":\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\"}');",
        )
        .unwrap();
        let cursor = Cursor::parse(&format!(
            "oc4:{}:{}:{}:0:m:0:3:0:1:3:0:1",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64)
        ))
        .unwrap()
        .unwrap();
        let scope = FactScope {
            epoch: &cursor.epoch,
            table: "message",
            session: "s",
            row: "msg_asst",
            revision: cursor.revision,
            time_ns: "0",
            operation: None,
            turn: None,
        };
        let facts = patch_part_facts(&db, &cursor, &scope).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["kind"], "file_change");
        assert_eq!(facts[0]["payload"]["files"], 1);
        let wire = serde_json::to_string(&facts).unwrap();
        assert!(!wire.contains("/a"));
    }

    #[test]
    fn patch_commitments_bind_paths_and_reject_ambiguous_counts() {
        let patch = json!({"type":"patch","files":["/private/a.rs"],"hash":"a".repeat(40)});
        let summary = opencode_patch_change(&patch).unwrap();
        assert!(summary.is_valid());
        let mut changed = patch.clone();
        changed["files"] = json!(["/private/b.rs"]);
        assert_ne!(
            opencode_patch_change(&changed).unwrap().digest,
            summary.digest
        );
        // Canonical object ordering/whitespace does not change the proof.
        let reordered: Value = serde_json::from_str(&format!(
            "{{ \"hash\":\"{}\", \"files\":[\"/private/a.rs\"],\"type\":\"patch\" }}",
            "a".repeat(40)
        ))
        .unwrap();
        assert_eq!(opencode_patch_change(&reordered), Some(summary));
        for files in [
            json!([]),
            json!(["/a", "/a"]),
            json!(["/a", null]),
            json!(vec!["/a"; 257]),
        ] {
            let mut invalid = patch.clone();
            invalid["files"] = files;
            assert!(opencode_patch_change(&invalid).is_none());
        }
    }
}
