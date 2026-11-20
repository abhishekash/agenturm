#![cfg(target_os = "linux")]

use agenturm_storage_lease::WriterLease;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agenturm-storage-lease-{}-{tick}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let db = root.join("fixture.db");
        fs::write(&db, b"unchanged scratch bytes").unwrap();
        Self { root, db }
    }

    fn assert_unchanged(&self) {
        assert_eq!(fs::read(&self.db).unwrap(), b"unchanged scratch bytes");
        assert!(!self.root.join("fixture.db-wal").exists());
        assert!(!self.root.join("fixture.db-shm").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn start_holder(db: &Path) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agenturm-storage-lease-probe"))
        .arg(db)
        .env("AGENTURM_STORAGE_LEASE_PROBE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "ready\n", "probe did not acquire lease");
    child
}

fn stop_holder(mut child: Child) {
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
}

fn acquire_after_release(db: &Path) -> WriterLease {
    // A concurrent test can fork while this process holds a lock FD. The
    // child's inherited FD closes on exec, but a nonblocking reacquire in the
    // parent may briefly see WouldBlock before that exec completes.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match WriterLease::acquire(db) {
            Ok(lease) => return lease,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("released writer lock stayed unavailable: {error}"),
        }
    }
}

#[test]
fn second_process_and_symlink_alias_contend_without_db_mutation() {
    let fixture = Fixture::new();
    let alias = fixture.root.join("alias.db");
    symlink(&fixture.db, &alias).unwrap();
    let child = start_holder(&fixture.db);
    assert!(WriterLease::acquire(&fixture.db).is_err());
    assert!(WriterLease::acquire(&alias).is_err());
    fixture.assert_unchanged();
    stop_holder(child);
    let lease = WriterLease::acquire(&alias).unwrap();
    lease.verify_admission().unwrap();
    fixture.assert_unchanged();
}

#[test]
fn same_process_acquisitions_share_lock_until_final_drop() {
    let fixture = Fixture::new();
    let alias = fixture.root.join("alias.db");
    symlink(&fixture.db, &alias).unwrap();
    let first = WriterLease::acquire(&fixture.db).unwrap();
    let second = WriterLease::acquire(&alias).unwrap();
    first.verify_admission().unwrap();
    second.verify_admission().unwrap();
    drop(first);
    let denied = Command::new(env!("CARGO_BIN_EXE_agenturm-storage-lease-probe"))
        .arg(&fixture.db)
        .env("AGENTURM_STORAGE_LEASE_PROBE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!denied.success());
    drop(second);
    stop_holder(start_holder(&fixture.db));
    fixture.assert_unchanged();
}

#[test]
fn killed_owner_releases_os_lock() {
    let fixture = Fixture::new();
    let mut child = start_holder(&fixture.db);
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let lease = WriterLease::acquire(&fixture.db).unwrap();
    lease.verify_admission().unwrap();
    fixture.assert_unchanged();
}

#[test]
fn lock_symlink_or_replacement_is_rejected() {
    let fixture = Fixture::new();
    let lock_path = fixture.root.join(".fixture.db.agenturm-writer.lock");
    let other = fixture.root.join("other");
    fs::write(&other, b"other").unwrap();
    symlink(&other, &lock_path).unwrap();
    assert!(WriterLease::acquire(&fixture.db).is_err());
    fs::remove_file(&lock_path).unwrap();
    let lease = WriterLease::acquire(&fixture.db).unwrap();
    lease.verify_admission().unwrap();
    fs::rename(&lock_path, fixture.root.join("held.lock")).unwrap();
    assert!(lease.verify_admission().is_err());
    File::create(&lock_path).unwrap();
    assert!(lease.verify_admission().is_err());
    fixture.assert_unchanged();
}

#[test]
fn symlink_retarget_and_atomic_db_replacement_are_rejected() {
    let fixture = Fixture::new();
    let alias = fixture.root.join("alias.db");
    let alternate = fixture.root.join("alternate.db");
    fs::write(&alternate, b"another scratch file").unwrap();
    symlink(&fixture.db, &alias).unwrap();
    let lease = WriterLease::acquire(&alias).unwrap();
    lease.verify_admission().unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&alternate, &alias).unwrap();
    assert!(lease.verify_admission().is_err());
    drop(lease);
    let direct = acquire_after_release(&fixture.db);
    fs::rename(&fixture.db, fixture.root.join("held.db")).unwrap();
    fs::write(&fixture.db, b"unchanged scratch bytes").unwrap();
    assert!(direct.verify_admission().is_err());
    // This experiment checks identity, never performs a database write.
    fixture.assert_unchanged();
}

#[test]
fn nonprivate_lock_mode_is_rejected() {
    let fixture = Fixture::new();
    let lock_path = fixture.root.join(".fixture.db.agenturm-writer.lock");
    fs::write(&lock_path, b"lock").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(WriterLease::acquire(&fixture.db).is_err());
    fixture.assert_unchanged();
}
