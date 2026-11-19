//! Standalone, opt-in Rust source adapter for an append-only JSONL harness.
//!
//! The process owns no Agenturm storage. The Python host authenticates the
//! executable, admits each batch and acknowledges only after its Store commit.

#![forbid(unsafe_code)]

use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const PLUGIN_ID: &str = "example.rust-sample-harness";
const CONTROL_LIMIT: usize = 64 * 1024;
const MAX_LINE: u64 = 16 * 1024;
const MAX_SCAN: u64 = 1024 * 1024;
const MAX_RECORDS: u64 = 256;
// Match observatory.plugin_wire.MAX_STRUCTURAL_TOKENS for the serialized batch.
const MAX_STRUCTURAL_TOKENS: usize = 8192;
const MAX_CURSOR: usize = 512;
const MAX_ID: usize = 199;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn no_null<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Usage {
    #[serde(
        default,
        deserialize_with = "no_null",
        skip_serializing_if = "Option::is_none"
    )]
    input_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "no_null",
        skip_serializing_if = "Option::is_none"
    )]
    output_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "no_null",
        skip_serializing_if = "Option::is_none"
    )]
    cache_read_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "no_null",
        skip_serializing_if = "Option::is_none"
    )]
    cache_write_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "no_null",
        skip_serializing_if = "Option::is_none"
    )]
    total_tokens: Option<u64>,
}

