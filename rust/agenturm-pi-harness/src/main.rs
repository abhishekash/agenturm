//! Standalone, opt-in Rust source adapter for an append-only JSONL harness.
//!
//! The process owns no Agenturm storage. The Python host authenticates the
//! executable, admits each batch and acknowledges only after its Store commit.

#![forbid(unsafe_code)]

use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const PLUGIN_ID: &str = "example.rust-pi-harness";
const CONTROL_LIMIT: usize = 64 * 1024;
const MAX_LINE: u64 = 1024 * 1024;
const MAX_SCAN: u64 = 1024 * 1024;
const MAX_RECORDS: u64 = 256;
// Match observatory.plugin_wire.MAX_STRUCTURAL_TOKENS for the serialized batch.
const MAX_STRUCTURAL_TOKENS: usize = 8192;
const MAX_CURSOR: usize = 512;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

mod pi;
use pi::{header_identity, translate_line};

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cursor {
    device: u64,
    inode: u64,
    offset: u64,
    digest: String,
    header_digest: String,
    generation: String,
    turn_id: String,
}

impl Cursor {
    fn parse(raw: &str) -> Result<Option<Self>> {
        if raw.is_empty() {
            return Ok(None);
        }
        if raw.len() > MAX_CURSOR {
            return Err(invalid("invalid cursor length"));
        }
        let fields: Vec<_> = raw.splitn(7, ':').collect();
        if fields.len() != 7
            || fields[..3]
                .iter()
                .any(|field| field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()))
            || fields[3].len() != 64
            || !fields[3]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || fields[4].len() != 64
            || !fields[4]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || fields[5].len() != 64
            || !fields[5]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || (!fields[6].is_empty() && !identity(fields[6]))
        {
            return Err(invalid("invalid cursor format"));
        }
        Ok(Some(Self {
            device: fields[0].parse()?,
            inode: fields[1].parse()?,
            offset: fields[2].parse()?,
            digest: fields[3].to_owned(),
            header_digest: fields[4].to_owned(),
            generation: fields[5].to_owned(),
            turn_id: fields[6].to_owned(),
        }))
    }

    fn encode(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.device,
            self.inode,
            self.offset,
            self.digest,
            self.header_digest,
            self.generation,
            self.turn_id
        )
    }
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
compile_error!("The Pi harness currently supports POSIX platforms only");

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
    let Some(header) = read_line(&mut file, 0)? else {
        // A previously observed source without a complete header is a gap,
        // not an idle poll. Keep its durable cursor and fail visibly.
        if previous.is_some() {
            return Err(invalid("observed Pi source header unavailable"));
        }
        return Ok(batch(source_id, raw_cursor, &[]));
    };
    let header_digest = format!("{:x}", Sha256::digest(&header));
    let mut context = header_identity(&header)?;
    let changed = previous.as_ref().is_some_and(|prior| {
        (prior.device, prior.inode) != (device, inode)
            || prior.offset > metadata.len()
            || prior.header_digest != header_digest
    }) || previous.as_ref().is_some_and(|prior| {
        prior.offset <= metadata.len()
            && tail_digest(&mut file, prior.offset).map_or(true, |digest| digest != prior.digest)
    });
    let mut records = Vec::new();
    let mut event_ids = HashSet::new();
    if !changed && let Some(prior) = &previous {
        context.turn_id.clone_from(&prior.turn_id);
    }
    let generation = match (&previous, changed) {
        (Some(prior), true) => format!(
            "{:x}",
            Sha256::digest(format!("{}:gap", prior.generation).as_bytes())
        ),
        (Some(prior), false) => prior.generation.clone(),
        (None, _) => format!(
            "{:x}",
            Sha256::digest(format!("{device}:{inode}:{header_digest}").as_bytes())
        ),
    };
    let origin = generation.as_str();
    let offset = if changed {
        0
    } else {
        previous.as_ref().map_or(0, |cursor| cursor.offset)
    };
    if changed {
        let evidence = format!("{}|({}, {})", raw_cursor, device, inode);
        let id = format!("pi-gap:{:x}", Sha256::digest(evidence));
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
        if line.len() as u64 > MAX_SCAN - (position - offset) {
            if records.is_empty() {
                return Err(invalid("Pi source line exceeds scan budget"));
            }
            break;
        }
        let (candidates, candidate_context) = translate_line(&line, &context, origin, position)?;
        let candidate_ids: Vec<String> = candidates
            .iter()
            .map(|record| {
                record["event_id"]
                    .as_str()
                    .ok_or("missing Pi fact event ID")
                    .map(str::to_owned)
            })
            .collect::<std::result::Result<_, _>>()?;
        if candidate_ids.iter().any(|id| event_ids.contains(id)) {
            // A batch may contain only one revision of each event. Leave the
            // repeated line unread so it is emitted after this batch is acked.
            break;
        }
        if candidates.len() > max_records - records.len() {
            if records.is_empty() {
                return Err(invalid("Pi source line exceeds record budget"));
            }
            break;
        }
        let next_position = position + u64::try_from(line.len())?;
        let next = Cursor {
            device,
            inode,
            offset: next_position,
            digest: tail_digest(&mut file, next_position)?,
            header_digest: header_digest.clone(),
            generation: generation.clone(),
            turn_id: candidate_context.turn_id.clone(),
        }
        .encode();
        let before = records.len();
        records.extend(candidates);
        let encoded = serde_json::to_vec(&batch(source_id, &next, &records))?;
        if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
            records.truncate(before);
            if before == 0 {
                return Err(invalid("Pi source line exceeds batch budget"));
            }
            break;
        }
        event_ids.extend(candidate_ids);
        position = next_position;
        context = candidate_context;
    }
    let next = Cursor {
        device,
        inode,
        offset: position,
        digest: tail_digest(&mut file, position)?,
        header_digest,
        generation,
        turn_id: context.turn_id,
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
                "agenturm-pi-reader-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn source(&self) -> PathBuf {
            self.0.join("session.jsonl")
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

    fn facts(value: &Value) -> &[Value] {
        value["records"].as_array().unwrap()
    }

    #[test]
    fn line_facts_are_atomic_and_turn_survives_ack_restart() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"pi-session"}"#);
        append(
            &path,
            r#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user","content":[{"type":"text","text":"private request"}]}}"#,
        );
        append(
            &path,
            r#"{"type":"message","id":"assistant-1","timestamp":"2026-10-24T12:00:01Z","message":{"role":"assistant","stopReason":"toolUse","usage":{"input":8,"output":3},"content":[{"type":"toolCall","id":"call-1","name":"read"},{"type":"toolCall","id":"call-2","name":"bash"}]}}"#,
        );
        let first = poll(&path, "source", "", 3, MAX_FRAME_BYTES).unwrap();
        assert_eq!(facts(&first).len(), 2); // Header plus entire user line.
        let cursor = first["cursor"].as_str().unwrap();
        assert!(cursor.len() <= MAX_CURSOR);
        assert!(cursor.ends_with(":user-1"));
        let second = poll(&path, "source", cursor, 5, MAX_FRAME_BYTES).unwrap();
        assert_eq!(facts(&second).len(), 5); // Assistant line is not split.
        assert!(
            facts(&second)
                .iter()
                .all(|fact| fact["identity"]["turn_id"] == "user-1")
        );
        assert_eq!(facts(&second)[2]["identity"]["operation_id"], "call-1");
        let second_cursor = second["cursor"].as_str().unwrap();
        assert_eq!(
            facts(&poll(&path, "source", second_cursor, 256, MAX_FRAME_BYTES).unwrap()).len(),
            0
        );
        assert!(poll(&path, "source", cursor, 3, MAX_FRAME_BYTES).is_err());
        assert!(poll(&path, "source", cursor, 256, 300).is_err());
    }

    #[test]
    fn replacement_emits_gap_and_resets_turn() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        append(
            &path,
            r#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#,
        );
        let first = poll(&path, "source", "", 256, MAX_FRAME_BYTES).unwrap();
        let replacement = scratch.0.join("next.jsonl");
        append(&replacement, r#"{"type":"session","id":"second"}"#);
        append(
            &replacement,
            r#"{"type":"message","id":"user-2","timestamp":"2026-10-24T12:01:00Z","message":{"role":"user"}}"#,
        );
        fs::rename(replacement, &path).unwrap();
        let after = poll(
            &path,
            "source",
            first["cursor"].as_str().unwrap(),
            256,
            MAX_FRAME_BYTES,
        )
        .unwrap();
        assert_eq!(facts(&after)[0]["payload"]["code"], "possible_source_gap");
        assert_eq!(facts(&after)[1]["identity"]["session_id"], "second");
        assert_eq!(facts(&after)[1]["identity"]["turn_id"], "user-2");
        assert_ne!(facts(&after)[1]["event_id"], facts(&first)[0]["event_id"]);
    }

    #[test]
    fn distant_header_rewrite_is_a_gap_and_cannot_keep_old_turn() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        append(
            &path,
            r#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#,
        );
        for index in 0..20 {
            append(
                &path,
                &format!(
                    r#"{{"type":"model_change","id":"model-{index}","timestamp":"2026-10-24T12:00:01Z"}}"#
                ),
            );
        }
        let first = poll(&path, "source", "", 256, MAX_FRAME_BYTES).unwrap();
        let before = first["cursor"].as_str().unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        fs::write(&path, raw.replacen("first", "other", 1)).unwrap();
        let after = poll(&path, "source", before, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(facts(&after)[0]["payload"]["code"], "possible_source_gap");
        assert_eq!(facts(&after)[1]["identity"]["session_id"], "other");
        assert_ne!(facts(&first)[0]["event_id"], facts(&after)[1]["event_id"]);
    }

    #[test]
    fn partial_and_oversized_lines_do_not_advance() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"first"}"#);
        fs::write(
            &path,
            format!(
                "{}\n{{\"type\":\"message\"",
                r#"{"type":"session","id":"first"}"#
            ),
        )
        .unwrap();
        let first = poll(&path, "source", "", 256, MAX_FRAME_BYTES).unwrap();
        let cursor = first["cursor"].as_str().unwrap();
        assert!(facts(&first).is_empty());
        assert!(cursor.len() > 64);
        let idle = poll(&path, "source", cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(idle["cursor"], cursor);
        assert!(facts(&idle).is_empty());
        append(
            &path,
            r#", "id":"u","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#,
        );
        let complete = poll(&path, "source", cursor, 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(facts(&complete).len(), 2);
        fs::write(&path, b"{\"type\":\"session\"").unwrap();
        assert!(
            poll(
                &path,
                "source",
                complete["cursor"].as_str().unwrap(),
                256,
                MAX_FRAME_BYTES
            )
            .is_err()
        );
    }

    #[test]
    fn structural_budget_pages_whole_pi_lines() {
        let scratch = Scratch::new();
        let path = scratch.source();
        append(&path, r#"{"type":"session","id":"pi-session"}"#);
        let blocks: Vec<Value> = (0..126)
            .map(|index| {
                json!({"type":"toolCall",
            "id":format!("call-{index}"),"name":"read"})
            })
            .collect();
        let row = json!({"type":"message","id":"assistant-1",
            "timestamp":"2026-10-24T12:00:01Z",
            "message":{"role":"assistant","stopReason":"toolUse","content":blocks}});
        append(&path, &row.to_string());
        let mut second = row;
        second["id"] = json!("assistant-2");
        for block in second["message"]["content"].as_array_mut().unwrap() {
            block["id"] = json!(format!("next-{}", block["id"].as_str().unwrap()));
        }
        append(&path, &second.to_string());
        let first = poll(&path, "source", "", 256, MAX_FRAME_BYTES).unwrap();
        assert_eq!(facts(&first).len(), 128);
        let resumed = poll(
            &path,
            "source",
            first["cursor"].as_str().unwrap(),
            256,
            MAX_FRAME_BYTES,
        )
        .unwrap();
        assert_eq!(facts(&resumed).len(), 128);
        let combined: Vec<Value> = facts(&first)
            .iter()
            .chain(facts(&resumed).iter())
            .cloned()
            .collect();
        assert_eq!(combined.len(), 256);
        let encoded = serde_json::to_vec(&batch("source", "cursor", &combined)).unwrap();
        assert!(encoded.len() < MAX_FRAME_BYTES);
        assert!(structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS);
    }
}
