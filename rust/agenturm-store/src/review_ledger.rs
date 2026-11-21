//! User review acknowledgements, independent of the observation writer.

use crate::{MAX_SESSION_KEY_BYTES, MAX_SESSION_PAGE_SIZE, validate_identifier};
use agenturm_kernel::ExtensionId;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const APPLICATION_ID: i64 = 0x4152_5657;
const SCHEMA_VERSION: i64 = 1;
type Result<T> = std::result::Result<T, ReviewError>;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReviewKey {
    pub harness_id: ExtensionId,
    pub session_id: String,
}

/// Identifies the retained evidence read for one session. Unrelated commits
/// do not change it. An admitted late event advances the session receipt
/// sequence, and pruning retained rows changes the count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewVersion {
    pub receipt_sequence: u64,
    pub retained_observations: u64,
}

#[derive(Debug)]
pub enum ReviewError {
    Invalid(&'static str),
    HistoryChanged,
    UnrecognizedLedger,
    Io(io::Error),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for ReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::HistoryChanged => {
                f.write_str("the history file changed since the review ledger was opened")
            }
            Self::UnrecognizedLedger => {
                f.write_str("the review file is not a recognized Agenturm review ledger")
            }
            Self::Io(error) => error.fmt(f),
            Self::Sqlite(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReviewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ReviewError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for ReviewError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Uses a separate SQLite file next to the canonical history path, allowing
/// the collector to keep its history writer lease. Startup and reads do not
/// create a ledger. The first explicit mark creates one.
pub struct ReviewLedger {
    history_path: PathBuf,
    sidecar: PathBuf,
    history_binding: [u8; 32],
}

impl ReviewLedger {
    pub fn open(history_path: impl AsRef<Path>) -> Result<Self> {
        let history_path = history_path.as_ref().canonicalize()?;
        let history_binding = file_binding(&history_path)?;
        let mut sidecar = history_path.as_os_str().to_owned();
        sidecar.push(".reviews.sqlite");
        Ok(Self {
            history_path,
            sidecar: PathBuf::from(sidecar),
            history_binding,
        })
    }

    /// Looks up the review marks for at most one session catalog page. Missing
    /// keys are omitted. The result is consistent with a single ledger read.
    pub fn reviewed(&self, keys: &[ReviewKey]) -> Result<BTreeMap<ReviewKey, ReviewVersion>> {
        if keys.len() > MAX_SESSION_PAGE_SIZE {
            return Err(ReviewError::Invalid(
                "review lookup exceeds the session page limit",
            ));
        }
        for key in keys {
            validate_key(key)?;
        }
        let mut marks = BTreeMap::new();
        if keys.is_empty() {
            return Ok(marks);
        }
        let Some(mut connection) = self.connect(false)? else {
            return Ok(marks);
        };
        let transaction = connection.transaction()?;
        if ledger_initialized(&transaction)? {
            let mut read = transaction.prepare_cached(
                "SELECT receipt_sequence, retained_observations
                 FROM agenturm_review_marks
                 WHERE history_binding = ?1 AND source_id = ?2 AND session_id = ?3",
            )?;
            for key in keys {
                let version = read
                    .query_row(
                        params![
                            self.history_binding,
                            key.harness_id.as_str(),
                            key.session_id
                        ],
                        |row| {
                            Ok(ReviewVersion {
                                receipt_sequence: row.get(0)?,
                                retained_observations: row.get(1)?,
                            })
                        },
                    )
                    .optional()?;
                if let Some(version) = version {
                    marks.insert(key.clone(), version);
                }
            }
        }
        self.verify_history()?;
        transaction.commit()?;
        Ok(marks)
    }

    /// Acknowledges exactly the evidence selected by the UI. The catalog's
    /// global snapshot is a write fence only. It never decides whether the
    /// current session's evidence still matches the acknowledgement. Returns
    /// false if the same or a newer snapshot already supplied a mark.
    pub fn mark_reviewed(
        &self,
        key: &ReviewKey,
        version: ReviewVersion,
        snapshot_id: u64,
    ) -> Result<bool> {
        validate_key(key)?;
        if version.receipt_sequence == 0
            || version.receipt_sequence > snapshot_id
            || version.retained_observations == 0
            || version.retained_observations > i64::MAX as u64
            || snapshot_id > i64::MAX as u64
        {
            return Err(ReviewError::Invalid("review version is invalid"));
        }
        self.verify_history()?;
        create_sidecar(&self.sidecar)?;
        let mut connection = self.connect(true)?.ok_or(ReviewError::UnrecognizedLedger)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !ledger_initialized(&transaction)? {
            transaction.execute_batch(
                "CREATE TABLE agenturm_review_marks (
                    history_binding BLOB NOT NULL CHECK (length(history_binding) = 32),
                    source_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    receipt_sequence INTEGER NOT NULL CHECK (receipt_sequence > 0),
                    retained_observations INTEGER NOT NULL CHECK (retained_observations > 0),
                    snapshot_id INTEGER NOT NULL CHECK (snapshot_id >= receipt_sequence),
                    PRIMARY KEY (history_binding, source_id, session_id)
                ) WITHOUT ROWID;",
            )?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        let changed = transaction.execute(
            "INSERT INTO agenturm_review_marks
                (history_binding, source_id, session_id, receipt_sequence,
                 retained_observations, snapshot_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (history_binding, source_id, session_id) DO UPDATE SET
                receipt_sequence = excluded.receipt_sequence,
                retained_observations = excluded.retained_observations,
                snapshot_id = excluded.snapshot_id
             WHERE excluded.snapshot_id > agenturm_review_marks.snapshot_id",
            params![
                self.history_binding,
                key.harness_id.as_str(),
                key.session_id,
                version.receipt_sequence,
                version.retained_observations,
                snapshot_id
            ],
        )?;
        self.verify_history()?;
        transaction.commit()?;
        Ok(changed != 0)
    }

    /// Removes one explicit acknowledgement. Clearing a missing ledger or a
    /// missing key is a no-op.
    pub fn clear(&self, key: &ReviewKey) -> Result<bool> {
        validate_key(key)?;
        let Some(mut connection) = self.connect(true)? else {
            return Ok(false);
        };
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !ledger_initialized(&transaction)? {
            return Ok(false);
        }
        let changed = transaction.execute(
            "DELETE FROM agenturm_review_marks
             WHERE history_binding = ?1 AND source_id = ?2 AND session_id = ?3",
            params![
                self.history_binding,
                key.harness_id.as_str(),
                key.session_id
            ],
        )?;
        self.verify_history()?;
        transaction.commit()?;
        Ok(changed != 0)
    }

    fn verify_history(&self) -> Result<()> {
        if file_binding(&self.history_path)? != self.history_binding {
            return Err(ReviewError::HistoryChanged);
        }
        Ok(())
    }

    fn connect(&self, write: bool) -> Result<Option<Connection>> {
        self.verify_history()?;
        match fs::symlink_metadata(&self.sidecar) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(ReviewError::UnrecognizedLedger),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let mode = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let connection = Connection::open_with_flags(&self.sidecar, mode)?;
        connection.busy_timeout(Duration::from_millis(500))?;
        Ok(Some(connection))
    }
}

fn validate_key(key: &ReviewKey) -> Result<()> {
    validate_identifier(&key.session_id, MAX_SESSION_KEY_BYTES, "session ID")
        .map_err(|_| ReviewError::Invalid("review session ID is invalid"))
}

fn ledger_initialized(connection: &Connection) -> Result<bool> {
    let application_id = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match (application_id, version) {
        (APPLICATION_ID, SCHEMA_VERSION) => Ok(true),
        (0, 0) => {
            let has_schema: bool =
                connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get(0)
                })?;
            if has_schema {
                Err(ReviewError::UnrecognizedLedger)
            } else {
                Ok(false)
            }
        }
        _ => Err(ReviewError::UnrecognizedLedger),
    }
}

fn create_sidecar(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn file_binding(path: &Path) -> Result<[u8; 32]> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(ReviewError::Invalid(
            "review history must be a regular file",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(b"agenturm-review-history-v1");
    bind_file_identity(&mut hash, &metadata)?;
    Ok(hash.finalize().into())
}

fn bind_file_identity(hash: &mut Sha256, metadata: &Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(metadata.dev().to_be_bytes());
        hash.update(metadata.ino().to_be_bytes());
    }
    match metadata.created() {
        Ok(created) => {
            let elapsed = created.duration_since(UNIX_EPOCH).unwrap_or_default();
            hash.update(elapsed.as_nanos().to_be_bytes());
        }
        #[cfg(unix)]
        Err(_) => {}
        #[cfg(not(unix))]
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn marks_are_explicit_durable_and_source_scoped() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        let writer = Store::open_collector(&history).unwrap();
        let codex = key("harness.codex");
        let pi = key("harness.pi");
        let version = version(2, 5);
        let ledger = ReviewLedger::open(&history).unwrap();
        let keys = [codex.clone(), pi.clone()];
        assert!(ledger.reviewed(&keys).unwrap().is_empty());
        assert!(!ledger.clear(&codex).unwrap());
        assert!(!scratch.0.join("history.sqlite.reviews.sqlite").exists());

        assert!(ledger.mark_reviewed(&codex, version, 3).unwrap());
        assert!(!ledger.mark_reviewed(&codex, version, 3).unwrap());
        assert!(writer.source_cursor("harness.codex").unwrap().is_none());
        let reopened = ReviewLedger::open(&history).unwrap();
        assert_eq!(
            reopened.reviewed(&keys).unwrap(),
            BTreeMap::from([(codex.clone(), version)])
        );
        assert!(reopened.clear(&codex).unwrap());
        assert!(!ledger.clear(&codex).unwrap());
        assert!(ledger.reviewed(&keys).unwrap().is_empty());
    }

    #[test]
    fn old_views_cannot_overwrite_a_newer_review_after_pruning() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        drop(Store::open(&history).unwrap());
        let stale = ReviewLedger::open(&history).unwrap();
        let current = ReviewLedger::open(&history).unwrap();
        let codex = key("harness.codex");
        let pre_retention = version(10, 5);
        let post_retention = version(8, 3);
        assert!(stale.mark_reviewed(&codex, pre_retention, 10).unwrap());
        assert!(current.mark_reviewed(&codex, post_retention, 11).unwrap());
        assert!(!stale.mark_reviewed(&codex, pre_retention, 10).unwrap());
        assert_eq!(
            current.reviewed(std::slice::from_ref(&codex)).unwrap()[&codex],
            post_retention
        );
    }

    #[test]
    fn a_replacement_history_does_not_inherit_old_reviews() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        drop(Store::open(&history).unwrap());
        let original = ReviewLedger::open(&history).unwrap();
        let codex = key("harness.codex");
        assert!(original.mark_reviewed(&codex, version(2, 2), 2).unwrap());

        let former = scratch.0.join("former.sqlite");
        fs::rename(&history, &former).unwrap();
        drop(Store::open(&history).unwrap());
        assert!(matches!(
            original.mark_reviewed(&codex, version(2, 2), 2),
            Err(ReviewError::HistoryChanged)
        ));
        let replacement = ReviewLedger::open(&history).unwrap();
        let keys = std::slice::from_ref(&codex);
        assert!(replacement.reviewed(keys).unwrap().is_empty());
        assert!(replacement.mark_reviewed(&codex, version(2, 3), 2).unwrap());
        assert_eq!(replacement.reviewed(keys).unwrap()[&codex], version(2, 3));
    }

    #[test]
    fn unrelated_sqlite_file_is_untouched() {
        let scratch = Scratch::new();
        let history = scratch.0.join("history.sqlite");
        drop(Store::open(&history).unwrap());
        let sidecar = scratch.0.join("history.sqlite.reviews.sqlite");
        let unrelated = Connection::open(&sidecar).unwrap();
        unrelated.execute_batch("CREATE TABLE example (a)").unwrap();
        drop(unrelated);
        let before = fs::read(&sidecar).unwrap();

        let ledger = ReviewLedger::open(&history).unwrap();
        assert!(matches!(
            ledger.mark_reviewed(&key("harness.codex"), version(1, 1), 1),
            Err(ReviewError::UnrecognizedLedger)
        ));
        assert_eq!(fs::read(&sidecar).unwrap(), before);
    }

    fn key(harness_id: &str) -> ReviewKey {
        ReviewKey {
            harness_id: ExtensionId::parse(harness_id).unwrap(),
            session_id: "shared-session".into(),
        }
    }

    fn version(receipt_sequence: u64, retained_observations: u64) -> ReviewVersion {
        ReviewVersion {
            receipt_sequence,
            retained_observations,
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "agenturm-reviews-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