impl Usage {
    fn valid(&self) -> bool {
        let values = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
            self.total_tokens,
        ];
        values.iter().any(Option::is_some)
            && values
                .into_iter()
                .flatten()
                .all(|value| value <= i64::MAX as u64)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRow {
    id: String,
    session: String,
    at_ns: String,
    kind: String,
    #[serde(default, deserialize_with = "no_null")]
    revision: Option<u32>,
    #[serde(default, deserialize_with = "no_null")]
    state: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    reason_code: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    usage: Option<Usage>,
    #[serde(default, deserialize_with = "no_null")]
    parent_session_id: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    relationship_kind: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    role: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    category: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    result: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    turn_id: Option<String>,
    #[serde(default, deserialize_with = "no_null")]
    operation_id: Option<String>,
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID + 1
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.as_bytes().iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

fn source_record(raw: &[u8]) -> Result<Value> {
    let row: SourceRow = serde_json::from_slice(raw)?;
    if !identity(&row.id)
        || !identity(&row.session)
        || row.at_ns.is_empty()
        || !row.at_ns.bytes().all(|byte| byte.is_ascii_digit())
        || (row.at_ns.starts_with('0') && row.at_ns != "0")
        || row
            .at_ns
            .parse::<u64>()
            .map_or(true, |value| value > i64::MAX as u64)
        || row.revision.is_some_and(|value| value > i32::MAX as u32)
        || row.turn_id.as_deref().is_some_and(|value| !identity(value))
        || row.operation_id.as_deref().is_some_and(|value| !identity(value))
        || (row.operation_id.is_some() && row.kind != "tool")
    {
        return Err(invalid("invalid source identity, timestamp or revision"));
    }
    let no_activity_fields = row.role.is_none() && row.category.is_none() && row.result.is_none();
    let no_other_fields = row.state.is_none()
        && row.reason_code.is_none()
        && row.usage.is_none()
        && row.parent_session_id.is_none()
        && row.relationship_kind.is_none();
    let payload = match row.kind.as_str() {
        "lifecycle"
            if row.usage.is_none()
                && row.parent_session_id.is_none()
                && row.relationship_kind.is_none()
                && no_activity_fields =>
        {
            let state = row.state.as_deref().ok_or("missing lifecycle state")?;
            if !matches!(
                state,
                "started"
                    | "running"
                    | "paused"
                    | "completed"
                    | "failed"
                    | "interrupted"
                    | "cancelled"
            ) || row.reason_code.as_deref().is_some_and(|value| !code(value))
            {
                return Err(invalid("invalid lifecycle metadata"));
            }
            let mut payload = json!({"state": state});
            if let Some(reason) = row.reason_code {
                payload["reason_code"] = json!(reason);
            }
            payload
        }
        "usage"
            if row.state.is_none()
                && row.reason_code.is_none()
                && row.parent_session_id.is_none()
                && row.relationship_kind.is_none()
                && no_activity_fields =>
        {
            let usage = row.usage.ok_or("missing usage")?;
            if !usage.valid() {
                return Err(invalid("invalid usage metadata"));
            }
            serde_json::to_value(usage)?
        }
        "relationship"
            if row.state.is_none()
                && row.reason_code.is_none()
                && row.usage.is_none()
                && no_activity_fields =>
        {
            let parent = row.parent_session_id.ok_or("missing parent session")?;
            let kind = row.relationship_kind.ok_or("missing relationship kind")?;
            if !identity(&parent) || !matches!(kind.as_str(), "delegated" | "review") {
                return Err(invalid("invalid relationship metadata"));
            }
            json!({"parent_session_id": parent, "kind": kind})
        }
        "message" if no_other_fields && row.category.is_none() && row.result.is_none() => {
            let role = row.role.ok_or("missing message role")?;
            if !matches!(role.as_str(), "user" | "assistant") {
                return Err(invalid("invalid message role"));
            }
            json!({"role": role})
        }
        "tool" if no_other_fields && row.role.is_none() => {
            let category = row.category.ok_or("missing tool category")?;
            let result = row.result.ok_or("missing tool result")?;
            if !matches!(category.as_str(), "shell" | "file" | "mcp" | "other")
                || !matches!(
                    result.as_str(),
                    "started" | "succeeded" | "failed" | "denied"
                )
            {
                return Err(invalid("invalid tool metadata"));
            }
            json!({"category": category, "result": result})
        }
        "check" if no_other_fields && row.role.is_none() => {
            let category = row.category.ok_or("missing check category")?;
            let result = row.result.ok_or("missing check result")?;
            if !matches!(
                category.as_str(),
                "test" | "lint" | "build" | "review" | "other"
            ) || !matches!(result.as_str(), "passed" | "failed" | "skipped" | "unknown")
            {
                return Err(invalid("invalid check metadata"));
            }
            json!({"category": category, "result": result})
        }
        _ => return Err(invalid("unsupported source record kind or fields")),
    };
    let mut source_identity = json!({"session_id": row.session});
    if let Some(turn) = row.turn_id {
        source_identity["turn_id"] = json!(turn);
    }
    if let Some(operation) = row.operation_id {
        source_identity["operation_id"] = json!(operation);
    }
    Ok(json!({"event_id": format!("rust-sample:{}", row.id),
              "revision": row.revision.unwrap_or(0), "kind": row.kind,
              "time_ns": row.at_ns, "identity": source_identity,
              "payload": payload}))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cursor {
    device: u64,
    inode: u64,
    offset: u64,
    digest: String,
}

impl Cursor {
    fn parse(raw: &str) -> Result<Option<Self>> {
        if raw.is_empty() {
            return Ok(None);
        }
        if raw.len() > MAX_CURSOR {
            return Err(invalid("invalid cursor length"));
        }
        let fields: Vec<_> = raw.split(':').collect();
        if fields.len() != 4
            || fields[..3]
                .iter()
                .any(|field| field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()))
            || fields[3].len() != 64
            || !fields[3]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(invalid("invalid cursor format"));
        }
        Ok(Some(Self {
            device: fields[0].parse()?,
            inode: fields[1].parse()?,
            offset: fields[2].parse()?,
            digest: fields[3].to_owned(),
        }))
    }

    fn encode(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.device, self.inode, self.offset, self.digest
        )
    }
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
compile_error!("The sample harness currently supports POSIX platforms only");

fn tail_digest(file: &mut File, offset: u64) -> Result<String> {
    let start = offset.saturating_sub(256);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; usize::try_from(offset - start)?];
    file.read_exact(&mut bytes)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn read_line(file: &mut File, offset: u64) -> Result<Option<Vec<u8>>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut line = Vec::new();
    let mut reader = BufReader::new(file.take(MAX_LINE + 1));
    reader.read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.len() as u64 > MAX_LINE {
        return Err(invalid("source line exceeds limit"));
    }
    if line.last() != Some(&b'\n') {
        return Ok(None);
    }
    Ok(Some(line))
}

