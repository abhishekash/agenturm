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
