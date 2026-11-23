//! Local desktop notification claims, independent of saved task archives.

use super::*;

const DESKTOP_APPLICATION_ID: i64 = 0x4157_4441;
const MAX_DESKTOP_HISTORIES: i64 = 128;
const MAX_DESKTOP_CLAIMS: i64 = 4_096;
const REQUEST_FRESHNESS_NS: i64 = 15_000_000_000;

/// A current request whose retained source clock has been verified as
/// comparable to the clock of the native reader.
pub struct DesktopAttentionRequest {
    pub anchor: WorkspaceAnchor,
    pub event_id: String,
    pub revision: u32,
    pub observed_at_unix_ns: u64,
}

impl WorkspaceLedger {
    /// Consumes at most one automatic notification attempt per exact request.
    /// The first complete inventory for a history incarnation is silent.
    /// Callers must pass only a successful, complete current inventory and
    /// must check the active history binding before claiming.
    pub fn claim_desktop_attention(
        &self,
        history_identity: &str,
        snapshot: ReadSnapshot,
        now_unix_ns: u64,
        requests: &[DesktopAttentionRequest],
    ) -> Result<usize> {
        if requests.len() > MAX_WORKSPACE_PAGE_SIZE
            || history_identity.len() != 32
            || !history_identity
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(WorkspaceError::Invalid(
                "invalid desktop attention inventory",
            ));
        }
        let now = positive_time(now_unix_ns)?;
        let watermark = i64::try_from(snapshot.watermark())
            .map_err(|_| WorkspaceError::Invalid("invalid desktop attention snapshot"))?;
        self.verify_history()?;

        let mut namespace = Sha256::new();
        namespace.update(b"agenturm-desktop-attention-v1");
        namespace.update(self.binding);
        namespace.update(history_identity);
        if let Some(incarnation) = snapshot.incarnation() {
            namespace.update(incarnation.to_string());
        }
        let namespace: [u8; 32] = namespace.finalize().into();
        let requests = requests
            .iter()
            .map(request_key)
            .collect::<Result<Vec<_>>>()?;

        let mut path = self.history.as_os_str().to_owned();
        path.push(".desktop-attention.sqlite");
        let path = Path::new(&path);
        create_private_file(path)?;
        let file = file_stamp(path)?;
        let mut db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_millis(500))?;
        db.execute_batch("PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        initialize(&tx)?;
        let previous: Option<(i64, i64, i64, i64)> = tx
            .query_row(
                "SELECT baseline_time, baseline_watermark, last_time, last_watermark
             FROM desktop_histories WHERE namespace=?1",
                [namespace],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let claimed = if let Some((baseline_time, baseline_watermark, last_time, last_watermark)) =
            previous
        {
            if now < last_time || watermark < last_watermark {
                return Err(WorkspaceError::Invalid(
                    "desktop attention inventory predates its last complete read",
                ));
            }
            tx.execute(
                "UPDATE desktop_histories SET last_time=?2, last_watermark=?3 WHERE namespace=?1",
                params![namespace, now, watermark],
            )?;
            let oldest = now.saturating_sub(REQUEST_FRESHNESS_NS);
            tx.execute(
                "DELETE FROM desktop_claims WHERE namespace=?1 AND observed_at<?2",
                params![namespace, oldest],
            )?;
            let retained: i64 =
                tx.query_row("SELECT COUNT(*) FROM desktop_claims", [], |row| row.get(0))?;
            let mut insert = tx.prepare(
                "INSERT INTO desktop_claims (namespace, digest, observed_at) VALUES (?1,?2,?3)
                 ON CONFLICT(namespace,digest) DO NOTHING",
            )?;
            let mut claimed = 0;
            for (digest, observed_at) in requests {
                if watermark > baseline_watermark
                    && observed_at > baseline_time
                    && (oldest..=now).contains(&observed_at)
                {
                    let inserted = insert.execute(params![namespace, digest, observed_at])?;
                    claimed += inserted;
                    if retained + (claimed as i64) > MAX_DESKTOP_CLAIMS {
                        return Err(WorkspaceError::Invalid(
                            "desktop attention claim capacity reached",
                        ));
                    }
                }
            }
            claimed
        } else {
            let retained: i64 =
                tx.query_row("SELECT COUNT(*) FROM desktop_histories", [], |row| {
                    row.get(0)
                })?;
            if retained >= MAX_DESKTOP_HISTORIES {
                return Err(WorkspaceError::Invalid(
                    "desktop attention history capacity reached",
                ));
            }
            tx.execute(
                "INSERT INTO desktop_histories VALUES (?1,?2,?3,?2,?3)",
                params![namespace, now, watermark],
            )?;
            0
        };
        self.verify_history()?;
        if file_stamp(path)? != file {
            return Err(WorkspaceError::HistoryChanged);
        }
        tx.commit()?;
        Ok(claimed)
    }
}

