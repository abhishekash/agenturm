//! Shared bounded process runtime for one explicitly configured JSONL source.
//!
//! Format-specific translation is supplied by a `SourceFormat` implementation.
//! The host owns admission, cursor commit and capture policy.

#![forbid(unsafe_code)]

use agenturm_protocol::MAX_FRAME_BYTES;
use agenturm_source_process::Poller;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

const MAX_LINE: u64 = 1024 * 1024;
const MAX_SCAN: u64 = 1024 * 1024;
// Full-prefix verification is intentionally capped. Crossing the ceiling
// stops this opt-in source before any cursor acknowledgment.
const MAX_VERIFIED_PREFIX: u64 = 512 * 1024 * 1024;
// Match observatory.plugin_wire.MAX_STRUCTURAL_TOKENS for the serialized batch.
pub const MAX_STRUCTURAL_TOKENS: usize = 8192;
pub const MAX_CURSOR: usize = 512;

pub const fn max_frame_bytes() -> usize {
    MAX_FRAME_BYTES
}

/// Adapter-specific decoding of a complete source line. A checkpoint is
/// persisted only when every fact from that line fits in the acknowledged batch.
pub trait SourceFormat {
    type Context: Clone;
    const PLUGIN_ID: &'static str;
    const GAP_EVENT_PREFIX: &'static str;
    /// Derive initial context from the configured path and first complete
    /// line. The line may be a metadata header or the first event, depending
    /// on the harness. It is still passed to `translate_line` for admission.
    fn header_identity(path: &Path, raw: &[u8]) -> Result<Self::Context>;
    /// Empty or a bounded ASCII identifier accepted by the durable cursor.
    fn checkpoint(context: &Self::Context) -> String;
    fn restore(context: &mut Self::Context, checkpoint: &str) -> Result<()>;
    fn translate_line(
        raw: &[u8],
        context: &Self::Context,
        origin: &str,
        offset: u64,
    ) -> Result<(Vec<Value>, Self::Context)>;
}

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn checked_checkpoint<F: SourceFormat>(context: &F::Context) -> Result<String> {
    let value = F::checkpoint(context);
    if !value.is_empty() && !identity(&value) {
        return Err(invalid("invalid source checkpoint"));
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cursor {
    version: u8,
    device: u64,
    inode: u64,
    offset: u64,
    digest: String,
    header_digest: String,
    generation: String,
    checkpoint: String,
}

impl Cursor {
    fn parse(raw: &str) -> Result<Option<Self>> {
        if raw.is_empty() {
            return Ok(None);
        }
        if raw.len() > MAX_CURSOR {
            return Err(invalid("invalid cursor length"));
        }
        let (version, payload) = if let Some(payload) = raw.strip_prefix("v2:") {
            (2, payload)
        } else {
            (1, raw)
        };
        let fields: Vec<_> = payload.splitn(7, ':').collect();
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
            version,
            device: fields[0].parse()?,
            inode: fields[1].parse()?,
            offset: fields[2].parse()?,
            digest: fields[3].to_owned(),
            header_digest: fields[4].to_owned(),
            generation: fields[5].to_owned(),
            checkpoint: fields[6].to_owned(),
        }))
    }

    fn encode(&self) -> String {
        format!(
            "v2:{}:{}:{}:{}:{}:{}:{}",
            self.device,
            self.inode,
            self.offset,
            self.digest,
            self.header_digest,
            self.generation,
            self.checkpoint
        )
    }
}

#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
compile_error!("The JSONL source runtime currently supports POSIX platforms only");

