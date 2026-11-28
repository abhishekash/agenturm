//! Shared bounded process runtime for one explicitly configured JSONL source.
//!
//! Format-specific translation is supplied by a `SourceFormat` implementation.
//! The host owns admission, cursor commit and capture policy.

#![forbid(unsafe_code)]

use agenturm_protocol::MAX_FRAME_BYTES;
use agenturm_source_process::Poller;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
const MAX_VERIFIED_PREFIX_CACHE_ENTRIES: usize = 128;

// The cursor is still verified from disk on a cache miss. Cached SHA state is
// reused only for the exact adapter/path/cursor and file stamp; a changed stamp
// forces the existing full-prefix proof before the source can advance.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct VerifiedPrefixKey {
    adapter: &'static str,
    path: PathBuf,
    cursor: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileStamp {
    device: u64,
    inode: u64,
    len: u64,
    modified_sec: i64,
    modified_nsec: i64,
    changed_sec: i64,
    changed_nsec: i64,
}

#[derive(Clone)]
struct VerifiedPrefix {
    stamp: FileStamp,
    hasher: Sha256,
}

static VERIFIED_PREFIX_CACHE: OnceLock<Mutex<HashMap<VerifiedPrefixKey, VerifiedPrefix>>> =
    OnceLock::new();

#[cfg(test)]
thread_local! {
    static PREFIX_HASH_BYTES: Cell<u64> = const { Cell::new(0) };
    static USE_PREFIX_CACHE: Cell<bool> = const { Cell::new(true) };
}

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

fn file_stamp(metadata: &Metadata) -> FileStamp {
    use std::os::unix::fs::MetadataExt;
    FileStamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        len: metadata.len(),
        modified_sec: metadata.mtime(),
        modified_nsec: metadata.mtime_nsec(),
        changed_sec: metadata.ctime(),
        changed_nsec: metadata.ctime_nsec(),
    }
}

fn cached_prefix_hasher(
    adapter: &'static str,
    path: &Path,
    cursor: &str,
    stamp: FileStamp,
) -> Option<Sha256> {
    #[cfg(test)]
    if !USE_PREFIX_CACHE.with(Cell::get) {
        return None;
    }
    let cache = VERIFIED_PREFIX_CACHE.get()?;
    cache.lock().ok().and_then(|cache| {
        cache
            .get(&VerifiedPrefixKey {
                adapter,
                path: path.to_owned(),
                cursor: cursor.to_owned(),
            })
            .filter(|prefix| prefix.stamp == stamp)
            .map(|prefix| prefix.hasher.clone())
    })
}

