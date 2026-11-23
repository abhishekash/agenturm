use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE: &str = "otlp.branch";

#[test]
fn cli_rejects_a_pre_restore_cursor_after_the_same_watermark_regrows() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let bundle = scratch.0.join("bundle");
    let restored = scratch.0.join("restored");

    import(&scratch.0, &db, "1111111111111111", 100);
    check(run(&["backup", "--db", arg(&db), "--bundle", arg(&bundle)]));
    import(&scratch.0, &db, "2222222222222222", 200);
    let prior = page(&db);
    let old = &prior["next_before"];
    assert_eq!(old["snapshot_id"], "2");
    assert!(older(&db, old).status.success());

    check(run(&[
        "restore",
        "--bundle",
        arg(&bundle),
        "--output",
        arg(&restored),
    ]));
    let restored = restored.join("history.sqlite");
    import(&scratch.0, &restored, "3333333333333333", 300);
    let regrown = page(&restored);
    let fresh = &regrown["next_before"];
    let snapshot_id = fresh["snapshot_id"].as_str().unwrap();
    assert!(snapshot_id.starts_with("r1:"));
    assert!(snapshot_id.ends_with(":2"));
    assert_ne!(fresh["snapshot_id"], old["snapshot_id"]);
    let denied = older(&restored, old);
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&denied.stderr)
            .contains("history snapshot belongs to a restored or replaced history")
    );
    let older_page: Value = serde_json::from_str(&check(older(&restored, fresh))).unwrap();
    assert_eq!(
        older_page["observations"][0]["observation"]["span_id"],
        "1111111111111111"
    );
}

fn import(root: &Path, db: &Path, span: &str, time: u32) {
    let input = root.join("spans.jsonl");
    std::fs::write(
        &input,
        json!({
            "trace_id": "33333333333333333333333333333333", "span_id": span,
            "start_unix_nano": time, "end_unix_nano": time + 1,
            "attributes": {"session.id": "fixture-session"},
        })
        .to_string(),
    )
    .unwrap();
    check(run(&[
        "import-jsonl",
        "--db",
        arg(db),
        "--source-id",
        SOURCE,
        "--file",
        arg(&input),
    ]));
}

fn page(db: &Path) -> Value {
    serde_json::from_str(&check(run(&[
        "query",
        "--db",
        arg(db),
        "--harness-id",
        SOURCE,
        "--limit",
        "1",
    ])))
    .unwrap()
}

fn older(db: &Path, cursor: &Value) -> Output {
    run(&[
        "query",
        "--db",
        arg(db),
        "--harness-id",
        SOURCE,
        "--limit",
        "1",
        "--snapshot-id",
        cursor["snapshot_id"].as_str().unwrap(),
        "--before-timestamp-ns",
        cursor["observed_at_unix_ns"].as_str().unwrap(),
        "--before-harness-id",
        cursor["harness_id"].as_str().unwrap(),
        "--before-event-id",
        cursor["event_id"].as_str().unwrap(),
        "--before-revision",
        &cursor["revision"].as_u64().unwrap().to_string(),
    ])
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(args)
        .output()
        .unwrap()
}

fn check(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("agenturm-read-incarnation-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
