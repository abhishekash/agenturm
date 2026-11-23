use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::Store;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const SOURCE: &str = "otlp.imported";
const HARNESS: &str = "harness.codex";
const SESSION: &str = "native-session";
const TURN: &str = "native-turn";
const WORKER: &str = "worker/one";
const TRACE: &str = "0123456789abcdef0123456789abcdef";
const ROOT: &str = "1111111111111111";
const CHILD: &str = "2222222222222222";
const START: u64 = 1_750_000_000_123_456_789;
const END: u64 = START + 100;
const PRIVATE: &str = "secret-body-do-not-persist";

#[test]
fn import_preserves_linked_evidence_reopens_and_replays_across_formats() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("spans.jsonl");
    let mut root = span(ROOT, None, "OK");
    root["trace_id"] = json!(TRACE.to_ascii_uppercase());
    let child = span(CHILD, Some(ROOT), "ERROR");
    write_jsonl(&file, &[root.clone(), child.clone()]);
    assert_eq!(
        check(import_file(&db, &file, &[])),
        "Imported 2 spans. Projected 2 structural spans.\n"
    );
    let queried = check(run(&db, &["query", "--harness-id", SOURCE]));
    assert!(!queried.contains(PRIVATE));
    let page: Value = serde_json::from_str(&queried).unwrap();
    let rows = page["observations"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["observed_at_unix_ns"], END.to_string());
    assert_eq!(rows[0]["observation"]["trace_id"], TRACE);
    assert_eq!(rows[0]["observation"]["span_id"], ROOT);
    assert_eq!(rows[0]["observation"]["parent_span_id"], Value::Null);
    assert_eq!(
        rows[0]["observation"]["started_at_unix_ns"],
        START.to_string()
    );
    assert_eq!(rows[0]["observation"]["status"], "ok");
    assert_eq!(rows[1]["observation"]["span_id"], CHILD);
    assert_eq!(rows[1]["observation"]["parent_span_id"], ROOT);
    assert_eq!(rows[1]["observation"]["status"], "error");
    assert_eq!(rows[1]["observation"]["target_source_id"], HARNESS);
    assert_eq!(
        rows[1]["identity"],
        json!({
            "session_id": SESSION, "turn_id": TURN, "worker_id": WORKER,
            "scope": "session", "operation_id": null,
        })
    );
    let cursor = source_cursor(&db);
    let page_args = ["query", "--harness-id", SOURCE, "--limit", "1"];
    let initial_watermark = check(run(&db, &page_args));

    root["name"] = json!("changed ignored name");
    write_jsonl(&file, &[child, root]);
    assert!(check(import_file(&db, &file, &[])).contains("Imported 0 spans"));
    assert_eq!(source_cursor(&db), cursor);
    assert_eq!(check(run(&db, &page_args)), initial_watermark);

    let otlp = scratch.0.join("spans.json");
    std::fs::write(
        &otlp,
        json!({"resourceSpans": [{"scopeSpans": [{"spans": [
            otlp_span(ROOT, None, 1),
            otlp_span(CHILD, Some(ROOT), 2),
        ]}]}]})
        .to_string(),
    )
    .unwrap();
    assert!(
        check(run(
            &db,
            &[
                "import-traces",
                "--file",
                otlp.to_str().unwrap(),
                "--source-id",
                SOURCE
            ]
        ))
        .contains("Imported 0 spans")
    );
    assert_eq!(source_cursor(&db), cursor);
    assert_eq!(check(run(&db, &page_args)), initial_watermark);
    assert!(
        !std::fs::read(&db)
            .unwrap()
            .windows(PRIVATE.len())
            .any(|bytes| bytes == PRIVATE.as_bytes())
    );

    let mut identity = ObservationIdentity::session(SESSION);
    identity.turn_id = Some(TURN.into());
    identity.worker_id = Some(WORKER.into());
    Store::open(&db)
        .unwrap()
        .commit(
            HARNESS,
            None,
            "native-checkpoint",
            &[Observation {
                harness_id: ExtensionId::parse(HARNESS).unwrap(),
                event_id: "user-occurrence".into(),
                revision: 0,
                identity,
                observed_at_unix_ns: START,
                kind: ObservationKind::Message {
                    role: "user".into(),
                },
            }],
        )
        .unwrap();
    let review = run(
        &db,
        &[
            "review",
            "--harness-id",
            HARNESS,
            "--session-id",
            SESSION,
            "--turn-id",
            TURN,
            "--worker-id",
            WORKER,
            "--linked-traces",
            "--format",
            "json",
        ],
    );
    let reviewed: Value = serde_json::from_str(&check(review)).unwrap();
    assert_eq!(reviewed["observations"].as_array().unwrap().len(), 2);
    assert_eq!(
        reviewed["observations"][1]["observation"]["parent_span_id"],
        ROOT
    );
}

