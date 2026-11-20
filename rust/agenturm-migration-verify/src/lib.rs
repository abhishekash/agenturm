//! Offline verification of three detached SQLite backups. This crate has no
//! production caller and never creates a target database or changes authority.

#![forbid(unsafe_code)]

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_BACKUP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SOURCES: usize = 4096;
const MAX_RECEIPTS: i64 = 1_000_000;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub receiver_id: String,
    pub telemetry: Backup,
    pub workflow: Backup,
    pub tasks: Backup,
    pub snapshot: Snapshot,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    pub path: String,
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub sha256: String,
    pub schema_version: u32,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub sources: Vec<SourceCursor>,
    pub workflow_notification_claims: i64,
    pub workflow_native_events: i64,
    pub saved_tasks: i64,
    pub max_task_revision: i64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceCursor {
    pub source_id: String,
    pub cursor: String,
    pub lease_epoch: i64,
    pub policy_epoch: i64,
    pub dirty_seq: i64,
    pub last_batch_digest: String,
    pub content_receipt: Option<ReplayReceipt>,
    pub command_receipt: Option<ReplayReceipt>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReplayReceipt {
    pub cursor: String,
    pub refs_digest: String,
}

fn bounded_text(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn verify_snapshot_claims(snapshot: &Snapshot) -> Result<()> {
    if snapshot.sources.len() > MAX_SOURCES
        || [
            snapshot.workflow_notification_claims,
            snapshot.workflow_native_events,
            snapshot.saved_tasks,
            snapshot.max_task_revision,
        ]
        .into_iter()
        .any(|n| !(0..=MAX_RECEIPTS).contains(&n))
    {
        return Err(invalid("snapshot claims exceed bounds"));
    }
    let mut last = "";
    for source in &snapshot.sources {
        if source.source_id.as_str() <= last
            || !bounded_text(&source.source_id, 256)
            || source.source_id.is_empty()
            || !bounded_text(&source.cursor, 4096)
            || source.lease_epoch <= 0
            || source.policy_epoch <= 0
            || source.dirty_seq < 0
            || !(source.last_batch_digest.is_empty() || valid_digest(&source.last_batch_digest))
        {
            return Err(invalid("invalid source cursor claim"));
        }
        for receipt in [&source.content_receipt, &source.command_receipt]
            .into_iter()
            .flatten()
        {
            if !bounded_text(&receipt.cursor, 4096) || !valid_digest(&receipt.refs_digest) {
                return Err(invalid("invalid replay receipt claim"));
            }
        }
        last = &source.source_id;
    }
    Ok(())
}

#[cfg(unix)]
fn check_file(backup: &Backup) -> Result<File> {
    let path = Path::new(&backup.path);
    if backup.path.len() > 4096
        || !path.is_absolute()
        || backup.size_bytes == 0
        || backup.size_bytes > MAX_BACKUP_BYTES
        || !valid_digest(&backup.sha256)
    {
        return Err(invalid("invalid backup descriptor"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("backup has no parent"))?;
    let dir = parent.symlink_metadata()?;
    if !dir.is_dir() || dir.mode() & 0o077 != 0 || parent.canonicalize()? != parent {
        return Err(invalid("backup parent must be a private real directory"));
    }
    let named = path.symlink_metadata()?;
    if !named.is_file()
        || named.mode() & 0o077 != 0
        || named.nlink() != 1
        || named.uid() != dir.uid()
        || named.dev() != backup.device
        || named.ino() != backup.inode
        || named.len() != backup.size_bytes
    {
        return Err(invalid("backup identity or private file check failed"));
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        if Path::new(&format!("{}{suffix}", backup.path))
            .symlink_metadata()
            .is_ok()
        {
            return Err(invalid("detached backup has SQLite sidecar"));
        }
    }
    let file = File::open(path)?;
    let opened = file.metadata()?;
    if opened.dev() != named.dev() || opened.ino() != named.ino() || opened.len() != named.len() {
        return Err(invalid("backup path changed while opening"));
    }
    Ok(file)
}

#[cfg(unix)]
fn check_digest(backup: &Backup, file: &mut File) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| invalid("backup size overflow"))?;
        if total > MAX_BACKUP_BYTES {
            return Err(invalid("backup exceeds byte limit"));
        }
        hasher.update(&buffer[..read]);
    }
    if total != backup.size_bytes || format!("{:x}", hasher.finalize()) != backup.sha256 {
        return Err(invalid("backup digest mismatch"));
    }
    Ok(())
}

#[cfg(unix)]
fn open_backup(backup: &Backup) -> Result<Connection> {
    let mut file = check_file(backup)?;
    check_digest(backup, &mut file)?;
    // Bind SQLite to the already checked descriptor on Linux, preventing a
    // pathname replacement between digest verification and SQLite open.
    #[cfg(target_os = "linux")]
    let uri = format!("file:/proc/self/fd/{}?immutable=1", file.as_raw_fd());
    #[cfg(not(target_os = "linux"))]
    let uri = immutable_uri(&backup.path);
    let db = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
    let check: String = db.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(invalid("backup quick_check failed"));
    }
    let named = Path::new(&backup.path).symlink_metadata()?;
    let opened = file.metadata()?;
    if named.dev() != backup.device
        || named.ino() != backup.inode
        || named.len() != backup.size_bytes
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
    {
        return Err(invalid("backup path changed during inspection"));
    }
    Ok(db)
}

#[cfg(all(unix, not(target_os = "linux")))]
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

