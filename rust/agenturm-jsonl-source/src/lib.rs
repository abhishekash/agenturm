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
use std::fmt;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const MAX_LINE: u64 = 1024 * 1024;
const MAX_SCAN: u64 = 1024 * 1024;
// Full-prefix verification is intentionally capped. Crossing the ceiling
// stops this opt-in source before any cursor acknowledgment.
const MAX_VERIFIED_PREFIX: u64 = 512 * 1024 * 1024;
// Match observatory.plugin_wire.MAX_STRUCTURAL_TOKENS for the serialized batch.
pub const MAX_STRUCTURAL_TOKENS: usize = 8192;
pub const MAX_CURSOR: usize = agenturm_source_process::MAX_CURSOR_BYTES;
// The encoded source cursor adds at most 261 bytes around its adapter
// checkpoint (three u64 fields, three SHA-256 digests and separators).
const MAX_CHECKPOINT: usize = MAX_CURSOR - 261;

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
    /// Optionally upgrade a cursor committed at the end of only its first
    /// complete line. The caller has verified the original file and prefix.
    /// The adapter must update its checkpoint to distinguish a committed upgrade.
    fn resume_verified_header(
        _raw: &[u8],
        _context: &mut Self::Context,
        _origin: &str,
    ) -> Result<Option<Value>> {
        Ok(None)
    }
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