#[test]
fn import_from_stdin_requires_unambiguous_session_scope() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let mut first = span(ROOT, None, "UNSET");
    first.as_object_mut().unwrap().remove("status");
    first["attributes"] = json!({"agent.session_id": SESSION, "langchain.session_id": SESSION});
    let mut child = span(CHILD, Some(ROOT), "OK");
    child.as_object_mut().unwrap().remove("attributes");
    let input = format!("\n{first}\r\n{child}");
    let result = import_stdin(&db, input.as_bytes(), &[]);
    assert!(failure(result).contains("explicit session identity"));
    assert!(!db.exists());

    let result = import_stdin(&db, input.as_bytes(), &["--session-id", "other-session"]);
    assert!(failure(result).contains("conflicting OTLP span session identity"));
    assert!(!db.exists());

    let result = import_stdin(&db, input.as_bytes(), &["--session-id", SESSION]);
    assert!(check(result).contains("Imported 2 spans"));
    assert!(check(run(&db, &["query", "--session-id", SESSION])).contains("\"status\": \"unset\""));
}

#[test]
fn malformed_and_oversized_inputs_leave_history_untouched() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("spans.jsonl");
    let root = span(ROOT, None, "OK");
    std::fs::write(&file, format!("{root}\ninvalid {PRIVATE}\n")).unwrap();
    assert!(failure(import_file(&db, &file, &[])).contains("line 2"));
    assert!(!db.exists());

    let mut conflict = root.clone();
    conflict["attributes"]["agenturm.worker.id"] = json!("worker/two");
    write_jsonl(&file, &[root.clone(), conflict]);
    assert!(failure(import_file(&db, &file, &[])).contains("conflicting span identity"));
    assert!(!db.exists());

    let cycle_root = span(ROOT, Some(CHILD), "OK");
    let cycle_child = span(CHILD, Some(ROOT), "OK");
    write_jsonl(&file, &[cycle_root, cycle_child]);
    assert!(failure(import_file(&db, &file, &[])).contains("parent cycle"));
    assert!(!db.exists());

    for (field, value) in [
        ("span_id", json!("0000000000000000")),
        ("parent_span_id", json!(ROOT)),
        ("start_unix_nano", json!(END + 1)),
        ("status", json!("FAILED")),
    ] {
        let mut invalid = root.clone();
        invalid[field] = value;
        write_jsonl(&file, &[invalid]);
        failure(import_file(&db, &file, &[]));
        assert!(!db.exists());
    }

    let mut conflicting_scope = root.clone();
    conflicting_scope["attributes"]["thread_id"] = json!("other-session");
    write_jsonl(&file, &[conflicting_scope]);
    assert!(
        failure(import_file(&db, &file, &[])).contains("conflicting OTLP span session identity")
    );
    assert!(!db.exists());

    let mut malformed_scope = root.clone();
    malformed_scope["attributes"]["thread_id"] = Value::Null;
    write_jsonl(&file, &[malformed_scope]);
    assert!(failure(import_file(&db, &file, &[])).contains("line 1"));
    assert!(!db.exists());

    std::fs::write(&file, format!("{root}\n").repeat(1_025)).unwrap();
    assert!(failure(import_file(&db, &file, &[])).contains("1,024 records"));
    assert!(!db.exists());

    std::fs::File::create(&file)
        .unwrap()
        .set_len(4 * 1024 * 1024 + 1)
        .unwrap();
    assert!(failure(import_file(&db, &file, &[])).contains("at most 4 MiB"));
    assert!(!db.exists());
}