fn file_time_ns(metadata: &Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos().min(i64::MAX as u128))
        .to_string()
}

fn batch(source_id: &str, cursor: &str, records: &[Value]) -> Value {
    json!({"type":"batch","protocol":1,"source_id":source_id,
           "cursor":cursor,"records":records})
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

fn poll(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    let previous = Cursor::parse(raw_cursor)?;
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && previous.is_none() => {
            return Ok(batch(source_id, raw_cursor, &[]));
        }
        Err(_) => return Err(invalid("source file unavailable")),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("source is not a regular file"));
    }
    let (device, inode) = file_identity(&metadata);
    let changed = previous.as_ref().is_some_and(|prior| {
        (prior.device, prior.inode) != (device, inode) || prior.offset > metadata.len()
    }) || previous.as_ref().is_some_and(|prior| {
        prior.offset <= metadata.len()
            && tail_digest(&mut file, prior.offset).map_or(true, |digest| digest != prior.digest)
    });
    let mut records = Vec::new();
    let mut event_ids = HashSet::new();
    let offset = if changed {
        0
    } else {
        previous.as_ref().map_or(0, |cursor| cursor.offset)
    };
    if changed {
        let evidence = format!("{}|({}, {})", raw_cursor, device, inode);
        let id = format!("rust-sample-gap:{:x}", Sha256::digest(evidence));
        event_ids.insert(id.clone());
        records.push(json!({"event_id":id,"revision":0,"kind":"diagnostic",
                            "time_ns":file_time_ns(&metadata),
                            "payload":{"code":"possible_source_gap","severity":"warning"}}));
    }
    let mut position = offset;
    while records.len() < max_records && position - offset < MAX_SCAN {
        let Some(line) = read_line(&mut file, position)? else {
            break;
        };
        let candidate = source_record(&line)?;
        let event_id = candidate["event_id"]
            .as_str()
            .ok_or("missing source event ID")?
            .to_owned();
        if event_ids.contains(&event_id) {
            // A batch may contain only one revision of each event. Leave the
            // repeated line unread so it is emitted after this batch is acked.
            break;
        }
        let next_position = position + u64::try_from(line.len())?;
        let next = Cursor {
            device,
            inode,
            offset: next_position,
            digest: tail_digest(&mut file, next_position)?,
        }
        .encode();
        records.push(candidate);
        let encoded = serde_json::to_vec(&batch(source_id, &next, &records))?;
        if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
            records.pop();
            if records.is_empty() {
                return Err(invalid("source record exceeds batch budget"));
            }
            break;
        }
        event_ids.insert(event_id);
        position = next_position;
    }
    let next = Cursor {
        device,
        inode,
        offset: position,
        digest: tail_digest(&mut file, position)?,
    }
    .encode();
    let response = batch(source_id, &next, &records);
    let encoded = serde_json::to_vec(&response)?;
    if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
        return Err(invalid("source diagnostic exceeds batch budget"));
    }
    Ok(response)
}

#[derive(Default)]
struct Session {
    source_id: String,
    max_frame: usize,
    pending: Option<String>,
    stopped: bool,
}

