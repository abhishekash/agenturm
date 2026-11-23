use super::{SpanJsonlFollowOptions, SpanJsonlImportOptions, span_jsonl_import};
use crate::trace_import::{MAX_EXPORT_BYTES, MAX_EXPORT_SPANS};
use agenturm_kernel::ObservationStore;
use agenturm_store::{CommitOutcome, ObservationBatch, Store};
use rustix::fs::{Mode, OFlags, open, openat};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const PREFIX: &str = "span-jsonl-follow/1:";
const MAX_PREFIX_BYTES: u64 = 128 * 1024 * 1024;
const READ_ERROR: &str = "Cannot read the watched span JSONL file";
const PATH_ERROR: &str = "Watched span JSONL requires one regular file at a path without symlinks";
const ISOLATION_ERROR: &str =
    "Watched span JSONL input must be separate from the history and its sidecars";
const LIMIT_ERROR: &str = "Watched span JSONL generation exceeds the 128 MiB verification limit";
const COMMIT_ERROR: &str =
    "Cannot commit the watched span JSONL batch. Check history and retained span identities.";

pub(super) fn execute(options: &SpanJsonlFollowOptions) -> Result<(), String> {
    let input = Input::bind(&options.import)?;
    input.open()?;
    let mut store = Store::open_collector(&options.import.db)
        .map_err(|_| "Cannot open watched span JSONL history")?;
    let prior = ObservationStore::source_cursor(&store, options.import.source_id.as_str())?;
    let mut follower = Follower::new(input, prior)?;
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = crate::WatchSignalRegistrations::new(stop.clone())?;
    let mut stdout = io::stdout().lock();
    while !stop.load(Ordering::Relaxed) {
        if let Some(progress) = follower.poll(&mut store, &options.import)? {
            let result = writeln!(
                stdout,
                "Committed {} watched span JSONL observations at byte {}{}.",
                progress.inserted,
                progress.offset,
                if progress.restarted {
                    " · source changed, possible gap"
                } else {
                    ""
                },
            )
            .and_then(|()| stdout.flush());
            match result {
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                Err(_) => return Err("Cannot write watched span JSONL progress".into()),
                Ok(()) if progress.backlog => continue,
                Ok(()) => {}
            }
        }
        let deadline = Instant::now() + options.interval;
        while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(
                Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Stamp {
    identity: (u64, u64),
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

impl From<&Metadata> for Stamp {
    fn from(metadata: &Metadata) -> Self {
        Self {
            identity: identity(metadata),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

struct Input {
    path: PathBuf,
    parent_identity: (u64, u64),
    protected: Vec<PathBuf>,
    fingerprint: String,
}

impl Input {
    fn bind(options: &SpanJsonlImportOptions) -> Result<Self, String> {
        let path = std::path::absolute(&options.file).map_err(|_| PATH_ERROR)?;
        let dir = open_directory(path.parent().ok_or(PATH_ERROR)?)?;
        let parent_identity = identity(&dir.metadata().map_err(|_| PATH_ERROR)?);
        let db = std::path::absolute(&options.db).map_err(|_| ISOLATION_ERROR)?;
        let db_name = db.file_name().ok_or(ISOLATION_ERROR)?;
        let db_parent = std::fs::canonicalize(db.parent().ok_or(ISOLATION_ERROR)?)
            .map_err(|_| ISOLATION_ERROR)?;
        let mut protected = vec![db_parent.join(db_name)];
        protected.extend(["-wal", "-shm", "-journal"].map(|suffix| {
            let mut member = db_name.to_os_string();
            member.push(suffix);
            db_parent.join(member)
        }));
        let mut marker = OsString::from(".");
        marker.push(db_name);
        marker.push(".agenturm-writer.lock");
        protected.push(db_parent.join(marker));
        if protected.contains(&path) || path.file_name().is_none() {
            return Err(ISOLATION_ERROR.into());
        }
        let config = serde_json::to_vec(&(
            parent_identity,
            options.source_id.as_str(),
            &options.session_id,
            &options.payload_binding,
            match options.payload_format {
                span_jsonl_import::PayloadFormat::Telemetry => "telemetry",
                span_jsonl_import::PayloadFormat::AgentHarness => "agent-harness",
            },
        ))
        .map_err(|_| PATH_ERROR)?;
        let mut fingerprint = Sha256::new();
        fingerprint.update(path.as_os_str().as_bytes());
        fingerprint.update([0]);
        fingerprint.update(config);
        Ok(Self {
            path,
            parent_identity,
            protected,
            fingerprint: format!("{:x}", fingerprint.finalize()),
        })
    }

    fn open(&self) -> Result<Option<(File, Stamp)>, String> {
        let parent = open_directory(self.path.parent().ok_or(PATH_ERROR)?)?;
        if identity(&parent.metadata().map_err(|_| PATH_ERROR)?) != self.parent_identity {
            return Err("Watched span JSONL parent directory changed".into());
        }
        let file = match openat(
            &parent,
            self.path.file_name().ok_or(PATH_ERROR)?,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(PATH_ERROR.into()),
        };
        let metadata = file.metadata().map_err(|_| READ_ERROR)?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(PATH_ERROR.into());
        }
        for protected in &self.protected {
            match std::fs::metadata(protected) {
                Ok(member) if identity(&member) == identity(&metadata) => {
                    return Err(ISOLATION_ERROR.into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(ISOLATION_ERROR.into()),
            }
        }
        use std::os::fd::AsRawFd;
        let pinned = format!("/proc/self/fd/{}", file.as_raw_fd());
        let opened = File::open(pinned).map_err(|_| READ_ERROR)?;
        let opened_metadata = opened.metadata().map_err(|_| READ_ERROR)?;
        if identity(&opened_metadata) != identity(&metadata) || opened_metadata.nlink() != 1 {
            return Err(PATH_ERROR.into());
        }
        Ok(Some((opened, Stamp::from(&opened_metadata))))
    }
}

fn open_directory(path: &Path) -> Result<File, String> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(PATH_ERROR.into());
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", flags, Mode::empty()).map_err(|_| PATH_ERROR)?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(PATH_ERROR.into());
        };
        directory = openat(directory, name, flags, Mode::empty()).map_err(|_| PATH_ERROR)?;
    }
    Ok(File::from(directory))
}

struct Checkpoint {
    identity: (u64, u64),
    offset: u64,
    digest: String,
    generation: String,
}

impl Checkpoint {
    fn parse(raw: &str, expected: &str) -> Option<Self> {
        let mut fields = raw.strip_prefix(PREFIX)?.split(':');
        if fields.next()? != expected {
            return None;
        }
        let identity = (fields.next()?.parse().ok()?, fields.next()?.parse().ok()?);
        let offset = fields.next()?.parse().ok()?;
        let digest = fields.next()?;
        let generation = fields.next()?;
        if fields.next().is_some()
            || !(1..=MAX_PREFIX_BYTES).contains(&offset)
            || !is_digest(digest)
            || !is_digest(generation)
        {
            return None;
        }
        Some(Self {
            identity,
            offset,
            digest: digest.to_owned(),
            generation: generation.to_owned(),
        })
    }

    fn encode(&self, fingerprint: &str) -> String {
        let (device, inode) = self.identity;
        format!(
            "{PREFIX}{fingerprint}:{device}:{inode}:{}:{}:{}",
            self.offset, self.digest, self.generation
        )
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

struct Progress {
    inserted: usize,
    offset: u64,
    restarted: bool,
    backlog: bool,
}

struct Follower {
    input: Input,
    prior: Option<String>,
    checkpoint: Option<Checkpoint>,
    idle: Option<Stamp>,
}

impl Follower {
    fn new(input: Input, prior: Option<String>) -> Result<Self, String> {
        let checkpoint = prior.as_deref().map(|raw| Checkpoint::parse(raw, &input.fingerprint)
            .ok_or("OTLP source identifier belongs to a different input, configuration, or collector"))
            .transpose()?;
        Ok(Self {
            input,
            prior,
            checkpoint,
            idle: None,
        })
    }

    fn poll(
        &mut self,
        store: &mut Store,
        options: &SpanJsonlImportOptions,
    ) -> Result<Option<Progress>, String> {
        let Some((mut file, stamp)) = self.input.open()? else {
            self.idle = None;
            return Ok(None);
        };
        if self.idle == Some(stamp) {
            return Ok(None);
        }
        let mut offset = 0;
        let mut prefix = Sha256::new();
        let mut generation = None;
        if let Some(prior) = &self.checkpoint
            && prior.identity == stamp.identity
            && prior.offset <= stamp.length
        {
            let Some(previous) = hash_prefix(&mut file, prior.offset)? else {
                return Ok(None);
            };
            if format!("{:x}", previous.clone().finalize()) == prior.digest {
                offset = prior.offset;
                prefix = previous;
                generation = Some(prior.generation.clone());
            }
        }
        let restarted = offset == 0 && self.checkpoint.is_some();
        let remaining = stamp.length - offset;
        let budget = remaining.min(MAX_EXPORT_BYTES + 1);
        file.seek(SeekFrom::Start(offset)).map_err(|_| READ_ERROR)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(budget)
            .read_to_end(&mut bytes)
            .map_err(|_| READ_ERROR)?;
        let mut end = complete_batch_length(&bytes)?;
        if end == 0 {
            self.idle = Some(stamp);
            return Ok(None);
        }
        let verified_end = offset + end as u64;
        if verified_end > MAX_PREFIX_BYTES {
            return Err(LIMIT_ERROR.into());
        }
        // Re-read the complete consumed prefix through the same opened inode.
        // Appends past this prefix do not invalidate the batch. A concurrent
        // rewrite must be noticed before decoding or advancing the checkpoint.
        let verified = prefix.clone().chain_update(&bytes[..end]).finalize();
        if !hash_prefix(&mut file, verified_end)?.is_some_and(|hash| hash.finalize() == verified) {
            return Ok(None);
        }
        let prepared = loop {
            match span_jsonl_import::decode_bytes(&bytes[..end], options) {
                Ok(prepared) => break prepared,
                Err(error) if error == span_jsonl_import::PAYLOAD_BUDGET_ERROR => {
                    // A following batch can retain the remainder. Preserve the
                    // normal one-shot limit without reducing the line limit.
                    let candidates = &bytes[..end - 1];
                    let middle = candidates.len() / 2;
                    end = candidates[..middle]
                        .iter()
                        .rposition(|&byte| byte == b'\n')
                        .or_else(|| {
                            candidates[middle..]
                                .iter()
                                .position(|&byte| byte == b'\n')
                                .map(|index| middle + index)
                        })
                        .map(|index| index + 1)
                        .ok_or(error)?;
                }
                Err(error) => {
                    return Err(format!("Watched span JSONL after byte {offset}: {error}"));
                }
            }
        };
        let next_offset = offset + end as u64;
        let digest = format!("{:x}", prefix.chain_update(&bytes[..end]).finalize());
        // Chaining a new generation to the prior durable cursor prevents a
        // repeated rewrite/rotation from matching an older transition receipt.
        let generation = generation.unwrap_or_else(|| {
            let (device, inode) = stamp.identity;
            let context = format!(
                "{}:{device}:{inode}:{}",
                self.input.fingerprint,
                self.prior.as_deref().unwrap_or_default()
            );
            format!("{:x}", Sha256::digest(context.as_bytes()))
        });
        let checkpoint = Checkpoint {
            identity: stamp.identity,
            offset: next_offset,
            digest,
            generation,
        };
        let next = checkpoint.encode(&self.input.fingerprint);
        store
            .apply_current_automatic_retention()
            .map_err(|_| COMMIT_ERROR)?;
        let inserted = match (&options.payload_binding, prepared.payloads.is_empty()) {
            (Some(binding), false) => {
                let batch = ObservationBatch::from_observations(
                    options.source_id.as_str(),
                    self.prior.as_deref(),
                    &next,
                    &prepared.observations,
                )
                .map_err(|_| COMMIT_ERROR)?;
                match store
                    .commit_telemetry_batch(&batch, binding, &prepared.payloads)
                    .map_err(|_| COMMIT_ERROR)?
                {
                    CommitOutcome::Replay => 0,
                    CommitOutcome::Committed { inserted } => inserted,
                }
            }
            _ => {
                ObservationStore::commit(
                    store,
                    options.source_id.as_str(),
                    self.prior.as_deref(),
                    &next,
                    &prepared.observations,
                )
                .map_err(|_| COMMIT_ERROR)?
                .inserted
            }
        };
        self.idle = None;
        self.checkpoint = Some(checkpoint);
        self.prior = Some(next);
        Ok(Some(Progress {
            inserted,
            offset: next_offset,
            restarted,
            backlog: next_offset < stamp.length,
        }))
    }
}

fn complete_batch_length(bytes: &[u8]) -> Result<usize, String> {
    let max = MAX_EXPORT_BYTES as usize;
    let mut end = 0;
    let mut rows = 0;
    for line in bytes[..bytes.len().min(max)].split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        rows += usize::from(!line.iter().all(u8::is_ascii_whitespace));
        end += line.len();
        if rows == MAX_EXPORT_SPANS {
            break;
        }
    }
    if end == 0 && bytes.len() > max {
        return Err("Watched span JSONL record exceeds 4 MiB".into());
    }
    Ok(end)
}

fn hash_prefix(file: &mut File, mut remaining: u64) -> Result<Option<Sha256>, String> {
    file.seek(SeekFrom::Start(0)).map_err(|_| READ_ERROR)?;
    let mut hash = Sha256::new();
    let mut read = [0; 64 * 1024];
    while remaining != 0 {
        let count = remaining.min(read.len() as u64) as usize;
        match file.read_exact(&mut read[..count]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(_) => return Err(READ_ERROR.into()),
        }
        hash.update(&read[..count]);
        remaining -= count as u64;
    }
    Ok(Some(hash))
}