#[derive(Debug)]
struct ContinuityError(&'static str);

impl fmt::Display for ContinuityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for ContinuityError {}

fn continuity(message: &'static str) -> Box<dyn Error> {
    Box::new(ContinuityError(message))
}

/// A discovered member can lose continuity after enumeration but before or
/// during its bounded read. This excludes cursor, format and budget errors.
pub fn is_continuity_error(error: &(dyn Error + 'static)) -> bool {
    error.is::<ContinuityError>()
}

fn classify_read(error: Box<dyn Error>) -> Box<dyn Error> {
    if error.is::<io::Error>() {
        continuity("source read unavailable")
    } else {
        error
    }
}

fn checkpoint_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CHECKPOINT
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn checked_checkpoint<F: SourceFormat>(context: &F::Context) -> Result<String> {
    let value = F::checkpoint(context);
    if !value.is_empty() && !checkpoint_identity(&value) {
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
            || (!fields[6].is_empty() && !checkpoint_identity(fields[6]))
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

/// Decoded, bounded source checkpoint for format-owned reference nominations.
/// This is observational only; the receiver must prove bytes independently.
pub struct CursorProof {
    pub version: u8,
    pub device: u64,
    pub inode: u64,
    pub offset: u64,
    pub digest: String,
    pub header_digest: String,
    pub generation: String,
}

pub fn cursor_proof(raw: &str) -> Result<Option<CursorProof>> {
    Ok(Cursor::parse(raw)?.map(|cursor| CursorProof {
        version: cursor.version,
        device: cursor.device,
        inode: cursor.inode,
        offset: cursor.offset,
        digest: cursor.digest,
        header_digest: cursor.header_digest,
        generation: cursor.generation,
    }))
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
            return Err(continuity("source prefix changed during verification"));
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
    poll_inner::<F>(
        path,
        source_id,
        raw_cursor,
        max_records,
        max_bytes,
        false,
        MAX_STRUCTURAL_TOKENS,
    )
}

/// Directory discovery refuses a final-component symlink after scanning.
/// Explicit single-file sources keep their existing path semantics.
pub fn poll_no_follow<F: SourceFormat>(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
) -> Result<Value> {
    // The directory envelope adds member_id and member_cursor fields.
    poll_inner::<F>(
        path,
        source_id,
        raw_cursor,
        max_records,
        max_bytes,
        true,
        MAX_STRUCTURAL_TOKENS - 32,
    )
}

fn poll_inner<F: SourceFormat>(
    path: &Path,
    source_id: &str,
    raw_cursor: &str,
    max_records: usize,
    max_bytes: usize,
    no_follow: bool,
    token_limit: usize,
) -> Result<Value> {
    let previous = Cursor::parse(raw_cursor)?;
    // A directory-discovered file swapped to a symlink after scanning must
    // not redirect the reader to a different final file.
    let opened = if no_follow {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    } else {
        File::open(path)
    };
    let mut file = match opened {
        Ok(file) => file,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && previous.is_none() && !no_follow =>
        {
            return Ok(batch(source_id, raw_cursor, &[]));
        }
        Err(_) => return Err(continuity("source file unavailable")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| continuity("source metadata unavailable"))?;
    if !metadata.is_file() {
        return Err(continuity("source is not a regular file"));
    }
    if no_follow {
        let path_metadata = path
            .symlink_metadata()
            .map_err(|_| continuity("source path metadata unavailable"))?;
        if path_metadata.file_type().is_symlink()
            || !path_metadata.is_file()
            || file_identity(&metadata) != file_identity(&path_metadata)
        {
            return Err(continuity("source identity changed during open"));
        }
    }
    let (device, inode) = file_identity(&metadata);
    let Some(header) = read_line(&mut file, 0).map_err(classify_read)? else {
        // A previously observed source without a complete header is a gap,
        // not an idle poll. Keep its durable cursor and fail visibly.
        if previous.is_some() {
            return Err(continuity("observed source header unavailable"));
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
    let mut verified_prefix = None;
    if !changed && let Some(prior) = &previous {
        let prefix = prefix_hasher(&mut file, prior.offset).map_err(classify_read)?;
        changed = format!("{:x}", prefix.clone().finalize()) != prior.digest;
        if !changed {
            verified_prefix = Some(prefix);
        }
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
    // Continue from the prefix already verified against the durable cursor.
    // Re-reading it here could adopt an in-place rewrite after verification
    // while still using the original header context for a one-shot upgrade.
    let mut prefix = if let Some(prefix) = verified_prefix {
        prefix
    } else {
        prefix_hasher(&mut file, offset).map_err(classify_read)?
    };
    let upgraded_header = if let Some(prior) = &previous
        && !changed
        && offset == header.len() as u64
    {
        let upgrade = F::resume_verified_header(&header, &mut context, origin)?;
        if upgrade.is_some()
            && (max_records == 0 || checked_checkpoint::<F>(&context)? == prior.checkpoint)
        {
            return Err(invalid(
                "source header upgrade must fit and advance its checkpoint",
            ));
        }
        upgrade
    } else {
        None
    };
    // Commit the newly established identity before parsing additional rows.
    // This also prevents an unrelated malformed append from blocking it.
    let upgrading_header = upgraded_header.is_some();
    records.extend(upgraded_header);
    if changed {
        let evidence = format!("{}|({}, {})", raw_cursor, device, inode);
        let id = format!("{}:{:x}", F::GAP_EVENT_PREFIX, Sha256::digest(evidence));
        event_ids.insert(id.clone());
        // The receiver can request the same batch after an ack crash. File
        // mtime may change without changing the source identity or bytes;
        // an observational wall timestamp would make exact replay diverge.
        // Zero means the gap's wall time is unknown, as in the OpenCode reader.
        records.push(json!({"event_id":id,"revision":0,"kind":"diagnostic",
                            "time_ns":"0",
                            "payload":{"code":"possible_source_gap","severity":"warning"}}));
    }
    let mut position = offset;
    while !upgrading_header && records.len() < max_records && position - offset < MAX_SCAN {
        let Some(line) = read_line(&mut file, position).map_err(classify_read)? else {
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
        if encoded.len() > max_bytes || structural_tokens(&encoded) > token_limit {
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
    if prefix_hasher(&mut file, position)
        .map_err(classify_read)?
        .finalize()
        != prefix.clone().finalize()
    {
        return Err(continuity("source changed during poll"));
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
    if encoded.len() > max_bytes || structural_tokens(&encoded) > token_limit {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    struct EmptyFormat;

    impl SourceFormat for EmptyFormat {
        type Context = ();
        const PLUGIN_ID: &'static str = "fixture.jsonl";
        const GAP_EVENT_PREFIX: &'static str = "fixture-gap";

        fn header_identity(_path: &Path, _raw: &[u8]) -> Result<Self::Context> {
            Ok(())
        }

        fn checkpoint(_context: &Self::Context) -> String {
            String::new()
        }

        fn restore(_context: &mut Self::Context, checkpoint: &str) -> Result<()> {
            if checkpoint.is_empty() {
                Ok(())
            } else {
                Err(invalid("unexpected fixture checkpoint"))
            }
        }

        fn translate_line(
            _raw: &[u8],
            _context: &Self::Context,
            _origin: &str,
            _offset: u64,
        ) -> Result<(Vec<Value>, Self::Context)> {
            Ok((Vec::new(), ()))
        }
    }

    fn fixture_file() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agenturm-jsonl-gap-replay-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let file = root.join("source.jsonl");
        fs::write(&file, b"{\"row\":1}\n").unwrap();
        (root, file)
    }

    fn read(file: &Path, cursor: &str) -> Value {
        poll::<EmptyFormat>(file, "fixture-source", cursor, 16, max_frame_bytes()).unwrap()
    }

    fn set_mtime(file: &Path, seconds: u64) {
        OpenOptions::new()
            .write(true)
            .open(file)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }

    #[test]
    fn header_upgrade_rejects_an_in_place_rewrite_after_prefix_verification() {
        struct RewriteOnRestore;
        impl SourceFormat for RewriteOnRestore {
            type Context = (PathBuf, bool);
            const PLUGIN_ID: &'static str = "fixture.header-upgrade";
            const GAP_EVENT_PREFIX: &'static str = "fixture-gap";

            fn header_identity(path: &Path, _raw: &[u8]) -> Result<Self::Context> {
                Ok((path.to_owned(), false))
            }
            fn checkpoint(context: &Self::Context) -> String {
                if context.1 {
                    "upgraded".into()
                } else {
                    String::new()
                }
            }
            fn restore(context: &mut Self::Context, _checkpoint: &str) -> Result<()> {
                // This callback follows verification of the original durable
                // prefix and precedes the header-only discovery upgrade.
                fs::write(&context.0, b"{\"row\":2}\n")?;
                Ok(())
            }
            fn resume_verified_header(
                _raw: &[u8],
                context: &mut Self::Context,
                _origin: &str,
            ) -> Result<Option<Value>> {
                context.1 = true;
                Ok(Some(json!({
                    "event_id":"fixture:header", "revision":0, "time_ns":"0",
                    "identity":{"session_id":"fixture"},
                    "kind":"session_discovered", "payload":{}
                })))
            }
            fn translate_line(
                _raw: &[u8],
                context: &Self::Context,
                _origin: &str,
                _offset: u64,
            ) -> Result<(Vec<Value>, Self::Context)> {
                Ok((Vec::new(), context.clone()))
            }
        }
        let (root, file) = fixture_file();
        let initial = read(&file, "");
        let cursor = initial["cursor"].as_str().unwrap();
        let result =
            poll::<RewriteOnRestore>(&file, "fixture-source", cursor, 16, max_frame_bytes());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "source changed during poll"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rotated_source_gap_is_identical_after_mtime_changes() {
        let (root, file) = fixture_file();
        let initial = read(&file, "");
        let cursor = initial["cursor"].as_str().unwrap();
        let replacement = root.join("replacement.jsonl");
        fs::write(&replacement, b"{\"row\":2}\n").unwrap();
        fs::rename(&replacement, &file).unwrap();
        set_mtime(&file, 10);
        let first = read(&file, cursor);
        set_mtime(&file, 20);
        let replay = read(&file, cursor);
        assert_eq!(first, replay);
        assert_eq!(first["records"][0]["time_ns"], "0");
        assert_eq!(
            first["records"][0]["payload"]["code"],
            "possible_source_gap"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn in_place_header_rewrite_gap_is_identical_after_mtime_changes() {
        let (root, file) = fixture_file();
        let initial = read(&file, "");
        let cursor = initial["cursor"].as_str().unwrap();
        fs::write(&file, b"{\"row\":2}\n").unwrap();
        set_mtime(&file, 30);
        let first = read(&file, cursor);
        set_mtime(&file, 40);
        let replay = read(&file, cursor);
        assert_eq!(first, replay);
        assert_eq!(first["records"][0]["time_ns"], "0");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkpoint_and_cursor_limits_expand_without_widening_source_ids() {
        let checkpoint = "a".repeat(MAX_CHECKPOINT);
        assert!(checkpoint_identity(&checkpoint));
        assert!(!checkpoint_identity(&format!("{checkpoint}a")));
        let cursor = Cursor {
            version: 2,
            device: u64::MAX,
            inode: u64::MAX,
            offset: u64::MAX,
            digest: "0".repeat(64),
            header_digest: "1".repeat(64),
            generation: "2".repeat(64),
            checkpoint,
        }
        .encode();
        assert_eq!(cursor.len(), MAX_CURSOR);
        assert!(Cursor::parse(&cursor).unwrap().is_some());
        assert!(Cursor::parse(&format!("{cursor}x")).is_err());
    }
}
