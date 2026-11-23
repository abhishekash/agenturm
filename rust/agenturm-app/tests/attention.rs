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

#[cfg(target_os = "linux")]
#[test]
fn desktop_attention_claims_only_fresh_exact_requests_once_across_processes() {
    use agenturm_kernel::native_review::{NativeRequest, NativeRequestKind, NativeRequestState};
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new();
    let database = scratch.0.join("history.sqlite");
    let out = scratch.0.join("notified");
    let notifier = scratch.0.join("notify-send");
    std::fs::write(&notifier, b"#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$AGENTURM_TEST_NOTIFICATION_LOG\"\nexit \"$AGENTURM_TEST_NOTIFY_STATUS\"\n").unwrap();
    std::fs::set_permissions(&notifier, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut writer = Store::open(&database).unwrap();
    let source = ExtensionId::parse("harness.codex").unwrap();
    // A raw page can be partial while its recent native-clock window is complete.
    let inactive = ExtensionId::parse("harness.inactive").unwrap();
    let mut inactive_cursor = None;
    for batch in 0..5 {
        let rows = (0..1000)
            .map(|row| Observation {
                harness_id: inactive.clone(),
                event_id: format!("old-{batch}-{row}"),
                revision: 0,
                identity: ObservationIdentity::session("inactive"),
                observed_at_unix_ns: 1,
                kind: ObservationKind::SessionStarted,
            })
            .collect::<Vec<_>>();
        let next = batch.to_string();
        ObservationStore::commit(
            &mut writer,
            inactive.as_str(),
            inactive_cursor.as_deref(),
            &next,
            &rows,
        )
        .unwrap();
        inactive_cursor = Some(next);
    }
    let mut cursor: Option<String> = None;
    let mut commit = |offset: u64, turn: &str, time: u64, corroborated: bool, state: &str| {
        let base = format!("codex:fixture:{offset}");
        let mut paused = lifecycle(
            &source,
            &format!("{base}:lifecycle"),
            state,
            (state == "paused").then_some("codex_approval_request"),
            time,
        );
        paused.identity.turn_id = Some(turn.into());
        let mut records = vec![paused.clone()];
        if corroborated {
            records.push(Observation {
                event_id: format!("{base}:native-request"),
                kind: ObservationKind::NativeRequest(NativeRequest {
                    request_id: format!("{offset:064x}"),
                    request_kind: NativeRequestKind::Approval,
                    state: NativeRequestState::Requested,
                }),
                ..paused
            });
        }
        let next = offset.to_string();
        ObservationStore::commit(
            &mut writer,
            source.as_str(),
            cursor.as_deref(),
            &next,
            &records,
        )
        .unwrap();
        cursor = Some(next);
    };
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    };
    let poll = |status: &str| {
        let output = attention(&database, &["watch", "--once", "--desktop"])
            .env("PATH", &scratch.0)
            .env("AGENTURM_TEST_NOTIFICATION_LOG", &out)
            .env("AGENTURM_TEST_NOTIFY_STATUS", status)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let state = scratch.0.join("history.sqlite.desktop-attention.sqlite");
    commit(1, "initial-turn", now(), true, "paused");
    assert_eq!(snapshot(&database, &["watch", "--once"])["partial"], true);
    assert!(!state.exists());
    assert!(poll("0")["desktop_delivery"].is_null());
    assert!(state.exists());
    assert_eq!(
        std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!out.exists());

    commit(2, "backfilled-turn", 1, true, "paused");
    assert!(poll("0")["desktop_delivery"].is_null());
    assert!(!out.exists());
    commit(3, "unverified-turn", now(), false, "paused");
    assert_eq!(poll("0")["desktop_delivery"], "deferred_source_clock");
    assert!(!out.exists());
    commit(4, "unverified-turn", now(), false, "completed");
    commit(5, "actual-turn", now(), true, "paused");
    assert_eq!(poll("0")["desktop_delivery"], "delivered");
    assert!(poll("0")["desktop_delivery"].is_null());
    commit(6, "seen-turn", now(), true, "paused");
    let ledger = WorkspaceLedger::open(&database).unwrap();
    let anchor = WorkspaceAnchor {
        harness_id: source.clone(),
        session_id: "session".into(),
        worker_id: None,
        turn_id: Some("seen-turn".into()),
    };
    assert!(
        ledger
            .mark_request_seen(&anchor, "codex:fixture:6:lifecycle", 0)
            .unwrap()
    );
    assert!(poll("0")["desktop_delivery"].is_null());
    commit(7, "failed-delivery-turn", now(), true, "paused");
    assert_eq!(poll("1")["desktop_delivery"], "failed");
    assert!(poll("0")["desktop_delivery"].is_null());
    let output = std::fs::read_to_string(out).unwrap();
    assert_eq!(output.lines().count(), 2);
    for line in output.lines() {
        assert_eq!(
            line,
            "--app-name=Agenturm -- Agenturm attention 1 recorded attention request(s). Open Agenturm to inspect and respond in your agent terminal."
        );
    }
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
