#![cfg(unix)]

use agenturm_migration_verify::{
    Backup, Manifest, ReplayReceipt, Snapshot, SourceCursor, verify_json,
};
use rusqlite::{Connection, MAIN_DB};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("agenturm-migration-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn descriptor(path: &Path, version: u32) -> Backup {
    let bytes = fs::read(path).unwrap();
    let stat = fs::metadata(path).unwrap();
    Backup {
        path: path.to_str().unwrap().into(),
        device: stat.dev(),
        inode: stat.ino(),
        size_bytes: stat.len(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        schema_version: version,
    }
}

fn detached(source: &Connection, path: &Path) {
    source.backup(MAIN_DB, path, None).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

struct Fixture {
    _dir: TempDir,
    source: PathBuf,
    manifest: Manifest,
}

fn fixture() -> Fixture {
    let dir = TempDir::new();
    let source = dir.path().join("live.db");
    let telemetry = Connection::open(&source).unwrap();
    telemetry.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    telemetry.execute_batch("CREATE TABLE schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE plugin_source_cursor_v1(source_id TEXT PRIMARY KEY,cursor TEXT NOT NULL,
        lease_epoch INTEGER NOT NULL,policy_epoch INTEGER NOT NULL,dirty_seq INTEGER NOT NULL,
        last_batch_digest TEXT NOT NULL);
        CREATE TABLE plugin_content_batch_v1(source_id TEXT PRIMARY KEY,cursor TEXT NOT NULL,refs_digest TEXT NOT NULL);
        CREATE TABLE plugin_command_batch_v1(source_id TEXT PRIMARY KEY,cursor TEXT NOT NULL,refs_digest TEXT NOT NULL);
        INSERT INTO schema_meta VALUES('receiver_id','receiver-1');
        INSERT INTO schema_meta VALUES('plugin_sessions_read_schema','1');").unwrap();
    // WAL-only committed row: keep this source connection open until backup.
    telemetry
        .execute(
            "INSERT INTO plugin_source_cursor_v1 VALUES('pi:a','cursor-2',1,1,2,?1)",
            ["a".repeat(64)],
        )
        .unwrap();
    telemetry
        .execute(
            "INSERT INTO plugin_content_batch_v1 VALUES('pi:a','content-2',?1)",
            ["b".repeat(64)],
        )
        .unwrap();
    let tele_backup = dir.path().join("telemetry.backup.db");
    detached(&telemetry, &tele_backup);
    let copied = Connection::open(&tele_backup).unwrap();
    let count: i64 = copied
        .query_row("SELECT COUNT(*) FROM plugin_source_cursor_v1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1, "committed WAL row must survive SQLite backup");
    drop(copied);

    let workflow_source = Connection::open_in_memory().unwrap();
    workflow_source
        .execute_batch(
            "CREATE TABLE workflow_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        INSERT INTO workflow_meta VALUES('schema_version','2');
        CREATE TABLE workflow_notification_claims_v1(id TEXT PRIMARY KEY);
        CREATE TABLE workflow_native_events_v1(id TEXT PRIMARY KEY);
        INSERT INTO workflow_native_events_v1 VALUES('event-1');",
        )
        .unwrap();
    let workflow_backup = dir.path().join("workflow.backup.db");
    detached(&workflow_source, &workflow_backup);

    let task_source = Connection::open_in_memory().unwrap();
    task_source
        .execute_batch(
            "CREATE TABLE task_schema_v1(version INTEGER NOT NULL);
        INSERT INTO task_schema_v1 VALUES(1);
        CREATE TABLE saved_tasks_v1(id TEXT PRIMARY KEY,revision INTEGER NOT NULL);
        INSERT INTO saved_tasks_v1 VALUES('task-1',3);",
        )
        .unwrap();
    let task_backup = dir.path().join("tasks.backup.db");
    detached(&task_source, &task_backup);

    let manifest = Manifest {
        format: "migration_manifest_v1".into(),
        receiver_id: "receiver-1".into(),
        telemetry: descriptor(&tele_backup, 1),
        workflow: descriptor(&workflow_backup, 2),
        tasks: descriptor(&task_backup, 1),
        snapshot: Snapshot {
            sources: vec![SourceCursor {
                source_id: "pi:a".into(),
                cursor: "cursor-2".into(),
                lease_epoch: 1,
                policy_epoch: 1,
                dirty_seq: 2,
                last_batch_digest: "a".repeat(64),
                content_receipt: Some(ReplayReceipt {
                    cursor: "content-2".into(),
                    refs_digest: "b".repeat(64),
                }),
                command_receipt: None,
            }],
            workflow_notification_claims: 0,
            workflow_native_events: 1,
            saved_tasks: 1,
            max_task_revision: 3,
        },
    };
    Fixture {
        _dir: dir,
        source,
        manifest,
    }
}

fn verify(manifest: &Manifest) -> bool {
    verify_json(&serde_json::to_string(manifest).unwrap()).is_ok()
}

#[test]
fn verifies_three_detached_backups_including_wal_only_row() {
    let f = fixture();
    assert!(verify(&f.manifest));
    let manifest_path = f._dir.path().join("candidate.json");
    fs::write(&manifest_path, serde_json::to_vec(&f.manifest).unwrap()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_agenturm-migration-verify"))
        .arg(manifest_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(output["receiver_id"], "receiver-1");
}

#[test]
fn rejects_digest_identity_and_path_replacement() {
    let mut f = fixture();
    let path = Path::new(&f.manifest.telemetry.path);
    let bytes = fs::read(path).unwrap();
    fs::write(path, b"tampered").unwrap();
    assert!(!verify(&f.manifest));
    fs::write(path, &bytes).unwrap();
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, &bytes).unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&replacement, path).unwrap();
    assert!(!verify(&f.manifest), "same bytes at a new inode must fail");
    f.manifest.telemetry = descriptor(path, 1);
    f.manifest.receiver_id = "other-receiver".into();
    assert!(!verify(&f.manifest));
}

#[test]
fn rejects_missing_auxiliary_symlink_and_nonprivate_backup() {
    let f = fixture();
    fs::remove_file(&f.manifest.workflow.path).unwrap();
    assert!(!verify(&f.manifest));
    let target = Path::new(&f.manifest.tasks.path);
    let real = target.with_extension("real");
    fs::rename(target, &real).unwrap();
    symlink(&real, target).unwrap();
    assert!(!verify(&f.manifest));
    fs::remove_file(target).unwrap();
    fs::rename(&real, target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!verify(&f.manifest));
}

#[test]
fn rejects_schema_receipt_counts_and_unknown_fields() {
    let mut f = fixture();
    f.manifest.tasks.schema_version = 2;
    assert!(!verify(&f.manifest));
    f.manifest.tasks.schema_version = 1;
    f.manifest.snapshot.sources[0]
        .content_receipt
        .as_mut()
        .unwrap()
        .refs_digest = "c".repeat(64);
    assert!(!verify(&f.manifest));
    f.manifest.snapshot.sources[0]
        .content_receipt
        .as_mut()
        .unwrap()
        .refs_digest = "b".repeat(64);
    f.manifest.snapshot.workflow_native_events = 0;
    assert!(!verify(&f.manifest));
    let mut json = serde_json::to_value(&f.manifest).unwrap();
    json.as_object_mut()
        .unwrap()
        .insert("new_field".into(), serde_json::json!(true));
    assert!(verify_json(&json.to_string()).is_err());
}

#[test]
fn rejects_database_schema_even_when_descriptor_is_refreshed() {
    let mut f = fixture();
    let path = Path::new(&f.manifest.workflow.path);
    let db = Connection::open(path).unwrap();
    db.execute(
        "UPDATE workflow_meta SET value='3' WHERE key='schema_version'",
        [],
    )
    .unwrap();
    drop(db);
    f.manifest.workflow = descriptor(path, 2);
    assert!(!verify(&f.manifest));

    let mut f = fixture();
    let path = Path::new(&f.manifest.tasks.path);
    let db = Connection::open(path).unwrap();
    db.execute("UPDATE task_schema_v1 SET version=2", [])
        .unwrap();
    drop(db);
    f.manifest.tasks = descriptor(path, 1);
    assert!(!verify(&f.manifest));
}

#[test]
fn interrupted_preparation_cannot_modify_live_source() {
    let f = fixture();
    let source_row = || -> i64 {
        Connection::open(&f.source)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM plugin_source_cursor_v1", [], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert_eq!(source_row(), 1);
    let before = fs::read(&f.source).unwrap();
    fs::write(&f.manifest.workflow.path, b"incomplete backup").unwrap();
    assert!(!verify(&f.manifest));
    assert_eq!(fs::read(&f.source).unwrap(), before);
    assert_eq!(source_row(), 1);
}
