//! Portable history and saved workbench backups.

use super::{
    BackupError, StagingDirectory, Store, copy_committed_snapshot, ensure_destination_is_fresh,
};
use crate::{ReviewLedger, WorkspaceLedger, review_ledger, workspace_ledger};
use rusqlite::{Connection, OpenFlags, params};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const HISTORY: &str = "history.sqlite";
const MANIFEST: &str = "agenturm-workspace-backup";
const MAGIC: &str = "agenturm-workspace-backup/1";
const ARCHIVE_BINDING: [u8; 32] = [0; 32];
type Result<T> = std::result::Result<T, BackupError>;

#[derive(Clone, Copy)]
enum Sidecar {
    Workspace,
    Review,
}

impl Sidecar {
    fn filename(self) -> &'static str {
        match self {
            Self::Workspace => "history.sqlite.workspace.sqlite",
            Self::Review => "history.sqlite.reviews.sqlite",
        }
    }

    fn ready(self, db: &Connection) -> Result<bool> {
        match self {
            Self::Workspace => workspace_ledger::schema_version(db)
                .map(|version| version != 0)
                .map_err(sidecar_error),
            Self::Review => review_ledger::ledger_initialized(db).map_err(sidecar_error),
        }
    }

    fn tables(self) -> &'static [&'static str] {
        match self {
            Self::Workspace => &[
                "agenturm_saved_tasks",
                "agenturm_saved_handoffs",
                "agenturm_seen_requests",
            ],
            Self::Review => &["agenturm_review_marks", "agenturm_review_notes"],
        }
    }
}

