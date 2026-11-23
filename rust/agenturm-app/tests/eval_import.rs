use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TRACE: &str = "11111111111111111111111111111111";
const SIBLING_ONLY: &str = "22222222222222222222222222222222";
const PRIMARY: &str = "otlp.primary";
const SIBLING: &str = "otlp.sibling";

#[test]
fn evaluation_import_reopens_exact_retained_trace_and_preserves_execution_status() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let trace_file = scratch.0.join("traces.json");
    let results_file = scratch.0.join("results.json");
    let import_traces = |source: &str, spans: Vec<Value>| {
        std::fs::write(
            &trace_file,
            json!({"resourceSpans":[{"scopeSpans":[{"spans":spans}]}]}).to_string(),
        )
        .unwrap();
        checked(run(
            &db,
            &[
                "import-traces",
                "--source-id",
                source,
                "--file",
                path(&trace_file),
            ],
        ));
    };
    import_traces(
        PRIMARY,
        vec![
            span(TRACE, "1111111111111111", 200),
            span(TRACE, "1111111111111112", 400),
        ],
    );
    import_traces(
        SIBLING,
        vec![
            span(TRACE, "1111111111111111", 600),
            span(SIBLING_ONLY, "2222222222222222", 600),
        ],
    );

    let original = json!({
        "trace_id":TRACE, "passed":false, "task_id":"retrieval-control",
        "provider_model":"openai/gpt-4o-mini",
        "scores":[{"scorer":"answer_regex","passed":false,"score":0.25,"detail":"private-score-detail"}],
        "error":"api_key=private-error-marker",
        "details":{"input":"private-result-body"},
    });
    let import = |source: &str, rows: Vec<Value>| {
        std::fs::write(&results_file, json!({"results": rows}).to_string()).unwrap();
        run(
            &db,
            &[
                "import-evals",
                "--source-id",
                source,
                "--file",
                path(&results_file),
            ],
        )
    };
    let eval = |db: &Path, source: &str| {
        serde_json::from_str::<Value>(&checked(run(
            db,
            &[
                "evals",
                "--source-id",
                source,
                "--trace-id",
                TRACE,
                "--json",
            ],
        )))
        .unwrap()
    };

    let wrong_source = json!({"trace_id":SIBLING_ONLY,"passed":true});
    let rejected = import(PRIMARY, vec![original.clone(), wrong_source]);
    assert!(failed(rejected).contains("absent from the selected retained source"));
    assert_eq!(eval(&db, PRIMARY), Value::Null);
    assert!(
        failed(import(PRIMARY, vec![original.clone(), original.clone()]))
            .contains("duplicate trace ID")
    );
    assert_eq!(eval(&db, PRIMARY), Value::Null);

    assert!(checked(import(PRIMARY, vec![original.clone()])).contains("Imported 1"));
    let first = eval(&db, PRIMARY);
    assert_eq!(first["trace_source_id"], PRIMARY);
    assert_eq!(first["revision"], 1);
    assert_eq!(first["outcome"]["passed"], false);
    assert_eq!(first["outcome"]["task_id"], "retrieval-control");
    assert_eq!(first["outcome"]["provider_model"], "openai/gpt-4o-mini");
    assert_eq!(first["outcome"]["scores"][0]["score"], 0.25);
    assert!(first["outcome"].get("error").is_none());
    assert!(first["outcome"]["scores"][0].get("detail").is_none());
    assert_eq!(eval(&db, SIBLING), Value::Null);
    assert!(
        checked(run(
            &db,
            &["evals", "--source-id", PRIMARY, "--trace-id", TRACE]
        ))
        .starts_with("Evaluation failed\n")
    );
    let execution = checked(run(&db, &["query", "--harness-id", PRIMARY]));
    let execution: Value = serde_json::from_str(&execution).unwrap();
    for row in execution["observations"].as_array().unwrap() {
        assert_eq!(row["observation"]["status"], "ok");
    }

    assert!(checked(import(PRIMARY, vec![original.clone()])).contains("Replayed 1"));
    let mut correction = original.clone();
    correction["passed"] = json!(true);
    assert!(checked(import(PRIMARY, vec![correction.clone()])).contains("Corrected 1"));
    assert!(checked(import(PRIMARY, vec![original.clone()])).contains("Replayed 1"));
    let corrected = eval(&db, PRIMARY);
    assert_eq!(corrected["revision"], 2);
    assert_eq!(corrected["outcome"]["passed"], true);
    assert!(checked(import(SIBLING, vec![original.clone()])).contains("Imported 1"));
    assert_eq!(eval(&db, SIBLING)["revision"], 1);

    let browse = |flags: &[&str]| {
        let mut args = vec!["evals", "--source-id", PRIMARY, "--json"];
        args.extend_from_slice(flags);
        serde_json::from_str::<Value>(&checked(run(&db, &args))).unwrap()
    };
    let history = browse(&["--trace-id", TRACE, "--history", "--limit", "1"]);
    assert_eq!(history["page"]["outcomes"][0]["revision"], 2);
    assert_eq!(history["page"]["next_before_revision"], 2);
    let older = browse(&["--trace-id", TRACE, "--history", "--before-revision", "2"]);
    assert_eq!(older["page"]["outcomes"][0]["revision"], 1);
    assert_eq!(older["page"]["outcomes"][0]["outcome"]["passed"], false);
    assert!(older["page"]["next_before_revision"].is_null());
    import_traces(PRIMARY, vec![span(SIBLING_ONLY, "2222222222222222", 400)]);
    assert!(
        checked(import(
            PRIMARY,
            vec![json!({"trace_id": SIBLING_ONLY, "passed":false})]
        ))
        .contains("Imported 1")
    );
    let summary = browse(&["--summary", "--limit", "1"]);
    assert_eq!(summary["passed"], 1);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["page"]["next_trace_id"], TRACE);
    let next = browse(&["--summary", "--after-trace-id", TRACE]);
    assert_eq!(next["page"]["outcomes"].as_array().unwrap().len(), 1);
    assert_eq!(
        next["page"]["outcomes"][0]["outcome"]["trace_id"],
        SIBLING_ONLY
    );
    assert_eq!(next["failed"], 1);
    assert!(next["page"]["next_trace_id"].is_null());

    let backup = scratch.0.join("backup.sqlite");
    checked(run(&db, &["backup", "--output", path(&backup)]));
    assert_eq!(eval(&backup, PRIMARY), corrected);
    let bytes = std::fs::read(&backup).unwrap();
    for private in [
        "private-error-marker",
        "private-score-detail",
        "private-result-body",
    ] {
        assert!(
            !bytes
                .windows(private.len())
                .any(|window| window == private.as_bytes())
        );
    }

    checked(run(&db, &["prune", "--before-unix-ns", "250"]));
    assert_eq!(eval(&db, PRIMARY), corrected);
    checked(run(&db, &["prune", "--before-unix-ns", "450"]));
    assert_eq!(eval(&db, PRIMARY), Value::Null);
    assert_eq!(eval(&db, SIBLING)["outcome"]["passed"], false);
    assert!(
        failed(import(PRIMARY, vec![correction.clone()]))
            .contains("absent from the selected retained source")
    );

    import_traces(PRIMARY, vec![span(TRACE, "1111111111111113", 700)]);
    assert_eq!(eval(&db, PRIMARY), Value::Null);
    assert!(checked(import(PRIMARY, vec![original.clone()])).contains("Replayed 1"));
    assert!(checked(import(PRIMARY, vec![correction.clone()])).contains("Replayed 1"));
    assert_eq!(eval(&db, PRIMARY), Value::Null);

    correction["scores"][0]["score"] = json!(0.75);
    assert!(checked(import(PRIMARY, vec![correction])).contains("Corrected 1"));
    let latest = eval(&db, PRIMARY);
    assert_eq!(latest["revision"], 3);
    assert_eq!(latest["outcome"]["scores"][0]["score"], 0.75);
    assert!(checked(import(PRIMARY, vec![original])).contains("Replayed 1"));
    assert_eq!(eval(&db, PRIMARY), latest);
    let after_expiry = browse(&["--trace-id", TRACE, "--history"]);
    assert_eq!(
        after_expiry["page"]["outcomes"].as_array().unwrap().len(),
        1
    );
    assert_eq!(after_expiry["page"]["outcomes"][0]["revision"], 3);
}