impl Session {
    fn handle(&mut self, request: Value, path: &Path) -> Result<Option<(Value, usize)>> {
        let object = request
            .as_object()
            .ok_or("control frame must be an object")?;
        if object.get("protocol").and_then(Value::as_u64) != Some(1) {
            return Err(invalid("protocol mismatch"));
        }
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or("missing control type")?;
        if self.source_id.is_empty() {
            if kind != "hello"
                || object.len() != 5
                || object.get("plugin_id").and_then(Value::as_str) != Some(PLUGIN_ID)
            {
                return Err(invalid("hello identity mismatch"));
            }
            let source = object
                .get("source_id")
                .and_then(Value::as_str)
                .ok_or("missing source")?;
            let limit = object
                .get("max_frame_bytes")
                .and_then(Value::as_u64)
                .ok_or("missing frame budget")?;
            if !identity(source) || limit == 0 || limit > MAX_FRAME_BYTES as u64 {
                return Err(invalid("invalid hello source or budget"));
            }
            self.source_id = source.to_owned();
            self.max_frame = limit as usize;
            return Ok(Some((
                json!({"type":"hello_ack","protocol":1,
                "plugin_id":PLUGIN_ID,
                "features":["source-adapter/1","configured-root/1"]}),
                self.max_frame,
            )));
        }
        if kind == "stop" && object.len() == 2 {
            self.stopped = true;
            return Ok(None);
        }
        if object.get("source_id").and_then(Value::as_str) != Some(self.source_id.as_str()) {
            return Err(invalid("source mismatch"));
        }
        match kind {
            "poll" if object.len() == 6 => {
                if self.pending.is_some() {
                    return Err(invalid("prior poll not acknowledged"));
                }
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing cursor")?;
                let count = object
                    .get("max_records")
                    .and_then(Value::as_u64)
                    .ok_or("missing record budget")?;
                let bytes = object
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .ok_or("missing byte budget")?;
                if count == 0 || count > MAX_RECORDS || bytes == 0 || bytes > self.max_frame as u64
                {
                    return Err(invalid("invalid poll budget"));
                }
                let response = poll(
                    path,
                    &self.source_id,
                    cursor,
                    count as usize,
                    bytes as usize,
                )?;
                self.pending = Some(
                    response["cursor"]
                        .as_str()
                        .ok_or("missing response cursor")?
                        .to_owned(),
                );
                Ok(Some((response, bytes as usize)))
            }
            "ack" if object.len() == 4 => {
                let cursor = object
                    .get("cursor")
                    .and_then(Value::as_str)
                    .ok_or("missing ack cursor")?;
                if self.pending.as_deref() != Some(cursor) {
                    return Err(invalid("ack mismatch"));
                }
                self.pending = None;
                Ok(None)
            }
            _ => Err(invalid("invalid control message")),
        }
    }
}

fn run() -> Result<()> {
    let configured =
        std::env::var_os("AGENTURM_PLUGIN_SOURCE_ROOT").ok_or("configured source root required")?;
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err(invalid("configured source path must be absolute"));
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut session = Session::default();
    loop {
        let Some(raw) = read_frame(&mut reader, CONTROL_LIMIT)? else {
            return Ok(());
        };
        let request: Value = serde_json::from_str(&raw)?;
        if let Some((response, limit)) = session.handle(request, &path)? {
            write_frame(&mut writer, &serde_json::to_string(&response)?, limit)?;
        }
        if session.stopped {
            return Ok(());
        }
    }
}