impl Store {
    /// Publishes a portable backup directory containing the history database
    /// and any saved workbench/review state for that history. Restore the
    /// archive with `restore_workspace_bundle` before opening its sidecars.
    pub fn backup_workspace_bundle(
        history_path: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<()> {
        let history_path = history_path.as_ref().canonicalize()?;
        let history = Self::open_read_only(&history_path)?;
        let workspace = WorkspaceLedger::open(&history_path).map_err(sidecar_error)?;
        let review = ReviewLedger::open(&history_path).map_err(sidecar_error)?;
        with_private_destination(destination.as_ref(), |staging| {
            // Sidecars are captured before the history checkpoint so their
            // acknowledged history cannot come from a future main snapshot.
            let has_workspace = copy_optional(
                workspace.open_for_backup().map_err(sidecar_error)?,
                staging,
                Sidecar::Workspace,
                workspace.backup_binding(),
                ARCHIVE_BINDING,
            )?;
            let has_review = copy_optional(
                review.open_for_backup().map_err(sidecar_error)?,
                staging,
                Sidecar::Review,
                review.backup_binding(),
                ARCHIVE_BINDING,
            )?;
            history.backup_to(staging.join(HISTORY))?;
            if WorkspaceLedger::open(&history_path)
                .map_err(sidecar_error)?
                .backup_binding()
                != workspace.backup_binding()
                || ReviewLedger::open(&history_path)
                    .map_err(sidecar_error)?
                    .backup_binding()
                    != review.backup_binding()
            {
                return Err(BackupError::InvalidSnapshot);
            }
            let mut manifest = private_file(&staging.join(MANIFEST))?;
            writeln!(
                manifest,
                "{MAGIC}\n{}\n{}",
                u8::from(has_workspace),
                u8::from(has_review)
            )?;
            manifest.sync_all()?;
            Ok(())
        })
    }

    /// Restores a portable backup to a new private directory. Open the
    /// resulting `history.sqlite` normally. Restore rebinds companion rows
    /// to the new history file and keeps private-content approvals disabled.
    pub fn restore_workspace_bundle(
        archive: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<()> {
        let archive = archive.as_ref().canonicalize()?;
        let manifest = read_manifest(&archive)?;
        let history_path = archive.join(HISTORY);
        validate_archive_database(&history_path)?;
        let history = Self::open_read_only(&history_path)?;
        with_private_destination(destination.as_ref(), |staging| {
            let restored_history = staging.join(HISTORY);
            history.backup_to(&restored_history)?;
            for (present, kind) in manifest
                .into_iter()
                .zip([Sidecar::Workspace, Sidecar::Review])
            {
                let path = archive.join(kind.filename());
                if !present {
                    match path.symlink_metadata() {
                        Err(error) if error.kind() == ErrorKind::NotFound => continue,
                        Ok(_) => return Err(BackupError::InvalidBundle),
                        Err(error) => return Err(error.into()),
                    }
                }
                validate_archive_database(&path)?;
                let binding = match kind {
                    Sidecar::Workspace => WorkspaceLedger::open(&restored_history)
                        .map_err(sidecar_error)?
                        .backup_binding(),
                    Sidecar::Review => ReviewLedger::open(&restored_history)
                        .map_err(sidecar_error)?
                        .backup_binding(),
                };
                let database = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                )?;
                if !copy_optional(Some(database), staging, kind, ARCHIVE_BINDING, binding)? {
                    return Err(BackupError::InvalidBundle);
                }
            }
            Ok(())
        })
    }
}

fn sidecar_error(error: impl std::fmt::Display) -> BackupError {
    BackupError::Sidecar(error.to_string())
}

fn copy_optional(
    database: Option<Connection>,
    staging: &Path,
    kind: Sidecar,
    source_binding: [u8; 32],
    destination_binding: [u8; 32],
) -> Result<bool> {
    let Some(database) = database else {
        return Ok(false);
    };
    if !kind.ready(&database)? {
        return Ok(false);
    }
    let path = staging.join(kind.filename());
    let file = private_file(&path)?;
    let mut copy = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    copy_committed_snapshot(&database, &mut copy)?;
    if !kind.ready(&copy)? {
        return Err(BackupError::InvalidSnapshot);
    }
    let mode: String = copy.query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(BackupError::InvalidSnapshot);
    }
    copy.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON; PRAGMA trusted_schema=OFF",
    )?;
    let transaction = copy.transaction()?;
    for table in kind.tables() {
        if *table == "agenturm_review_notes"
            && transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?
                == 1
        {
            continue;
        }
        let removed = transaction.execute(
            &format!("DELETE FROM {table} WHERE history_binding <> ?1"),
            [source_binding],
        )?;
        if source_binding == ARCHIVE_BINDING && removed != 0 {
            return Err(BackupError::InvalidBundle);
        }
        transaction.execute(
            &format!("UPDATE {table} SET history_binding = ?2 WHERE history_binding = ?1"),
            params![source_binding, destination_binding],
        )?;
    }
    transaction.commit()?;
    // Remove freed pages which may contain a replaced history's private notes.
    copy.execute_batch("VACUUM")?;
    let integrity: String = copy.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    let has_bad_reference = copy
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some();
    if integrity != "ok" || has_bad_reference {
        return Err(BackupError::InvalidSnapshot);
    }
    copy.close().map_err(|(_, error)| error)?;
    file.sync_all()?;
    Ok(true)
}

fn private_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

fn read_manifest(archive: &Path) -> Result<[bool; 2]> {
    let path = archive.join(MANIFEST);
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() > 128 {
        return Err(BackupError::InvalidBundle);
    }
    let mut contents = String::new();
    std::fs::File::open(path)?
        .take(128)
        .read_to_string(&mut contents)?;
    let mut lines = contents.lines();
    if lines.next() != Some(MAGIC) {
        return Err(BackupError::InvalidBundle);
    }
    let mut presence = [false; 2];
    for present in &mut presence {
        *present = match lines.next() {
            Some("0") => false,
            Some("1") => true,
            _ => return Err(BackupError::InvalidBundle),
        };
    }
    if lines.next().is_some() {
        return Err(BackupError::InvalidBundle);
    }
    Ok(presence)
}

