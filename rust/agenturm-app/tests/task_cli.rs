use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::{Store, WorkspaceAnchor, WorkspaceLedger};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn tasks_share_the_cockpit_ledger_and_validate_exact_history_links() {
    let scratch = Scratch::new();
    let database = scratch.0.join("history.sqlite");
    let mut writer = Store::open(&database).unwrap();
    let codex = seed(&mut writer, "harness.codex");
    let pi = seed(&mut writer, "harness.pi");
    let ledger = WorkspaceLedger::open(&database).unwrap();

    let empty = json(&database, &["list"]);
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);
    assert!(!scratch.0.join("history.sqlite.workspace.sqlite").exists());

    let initial = ledger.create_task(Some(&pi), "Cockpit task", "").unwrap();
    let cli = json(
        &database,
        &["add", "--title", "CLI task", "--notes", "Operator note"],
    );
    let task_id = cli["id"].as_u64().unwrap();
    let id = task_id.to_string();
    assert_eq!(cli["notes"], "Operator note");
    assert!(cli["anchor"].is_null());

    let newest = json(&database, &["list", "--limit", "1"]);
    assert_eq!(newest["items"][0], cli);
    assert_eq!(newest["next_before"], task_id);
    let older = json(&database, &["list", "--before", &id]);
    assert_eq!(older["items"][0]["id"], initial.id);
    assert_eq!(older["items"][0]["anchor"]["source_id"], "harness.pi");
    assert!(older["next_before"].is_null());

    let wrong_source = [
        "link",
        &id,
        "--source-id",
        "harness.unknown",
        "--session-id",
        "session",
    ];
    assert_failure(run(&database, &wrong_source));
    let exact = [
        &id,
        "--source-id",
        "harness.codex",
        "--session-id",
        "session",
        "--turn-id",
        "turn",
        "--worker-id",
        "worker",
    ];
    assert_eq!(
        json(&database, &[&["link"], exact.as_slice()].concat())["changed"],
        true
    );
    assert_eq!(
        json(&database, &[&["link"], exact.as_slice()].concat())["changed"],
        false
    );
    assert_eq!(ledger.task_links(task_id, 10).unwrap(), vec![codex.clone()]);

    let shown = json(&database, &["show", &id]);
    assert_eq!(shown["task"]["anchor"], shown["links"][0]);
    assert_eq!(shown["links"][0]["worker_id"], "worker");
    assert_eq!(shown["links"][0]["turn_id"], "turn");
    assert!(shown["git_snapshot"].is_null());
    assert_eq!(json(&database, &["complete", &id])["completed"], true);
    assert!(ledger.list_tasks(1, None).unwrap().items[0].completed);
    assert_eq!(json(&database, &["reopen", &id])["completed"], false);
    assert!(!ledger.list_tasks(1, None).unwrap().items[0].completed);
    assert_eq!(
        json(&database, &[&["unlink"], exact.as_slice()].concat())["changed"],
        true
    );
    assert!(ledger.task_links(task_id, 10).unwrap().is_empty());
    assert!(json(&database, &["show", &id])["task"]["anchor"].is_null());

    let handoff = ledger
        .create_handoff(&codex, &pi, "Continue <this>")
        .unwrap();
    let entries = json(&database, &["handoffs"]);
    assert_eq!(entries["items"][0]["id"], handoff.id);
    assert_eq!(entries["items"][0]["from"]["source_id"], "harness.codex");
    assert_eq!(entries["items"][0]["to"]["source_id"], "harness.pi");
    let exported = json(&database, &["export", &handoff.id.to_string()]);
    let markdown = Path::new(exported["markdown"].as_str().unwrap());
    let html = Path::new(exported["html"].as_str().unwrap());
    assert!(markdown.starts_with(database.parent().unwrap().join("handoffs")));
    assert!(
        std::fs::read_to_string(markdown)
            .unwrap()
            .contains("Continue <this>")
    );
    assert!(
        std::fs::read_to_string(html)
            .unwrap()
            .contains("Continue &lt;this&gt;")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(html).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_failure(run(&database, &["show", "999"]));
    assert_failure(run(&database, &["export", "999"]));
}

#[test]
fn tasks_refuse_missing_or_unrelated_history_without_creating_workspace() {
    let scratch = Scratch::new();
    let missing = scratch.0.join("missing.sqlite");
    assert_failure(run(&missing, &["list"]));
    assert_failure(run(&missing, &["add", "--title", "New task"]));
    let foreign = scratch.0.join("foreign.sqlite");
    std::fs::write(&foreign, "not an Agenturm database").unwrap();
    assert_failure(run(&foreign, &["add", "--title", "New task"]));
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);

    let help = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(["task", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("agenturm task add")
    );
}

fn seed(writer: &mut Store, source: &str) -> WorkspaceAnchor {
    let harness_id = ExtensionId::parse(source).unwrap();
    let mut identity = ObservationIdentity::session("session");
    identity.worker_id = Some("worker".into());
    identity.turn_id = Some("turn".into());
    ObservationStore::commit(
        writer,
        source,
        None,
        "first",
        &[Observation {
            harness_id: harness_id.clone(),
            event_id: "turn-start".into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 1,
            kind: ObservationKind::Lifecycle {
                state: "running".into(),
                reason_code: None,
            },
        }],
    )
    .unwrap();
    WorkspaceAnchor {
        harness_id,
        session_id: "session".into(),
        worker_id: Some("worker".into()),
        turn_id: Some("turn".into()),
    }
}

fn run(database: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .arg("task")
        .args(args)
        .arg("--db")
        .arg(database)
        .output()
        .unwrap()
}

fn json(database: &Path, args: &[&str]) -> Value {
    let output = run(database, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agenturm-task-cli-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