fn initialize(db: &Connection) -> Result<()> {
    let id: i64 = db.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match (id, version) {
        (DESKTOP_APPLICATION_ID, 1) => Ok(()),
        (0, 0) => {
            let has_tables: bool =
                db.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get(0)
                })?;
            if has_tables {
                return Err(WorkspaceError::Invalid(
                    "unrecognized desktop attention state",
                ));
            }
            db.execute_batch(
                "CREATE TABLE desktop_histories (
                     namespace BLOB PRIMARY KEY NOT NULL,
                     baseline_time INTEGER NOT NULL,
                     baseline_watermark INTEGER NOT NULL,
                     last_time INTEGER NOT NULL,
                     last_watermark INTEGER NOT NULL
                 );
                 CREATE TABLE desktop_claims (
                     namespace BLOB NOT NULL, digest BLOB NOT NULL, observed_at INTEGER NOT NULL,
                     PRIMARY KEY (namespace,digest)
                 ) WITHOUT ROWID;",
            )?;
            db.pragma_update(None, "application_id", DESKTOP_APPLICATION_ID)?;
            db.pragma_update(None, "user_version", 1)?;
            Ok(())
        }
        _ => Err(WorkspaceError::Invalid(
            "unrecognized desktop attention state",
        )),
    }
}

fn request_key(request: &DesktopAttentionRequest) -> Result<([u8; 32], i64)> {
    validate_request_key(&request.anchor, &request.event_id)?;
    let observed_at = positive_time(request.observed_at_unix_ns)?;
    let anchor = &request.anchor;
    let mut hash = Sha256::new();
    for part in [
        Some(anchor.harness_id.as_str()),
        Some(&anchor.session_id),
        anchor.worker_id.as_deref(),
        anchor.turn_id.as_deref(),
        Some(&request.event_id),
    ] {
        hash.update(part.unwrap_or_default().as_bytes());
        hash.update([0]);
    }
    hash.update(request.revision.to_be_bytes());
    Ok((hash.finalize().into(), observed_at))
}

fn positive_time(time: u64) -> Result<i64> {
    i64::try_from(time)
        .ok()
        .filter(|time| *time > 0)
        .ok_or(WorkspaceError::Invalid("invalid desktop attention time"))
}

#[cfg(unix)]
fn file_stamp(path: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let file = fs::symlink_metadata(path)?;
    if !file.is_file() || file.nlink() != 1 || file.mode() & 0o077 != 0 {
        return Err(WorkspaceError::Invalid(
            "desktop attention state must be a private SQLite file",
        ));
    }
    Ok((file.dev(), file.ino()))
}

#[cfg(not(unix))]
fn file_stamp(_path: &Path) -> Result<(u64, u64)> {
    Err(WorkspaceError::Invalid(
        "desktop attention state is unavailable on this platform",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "agenturm-desktop-test-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn claims_are_atomic_and_a_restored_read_incarnation_starts_silent() {
        let root = Scratch::new();
        let history = root.0.join("history.sqlite");
        let writer = Store::open_collector(&history).unwrap();
        let identity = writer.history_identity().unwrap();
        let ledger = WorkspaceLedger::open(history).unwrap();
        let request = DesktopAttentionRequest {
            anchor: WorkspaceAnchor {
                harness_id: ExtensionId::parse("harness.codex").unwrap(),
                session_id: "session".into(),
                turn_id: Some("turn".into()),
                worker_id: None,
            },
            event_id: "codex:fixture:1:lifecycle".into(),
            revision: 0,
            observed_at_unix_ns: 11,
        };
        let rows = std::slice::from_ref(&request);
        assert_eq!(
            ledger
                .claim_desktop_attention(&identity, 1.into(), 10, &[])
                .unwrap(),
            0
        );
        let claim = || {
            ledger
                .claim_desktop_attention(&identity, 2.into(), 12, rows)
                .unwrap()
        };
        let claims = std::thread::scope(|scope| {
            let left = scope.spawn(claim);
            let right = scope.spawn(claim);
            left.join().unwrap() + right.join().unwrap()
        });
        assert_eq!(claims, 1);
        assert_eq!(claim(), 0);
        assert!(
            ledger
                .claim_desktop_attention(&identity, 1.into(), 12, rows)
                .is_err()
        );

        let incarnation = "1".repeat(32).parse().unwrap();
        let restored = ReadSnapshot::new(2, Some(incarnation));
        assert_eq!(
            ledger
                .claim_desktop_attention(&identity, restored, 13, rows)
                .unwrap(),
            0
        );
        assert_eq!(
            ledger
                .claim_desktop_attention(&identity, restored, 14, rows)
                .unwrap(),
            0
        );
    }
}
