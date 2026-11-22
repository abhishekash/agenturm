#![cfg(target_os = "linux")]

use agenturm_store::{Store, WorkspaceLedger};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn cli_workbench_backup_and_restore_leave_history_only_backup_available() {
    let scratch = Scratch::new();
    let db = scratch.0.join("live.sqlite");
    let _collector = Store::open_collector(&db).unwrap();
    let task = WorkspaceLedger::open(&db)
        .unwrap()
        .create_task(None, "Saved follow-up", "Workspace note")
        .unwrap();
    let standalone = scratch.0.join("standalone.sqlite");
    checked(run([
        "backup",
        "--db",
        path(&db),
        "--output",
        path(&standalone),
    ]));
    assert!(Store::open_read_only(&standalone).is_ok());
    assert!(
        WorkspaceLedger::open(&standalone)
            .unwrap()
            .list_tasks(1, None)
            .unwrap()
            .items
            .is_empty()
    );

    let bundle = scratch.0.join("bundle");
    let restored = scratch.0.join("restored");
    checked(run([
        "backup",
        "--bundle",
        path(&bundle),
        "--db",
        path(&db),
    ]));
    let output = checked(run([
        "restore",
        "--output",
        path(&restored),
        "--bundle",
        path(&bundle),
    ]));
    let restored_db = restored.join("history.sqlite");
    assert!(output.contains(path(&restored_db)));
    assert_eq!(
        WorkspaceLedger::open(&restored_db)
            .unwrap()
            .list_tasks(1, None)
            .unwrap()
            .items,
        [task],
    );
    let conflict = run([
        "restore",
        "--bundle",
        path(&bundle),
        "--output",
        path(&restored),
    ]);
    assert!(!conflict.status.success());
    assert_eq!(
        run([
            "backup",
            "--db",
            path(&db),
            "--output",
            path(&standalone),
            "--bundle",
            path(&bundle)
        ])
        .status
        .code(),
        Some(2)
    );
}

fn run<const N: usize>(args: [&str; N]) -> Output {
    let binary = std::env::var_os("AGENTURM_TEST_WORKSPACE_BACKUP_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_agenturm").into());
    Command::new(binary).args(args).output().unwrap()
}

fn checked(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agenturm-workspace-backup-{}-{unique}",
            std::process::id()
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
