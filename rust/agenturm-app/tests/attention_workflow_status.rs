//! Process checks for the local, read-only Workflow attention report.

use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationQuery,
    ObservationStore,
    native_review::{NativeRequest, NativeRequestKind, NativeRequestState},
    workflow_notifications::{
        WorkflowDeliveryState, WorkflowNotificationAuthority, WorkflowPolicyChange,
        WorkflowThresholds,
    },
};
use agenturm_store::{
    DesktopAttentionRequest, Store, WorkflowNotificationRead, WorkspaceAnchor, WorkspaceLedger,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
#[test]
fn workflow_status_binds_current_incidents_and_durable_exact_request_receipts() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let workspace = scratch.0.join("history.sqlite.workspace.sqlite");
    let delivery_db = scratch.0.join("history.sqlite.desktop-attention.sqlite");
    let mut writer = Store::open(&db).unwrap();
    let ledger = WorkspaceLedger::open(&db).unwrap();
    let source = ExtensionId::parse("harness.codex").unwrap();
    let sibling = ExtensionId::parse("harness.sibling").unwrap();
    let empty = authority(&writer);
    let policy = ledger
        .change_workflow_notification_policy(
            &empty,
            0,
            1,
            WorkflowPolicyChange::SetDesktop(true),
            || Ok(()),
        )
        .unwrap();
    let poll = |authority, clock| WorkflowNotificationRead {
        authority,
        clock_unix_ns: clock,
        policy_revision: policy.revision,
        desktop_opt_in: false,
    };
    assert!(
        ledger
            .claim_workflow_delivery(poll(&empty, 1), &[], &[], None)
            .unwrap()
            .is_empty()
    );
    let paused = |source: &ExtensionId, session| {
        lifecycle(
            source,
            session,
            "codex:fixture:2:lifecycle",
            "paused",
            Some("codex_approval_request"),
            2,
        )
    };
    let main_request = paused(&source, "session");
    let unrelated_reason = "excluded_native_failure_reason";
    let failure = lifecycle(
        &source,
        "summary",
        "fault",
        "failed",
        Some(unrelated_reason),
        3,
    );
    let comparison = lifecycle(&source, "complete", "done", "completed", None, 4);
    ObservationStore::commit(
        &mut writer,
        source.as_str(),
        None,
        "main",
        &[
            main_request.clone(),
            Observation {
                event_id: "codex:fixture:2:native-request".into(),
                kind: ObservationKind::NativeRequest(NativeRequest {
                    request_id: "a".repeat(64),
                    request_kind: NativeRequestKind::Approval,
                    state: NativeRequestState::Requested,
                }),
                ..main_request.clone()
            },
            failure,
            comparison,
        ],
    )
    .unwrap();
    ObservationStore::commit(
        &mut writer,
        sibling.as_str(),
        None,
        "peer",
        &[paused(&sibling, "session")],
    )
    .unwrap();
    let current = authority(&writer);
    let request = DesktopAttentionRequest {
        anchor: WorkspaceAnchor {
            harness_id: source,
            session_id: "session".into(),
            turn_id: Some("turn".into()),
            worker_id: None,
        },
        event_id: main_request.event_id,
        revision: 0,
        observed_at_unix_ns: 2,
    };
    let claims = ledger
        .claim_workflow_delivery(poll(&current, 4), &[request], &[], None)
        .unwrap();
    assert_eq!(claims.len(), 1);
    ledger
        .finish_workflow_delivery(claims, WorkflowDeliveryState::Failed)
        .unwrap();
    let policy = ledger
        .change_workflow_notification_policy(
            &current,
            policy.revision,
            5,
            WorkflowPolicyChange::SetThresholds(WorkflowThresholds {
                tokens: 9_007_199_254_740_993,
                usd_nanos: 125_000_000,
                duration_ns: 0,
            }),
            || Ok(()),
        )
        .unwrap();
    let before = [fs::read(&db).unwrap(), fs::read(&delivery_db).unwrap()];
    let marker = scratch.0.join("unexpected-notifier");
    let notifier = scratch.0.join("notify-send");
    fs::write(&notifier, b"#!/bin/sh\n: > \"$UNEXPECTED_NOTIFIER\"\n").unwrap();
    fs::set_permissions(&notifier, fs::Permissions::from_mode(0o700)).unwrap();
    let run = |args: &[&str]| {
        let output = command(&db, args)
            .env("PATH", &scratch.0)
            .env("UNEXPECTED_NOTIFIER", &marker)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(unrelated_reason));
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let status = run(&["status", "--workflow"]);
    assert_eq!(status, run(&["watch", "--once", "--workflow"]));
    assert_eq!(status["type"], "snapshot");
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["pending"].as_array().unwrap().len(), 2);
    assert!(status.get("desktop_delivery").is_none());
    assert!(run(&["status"]).get("workflow").is_none());
    let view = &status["workflow"];
    assert_eq!(view["history_identity"], current.history_identity);
    assert_eq!(view["snapshot_id"], current.snapshot_id.to_string());
    assert_eq!(view["delivery_scope"], "matched_current_evidence");
    assert_eq!(view["policy"]["revision"], policy.revision.to_string());
    assert_eq!(view["policy"]["desktop_enabled"], true);
    assert_eq!(view["policy"]["token_threshold"], "9007199254740993");
    let receipts = view["request_deliveries"].as_array().unwrap();
    let [receipt] = receipts.as_slice() else {
        panic!("Expected one exact receipt");
    };
    assert_eq!(receipt["source_id"], "harness.codex");
    assert_eq!(receipt["event_id"], "codex:fixture:2:lifecycle");
    assert_eq!(receipt["state"], "failed");
    assert_eq!(receipt["attempts"], "1");
    let incidents = view["incidents"].as_array().unwrap();
    assert!(
        incidents
            .iter()
            .any(|row| row["kind"] == "failure" && row["evidence"]["event_id"] == "fault")
    );
    assert!(
        incidents
            .iter()
            .any(|row| row["kind"] == "completion" && row["quiet"] == true)
    );
    assert!(!marker.exists());
    assert!(!workspace.exists());
    assert_eq!(
        before,
        [fs::read(&db).unwrap(), fs::read(&delivery_db).unwrap()]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn workflow_status_keeps_an_exact_incident_delivery() {
    use agenturm_kernel::native_attention::{
        NativeAttentionCapture, NativeAttentionKind, NativeAttentionScope,
    };
    use std::time::Duration;

    let scratch = Scratch::new();
    let db = scratch.0.join("incidents.sqlite");
    let mut writer = Store::open(&db).unwrap();
    let ledger = WorkspaceLedger::open(&db).unwrap();
    let source = ExtensionId::parse("harness.codex").unwrap();
    let bridge = ExtensionId::parse("otlp.bridge").unwrap();
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    };
    let running = lifecycle(&source, "session", "running", "running", None, now());
    ObservationStore::commit(&mut writer, source.as_str(), None, "native", &[running]).unwrap();
    let point = authority(&writer);
    let policy = ledger
        .change_workflow_notification_policy(
            &point,
            0,
            now(),
            WorkflowPolicyChange::SetThresholds(WorkflowThresholds {
                duration_ns: 2_000_000_000,
                ..WorkflowThresholds::default()
            }),
            || Ok(()),
        )
        .unwrap();
    ledger
        .change_workflow_notification_policy(
            &point,
            policy.revision,
            now(),
            WorkflowPolicyChange::SetDesktop(true),
            || Ok(()),
        )
        .unwrap();
    let poll = || {
        let output = command(&db, &["watch", "--once", "--desktop"])
            .env("PATH", &scratch.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert!(poll().get("desktop_delivery").is_none());
    writer
        .admit_native_attention(
            &bridge,
            &NativeAttentionCapture {
                history_binding: point.history_identity,
                scope: NativeAttentionScope {
                    source_id: source.as_str().into(),
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    worker_id: None,
                },
                sender_id: "1".repeat(32),
                event_id: "start".into(),
                sequence: 1,
                kind: NativeAttentionKind::TurnStarted,
                request_id: None,
                source_time_unix_ns: None,
            },
            None,
        )
        .unwrap();
    assert!(poll().get("desktop_delivery").is_none());
    std::thread::sleep(Duration::from_millis(2_025));
    assert_eq!(poll()["desktop_delivery"], "unavailable");
    let state_path = scratch.0.join("incidents.sqlite.desktop-attention.sqlite");
    let before = fs::read(&state_path).unwrap();
    let output = invoke(&db, &["status", "--workflow"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let [elapsed] = report["workflow"]["incidents"]
        .as_array()
        .unwrap()
        .as_slice()
    else {
        panic!("Expected one retained elapsed incident");
    };
    assert_eq!(elapsed["kind"], "elapsed");
    assert_eq!(elapsed["evidence"]["clock"], "receiver_receipt");
    assert_eq!(elapsed["scope"]["turn_id"], "turn");
    assert_eq!(
        elapsed["delivery"],
        serde_json::json!({"state":"unavailable","attempts":"1"})
    );
    assert_eq!(fs::read(state_path).unwrap(), before);
}

#[test]
fn fresh_workflow_report_leaves_state_absent_and_rejects_delivery_flags() {
    let scratch = Scratch::new();
    let db = scratch.0.join("fresh.sqlite");
    let writer = Store::open(&db).unwrap();
    let identity = writer.history_identity().unwrap();
    let output = invoke(&db, &["status", "--workflow"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let view = &report["workflow"];
    assert_eq!(view["history_identity"], identity);
    assert_eq!(view["policy"]["desktop_enabled"], Value::Null);
    assert_eq!(view["policy"]["revision"], "0");
    assert_eq!(view["incidents"], serde_json::json!([]));
    assert_eq!(view["request_deliveries"], serde_json::json!([]));
    for flags in [
        &["watch", "--workflow"][..],
        &["watch", "--workflow", "--once", "--desktop"],
        &["status", "--workflow", "--workflow"],
    ] {
        let output = invoke(&db, flags);
        assert!(!output.status.success() && output.stdout.is_empty());
    }
    assert!(
        !scratch
            .0
            .join("fresh.sqlite.desktop-attention.sqlite")
            .exists()
    );
    assert!(!scratch.0.join("fresh.sqlite.workspace.sqlite").exists());
}

fn authority(store: &Store) -> WorkflowNotificationAuthority {
    WorkflowNotificationAuthority {
        history_identity: store.history_identity().unwrap(),
        snapshot_id: store
            .query_observations(&ObservationQuery::latest(1))
            .unwrap()
            .snapshot_id,
    }
}

fn lifecycle(
    source: &ExtensionId,
    session: &str,
    id: &str,
    state: &str,
    reason: Option<&str>,
    stamp: u64,
) -> Observation {
    let mut identity = ObservationIdentity::session(session);
    identity.turn_id = Some("turn".into());
    Observation {
        harness_id: source.clone(),
        event_id: id.into(),
        revision: 0,
        identity,
        observed_at_unix_ns: stamp,
        kind: ObservationKind::Lifecycle {
            state: state.into(),
            reason_code: reason.map(str::to_owned),
        },
    }
}

fn command(db: &Path, flags: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agenturm"));
    command.arg("attention").args(flags).arg("--db").arg(db);
    command
}

fn invoke(db: &Path, flags: &[&str]) -> Output {
    command(db, flags).output().unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-workflow-status-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