fn prefix_hasher(file: &mut File, offset: u64) -> Result<Sha256> {
    if offset > MAX_VERIFIED_PREFIX {
        return Err(invalid("source prefix exceeds verification limit"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut remaining = offset;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(buffer.len() as u64))?;
        let count = file.read(&mut buffer[..limit])?;
        if count == 0 {
            return Err(invalid("source prefix changed during verification"));
        }
        hasher.update(&buffer[..count]);
        remaining -= u64::try_from(count)?;
    }
    Ok(hasher)
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

pub fn batch(source_id: &str, cursor: &str, records: &[Value]) -> Value {
    json!({"type":"batch","protocol":1,"source_id":source_id,
           "cursor":cursor,"records":records})
}

pub fn structural_tokens(raw: &[u8]) -> usize {
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

pub fn poll<F: SourceFormat>(
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
            return Err(invalid("observed source header unavailable"));
        }
        return Ok(batch(source_id, raw_cursor, &[]));
    };
    let header_digest = format!("{:x}", Sha256::digest(&header));
    let mut context = F::header_identity(path, &header)?;
    if previous
        .as_ref()
        .is_some_and(|prior| prior.offset > MAX_VERIFIED_PREFIX)
    {
        return Err(invalid("source prefix exceeds verification limit"));
    }
    let mut changed = previous.as_ref().is_some_and(|prior| {
        prior.version != 2
            || (prior.device, prior.inode) != (device, inode)
            || prior.offset > metadata.len()
            || prior.header_digest != header_digest
    });
    if !changed && let Some(prior) = &previous {
        changed =
            format!("{:x}", prefix_hasher(&mut file, prior.offset)?.finalize()) != prior.digest;
    }
    let mut records = Vec::new();
    let mut event_ids = HashSet::new();
    if !changed && let Some(prior) = &previous {
        F::restore(&mut context, &prior.checkpoint)?;
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
    let mut prefix = prefix_hasher(&mut file, offset)?;
    if changed {
        let evidence = format!("{}|({}, {})", raw_cursor, device, inode);
        let id = format!("{}:{:x}", F::GAP_EVENT_PREFIX, Sha256::digest(evidence));
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
                return Err(invalid("source line exceeds scan budget"));
            }
            break;
        }
        let (candidates, candidate_context) = F::translate_line(&line, &context, origin, position)?;
        let candidate_ids: Vec<String> = candidates
            .iter()
            .map(|record| {
                record["event_id"]
                    .as_str()
                    .ok_or("missing source fact event ID")
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
                return Err(invalid("source line exceeds record budget"));
            }
            break;
        }
        let next_position = position + u64::try_from(line.len())?;
        if next_position > MAX_VERIFIED_PREFIX {
            return Err(invalid("source prefix exceeds verification limit"));
        }
        let mut candidate_prefix = prefix.clone();
        candidate_prefix.update(&line);
        let next = Cursor {
            version: 2,
            device,
            inode,
            offset: next_position,
            digest: format!("{:x}", candidate_prefix.clone().finalize()),
            header_digest: header_digest.clone(),
            generation: generation.clone(),
            checkpoint: checked_checkpoint::<F>(&candidate_context)?,
        }
        .encode();
        let before = records.len();
        records.extend(candidates);
        let encoded = serde_json::to_vec(&batch(source_id, &next, &records))?;
        if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
            records.truncate(before);
            if before == 0 {
                return Err(invalid("source line exceeds batch budget"));
            }
            break;
        }
        event_ids.extend(candidate_ids);
        position = next_position;
        prefix = candidate_prefix;
        context = candidate_context;
    }
    if prefix_hasher(&mut file, position)?.finalize() != prefix.clone().finalize() {
        return Err(invalid("source changed during poll"));
    }
    let next = Cursor {
        version: 2,
        device,
        inode,
        offset: position,
        digest: format!("{:x}", prefix.finalize()),
        header_digest,
        generation,
        checkpoint: checked_checkpoint::<F>(&context)?,
    }
    .encode();
    let response = batch(source_id, &next, &records);
    let encoded = serde_json::to_vec(&response)?;
    if encoded.len() > max_bytes || structural_tokens(&encoded) > MAX_STRUCTURAL_TOKENS {
        return Err(invalid("source diagnostic exceeds batch budget"));
    }
    Ok(response)
}

struct JsonlPoller<F>(std::marker::PhantomData<F>);

impl<F: SourceFormat> Poller for JsonlPoller<F> {
    const PLUGIN_ID: &'static str = F::PLUGIN_ID;

    fn poll(
        path: &Path,
        source_id: &str,
        cursor: &str,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Value> {
        poll::<F>(path, source_id, cursor, max_records, max_bytes)
    }
}

pub fn run<F: SourceFormat>() -> Result<()> {
    agenturm_source_process::run::<JsonlPoller<F>>()
}