fn main() {
    // The host receives only a nonzero exit and its own coarse worker state;
    // never leak configured paths or source payloads through stderr.
    if run().is_err() {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agenturm-rust-sample-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn source(&self) -> PathBuf {
            self.0.join("source.jsonl")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn append(path: &Path, line: &str) {
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap(),
            "{line}"
        )
        .unwrap();
    }

    #[test]
    fn strict_source_fields_and_typed_metadata() {
        let lifecycle = source_record(br#"{"id":"start","session":"child","at_ns":"9007199254740993","kind":"lifecycle","state":"started"}"#).unwrap();
        assert_eq!(lifecycle["time_ns"], "9007199254740993");
        let usage = source_record(br#"{"id":"tokens","session":"child","at_ns":"2","kind":"usage","usage":{"input_tokens":0,"output_tokens":9223372036854775807}}"#).unwrap();
        assert_eq!(
            usage["payload"]["output_tokens"].as_u64(),
            Some(i64::MAX as u64)
        );
        let relationship = source_record(br#"{"id":"parent","session":"child","at_ns":"3","kind":"relationship","parent_session_id":"root","relationship_kind":"delegated"}"#).unwrap();
        assert_eq!(
            relationship["payload"],
            json!({"parent_session_id":"root","kind":"delegated"})
        );
        let message = source_record(
            br#"{"id":"request","session":"child","at_ns":"4","kind":"message","role":"user"}"#,
        )
        .unwrap();
        assert_eq!(message["payload"], json!({"role":"user"}));
        let tool = source_record(br#"{"id":"call","session":"child","at_ns":"5","kind":"tool","category":"shell","result":"failed","turn_id":"turn-1","operation_id":"call-1"}"#).unwrap();
        assert_eq!(
            tool["payload"],
            json!({"category":"shell","result":"failed"})
        );
        assert_eq!(tool["identity"]["operation_id"], "call-1");
        assert_eq!(tool["identity"]["turn_id"], "turn-1");
        let check = source_record(br#"{"id":"test","session":"child","at_ns":"6","kind":"check","category":"test","result":"passed"}"#).unwrap();
        assert_eq!(
            check["payload"],
            json!({"category":"test","result":"passed"})
        );
        for bad in [
            r#"{"id":"x","session":"child","at_ns":"1","kind":"lifecycle","state":"started","prompt":"secret"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"lifecycle","state":"started","state":"failed"}"#,
            r#"{"id":"x","session":"child","at_ns":"01","kind":"lifecycle","state":"started"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"usage","usage":{"input_tokens":-1}}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"relationship","parent_session_id":"../secret","relationship_kind":"delegated"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"usage","usage":{"input_tokens":1,"input_tokens":2}}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"message","role":"user","text":"secret"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"message","role":"user","state":"completed"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"message","role":null}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"tool","category":"shell","result":"succeeded","command":"cat secret"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"tool","category":"shell","result":"passed"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"check","category":"test"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"check","category":"test","result":"failed","output":"secret"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"message","role":"user","operation_id":"call-1"}"#,
            r#"{"id":"x","session":"child","at_ns":"1","kind":"tool","category":"shell","result":"started","operation_id":"../secret"}"#,
        ] {
            assert!(source_record(bad.as_bytes()).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn append_cursor_and_correction_are_stable() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(
            &path,
            r#"{"id":"start","session":"one","at_ns":"1","kind":"lifecycle","state":"started"}"#,
        );
        let first = poll(&path, "fixture", "", 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(first["records"].as_array().unwrap().len(), 1);
        let cursor = first["cursor"].as_str().unwrap();
        let idle = poll(&path, "fixture", cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(idle["cursor"], cursor);
        assert_eq!(idle["records"], json!([]));
        append(
            &path,
            r#"{"id":"start","session":"one","at_ns":"1","kind":"lifecycle","revision":1,"state":"completed"}"#,
        );
        let corrected = poll(&path, "fixture", cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(corrected["records"][0]["revision"], 1);
        assert_eq!(corrected["records"][0]["event_id"], "rust-sample:start");
    }

    #[test]
    fn corrections_present_before_first_poll_are_paged_without_skipping() {
        let scratch = Scratch::new();
        let path = scratch.source();
        for line in [
            r#"{"id":"start","session":"one","at_ns":"1","kind":"lifecycle","state":"started"}"#,
            r#"{"id":"start","session":"one","at_ns":"1","kind":"lifecycle","revision":1,"state":"running"}"#,
            r#"{"id":"start","session":"one","at_ns":"1","kind":"lifecycle","revision":2,"state":"completed"}"#,
            r#"{"id":"tail","session":"one","at_ns":"2","kind":"usage","usage":{"input_tokens":3}}"#,
        ] {
            append(&path, line);
        }
        let first = poll(&path, "fixture", "", 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(first["records"].as_array().unwrap().len(), 1);
        assert_eq!(first["records"][0]["revision"], 0);
        let first_cursor = first["cursor"].as_str().unwrap();
        let exact_bytes = serde_json::to_vec(&first).unwrap().len();
        assert_eq!(poll(&path, "fixture", "", 256, exact_bytes).unwrap(), first);
        assert!(poll(&path, "fixture", "", 256, exact_bytes - 1).is_err());

        let second = poll(&path, "fixture", first_cursor, 1, MAX_FRAME_BYTES).unwrap();
        assert_eq!(second["records"].as_array().unwrap().len(), 1);
        assert_eq!(second["records"][0]["revision"], 1);
        let second_cursor = second["cursor"].as_str().unwrap();
        assert_ne!(second_cursor, first_cursor);
        let third = poll(&path, "fixture", second_cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(third["records"].as_array().unwrap().len(), 2);
        assert_eq!(third["records"][0]["revision"], 2);
        assert_eq!(third["records"][1]["event_id"], "rust-sample:tail");
        let end_cursor = third["cursor"].as_str().unwrap();
        assert_eq!(
            poll(&path, "fixture", end_cursor, 256, MAX_FRAME_BYTES).unwrap()["records"],
            json!([])
        );
    }

    #[test]
    fn full_usage_backlog_respects_receiver_structure_budget() {
        let scratch = Scratch::new();
        let path = scratch.source();
        for index in 0..256 {
            append(
                &path,
                &format!(
                    r#"{{"id":"usage-{index}","session":"one","at_ns":"{index}","kind":"usage","usage":{{"input_tokens":1,"output_tokens":2,"cache_read_tokens":3,"cache_write_tokens":4,"total_tokens":5}}}}"#
                ),
            );
        }
        let mut cursor = String::new();
        let (mut batches, mut received) = (0, 0);
        loop {
            let response = poll(&path, "fixture", &cursor, 256, MAX_FRAME_BYTES).unwrap();
            let encoded = serde_json::to_vec(&response).unwrap();
            assert!(structural_tokens(&encoded) <= MAX_STRUCTURAL_TOKENS);
            let records = response["records"].as_array().unwrap();
            if records.is_empty() {
                break;
            }
            let next = response["cursor"].as_str().unwrap();
            assert_ne!(next, cursor);
            cursor = next.to_owned();
            batches += 1;
            received += records.len();
        }
        assert!(batches > 1);
        assert_eq!(received, 256);
    }

    #[test]
    fn partial_line_and_rewrite_emit_gap_without_stale_bytes() {
        let scratch = Scratch::new();
        let path = scratch.source();
        fs::write(&path, b"{\"id\":\"pending\"").unwrap();
        let first = poll(&path, "fixture", "", 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(first["records"], json!([]));
        let cursor = first["cursor"].as_str().unwrap();
        let pending = poll(&path, "fixture", cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(pending["records"], json!([]));
        append(
            &path,
            r#", "session":"one","at_ns":"1","kind":"lifecycle","state":"running"}"#,
        );
        let ready = poll(&path, "fixture", cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(ready["records"][0]["event_id"], "rust-sample:pending");
        let before = ready["cursor"].as_str().unwrap();
        let old_size = fs::metadata(&path).unwrap().len();
        let existing = fs::read_to_string(&path).unwrap();
        fs::write(&path, existing.replace("pending", "changed")).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), old_size);
        let changed = poll(&path, "fixture", before, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            changed["records"][0]["payload"]["code"],
            "possible_source_gap"
        );
        assert_eq!(changed["records"][1]["event_id"], "rust-sample:changed");
    }

    #[test]
    fn control_ack_and_limits() {
        let scratch = Scratch::new();
        let mut session = Session::default();
        let hello = session
            .handle(
                json!({"type":"hello","protocol":1,
            "plugin_id":PLUGIN_ID,"source_id":"sample","max_frame_bytes":MAX_FRAME_BYTES}),
                &scratch.source(),
            )
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(
            hello["features"],
            json!(["source-adapter/1", "configured-root/1"])
        );
        let poll_request = json!({"type":"poll","protocol":1,"source_id":"sample",
                                  "cursor":"","max_records":256,"max_bytes":MAX_FRAME_BYTES});
        let idle = session
            .handle(poll_request.clone(), &scratch.source())
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(idle["cursor"], "");
        assert!(
            session
                .handle(poll_request.clone(), &scratch.source())
                .is_err()
        );
        assert!(
            session
                .handle(
                    json!({"type":"ack","protocol":1,
            "source_id":"sample","cursor":"wrong"}),
                    &scratch.source()
                )
                .is_err()
        );
        session
            .handle(
                json!({"type":"ack","protocol":1,
            "source_id":"sample","cursor":""}),
                &scratch.source(),
            )
            .unwrap();
        assert!(session.handle(poll_request, &scratch.source()).is_ok());
    }
}
