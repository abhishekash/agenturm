//! Online SQLite backup of the Rust-owned store.

mod bundle;

use crate::{Store, StoreError};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use std::error::Error;
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const BACKUP_BUSY_WAIT: Duration = Duration::from_secs(5);
const BACKUP_RETRY_DELAY: Duration = Duration::from_millis(10);
const PAGES_PER_STEP: i32 = 128;

#[derive(Debug)]
pub enum BackupError {
    InvalidDestination,
    DestinationExists,
    Busy,
    InvalidSnapshot,
    InvalidBundle,
    Sidecar(String),
    PublishedDirectorySync(io::Error),
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Store(StoreError),
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDestination => f.write_str("backup destination needs a filename"),
            Self::DestinationExists => f.write_str("backup destination or a SQLite sidecar exists"),
            Self::Busy => f.write_str("SQLite backup remained busy"),
            Self::InvalidSnapshot => f.write_str("backup snapshot failed its integrity check"),
            Self::InvalidBundle => {
                f.write_str("directory is not a valid Agenturm workspace backup")
            }
            Self::Sidecar(error) => write!(f, "workspace backup sidecar: {error}"),
            Self::PublishedDirectorySync(error) => {
                write!(
                    f,
                    "backup was published but its directory could not be synced: {error}"
                )
            }
            Self::Io(error) => error.fmt(f),
            Self::Sqlite(error) => error.fmt(f),
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl Error for BackupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) | Self::PublishedDirectorySync(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for BackupError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for BackupError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<StoreError> for BackupError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl Store {
    /// Verifies SQLite consistency and foreign-key links without exposing
    /// individual database rows in its result.
    pub fn check_history_integrity(&self) -> crate::Result<()> {
        if self.history_integrity_passes()? {
            Ok(())
        } else {
            Err(StoreError::Invalid("history integrity check failed"))
        }
    }

    fn history_integrity_passes(&self) -> rusqlite::Result<bool> {
        let integrity: String = self
            .connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        let foreign_key_violation = self
            .connection
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some();
        Ok(integrity == "ok" && !foreign_key_violation)
    }

    /// Takes a consistent snapshot of committed data, including WAL frames.
    /// The verified result is a standalone SQLite file. A fresh path in an
    /// existing, user-controlled directory is required. The file can be opened
    /// directly as a Rust store, including for subsequent collection.
    /// Private content approvals require fresh authorization after restore.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), BackupError> {
        let destination = destination.as_ref();
        let filename = destination
            .file_name()
            .ok_or(BackupError::InvalidDestination)?;
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        let destination = parent.join(filename);
        ensure_destination_is_fresh(&destination)?;

        let staging = StagingDirectory::new(&parent)?;
        let snapshot_path = staging.0.join("snapshot.sqlite");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let snapshot_file = options.open(&snapshot_path)?;
        let mut snapshot =
            Connection::open_with_flags(&snapshot_path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        copy_committed_snapshot(&self.connection, &mut snapshot)?;
        let staged = snapshot.transaction()?;
        crate::content_grant::disable_backup_grants(&staged)?;
        crate::retention::renew_read_incarnation(&staged)?;
        staged.commit()?;
        let journal_mode: String =
            snapshot.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("delete") {
            return Err(BackupError::InvalidSnapshot);
        }
        snapshot.close().map_err(|(_, error)| error)?;

        let restored = Self::open_read_only(&snapshot_path)?;
        if !restored.history_integrity_passes()? {
            return Err(BackupError::InvalidSnapshot);
        }
        drop(restored);
        snapshot_file.sync_all()?;
        drop(snapshot_file);

        ensure_destination_is_fresh(&destination)?;
        match publish_snapshot(&snapshot_path, &destination) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(BackupError::DestinationExists);
            }
            Err(error) => return Err(error.into()),
        }
        drop(staging);
        #[cfg(unix)]
        File::open(&parent)
            .and_then(|directory| directory.sync_all())
            .map_err(BackupError::PublishedDirectorySync)?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn publish_snapshot(snapshot: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    // Publish atomically with one link so a crash cannot leave the collector
    // with a multiply linked restore file that fails the writer lease.
    renameat_with(CWD, snapshot, CWD, destination, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn publish_snapshot(snapshot: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(snapshot, destination)
}

fn ensure_destination_is_fresh(destination: &Path) -> Result<(), BackupError> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut sibling = destination.as_os_str().to_owned();
        sibling.push(suffix);
        match fs::symlink_metadata(sibling) {
            Ok(_) => return Err(BackupError::DestinationExists),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn copy_committed_snapshot(from: &Connection, to: &mut Connection) -> Result<(), BackupError> {
    // An explicit read transaction pins a point-in-time source snapshot even
    // if other connections commit while SQLite copies the pages.
    let transaction = from.unchecked_transaction()?;
    transaction.query_row("SELECT COUNT(*) FROM sqlite_schema", [], |_| Ok(()))?;
    {
        let backup = Backup::new(&transaction, to)?;
        let mut last_progress = Instant::now();
        loop {
            match backup.step(PAGES_PER_STEP)? {
                StepResult::Done => break,
                StepResult::More => last_progress = Instant::now(),
                StepResult::Busy | StepResult::Locked
                    if last_progress.elapsed() < BACKUP_BUSY_WAIT =>
                {
                    std::thread::sleep(BACKUP_RETRY_DELAY);
                }
                StepResult::Busy | StepResult::Locked => return Err(BackupError::Busy),
                _ => return Err(BackupError::InvalidSnapshot),
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

struct StagingDirectory(PathBuf);

impl StagingDirectory {
    fn new(parent: &Path) -> io::Result<Self> {
        let builder = &mut DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        for _ in 0..32 {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".agenturm-backup-{}-{sequence}",
                std::process::id()
            ));
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            ErrorKind::AlreadyExists,
            "could not reserve backup workspace",
        ))
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CommitOutcome, ObservationBatch, ObservationIdentity, ObservationKind, StoredObservation,
        StoredScope,
    };

    fn batch(event_id: &str, expected: Option<&str>, next: &str) -> ObservationBatch {
        ObservationBatch::new_deterministic(
            "harness.test",
            expected.map(str::to_owned),
            next,
            vec![StoredObservation {
                event_id: event_id.into(),
                revision: 0,
                identity: ObservationIdentity {
                    scope: StoredScope::Session,
                    session_id: Some("session".into()),
                    worker_id: None,
                    turn_id: None,
                    operation_id: None,
                },
                observed_at_unix_ns: 1,
                kind: ObservationKind::SessionStarted,
            }],
        )
        .unwrap()
    }

    #[test]
    fn online_backup_restores_committed_wal_observations_cursors_and_receipts() {
        let directory = StagingDirectory::new(&std::env::temp_dir()).unwrap();
        let source = directory.0.join("source.sqlite");
        let dest = directory.0.join("restore.sqlite");
        let mut collector = Store::open(&source).unwrap();
        let first = batch("event-1", None, "cursor-1");
        collector.commit_batch(&first).unwrap();
        assert!(
            directory
                .0
                .join("source.sqlite-wal")
                .metadata()
                .unwrap()
                .len()
                > 0
        );

        let mut other = Connection::open(&source).unwrap();
        let pending = other.transaction().unwrap();
        pending
            .execute(
                "UPDATE agenturm_source_state SET cursor = 'uncommitted'",
                [],
            )
            .unwrap();
        Store::open_read_only(&source)
            .unwrap()
            .backup_to(&dest)
            .unwrap();
        pending.rollback().unwrap();

        let second = batch("event-2", Some("cursor-1"), "cursor-2");
        collector.commit_batch(&second).unwrap();
        assert!(
            directory
                .0
                .join("restore.sqlite-wal")
                .symlink_metadata()
                .is_err()
        );
        assert!(
            directory
                .0
                .join("restore.sqlite-shm")
                .symlink_metadata()
                .is_err()
        );
        let mut restored = Store::open_collector(&dest).unwrap();
        assert_eq!(restored.observations().unwrap().len(), 1);
        assert_eq!(
            restored.source_cursor("harness.test").unwrap().as_deref(),
            Some("cursor-1")
        );
        assert_eq!(
            restored.commit_batch(&first).unwrap(),
            CommitOutcome::Replay
        );
        assert_eq!(
            restored.commit_batch(&second).unwrap(),
            CommitOutcome::Committed { inserted: 1 }
        );
        assert_eq!(
            restored.observations().unwrap(),
            collector.observations().unwrap()
        );
    }

    #[test]
    fn backup_refuses_existing_destination_and_stale_sqlite_sidecars() {
        let directory = StagingDirectory::new(&std::env::temp_dir()).unwrap();
        let source = directory.0.join("source.sqlite");
        let dest = directory.0.join("restore.sqlite");
        let stale_wal = directory.0.join("restore.sqlite-wal");
        let store = Store::open(&source).unwrap();
        fs::write(&dest, b"existing").unwrap();
        assert!(matches!(
            store.backup_to(&dest),
            Err(BackupError::DestinationExists)
        ));
        assert_eq!(fs::read(&dest).unwrap(), b"existing");
        fs::remove_file(&dest).unwrap();
        fs::write(&stale_wal, b"stale wal").unwrap();
        assert!(matches!(
            store.backup_to(&dest),
            Err(BackupError::DestinationExists)
        ));
        assert!(!dest.exists());
        assert_eq!(fs::read(&stale_wal).unwrap(), b"stale wal");
        assert!(matches!(
            store.backup_to(&source),
            Err(BackupError::DestinationExists)
        ));
    }
}
