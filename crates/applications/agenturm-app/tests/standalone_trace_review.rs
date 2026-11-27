use agenturm_store::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const TRACE: &str = "11111111111111111111111111111111";
const OTHER_TRACE: &str = "22222222222222222222222222222222";
const SOURCE: &str = "otlp.standalone-fixture";
const SESSION: &str = "shared-session";
const PRIVATE: &str = "private-producer-text-should-not-be-in-review";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "agenturm-standalone-trace-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn import(&self, db: &Path, source: &str, spans: &[Value]) {
        let input = self.0.join("trace.json");
        std::fs::write(
            &input,
            json!({"resourceSpans":[{"scopeSpans":[{"spans":spans}]}]}).to_string(),
        )
        .unwrap();
        let result = run(
            db,
            &[
                "import-traces",
                "--file",
                input.to_str().unwrap(),
                "--source-id",
                source,
            ],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(args)
        .arg("--db")
        .arg(db)
        .output()
        .unwrap()
}

fn page(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(PRIVATE));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn review(db: &Path, trace_id: &str, options: &[&str]) -> Output {
    let mut args = vec![
        "review",
        "--trace-id",
        trace_id,
        "--trace-source-id",
        SOURCE,
        "--session-id",
        SESSION,
        "--format",
        "json",
    ];
    args.extend(options);
    run(db, &args)
}

fn span(trace_id: &str, id: u64, session: &str, worker: Option<&str>) -> Value {
    let mut attributes = vec![
        json!({"key":"session.id","value":{"stringValue":session}}),
        json!({"key":"gen_ai.input.messages","value":{"stringValue":PRIVATE}}),
    ];
    if id == 1 {
        attributes.extend([
            json!({"key":"agenturm.reported_cost.amount","value":{"stringValue":"0.000000000"}}),
            json!({"key":"agenturm.reported_cost.currency","value":{"stringValue":"USD"}}),
            json!({"key":"gen_ai.usage.input_tokens","value":{"intValue":"0"}}),
        ]);
    }
    if let Some(worker) = worker {
        attributes.extend([
            json!({"key":"agenturm.source.id","value":{"stringValue":"harness.unretained"}}),
            json!({"key":"agenturm.turn.id","value":{"stringValue":"recorded-turn"}}),
            json!({"key":"agenturm.worker.id","value":{"stringValue":worker}}),
        ]);
    }
    json!({"traceId":trace_id,"spanId":format!("{id:016x}"),"name":"agent.run",
        "status":{"code":2,"message":PRIVATE},"startTimeUnixNano":"1",
        "endTimeUnixNano":(10+id).to_string(),"attributes":attributes})
}

fn span_ids(receipt: &Value) -> Vec<&str> {
    receipt["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            let observation = &row["observation"];
            (observation["kind"] == "trace_span").then(|| observation["span_id"].as_str().unwrap())
        })
        .collect()
}

#[test]
fn standalone_review_is_exact_and_keeps_snapshot_across_pages_and_restore() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.db");
    scratch.import(
        &db,
        SOURCE,
        &[
            span(TRACE, 1, SESSION, None),
            span(TRACE, 2, SESSION, Some("worker/linked")),
            span(TRACE, 3, "sibling-session", None),
            span(OTHER_TRACE, 4, SESSION, None),
            span(OTHER_TRACE, 5, SESSION, None),
        ],
    );
    scratch.import(&db, "otlp.sibling", &[span(TRACE, 1, SESSION, None)]);
    let all = page(review(&db, TRACE, &[]));
    assert_eq!(all["scope"]["source_id"], SOURCE);
    assert_eq!(all["scope"]["trace_id"], TRACE);
    assert_eq!(all["scope"]["session_id"], SESSION);
    assert!(all["scope"]["turn_id"].is_null());
    assert_eq!(all["scope"]["worker_scope"], "all_recorded");
    assert_eq!(span_ids(&all), ["0000000000000001", "0000000000000002"]);
    let metadata = all["observations"].as_array().unwrap();
    assert!(metadata.iter().all(|row| row["harness_id"] == SOURCE));
    for kind in ["trace_span_summary", "span_token_usage", "reported_cost"] {
        assert!(
            metadata
                .iter()
                .any(|row| row["observation"]["kind"] == kind),
            "missing {kind}"
        );
    }

    let linked = page(review(&db, TRACE, &["--worker-id", "worker/linked"]));
    assert_eq!(linked["scope"]["worker_scope"], "exact");
    assert_eq!(span_ids(&linked), ["0000000000000002"]);
    assert!(
        linked["observations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| {
                row["identity"]["worker_id"] == "worker/linked"
                    && row["identity"]["turn_id"] == "recorded-turn"
            })
    );
    let first = page(review(&db, TRACE, &["--limit", "2"]));
    let unrelated = page(review(&db, OTHER_TRACE, &["--limit", "1"]));
    assert_eq!(first["snapshot_id"], unrelated["snapshot_id"]);
    assert_eq!(first["has_more"], true);
    let pinned = first["snapshot_id"].as_str().unwrap();
    let continuation = |target: &Path, page: &Value| {
        let cursor = &page["next_before"];
        let time = cursor["observed_at_unix_ns"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| cursor["observed_at_unix_ns"].to_string());
        review(
            target,
            TRACE,
            &[
                "--snapshot-id",
                pinned,
                "--before-timestamp-ns",
                &time,
                "--before-harness-id",
                cursor["harness_id"].as_str().unwrap(),
                "--before-event-id",
                cursor["event_id"].as_str().unwrap(),
                "--before-revision",
                &cursor["revision"].to_string(),
            ],
        )
    };
    scratch.import(&db, SOURCE, &[span(TRACE, 6, SESSION, None)]);
    let older = page(continuation(&db, &first));
    assert_eq!(older["snapshot_id"], pinned);
    assert_eq!(span_ids(&older), ["0000000000000001"]);
    assert!(!continuation(&db, &unrelated).status.success());
    assert!(
        page(review(&db, TRACE, &[]))["observations"]
            .as_array()
            .unwrap()
            .len()
            > metadata.len()
    );

    let backup = scratch.0.join("backup.db");
    Store::open_read_only(&db)
        .unwrap()
        .backup_to(&backup)
        .unwrap();
    assert!(!continuation(&backup, &first).status.success());
    assert!(
        !review(&backup, TRACE, &["--snapshot-id", pinned])
            .status
            .success()
    );
    assert_eq!(
        span_ids(&page(review(&backup, TRACE, &[]))),
        ["0000000000000001", "0000000000000002", "0000000000000006",]
    );
    assert!(
        !review(&db, TRACE, &["--turn-id", "fake-turn"])
            .status
            .success()
    );
    assert!(!review(&db, TRACE, &["--linked-traces"]).status.success());
}