#[cfg(unix)]
fn read_telemetry(db: &Connection, receiver: &str) -> Result<Vec<SourceCursor>> {
    let actual: String = db.query_row(
        "SELECT value FROM schema_meta WHERE key='receiver_id'",
        [],
        |r| r.get(0),
    )?;
    let schema: String = db.query_row(
        "SELECT value FROM schema_meta WHERE key='plugin_sessions_read_schema'",
        [],
        |r| r.get(0),
    )?;
    if actual != receiver || schema != "1" {
        return Err(invalid("telemetry receiver or schema mismatch"));
    }
    let mut query = db.prepare(
        "SELECT s.source_id,s.cursor,s.lease_epoch,s.policy_epoch,s.dirty_seq,s.last_batch_digest,\
        c.cursor,c.refs_digest,k.cursor,k.refs_digest FROM plugin_source_cursor_v1 s \
        LEFT JOIN plugin_content_batch_v1 c USING(source_id) \
        LEFT JOIN plugin_command_batch_v1 k USING(source_id) ORDER BY s.source_id LIMIT 4097",
    )?;
    let rows = query.query_map([], |r| {
        let content_cursor: Option<String> = r.get(6)?;
        let content_digest: Option<String> = r.get(7)?;
        let command_cursor: Option<String> = r.get(8)?;
        let command_digest: Option<String> = r.get(9)?;
        Ok(SourceCursor {
            source_id: r.get(0)?,
            cursor: r.get(1)?,
            lease_epoch: r.get(2)?,
            policy_epoch: r.get(3)?,
            dirty_seq: r.get(4)?,
            last_batch_digest: r.get(5)?,
            content_receipt: content_cursor
                .zip(content_digest)
                .map(|(cursor, refs_digest)| ReplayReceipt {
                    cursor,
                    refs_digest,
                }),
            command_receipt: command_cursor
                .zip(command_digest)
                .map(|(cursor, refs_digest)| ReplayReceipt {
                    cursor,
                    refs_digest,
                }),
        })
    })?;
    let sources: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
    if sources.len() > MAX_SOURCES {
        return Err(invalid("too many source cursors"));
    }
    Ok(sources)
}

#[cfg(unix)]
fn bounded_count(db: &Connection, sql: &str) -> Result<i64> {
    let count: i64 = db.query_row(sql, [], |r| r.get(0))?;
    if !(0..=MAX_RECEIPTS).contains(&count) {
        return Err(invalid("receipt count exceeds limit"));
    }
    Ok(count)
}

#[cfg(unix)]
fn read_workflow(db: &Connection) -> Result<(i64, i64)> {
    let version: String = db.query_row(
        "SELECT value FROM workflow_meta WHERE key='schema_version'",
        [],
        |r| r.get(0),
    )?;
    if version != "2" {
        return Err(invalid("unsupported workflow schema"));
    }
    Ok((
        bounded_count(db, "SELECT COUNT(*) FROM workflow_notification_claims_v1")?,
        bounded_count(db, "SELECT COUNT(*) FROM workflow_native_events_v1")?,
    ))
}

#[cfg(unix)]
fn read_tasks(db: &Connection) -> Result<(i64, i64)> {
    let version: i64 = db.query_row("SELECT version FROM task_schema_v1", [], |r| r.get(0))?;
    if version != 1 {
        return Err(invalid("unsupported task schema"));
    }
    let count = bounded_count(db, "SELECT COUNT(*) FROM saved_tasks_v1")?;
    let max: i64 = db.query_row(
        "SELECT COALESCE(MAX(revision),0) FROM saved_tasks_v1",
        [],
        |r| r.get(0),
    )?;
    if !(0..=MAX_RECEIPTS).contains(&max) {
        return Err(invalid("task revision exceeds limit"));
    }
    Ok((count, max))
}

/// Verify a trusted candidate manifest and its detached backups. This
/// historical result does not exclude an old writer or authorize cutover.
pub fn verify_json(input: &str) -> Result<String> {
    if input.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("manifest exceeds byte limit"));
    }
    let manifest: Manifest = serde_json::from_str(input)?;
    if manifest.format != "migration_manifest_v1"
        || manifest.receiver_id.is_empty()
        || !bounded_text(&manifest.receiver_id, 256)
        || manifest.telemetry.schema_version != 1
        || manifest.workflow.schema_version != 2
        || manifest.tasks.schema_version != 1
        || manifest.telemetry.path == manifest.workflow.path
        || manifest.telemetry.path == manifest.tasks.path
        || manifest.workflow.path == manifest.tasks.path
    {
        return Err(invalid("unsupported migration manifest"));
    }
    verify_snapshot_claims(&manifest.snapshot)?;
    #[cfg(unix)]
    {
        let telemetry = open_backup(&manifest.telemetry)?;
        let workflow = open_backup(&manifest.workflow)?;
        let tasks = open_backup(&manifest.tasks)?;
        let sources = read_telemetry(&telemetry, &manifest.receiver_id)?;
        let (claims, events) = read_workflow(&workflow)?;
        let (saved, revision) = read_tasks(&tasks)?;
        let actual = Snapshot {
            sources,
            workflow_notification_claims: claims,
            workflow_native_events: events,
            saved_tasks: saved,
            max_task_revision: revision,
        };
        verify_snapshot_claims(&actual)?;
        if actual != manifest.snapshot {
            return Err(invalid("migration snapshot claim mismatch"));
        }
        for (db, backup) in [
            (&telemetry, &manifest.telemetry),
            (&workflow, &manifest.workflow),
            (&tasks, &manifest.tasks),
        ] {
            db.execute_batch("ROLLBACK;")?;
            let mut file = check_file(backup)?;
            check_digest(backup, &mut file)?;
        }
        Ok(manifest.receiver_id)
    }
    #[cfg(not(unix))]
    {
        Err(invalid(
            "migration verifier requires Unix file identity checks",
        ))
    }
}

pub fn verify_file(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(invalid("manifest must be a bounded regular file"));
    }
    let input = fs::read_to_string(path)?;
    verify_json(&input)
}
