use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::Store;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn one_shot_summary_keeps_source_scope_unknown_state_and_page_coverage() {
    let root = Scratch::new();
    let db = root.0.join("history.sqlite");
    let mut store = Store::open(&db).unwrap();
    let row = |source: &str, event: &str, time, kind| Observation {
        harness_id: ExtensionId::parse(source).unwrap(),
        event_id: event.into(),
        revision: 0,
        identity: ObservationIdentity::session("same-session"),
        observed_at_unix_ns: time,
        kind,
    };
    let mut interrupted = row(
        "harness.codex",
        "ended",
        2,
        ObservationKind::Lifecycle {
            state: "interrupted".into(),
            reason_code: None,
        },
    );
    interrupted.identity.turn_id = Some("turn-one".into());
    store
        .commit(
            "harness.codex",
            None,
            "one",
            &[
                row("harness.codex", "start", 1, ObservationKind::SessionStarted),
                interrupted,
            ],
        )
        .unwrap();
    store
        .commit(
            "harness.pi",
            None,
            "one",
            &[row(
                "harness.pi",
                "start",
                3,
                ObservationKind::SessionStarted,
            )],
        )
        .unwrap();
    drop(store);
    let before = std::fs::read(&db).unwrap();
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args(args)
            .arg("--db")
            .arg(&db)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let json: Value = serde_json::from_str(&invoke(&["cockpit", "--once", "--json"])).unwrap();
    assert_eq!(json["format"], "agenturm-summary/1");
    assert_eq!(json["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(json["sessions"][0]["harness_id"], "harness.pi");
    assert!(json["sessions"][0]["lifecycle"].is_null());
    assert_eq!(json["sessions"][1]["harness_id"], "harness.codex");
    assert_eq!(json["sessions"][1]["lifecycle"]["state"], "interrupted");
    assert_eq!(json["has_more"], false);
    let page: Value =
        serde_json::from_str(&invoke(&["cockpit", "--once", "--json", "--limit", "1"])).unwrap();
    assert_eq!(page["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(page["has_more"], true);
    assert!(page["next_before"].is_object());
    let text = invoke(&["--once"]);
    assert!(text.contains("harness.pi / same-session · unknown"));
    assert!(text.contains("harness.codex / same-session · interrupted"));
    assert!(!text.contains('\u{1b}'));
    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert!(!db.with_extension("sqlite.workspace.sqlite").exists());
}

#[test]
fn invalid_summary_flags_fail_before_history_creation() {
    let root = Scratch::new();
    let db = root.0.join("absent.sqlite");
    for args in [
        vec!["cockpit", "--once", "--limit", "0"],
        vec!["cockpit", "--once", "--limit", "257"],
        vec!["cockpit", "--once", "--once"],
        vec!["cockpit", "--once", "--json", "--json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args(args)
            .arg("--db")
            .arg(&db)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!db.exists());
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("agenturm-summary-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
