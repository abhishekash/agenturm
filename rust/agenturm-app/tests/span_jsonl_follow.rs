#![cfg(target_os = "linux")]

use agenturm_store::{ObservationKind, Store};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, PipeReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const SOURCE: &str = "otlp.watch-fixture";
const TRACE: &str = "11111111111111111111111111111111";
const PRIVATE: &str = "private-fixture-payload";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "agenturm-watch-jsonl-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn input(&self) -> PathBuf {
        self.0.join("spans.jsonl")
    }

    fn db(&self) -> PathBuf {
        self.0.join("history.db")
    }

    fn command(&self) -> Command {
        watch_command(&self.input(), &self.db())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn watch_command(input: &Path, database: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
    command
        .args(["watch-jsonl", "--file"])
        .arg(input)
        .arg("--db")
        .arg(database)
        .args(["--source-id", SOURCE, "--interval-ms", "100"]);
    command
}

struct Watch(Child);

impl Watch {
    fn start(mut command: Command) -> Self {
        Self(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn wait_for(&mut self, db: &Path, check: impl Fn(&Store) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            assert_eq!(
                self.0.try_wait().unwrap(),
                None,
                "watch stopped before the checkpoint"
            );
            if Store::open_read_only(db).is_ok_and(|store| check(&store)) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "watch did not reach the checkpoint"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn terminate(&mut self) -> Output {
        // SAFETY: live spawned child PID, checked by each caller before termination.
        assert_eq!(unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                use std::io::Read;
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                self.0
                    .stdout
                    .take()
                    .unwrap()
                    .read_to_end(&mut stdout)
                    .unwrap();
                self.0
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_end(&mut stderr)
                    .unwrap();
                return Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            assert!(Instant::now() < deadline, "watch did not stop on SIGTERM");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn hold_progress(mut command: Command) -> (Watch, PipeReader, usize) {
    let (reader, mut writer) = std::io::pipe().unwrap();
    // SAFETY: writer owns a live pipe descriptor throughout these calls.
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let mut written = 0;
    for size in [4096, 1] {
        let bytes = vec![b'x'; size];
        loop {
            match writer.write(&bytes) {
                Ok(amount) => written += amount,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("cannot fill fixture pipe: {error}"),
            }
        }
    }
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        0
    );
    let child = command
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Command retains its Stdio configuration unless dropped.
    drop(command);
    (Watch(child), reader, written)
}

fn span(id: u64, parent: Option<u64>) -> Value {
    json!({"trace_id":TRACE,"span_id":format!("{id:016x}"),
        "parent_span_id":parent.map(|parent| format!("{parent:016x}")),
        "start_unix_nano":"1750000000000000000","end_unix_nano":"1750000000000000001",
        "status":"OK","name":PRIVATE,"attributes":{"session.id":"fixture",
            "agenturm.payload.input":PRIVATE}})
}

fn data(path: &Path, rows: &[Value]) {
    let document = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    fs::write(path, document).unwrap();
}

fn append(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn count(store: &Store) -> usize {
    store
        .observations()
        .unwrap()
        .iter()
        .filter(|(_, row)| matches!(row.kind, ObservationKind::TraceSpan(_)))
        .count()
}

#[test]
fn watch_resumes_complete_records_and_replays_rotation_and_same_inode_rewrites() {
    let scratch = Fixture::new();
    let db = scratch.db();
    let input = scratch.input();
    let (mut watcher, mut progress, filler) = hold_progress(scratch.command());
    watcher.wait_for(&db, |store| count(store) == 0);
    let first = span(1, None);
    let second = span(2, Some(1));
    fs::write(&input, format!("{first}\n{second}")).unwrap();
    watcher.wait_for(&db, |store| count(store) == 1);
    let committed = Store::open_read_only(&db)
        .unwrap()
        .source_cursor(SOURCE)
        .unwrap()
        .unwrap();
    assert!(committed.starts_with("span-jsonl-follow/1:"));
    watcher.0.kill().unwrap();
    watcher.0.wait().unwrap();
    let mut reported = Vec::new();
    progress.read_to_end(&mut reported).unwrap();
    assert_eq!(reported, vec![b'x'; filler]);

    let mut watcher = Watch::start(scratch.command());
    append(&input, b"\n");
    watcher.wait_for(&db, |store| count(store) == 2);
    let rotated = scratch.0.join("spans.old");
    fs::rename(&input, &rotated).unwrap();
    data(&input, &[first, second, span(3, Some(2))]);
    watcher.wait_for(&db, |store| count(store) == 3);
    // Rewrite the existing inode to the same size. An offset-only watcher
    // would skip this new evidence even though the consumed prefix changed.
    let previous_length = fs::metadata(&input).unwrap().len();
    data(&input, &[span(4, None), span(5, Some(4)), span(6, Some(5))]);
    assert_eq!(fs::metadata(&input).unwrap().len(), previous_length);
    watcher.wait_for(&db, |store| count(store) == 6);
    let output = watcher.terminate();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("source changed, possible gap"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(PRIVATE));
    let reopened = Store::open_read_only(&db).unwrap();
    assert!(reopened.observations().unwrap().iter().any(|(_, event)| {
        matches!(&event.kind, ObservationKind::TraceSpan(span) if span.span_id == "0000000000000006" && span.parent_span_id.as_deref() == Some("0000000000000005"))
    }));
    assert!(
        !fs::read(&db)
            .unwrap()
            .windows(PRIVATE.len())
            .any(|bytes| bytes == PRIVATE.as_bytes())
    );
}

#[test]
fn malformed_batch_and_foreign_checkpoint_cannot_advance_history() {
    let scratch = Fixture::new();
    let input = scratch.input();
    let db = scratch.db();
    fs::write(&input, format!("{}\ninvalid {PRIVATE}\n", span(1, None))).unwrap();
    let failed = scratch.command().output().unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("line 2"));
    assert!(!String::from_utf8_lossy(&failed.stderr).contains(PRIVATE));
    let old = Store::open_read_only(&db).unwrap();
    assert_eq!(count(&old), 0);
    assert!(old.source_cursor(SOURCE).unwrap().is_none());
    drop(old);
    data(&input, &[span(1, None)]);
    let mut watcher = Watch::start(scratch.command());
    watcher.wait_for(&db, |store| count(store) == 1);
    assert!(watcher.terminate().status.success());

    let other = scratch.0.join("different.jsonl");
    data(&other, &[span(2, None)]);
    let foreign = watch_command(&other, &db).output().unwrap();
    assert!(!foreign.status.success());
    assert!(String::from_utf8_lossy(&foreign.stderr).contains("different input"));
    let other_scope = scratch
        .command()
        .args(["--session-id", "other-session"])
        .output()
        .unwrap();
    assert!(!other_scope.status.success());
    assert_eq!(count(&Store::open_read_only(&db).unwrap()), 1);
}

#[test]
fn input_cannot_alias_the_history_wal_lock_or_symlink() {
    let scratch = Fixture::new();
    let db = scratch.db();
    let input = scratch.input();
    for dangerous in [
        &db,
        &scratch.0.join("history.db-wal"),
        &scratch.0.join(".history.db.agenturm-writer.lock"),
    ] {
        let result = watch_command(dangerous, &db).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("separate from the history"));
    }
    assert!(!db.exists());
    let elsewhere = scratch.0.join("regular.jsonl");
    data(&elsewhere, &[span(1, None)]);
    symlink(&elsewhere, &input).unwrap();
    let result = scratch.command().output().unwrap();
    assert!(!result.status.success());
    assert!(!db.exists());
}

#[test]
fn opt_in_retains_bounded_payloads_across_follow_batches() {
    let scratch = Fixture::new();
    let db = scratch.db();
    let input = scratch.input();
    let secret = "bound producer text ".repeat(100);
    let rows = (1..=40)
        .map(|id| {
            let mut row = span(id, None);
            row["attributes"]["agenturm.payload.input"] = json!(secret);
            row
        })
        .collect::<Vec<_>>();
    data(&input, &rows);
    let mut command = scratch.command();
    command.args(["--payload-scope", "standalone"]);
    let mut watcher = Watch::start(command);
    watcher.wait_for(&db, |store| count(store) == 40);
    let output = watcher.terminate();
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&secret));
    let result = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(["query", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains(&secret));
    let selection = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args([
            "trace-payload",
            "snapshot",
            "--source-id",
            SOURCE,
            "--session-id",
            "fixture",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(selection.status.success());
    let snapshot: Value = serde_json::from_slice(&selection.stdout).unwrap();
    let selection = json!({"history_identity":snapshot["history_identity"], "snapshot_id":snapshot["snapshot_id"],
        "binding":{"source_id":SOURCE},"trace_id":TRACE,"span_id":"0000000000000028","session_id":"fixture"});
    let selection_path = scratch.0.join("selection.json");
    fs::write(&selection_path, selection.to_string()).unwrap();
    let grant = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args([
            "trace-payload",
            "grant",
            "--source-id",
            SOURCE,
            "--payload-scope",
            "standalone",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(grant.status.success());
    let preview = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(["trace-payload", "preview", "--selection"])
        .arg(&selection_path)
        .arg("--db")
        .arg(&db)
        .output()
        .unwrap();
    assert!(preview.status.success());
    let revealed: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(revealed["payload"]["input"], secret);
}
