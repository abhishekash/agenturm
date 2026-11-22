use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::{Store, WorkspaceAnchor, WorkspaceLedger};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn attention_cli_reopens_seen_state_and_watches_only_changes() {
    let scratch = Scratch::new();
    let database = scratch.0.join("history.sqlite");
    let workspace = scratch.0.join("history.sqlite.workspace.sqlite");
    let source_id = ExtensionId::parse("harness.codex").unwrap();
    let mut writer = Store::open(&database).unwrap();
    let pending = lifecycle(
        &source_id,
        "request",
        "paused",
        Some("codex_approval_request"),
        1,
    );
    ObservationStore::commit(&mut writer, source_id.as_str(), None, "first", &[pending]).unwrap();

    let status = snapshot(&database, &["status"]);
    assert_eq!(status["type"], "snapshot");
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["partial"], false);
    assert_eq!(
        status["pending"][0]["reason_code"],
        "codex_approval_request"
    );
    assert_eq!(status["pending"][0]["turn_id"], "turn");
    assert_eq!(status["pending"][0]["seen"], false);
    assert_eq!(snapshot(&database, &["watch", "--once"]), status);
    assert!(!workspace.exists());

    let mut watcher = attention(
        &database,
        &["watch", "--interval-ms", "100", "--max-polls", "20"],
    )
    .stdout(Stdio::piped())
    .spawn()
    .unwrap();
    let mut output = BufReader::new(watcher.stdout.take().unwrap());
    assert_eq!(next_json(&mut output), status);

    // Allow more than one unchanged poll before the first real update.
    std::thread::sleep(Duration::from_millis(220));
    let anchor = WorkspaceAnchor {
        harness_id: source_id.clone(),
        session_id: "session".into(),
        worker_id: None,
        turn_id: Some("turn".into()),
    };
    let ledger = WorkspaceLedger::open(&database).unwrap();
    ledger.mark_request_seen(&anchor, "request", 0).unwrap();
    let seen = next_json(&mut output);
    assert_eq!(seen["type"], "change");
    assert_eq!(seen["pending"][0]["seen"], true);
    assert_eq!(seen["pending"][0]["event_id"], "request");
    let reopened = snapshot(&database, &["status"]);
    assert_eq!(reopened["pending"], seen["pending"]);

    let completed = lifecycle(&source_id, "completed", "completed", None, 2);
    ObservationStore::commit(
        &mut writer,
        source_id.as_str(),
        Some("first"),
        "second",
        &[completed],
    )
    .unwrap();
    let changed = next_json(&mut output);
    assert_eq!(changed["type"], "change");
    assert!(changed["pending"].as_array().unwrap().is_empty());
    let mut tail = String::new();
    assert_eq!(output.read_line(&mut tail).unwrap(), 0);
    assert!(watcher.wait().unwrap().success());
    assert_eq!(
        snapshot(&database, &["status"])["pending"],
        changed["pending"]
    );
}

#[test]
fn attention_rejects_missing_history_without_creating_state() {
    let scratch = Scratch::new();
    let missing = scratch.0.join("absent.sqlite");
    let output = attention(&missing, &["status"]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
}

fn lifecycle(
    source: &ExtensionId,
    event: &str,
    state: &str,
    reason: Option<&str>,
    timestamp: u64,
) -> Observation {
    let mut identity = ObservationIdentity::session("session");
    identity.turn_id = Some("turn".into());
    Observation {
        harness_id: source.clone(),
        event_id: event.into(),
        revision: 0,
        identity,
        observed_at_unix_ns: timestamp,
        kind: ObservationKind::Lifecycle {
            state: state.into(),
            reason_code: reason.map(str::to_owned),
        },
    }
}

fn attention(database: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
    command
        .arg("attention")
        .args(args)
        .arg("--db")
        .arg(database);
    command
}

fn next_json(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    assert_ne!(
        reader.read_line(&mut line).unwrap(),
        0,
        "expected attention notification"
    );
    serde_json::from_str(&line).unwrap()
}

fn snapshot(database: &Path, args: &[&str]) -> Value {
    let output = attention(database, args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agenturm-attention-test-{}-{nonce}",
            std::process::id(),
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