#[test]
fn a_conflict_with_retained_evidence_rolls_back_new_spans_and_the_checkpoint() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("spans.jsonl");
    let child = span(CHILD, Some(ROOT), "OK");
    write_jsonl(&file, std::slice::from_ref(&child));
    assert!(check(import_file(&db, &file, &[])).contains("Imported 1 spans"));
    let before = check(run(&db, &["query", "--harness-id", SOURCE]));
    let cursor = source_cursor(&db);

    let mut conflicting_child = child;
    conflicting_child["status"] = json!("ERROR");
    write_jsonl(&file, &[span(ROOT, None, "OK"), conflicting_child]);
    let error = failure(import_file(&db, &file, &[]));
    assert!(error.contains("Cannot commit span JSONL"));
    assert!(!error.contains(CHILD));
    assert_eq!(source_cursor(&db), cursor);
    assert_eq!(check(run(&db, &["query", "--harness-id", SOURCE])), before);
}

#[test]
fn reported_cost_admits_explicit_zero_and_currency_without_changing_span_replay() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("spans.jsonl");
    let mut root = span(ROOT, None, "OK");
    let mut child = span(CHILD, Some(ROOT), "ERROR");
    write_jsonl(&file, &[root.clone(), child.clone()]);
    assert!(check(import_file(&db, &file, &[])).contains("Imported 2 spans"));
    let before = check(run(&db, &["query", "--harness-id", SOURCE]));
    let baseline: Value = serde_json::from_str(&before).unwrap();

    root["attributes"]["llm.cost_usd"] = json!(0);
    child["attributes"]["agenturm.reported_cost.amount"] = json!("0.000000123");
    child["attributes"]["agenturm.reported_cost.currency"] = json!("EUR");
    let invalid: Vec<_> = [
        json!({"session.id":SESSION, "llm.cost.total":10}),
        json!({"session.id":SESSION, "llm.cost.total":10, "gen_ai.usage.cost.currency":"EUR"}),
        json!({"session.id":SESSION, "llm.cost_usd":0, "llm.cost_known":false}),
        json!({"session.id":SESSION, "llm.cost_usd":0, "llm.cost_known":null}),
        json!({"session.id":SESSION, "llm.cost_usd":1, "llm.cost.currency":"EUR"}),
        json!({"session.id":SESSION, "llm.cost_usd":"1e-11"}),
        json!({"session.id":SESSION, "llm.cost_usd":PRIVATE}),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, attributes)| {
        let mut row = span(&format!("{:016x}", i + 10), Some(ROOT), "OK");
        row["attributes"] = attributes;
        row
    })
    .collect();
    let batch: Vec<_> = [root.clone(), child.clone()]
        .into_iter()
        .chain(invalid)
        .collect();
    write_jsonl(&file, &batch);
    assert_eq!(
        check(import_file(&db, &file, &[])),
        "Imported 9 observations from 9 spans and 2 reported cost records.\n"
    );
    let queried = check(run(&db, &["query", "--harness-id", SOURCE]));
    assert!(!queried.contains(PRIVATE));
    let page: Value = serde_json::from_str(&queried).unwrap();
    let rows = page["observations"].as_array().unwrap();
    assert_eq!(rows.len(), 11);
    for first in baseline["observations"].as_array().unwrap() {
        assert!(rows.contains(first));
    }
    let costs: Vec<_> = rows
        .iter()
        .filter(|row| row["observation"]["kind"] == "reported_cost")
        .collect();
    assert_eq!(costs.len(), 2);
    for (span_id, currency, nanounits) in [(ROOT, "USD", "0"), (CHILD, "EUR", "123")] {
        let cost = costs
            .iter()
            .find(|row| row["observation"]["span"]["span_id"] == span_id)
            .unwrap();
        assert_eq!(
            cost["observation"],
            json!({
                "kind":"reported_cost", "currency":currency, "nanounits":nanounits,
                "span":{"trace_id":TRACE,"span_id":span_id},
            })
        );
        assert_eq!(cost["identity"]["session_id"], SESSION);
        assert_eq!(cost["identity"]["turn_id"], TURN);
        assert_eq!(cost["identity"]["worker_id"], WORKER);
    }
    assert!(check(import_file(&db, &file, &[])).contains("Imported 0 observations"));

    let otlp = scratch.0.join("spans.json");
    let mut native = otlp_span(CHILD, Some(ROOT), 2);
    native["attributes"].as_array_mut().unwrap().extend([
        json!({"key":"gen_ai.usage.cost","value":{"doubleValue":0.000000123}}),
        json!({"key":"gen_ai.usage.cost.currency","value":{"stringValue":"EUR"}}),
    ]);
    std::fs::write(
        &otlp,
        json!({"resourceSpans":[{"scopeSpans":[{"spans":[native]}]}]}).to_string(),
    )
    .unwrap();
    assert!(
        check(run(
            &db,
            &[
                "import-traces",
                "--file",
                otlp.to_str().unwrap(),
                "--source-id",
                SOURCE
            ]
        ))
        .contains("Imported 0 observations")
    );

    child["attributes"]["agenturm.reported_cost.amount"] = json!("0.000000124");
    write_jsonl(&file, &[child]);
    assert!(failure(import_file(&db, &file, &[])).contains("Cannot commit span JSONL"));
    let backup = scratch.0.join("backup.sqlite");
    check(run(&db, &["backup", "--output", backup.to_str().unwrap()]));
    let backup_query = check(run(&backup, &["query", "--harness-id", SOURCE]));
    let backup_page: Value = serde_json::from_str(&backup_query).unwrap();
    assert_eq!(backup_page["observations"], page["observations"]);
}