fn remember_verified_prefix(
    adapter: &'static str,
    path: &Path,
    cursor: &str,
    stamp: FileStamp,
    hasher: Sha256,
) {
    let cache = VERIFIED_PREFIX_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    let key = VerifiedPrefixKey {
        adapter,
        path: path.to_owned(),
        cursor: cursor.to_owned(),
    };
    cache.retain(|existing, _| {
        existing.path.as_path() != path || existing.adapter != adapter || existing == &key
    });
    if cache.len() >= MAX_VERIFIED_PREFIX_CACHE_ENTRIES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, VerifiedPrefix { stamp, hasher });
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
        #[cfg(test)]
        PREFIX_HASH_BYTES.with(|bytes| bytes.set(bytes.get().saturating_add(count as u64)));
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
    let initial_stamp = file_stamp(&metadata);
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
    let mut cached_prefix = previous.as_ref().and_then(|prior| {
        (prior.version == 2
            && (prior.device, prior.inode) == (device, inode)
            && prior.offset <= metadata.len()
            && prior.offset <= MAX_VERIFIED_PREFIX
            && prior.header_digest == header_digest)
            .then(|| {
                cached_prefix_hasher(std::any::type_name::<F>(), path, raw_cursor, initial_stamp)
            })
            .flatten()
    });
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
    let cached_state_used = !changed && cached_prefix.is_some();
    let mut verified_prefix = (!changed).then(|| cached_prefix.take()).flatten();
    if !changed
        && verified_prefix.is_none()
        && let Some(prior) = &previous
    {
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
        // Parse the exact header used to establish identity and generation.
        // A second read here could silently substitute changed header bytes;
        // cache hits also compare the file stamp again before cursor commit.
        let line = if position == 0 {
            Some(header.clone())
        } else {
            read_line(&mut file, position).map_err(classify_read)?
        };
        let Some(line) = line else {
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
    let final_stamp = file_stamp(
        &file
            .metadata()
            .map_err(|error| classify_read(Box::new(error)))?,
    );
    if (!cached_state_used || initial_stamp != final_stamp)
        && prefix_hasher(&mut file, position)
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
        digest: format!("{:x}", prefix.clone().finalize()),
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
    if initial_stamp == final_stamp {
        remember_verified_prefix(
            std::any::type_name::<F>(),
            path,
            &next,
            final_stamp,
            prefix.clone(),
        );
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
    use std::io::Write;
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

    fn prefix_hash_bytes() -> u64 {
        PREFIX_HASH_BYTES.with(Cell::get)
    }

    fn fixed_size_jsonl(file_bytes: usize, line_bytes: usize) -> Vec<u8> {
        let prefix = b"{\"row\":1,\"padding\":\"";
        let suffix = b"\"}\n";
        assert_eq!(file_bytes % line_bytes, 0);
        let padding = vec![b'x'; line_bytes - prefix.len() - suffix.len()];
        let mut contents = Vec::with_capacity(file_bytes);
        while contents.len() < file_bytes {
            contents.extend_from_slice(prefix);
            contents.extend_from_slice(&padding);
            contents.extend_from_slice(suffix);
        }
        contents
    }

    #[test]
    fn unchanged_file_reuses_its_verified_cursor_prefix() {
        let (root, file) = fixture_file();
        OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"{\"row\":2}\n")
            .unwrap();
        let initial = read(&file, "");
        let cursor = initial["cursor"].as_str().unwrap().to_owned();

        PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
        let idle = read(&file, &cursor);

        assert_eq!(idle["cursor"], cursor);
        assert!(idle["records"].as_array().unwrap().is_empty());
        assert_eq!(prefix_hash_bytes(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_append_invalidates_the_cached_prefix_verification() {
        let (root, file) = fixture_file();
        let initial = read(&file, "");
        let cursor = initial["cursor"].as_str().unwrap().to_owned();
        OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"{\"row\":2}\n")
            .unwrap();

        PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
        let appended = read(&file, &cursor);

        assert_ne!(appended["cursor"], cursor);
        assert_eq!(
            cursor_proof(appended["cursor"].as_str().unwrap())
                .unwrap()
                .unwrap()
                .offset,
            20
        );
        assert!(prefix_hash_bytes() > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_cursor_with_complete_unread_rows_continues_scanning_incrementally() {
        let (root, file) = fixture_file();
        fs::write(&file, fixed_size_jsonl(2 * MAX_SCAN as usize, 4 * 1024)).unwrap();
        let first = read(&file, "");
        let first_cursor = first["cursor"].as_str().unwrap().to_owned();
        assert_eq!(
            cursor_proof(&first_cursor).unwrap().unwrap().offset,
            MAX_SCAN
        );

        PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
        let second = read(&file, &first_cursor);

        assert_eq!(
            cursor_proof(second["cursor"].as_str().unwrap())
                .unwrap()
                .unwrap()
                .offset,
            2 * MAX_SCAN
        );
        assert_eq!(prefix_hash_bytes(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_incomplete_tail_does_not_rehash_committed_prefix() {
        let (root, file) = fixture_file();
        let initial = read(&file, "");
        let previous = initial["cursor"].as_str().unwrap().to_owned();
        OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"{\"row\":2}\n{\"row\":3")
            .unwrap();
        let first_idle = read(&file, &previous);
        let cursor = first_idle["cursor"].as_str().unwrap().to_owned();
        assert_ne!(cursor, previous);
        assert_eq!(cursor_proof(&cursor).unwrap().unwrap().offset, 20);

        PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
        let repeated_idle = read(&file, &cursor);

        assert_eq!(repeated_idle["cursor"], cursor);
        assert!(repeated_idle["records"].as_array().unwrap().is_empty());
        assert_eq!(prefix_hash_bytes(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_size_prefix_rewrite_with_restored_mtime_is_detected_by_ctime() {
        let (root, file) = fixture_file();
        OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"{\"row\":2}\n")
            .unwrap();
        let initial = read(&file, "");
        let cursor = initial["cursor"].as_str().unwrap().to_owned();
        let before = fs::metadata(&file).unwrap();
        let original_mtime = before.modified().unwrap();
        let before_stamp = file_stamp(&before);

        fs::write(&file, b"{\"row\":1}\n{\"row\":3}\n").unwrap();
        OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(original_mtime))
            .unwrap();
        let after_stamp = file_stamp(&fs::metadata(&file).unwrap());
        assert_eq!(
            (before_stamp.modified_sec, before_stamp.modified_nsec),
            (after_stamp.modified_sec, after_stamp.modified_nsec)
        );
        assert_ne!(
            (before_stamp.changed_sec, before_stamp.changed_nsec),
            (after_stamp.changed_sec, after_stamp.changed_nsec)
        );

        PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
        let rewritten = read(&file, &cursor);

        assert_eq!(
            rewritten["records"][0]["payload"]["code"],
            "possible_source_gap"
        );
        assert!(prefix_hash_bytes() > 0);
        fs::remove_dir_all(root).unwrap();
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
        let initial =
            poll::<RewriteOnRestore>(&file, "fixture-source", "", 16, max_frame_bytes()).unwrap();
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
    fn initial_or_replaced_header_cannot_change_between_identity_and_translation() {
        struct RewriteHeader;
        impl SourceFormat for RewriteHeader {
            type Context = ();
            const PLUGIN_ID: &'static str = "fixture.header-read";
            const GAP_EVENT_PREFIX: &'static str = "fixture-gap";
            fn header_identity(path: &Path, _raw: &[u8]) -> Result<()> {
                fs::write(path, b"{\"row\":2}\n")?;
                Ok(())
            }
            fn checkpoint(context: &()) -> String {
                EmptyFormat::checkpoint(context)
            }
            fn restore(context: &mut (), checkpoint: &str) -> Result<()> {
                EmptyFormat::restore(context, checkpoint)
            }
            fn translate_line(
                raw: &[u8],
                context: &(),
                origin: &str,
                offset: u64,
            ) -> Result<(Vec<Value>, ())> {
                EmptyFormat::translate_line(raw, context, origin, offset)
            }
        }
        for replaced in [false, true] {
            let (root, file) = fixture_file();
            let initial = read(&file, "");
            let prior = if replaced {
                fs::write(&file, b"{\"row\":3}\n").unwrap();
                initial["cursor"].as_str().unwrap()
            } else {
                ""
            };
            let result =
                poll::<RewriteHeader>(&file, "fixture-source", prior, 16, max_frame_bytes());
            assert_eq!(
                result.unwrap_err().to_string(),
                "source changed during poll"
            );
            fs::remove_dir_all(root).unwrap();
        }
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

    #[test]
    #[ignore = "synthetic warm JSONL polling benchmark; run with --ignored --nocapture"]
    fn benchmark_idle_poll_large_committed_prefix() {
        use std::time::Instant;

        const FILE_BYTES: usize = 16 * 1024 * 1024;
        const SAMPLES: usize = 12;
        const LINE_BYTES: usize = 4 * 1024;
        let (root, file) = fixture_file();
        let contents = fixed_size_jsonl(FILE_BYTES, LINE_BYTES);
        let expected_offset = contents.len() as u64;
        fs::write(&file, contents).unwrap();

        let median_micros = |durations: &[std::time::Duration]| {
            ((durations[SAMPLES / 2 - 1] + durations[SAMPLES / 2]) / 2).as_micros()
        };
        let benchmark_idle = |use_cache: bool| {
            USE_PREFIX_CACHE.with(|enabled| enabled.set(use_cache));
            let mut cursor = String::new();
            for _ in 0..=(FILE_BYTES / MAX_SCAN as usize) {
                cursor = read(&file, &cursor)["cursor"].as_str().unwrap().to_owned();
                if cursor_proof(&cursor).unwrap().unwrap().offset >= expected_offset {
                    break;
                }
            }
            assert_eq!(
                cursor_proof(&cursor).unwrap().unwrap().offset,
                expected_offset
            );
            let mut durations = Vec::with_capacity(SAMPLES);
            let mut hashed_bytes = 0_u64;
            for _ in 0..SAMPLES {
                PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
                let started = Instant::now();
                let response = read(&file, &cursor);
                durations.push(started.elapsed());
                hashed_bytes += prefix_hash_bytes();
                assert_eq!(response["cursor"], cursor);
                assert!(response["records"].as_array().unwrap().is_empty());
            }
            USE_PREFIX_CACHE.with(|enabled| enabled.set(true));
            durations.sort_unstable();
            (durations, hashed_bytes)
        };
        let (idle_baseline, idle_baseline_bytes) = benchmark_idle(false);
        let (idle_optimized, idle_optimized_bytes) = benchmark_idle(true);
        eprintln!(
            "idle benchmark: file_bytes={} samples={} baseline_median_us={} baseline_min_us={} baseline_max_us={} optimized_median_us={} optimized_min_us={} optimized_max_us={} baseline_prefix_hash_bytes={} optimized_prefix_hash_bytes={}",
            FILE_BYTES,
            SAMPLES,
            median_micros(&idle_baseline),
            idle_baseline[0].as_micros(),
            idle_baseline[SAMPLES - 1].as_micros(),
            median_micros(&idle_optimized),
            idle_optimized[0].as_micros(),
            idle_optimized[SAMPLES - 1].as_micros(),
            idle_baseline_bytes,
            idle_optimized_bytes,
        );

        let benchmark_catch_up = |use_cache: bool| {
            USE_PREFIX_CACHE.with(|enabled| enabled.set(use_cache));
            let mut cursor = read(&file, "")["cursor"].as_str().unwrap().to_owned();
            assert_eq!(cursor_proof(&cursor).unwrap().unwrap().offset, MAX_SCAN);
            let mut durations = Vec::with_capacity(SAMPLES);
            let mut hashed_bytes = 0_u64;
            for _ in 0..SAMPLES {
                PREFIX_HASH_BYTES.with(|bytes| bytes.set(0));
                let started = Instant::now();
                cursor = read(&file, &cursor)["cursor"].as_str().unwrap().to_owned();
                durations.push(started.elapsed());
                hashed_bytes += prefix_hash_bytes();
            }
            USE_PREFIX_CACHE.with(|enabled| enabled.set(true));
            durations.sort_unstable();
            (durations, hashed_bytes)
        };
        let (baseline, baseline_hashed_bytes) = benchmark_catch_up(false);
        let (optimized, optimized_hashed_bytes) = benchmark_catch_up(true);
        eprintln!(
            "catch-up benchmark: file_bytes={} samples={} baseline_median_us={} baseline_min_us={} baseline_max_us={} optimized_median_us={} optimized_min_us={} optimized_max_us={} baseline_prefix_hash_bytes={} optimized_prefix_hash_bytes={}",
            FILE_BYTES,
            SAMPLES,
            median_micros(&baseline),
            baseline[0].as_micros(),
            baseline[SAMPLES - 1].as_micros(),
            median_micros(&optimized),
            optimized[0].as_micros(),
            optimized[SAMPLES - 1].as_micros(),
            baseline_hashed_bytes,
            optimized_hashed_bytes,
        );
        fs::remove_dir_all(root).unwrap();
    }
}
