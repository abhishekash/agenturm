use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::Store;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};

#[test]
fn receipt_preserves_the_exact_scope_and_private_destinations() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let destination = scratch.0.join("receipt");
    let mut store = Store::open(&db).unwrap();
    let codex = ExtensionId::parse("harness.codex").unwrap();
    let pi = ExtensionId::parse("harness.pi").unwrap();
    let row = |source: &ExtensionId,
               event: &str,
               timestamp,
               session: &str,
               turn,
               worker: Option<&str>| {
        let mut identity = ObservationIdentity::session(session);
        identity.turn_id = Some(turn);
        identity.worker_id = worker.map(str::to_owned);
        Observation {
            harness_id: source.clone(),
            event_id: event.into(),
            revision: 0,
            identity,
            observed_at_unix_ns: timestamp,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        }
    };
    for (source, rows) in [
        (
            &codex,
            vec![
                row(&codex, "root-older", 1, "session", "turn".into(), None),
                row(&codex, "root-newer", 2, "session", "turn".into(), None),
                row(
                    &codex,
                    "other-worker",
                    3,
                    "session",
                    "turn".into(),
                    Some("worker"),
                ),
                row(&codex, "other-session", 4, "sibling", "turn".into(), None),
                row(&codex, "other-turn", 5, "session", "sibling".into(), None),
            ],
        ),
        (
            &pi,
            vec![row(&pi, "other-source", 6, "session", "turn".into(), None)],
        ),
    ] {
        store
            .commit(source.as_str(), None, "loaded", &rows)
            .unwrap();
    }
    drop(store);
    let review = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args([
                "review",
                "--harness-id",
                "harness.codex",
                "--session-id",
                "session",
                "--turn-id",
                "turn",
                "--limit",
                "1",
            ])
            .arg("--db")
            .arg(&db)
            .args(extra)
            .output()
            .unwrap()
    };
    let read_json = |output: Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let legacy = read_json(review(&[]));
    assert_eq!(legacy.as_object().unwrap().len(), 3);
    let directory = destination.to_str().unwrap();
    let receipt = read_json(review(&["--format", "json", "--output", directory]));
    for key in ["observations", "has_more", "next_before"] {
        assert_eq!(receipt[key], legacy[key]);
    }
    assert_eq!(receipt["format"], "agenturm-review-receipt/1");
    assert_eq!(
        receipt["scope"],
        json!({
            "harness_id":"harness.codex",
            "session_id":"session",
            "turn_id":"turn",
            "worker_id":null,
        })
    );
    assert_eq!(
        receipt["snapshot_id"],
        receipt["next_before"]["snapshot_id"]
    );
    assert_eq!(receipt["observations"][0]["event_id"], "root-newer");
    assert_eq!(
        receipt["observations"][0]["observation"],
        json!({"kind":"message","role":"user"})
    );
    let saved_json = std::fs::read(destination.join("review.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&saved_json).unwrap(),
        receipt
    );
    let markdown = std::fs::read_to_string(destination.join("review.md")).unwrap();
    assert!(markdown.contains("# Review receipt"));
    assert!(markdown.contains("Worker   Workerless"));
    assert!(markdown.contains("## Older evidence cursor"));
    let html = std::fs::read_to_string(destination.join("review.html")).unwrap();
    assert!(html.contains("&quot;event_id&quot;: &quot;root-newer&quot;"));
    assert!(!review(&["--output", directory]).status.success());
    assert_eq!(
        std::fs::read(destination.join("review.json")).unwrap(),
        saved_json
    );

    let worker = read_json(review(&["--format", "json", "--worker-id", "worker"]));
    assert_eq!(worker["scope"]["worker_id"], "worker");
    assert_eq!(worker["observations"][0]["event_id"], "other-worker");
    assert_eq!(worker["has_more"], false);
    assert!(worker["snapshot_id"].is_string());
    assert!(!review(&["--format", "unsupported"]).status.success());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in ["review.json", "review.md", "review.html"] {
            assert_eq!(
                std::fs::metadata(destination.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("agenturm-review-receipt-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