fn span(id: &str, parent: Option<&str>, status: &str) -> Value {
    json!({
        "trace_id": TRACE, "span_id": id, "parent_span_id": parent,
        "start_unix_nano": START, "end_unix_nano": END.to_string(),
        "status": status, "name": PRIVATE,
        "attributes": {
            "session.id": SESSION, "agenturm.source.id": HARNESS,
            "agenturm.turn.id": TURN, "agenturm.worker.id": WORKER,
            "gen_ai.input.messages": PRIVATE, "gen_ai.usage.input_tokens": 345,
        },
        "events": [{"name": PRIVATE}], "links": [{"attributes": {"private": PRIVATE}}],
        "resource_attributes": {"secret": PRIVATE},
    })
}

fn otlp_span(id: &str, parent: Option<&str>, status: u8) -> Value {
    let attributes = [
        ("session.id", SESSION),
        ("agenturm.source.id", HARNESS),
        ("agenturm.turn.id", TURN),
        ("agenturm.worker.id", WORKER),
    ]
    .map(|(key, value)| json!({"key": key, "value": {"stringValue": value}}));
    json!({
        "traceId": TRACE, "spanId": id, "parentSpanId": parent,
        "startTimeUnixNano": START.to_string(), "endTimeUnixNano": END.to_string(),
        "status": {"code": status}, "attributes": attributes,
    })
}

fn write_jsonl(path: &Path, spans: &[Value]) {
    let mut file = std::fs::File::create(path).unwrap();
    for span in spans {
        writeln!(file, "{span}").unwrap();
    }
}

fn source_cursor(db: &Path) -> String {
    Store::open(db)
        .unwrap()
        .source_cursor(SOURCE)
        .unwrap()
        .unwrap()
}

fn import_file(db: &Path, file: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "import-jsonl",
        "--file",
        file.to_str().unwrap(),
        "--source-id",
        SOURCE,
    ];
    args.extend(extra);
    run(db, &args)
}

fn import_stdin(db: &Path, bytes: &[u8], extra: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(["import-jsonl", "--file", "-", "--source-id", SOURCE])
        .args(extra)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}

fn run(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agenturm"))
        .args(args)
        .arg("--db")
        .arg(db)
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

fn failure(output: Output) -> String {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(!error.contains(PRIVATE));
    error
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "agenturm-span-jsonl-import-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