fn validate_archive_database(path: &Path) -> Result<()> {
    if !path.symlink_metadata()?.is_file() {
        return Err(BackupError::InvalidBundle);
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sibling = path.as_os_str().to_owned();
        sibling.push(suffix);
        match fs::symlink_metadata(PathBuf::from(sibling)) {
            Ok(_) => return Err(BackupError::InvalidBundle),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn with_private_destination(
    destination: &Path,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
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
    write(&staging.0)?;
    #[cfg(unix)]
    File::open(&staging.0)?.sync_all()?;
    publish_directory(&staging.0, &destination)?;
    #[cfg(unix)]
    File::open(&parent)
        .and_then(|directory| directory.sync_all())
        .map_err(BackupError::PublishedDirectorySync)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn publish_directory(staging: &Path, destination: &Path) -> Result<()> {
    match super::publish_snapshot(staging, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Err(BackupError::DestinationExists)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn publish_directory(_staging: &Path, _destination: &Path) -> Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "workspace backup and restore require Linux",
    )
    .into())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        ContentGrantScope, ContentKind, ObservationBatch, ObservationIdentity, ObservationKind,
        ReviewKey, ReviewVersion, StoredObservation, StoredScope, WorkspaceAnchor,
        WorkspaceGitSnapshot,
    };
    use agenturm_kernel::ExtensionId;

    #[test]
    fn a_copied_bundle_restores_active_workbench_and_reviews_beside_history() {
        let scratch = StagingDirectory::new(&std::env::temp_dir()).unwrap();
        let db = scratch.0.join("live.sqlite");
        let mut collector = Store::open_collector(&db).unwrap();
        let first = seed(&mut collector, "harness.pi", "first");
        let second = seed(&mut collector, "harness.kiro", "second");
        let workspace = WorkspaceLedger::open(&db).unwrap();
        let task = workspace
            .create_task(Some(&first), "Keep this task", "Local note")
            .unwrap();
        assert!(workspace.link_task(task.id, &second).unwrap());
        let git = WorkspaceGitSnapshot {
            repository_root: scratch.0.display().to_string(),
            branch: Some("main".into()),
            head_commit: Some("a".repeat(40)),
            staged_changes: 1,
            unstaged_changes: 2,
            untracked_files: 3,
            captured_at_unix_ns: 123,
        };
        assert!(
            workspace
                .save_task_workspace_snapshot(task.id, &git)
                .unwrap()
        );
        let task = workspace.task(task.id).unwrap().unwrap();
        assert_eq!(task.revision, 3);
        let handoff = workspace
            .create_handoff(&first, &second, "Look at this turn")
            .unwrap();
        assert!(workspace.mark_request_seen(&second, "request", 0).unwrap());
        let reviewed = ReviewKey {
            harness_id: first.harness_id.clone(),
            session_id: first.session_id.clone(),
        };
        let version = ReviewVersion {
            receipt_sequence: 1,
            retained_observations: 1,
        };
        let review = ReviewLedger::open(&db).unwrap();
        assert!(review.mark_reviewed(&reviewed, version, 2).unwrap());
        review
            .save_decision_note(&reviewed, version, 2, "Retain this decision")
            .unwrap();
        let scope =
            ContentGrantScope::new(first.harness_id.clone(), ContentKind::PiUserText, [9; 32]);
        collector.grant_content(&scope).unwrap();

        // The local sidecar can still have state for a replaced history.
        // Those private notes must be absent even from archived free pages.
        let obsolete_note = "OBSOLETE_HISTORY_NOTE_MUST_NOT_LEAVE_THIS_DATABASE";
        let live_sidecar =
            Connection::open(scratch.0.join("live.sqlite.workspace.sqlite")).unwrap();
        live_sidecar.execute(
            "INSERT INTO agenturm_saved_tasks (history_binding, title, note) VALUES (?1, 'Obsolete', ?2)",
            params![[4_u8; 32], obsolete_note],
        ).unwrap();
        let obsolete_id = live_sidecar.last_insert_rowid();
        live_sidecar
            .execute(
                "INSERT INTO agenturm_task_briefs (task_id, brief_json) VALUES (?1, ?2)",
                params![
                    obsolete_id,
                    serde_json::json!({"intent":obsolete_note}).to_string()
                ],
            )
            .unwrap();
        live_sidecar.execute(
            "INSERT INTO agenturm_task_decisions (task_id, kind, text, rationale, scope, created_at_unix_ns)
             VALUES (?1, 'accepted', ?2, '', '', 1)",
            params![obsolete_id, obsolete_note],
        ).unwrap();
        drop(live_sidecar);
        let review_sidecar =
            Connection::open(scratch.0.join("live.sqlite.reviews.sqlite")).unwrap();
        review_sidecar
            .execute(
                "INSERT INTO agenturm_review_notes
                 (history_binding, source_id, session_id, receipt_sequence, retained_observations,
                  snapshot_id, note, note_digest)
             VALUES (?1, 'harness.pi', 'first', 1, 1, 2, ?2, ?3)",
                params![[4_u8; 32], obsolete_note, [5_u8; 32]],
            )
            .unwrap();
        drop(review_sidecar);

        let archive = scratch.0.join("archive");
        Store::backup_workspace_bundle(&db, &archive).unwrap();
        for kind in [Sidecar::Workspace, Sidecar::Review] {
            let bytes = fs::read(archive.join(kind.filename())).unwrap();
            assert!(
                !bytes
                    .windows(obsolete_note.len())
                    .any(|bytes| bytes == obsolete_note.as_bytes())
            );
        }
        let copied = scratch.0.join("copied");
        fs::create_dir(&copied).unwrap();
        for entry in fs::read_dir(&archive).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), copied.join(entry.file_name())).unwrap();
        }
        let restored_directory = scratch.0.join("restored");
        Store::restore_workspace_bundle(&copied, &restored_directory).unwrap();
        let restored_db = restored_directory.join(HISTORY);
        let restored = Store::open_collector(&restored_db).unwrap();
        assert_eq!(
            restored.observations().unwrap(),
            collector.observations().unwrap()
        );
        assert!(restored.current_content_grant(&scope).unwrap().is_none());
        assert!(collector.current_content_grant(&scope).unwrap().is_some());
        let restored_workspace = WorkspaceLedger::open(&restored_db).unwrap();
        assert_eq!(
            restored_workspace
                .list_tasks(10, None)
                .unwrap()
                .items
                .as_slice(),
            std::slice::from_ref(&task),
        );
        assert_eq!(
            restored_workspace.task_links(task.id, 10).unwrap(),
            [second.clone(), first]
        );
        assert_eq!(
            restored_workspace.task_workspace_snapshot(task.id).unwrap(),
            Some(git)
        );
        assert_eq!(
            restored_workspace.list_handoffs(10, None).unwrap().items,
            [handoff]
        );
        let seen = (second, "request".into(), 0);
        assert!(
            restored_workspace
                .seen_requests(std::slice::from_ref(&seen))
                .unwrap()
                .contains(&seen)
        );
        let restored_review = ReviewLedger::open(&restored_db).unwrap();
        assert_eq!(
            restored_review
                .reviewed(std::slice::from_ref(&reviewed))
                .unwrap()[&reviewed],
            version
        );
        let decisions = restored_review.decision_notes(&reviewed).unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].note, "Retain this decision");
        assert_eq!(decisions[0].version, version);
        assert!(matches!(
            Store::restore_workspace_bundle(&copied, &restored_directory),
            Err(BackupError::DestinationExists)
        ));
    }

    #[test]
    fn a_history_without_sidecars_restores_and_a_missing_declared_file_is_rejected() {
        let scratch = StagingDirectory::new(&std::env::temp_dir()).unwrap();
        let db = scratch.0.join("live.sqlite");
        drop(Store::open_collector(&db).unwrap());
        let archive = scratch.0.join("archive");
        let restored = scratch.0.join("restored");
        Store::backup_workspace_bundle(&db, &archive).unwrap();
        Store::restore_workspace_bundle(&archive, &restored).unwrap();
        assert!(restored.join(HISTORY).is_file());
        assert!(!restored.join(Sidecar::Workspace.filename()).exists());
        assert!(!restored.join(Sidecar::Review.filename()).exists());
        fs::write(archive.join(MANIFEST), format!("{MAGIC}\n1\n0\n")).unwrap();
        let denied = scratch.0.join("denied");
        assert!(Store::restore_workspace_bundle(&archive, &denied).is_err());
        assert!(!denied.exists());
    }

    #[test]
    fn a_version_one_review_ledger_restores_its_marks() {
        let scratch = StagingDirectory::new(&std::env::temp_dir()).unwrap();
        let db = scratch.0.join("live.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        let anchor = seed(&mut store, "harness.pi", "session");
        let key = ReviewKey {
            harness_id: anchor.harness_id,
            session_id: anchor.session_id,
        };
        let version = ReviewVersion {
            receipt_sequence: 1,
            retained_observations: 1,
        };
        ReviewLedger::open(&db)
            .unwrap()
            .mark_reviewed(&key, version, 1)
            .unwrap();
        let archive = scratch.0.join("archive");
        let restored = scratch.0.join("restored");
        Store::backup_workspace_bundle(&db, &archive).unwrap();
        Store::restore_workspace_bundle(&archive, &restored).unwrap();
        let review = ReviewLedger::open(restored.join(HISTORY)).unwrap();
        assert_eq!(
            review.reviewed(std::slice::from_ref(&key)).unwrap()[&key],
            version
        );
        assert!(review.decision_notes(&key).unwrap().is_empty());
    }

    #[test]
    fn version_ten_history_can_be_backed_up_and_read_before_migration() {
        let scratch = StagingDirectory::new(&std::env::temp_dir()).unwrap();
        let db = scratch.0.join("old.sqlite");
        let mut store = Store::open_collector(&db).unwrap();
        seed(&mut store, "harness.pi", "session");
        let original = store.observation_page(4).unwrap();
        store
            .connection
            .execute_batch(
                "DROP TABLE agenturm_content_grants; DROP TABLE agenturm_content_cursor_proofs;",
            )
            .unwrap();
        crate::content_grant::create_schema(&store.connection, 9).unwrap();
        store
            .connection
            .execute(
                "UPDATE agenturm_store_meta SET value = ?1 WHERE key = 'format'",
                [crate::STORE_MARKER_V10],
            )
            .unwrap();
        store
            .connection
            .pragma_update(None, "user_version", 10)
            .unwrap();
        drop(store);

        let archive = scratch.0.join("archive");
        let restored = scratch.0.join("restored");
        Store::backup_workspace_bundle(&db, &archive).unwrap();
        Store::restore_workspace_bundle(&archive, &restored).unwrap();
        let restored = restored.join(HISTORY);
        let reader = Store::open_read_only(&restored).unwrap();
        assert_eq!(reader.observation_page(4).unwrap(), original);
        drop(reader);
        let writer = Store::open_collector(&restored).unwrap();
        assert_eq!(writer.observation_page(4).unwrap(), original);
    }

    fn seed(store: &mut Store, source: &str, session: &str) -> WorkspaceAnchor {
        let batch = ObservationBatch::new_deterministic(
            source,
            None,
            "cursor",
            vec![StoredObservation {
                event_id: "request".into(),
                revision: 0,
                identity: ObservationIdentity {
                    scope: StoredScope::Session,
                    session_id: Some(session.into()),
                    worker_id: None,
                    turn_id: None,
                    operation_id: None,
                },
                observed_at_unix_ns: 1,
                kind: ObservationKind::SessionStarted,
            }],
        )
        .unwrap();
        store.commit_batch(&batch).unwrap();
        WorkspaceAnchor {
            harness_id: ExtensionId::parse(source).unwrap(),
            session_id: session.into(),
            worker_id: None,
            turn_id: None,
        }
    }
}