#[test]
fn malformed_evaluations_report_safe_errors_before_opening_history() {
    let scratch = Scratch::new();
    let db = scratch.0.join("missing.sqlite");
    let results = scratch.0.join("results.json");
    let bad =
        json!({"results":[{"trace_id": TRACE, "passed":false, "task_id":"sk-private-key-marker"}]});
    std::fs::write(&results, bad.to_string()).unwrap();
    let outcome = failed(run(
        &db,
        &[
            "import-evals",
            "--file",
            path(&results),
            "--source-id",
            PRIMARY,
        ],
    ));
    assert!(outcome.contains("Invalid evaluation metadata"));
    assert!(!outcome.contains("private-key-marker"));
    assert!(!db.exists());
}

#[test]
fn evaluation_detail_is_atomic_exact_and_fenced_by_both_receipts() {
    use agenturm_kernel::ObservationQuery;
    use agenturm_store::Store;

    let scratch = Scratch::new();
    let db = scratch.0.join("private.sqlite");
    let backup = scratch.0.join("backup.sqlite");
    let trace_file = scratch.0.join("traces.json");
    let results = scratch.0.join("results.json");
    let selection = scratch.0.join("selection.json");
    let trace = |spans| {
        std::fs::write(
            &trace_file,
            json!({"resourceSpans":[{"scopeSpans":[{"spans":spans}]}]}).to_string(),
        )
        .unwrap();
        checked(run(
            &db,
            &[
                "import-traces",
                "--source-id",
                PRIMARY,
                "--file",
                path(&trace_file),
            ],
        ));
    };
    let import_at = |history, rows, detail| {
        std::fs::write(&results, json!({"results":rows}).to_string()).unwrap();
        let mut args = vec![
            "import-evals",
            "--source-id",
            PRIMARY,
            "--file",
            path(&results),
        ];
        if detail {
            args.push("--include-detail");
        }
        run(history, &args)
    };
    let import = |rows, detail| import_at(&db, rows, detail);
    let metadata = |history| {
        let raw = checked(run(
            history,
            &[
                "evals",
                "--source-id",
                PRIMARY,
                "--trace-id",
                TRACE,
                "--json",
            ],
        ));
        assert!(!raw.contains("bounded scorer explanation"));
        assert!(!raw.contains("bounded evaluator error"));
        assert!(!raw.contains("untyped provider body"));
        serde_json::from_str::<Value>(&raw).unwrap()
    };
    let snapshot = |history| {
        Store::open_read_only(history)
            .unwrap()
            .query_observations(&ObservationQuery::latest(2))
            .unwrap()
            .snapshot_id
    };
    let cursor = || {
        Store::open_read_only(&db)
            .unwrap()
            .source_cursor(PRIMARY)
            .unwrap()
    };
    let approve = |history| {
        checked(run(
            history,
            &["eval-detail", "grant", "--source-id", PRIMARY],
        ))
    };
    let preview = |history, value: &Value| {
        std::fs::write(&selection, value.to_string()).unwrap();
        run(
            history,
            &["eval-detail", "preview", "--selection", path(&selection)],
        )
    };
    let outcome = json!({"trace_id": TRACE, "passed":false,
        "error":"bounded evaluator error", "details":{"raw":"untyped provider body"},
        "scores":[{"scorer":"first", "passed":false, "detail":{"json":"untyped provider body"}},
                  {"scorer":"second", "score":0.4, "detail":"bounded scorer explanation"}]});
    let mut unsafe_outcome =
        json!({"trace_id": SIBLING_ONLY, "passed":false, "error":"api_key=fixture-secret-marker"});
    trace(vec![
        span(TRACE, "1111111111111111", 300),
        span(SIBLING_ONLY, "2222222222222222", 400),
    ]);
    let before = snapshot(&db);
    let before_cursor = cursor();
    let rejected = failed(import(vec![outcome.clone(), unsafe_outcome.clone()], true));
    assert!(rejected.contains("text was withheld") && !rejected.contains("fixture-secret-marker"));
    assert!(metadata(&db).is_null());
    assert_eq!(snapshot(&db), before);
    unsafe_outcome["error"] = json!("x".repeat(2049));
    assert!(
        failed(import(vec![outcome.clone(), unsafe_outcome], true)).contains("text was withheld")
    );
    assert!(metadata(&db).is_null());

    assert!(checked(import(vec![outcome.clone()], false)).contains("Imported 1"));
    assert_eq!(snapshot(&db), before);
    assert!(metadata(&db).get("detail_selection").is_none());
    assert!(
        failed(import(vec![outcome.clone()], true)).contains("differs from its original import")
    );
    assert!(metadata(&db).get("detail_selection").is_none());

    let mut correction = outcome.clone();
    correction["passed"] = json!(true);
    assert!(checked(import(vec![correction.clone()], true)).contains("Corrected 1"));
    let admitted = snapshot(&db);
    assert_eq!(admitted.watermark(), before.watermark() + 1);
    assert_eq!(cursor(), before_cursor);
    let meta = metadata(&db);
    assert_eq!(meta["revision"], 2);
    assert_eq!(meta["detail_selection"]["snapshot_id"], json!(admitted));
    let captured = &meta["detail_selection"];
    checked(run(
        &db,
        &[
            "trace-payload",
            "grant",
            "--source-id",
            PRIMARY,
            "--payload-scope",
            "standalone",
        ],
    ));
    assert!(failed(preview(&db, captured)).contains("grant"));
    approve(&db);
    let expected = json!({"provenance":"imported evaluation text","detail":{
        "error":"bounded evaluator error", "scores":[{"score_index":1,"detail":"bounded scorer explanation"}]}});
    assert_eq!(
        serde_json::from_str::<Value>(&checked(preview(&db, captured))).unwrap(),
        expected
    );
    let mut old = captured.clone();
    old["snapshot_id"] = json!(before);
    assert!(failed(preview(&db, &old)).contains("unavailable"));
    old = captured.clone();
    old["revision"] = json!(1);
    assert!(failed(preview(&db, &old)).contains("unavailable"));
    checked(run(&db, &["eval-detail", "grant", "--source-id", SIBLING]));
    old = captured.clone();
    old["trace_source_id"] = json!(SIBLING);
    assert!(failed(preview(&db, &old)).contains("unavailable"));

    assert!(checked(import(vec![correction.clone()], true)).contains("Replayed 1"));
    assert!(checked(import(vec![correction.clone()], false)).contains("Replayed 1"));
    let mut conflict = correction.clone();
    conflict["error"] = json!("different bounded text");
    assert!(failed(import(vec![conflict], true)).contains("differs from its original import"));
    assert_eq!(snapshot(&db), admitted);
    checked(run(&db, &["backup", "--output", path(&backup)]));
    assert!(failed(preview(&backup, captured)).contains("grant"));
    approve(&backup);
    assert!(failed(preview(&backup, captured)).contains("restored or replaced"));
    let fresh = metadata(&backup)["detail_selection"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(&checked(preview(&backup, &fresh))).unwrap(),
        expected
    );

    let other = scratch.0.join("other.sqlite");
    checked(run(
        &other,
        &[
            "import-traces",
            "--source-id",
            PRIMARY,
            "--file",
            path(&trace_file),
        ],
    ));
    checked(import_at(&other, vec![outcome], false));
    let mut alternate = correction.clone();
    alternate["error"] = json!("alternate evaluator text");
    checked(import_at(&other, vec![alternate], true));
    approve(&other);
    let other_selection = metadata(&other)["detail_selection"].clone();
    assert_eq!(other_selection["snapshot_id"], captured["snapshot_id"]);
    assert_eq!(other_selection["revision"], captured["revision"]);
    assert_ne!(other_selection["receipt_id"], captured["receipt_id"]);
    assert!(failed(preview(&other, captured)).contains("unavailable"));
    assert!(checked(preview(&other, &other_selection)).contains("alternate evaluator text"));

    checked(run(&db, &["eval-detail", "revoke", "--source-id", PRIMARY]));
    assert!(failed(preview(&db, captured)).contains("grant"));
    approve(&db);
    trace(vec![span(TRACE, "1111111111111112", 900)]);
    let next = snapshot(&db);
    assert_eq!(next.watermark(), admitted.watermark() + 1);
    checked(run(&db, &["prune", "--before-unix-ns", "500"]));
    let pruned = snapshot(&db);
    assert_eq!(pruned.watermark(), next.watermark() + 1);
    assert!(failed(preview(&db, captured)).contains("expired"));
    assert_eq!(metadata(&db)["revision"], 2);
    assert!(metadata(&db).get("detail_selection").is_none());
    assert!(checked(import(vec![correction], true)).contains("Replayed 1"));
    assert_eq!(snapshot(&db), pruned);
    assert!(metadata(&db).get("detail_selection").is_none());
}

fn span(trace: &str, span: &str, end: u64) -> Value {
    json!({
        "traceId":trace, "spanId":span, "name":"fixture",
        "startTimeUnixNano":(end-1).to_string(), "endTimeUnixNano":end.to_string(),
        "status":{"code":1},
        "attributes":[{"key":"session.id","value":{"stringValue":"fixture-session"}}]
    })
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn run(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(args)
        .arg("--db")
        .arg(db)
        .output()
        .unwrap()
}

fn checked(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn failed(output: Output) -> String {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agenturm-eval-import-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
