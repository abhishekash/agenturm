use agenturm_kernel::{
    ExtensionId, Observation, ObservationIdentity, ObservationKind, ObservationStore,
};
use agenturm_store::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const NATIVE_SOURCE: &str = "harness.codex";
const TRACE_SOURCE: &str = "otlp.exact";
const SESSION: &str = "same-session";
const TURN: &str = "same-turn";
const WORKER: &str = "worker/one";
const WORKER_TRACE: &str = "11111111111111111111111111111111";
const ROOT_TRACE: &str = "22222222222222222222222222222222";
const LEGACY_TRACE: &str = "33333333333333333333333333333333";

#[test]
fn linked_review_requires_an_exact_retained_trace_scope_at_the_selected_snapshot() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let native = |source: &str, worker: Option<&str>, event: &str| {
        let mut identity = ObservationIdentity::session(SESSION);
        identity.turn_id = Some(TURN.into());
        identity.worker_id = worker.map(str::to_owned);
        Observation {
            harness_id: ExtensionId::parse(source).unwrap(),
            event_id: event.into(),
            revision: 0,
            identity,
            observed_at_unix_ns: 1,
            kind: ObservationKind::Message {
                role: "user".into(),
            },
        }
    };
    let write_native = |row: Observation, previous: Option<&str>| {
        let mut store = Store::open(&db).unwrap();
        let source = row.harness_id.as_str().to_owned();
        let cursor = row.event_id.clone();
        store.commit(&source, previous, &cursor, &[row]).unwrap();
    };
    write_native(native(NATIVE_SOURCE, None, "root-event"), None);
    write_native(native("harness.pi", Some(WORKER), "sibling-source"), None);
    let import = |spans: Vec<Value>| {
        let file = scratch.0.join("traces.json");
        std::fs::write(
            &file,
            json!({"resourceSpans":[{"scopeSpans":[{"spans":spans}]}]}).to_string(),
        )
        .unwrap();
        run(
            &db,
            &[
                "import-traces",
                "--file",
                file.to_str().unwrap(),
                "--source-id",
                TRACE_SOURCE,
            ],
        )
    };
    let initial_spans = || {
        vec![
            span(
                WORKER_TRACE,
                "1111111111111111",
                NATIVE_SOURCE,
                Some(TURN),
                Some(WORKER),
            ),
            span(
                WORKER_TRACE,
                "1111111111111112",
                NATIVE_SOURCE,
                Some(TURN),
                Some(WORKER),
            ),
            span(
                ROOT_TRACE,
                "2222222222222222",
                NATIVE_SOURCE,
                Some(TURN),
                None,
            ),
            span(LEGACY_TRACE, "3333333333333333", NATIVE_SOURCE, None, None),
        ]
    };
    check(import(initial_spans()));
    assert!(check(import(initial_spans())).contains("Imported 0 spans"));

    let review = |source: &str, session: &str, selection: &[&str]| {
        let mut args = vec![
            "review",
            "--harness-id",
            source,
            "--session-id",
            session,
            "--format",
            "json",
        ];
        args.extend(selection);
        run(&db, &args)
    };
    let root_trace = review(
        NATIVE_SOURCE,
        SESSION,
        &["--turn-id", TURN, "--linked-traces"],
    );
    let root_trace = parse(root_trace);
    assert_eq!(root_trace["observations"].as_array().unwrap().len(), 1);
    assert_eq!(
        root_trace["observations"][0]["observation"]["trace_id"],
        ROOT_TRACE
    );
    assert_eq!(root_trace["scope"]["worker_id"], Value::Null);
    let early = root_trace["snapshot_id"].as_str().unwrap();
    let worker_turn = [
        "--turn-of-trace",
        WORKER_TRACE,
        "--trace-source-id",
        TRACE_SOURCE,
    ];
    failed(review(NATIVE_SOURCE, SESSION, &worker_turn));
    failed(review("harness.pi", SESSION, &worker_turn));
    failed(review(NATIVE_SOURCE, "another-session", &worker_turn));
    failed(review(
        NATIVE_SOURCE,
        SESSION,
        &[
            "--turn-of-trace",
            LEGACY_TRACE,
            "--trace-source-id",
            TRACE_SOURCE,
        ],
    ));

    write_native(
        native(NATIVE_SOURCE, Some(WORKER), "worker-event"),
        Some("root-event"),
    );
    let resolved = parse(review(NATIVE_SOURCE, SESSION, &worker_turn));
    assert_eq!(resolved["scope"]["turn_id"], TURN);
    assert_eq!(resolved["scope"]["worker_id"], WORKER);
    assert_eq!(resolved["observations"][0]["event_id"], "worker-event");
    let pinned = resolved["snapshot_id"].as_str().unwrap();
    failed(review(
        NATIVE_SOURCE,
        SESSION,
        &[&worker_turn[..], &["--snapshot-id", early]].concat(),
    ));
    failed(review(
        NATIVE_SOURCE,
        SESSION,
        &[&worker_turn[..], &["--worker-id", "sibling-worker"]].concat(),
    ));

    let selection = [
        "--turn-id",
        TURN,
        "--worker-id",
        WORKER,
        "--linked-traces",
        "--limit",
        "1",
    ];
    let first = parse(review(NATIVE_SOURCE, SESSION, &selection));
    assert_eq!(
        first["observations"][0]["observation"]["span_id"],
        "1111111111111112"
    );
    assert_eq!(first["observations"][0]["harness_id"], TRACE_SOURCE);
    let cursor = &first["next_before"];
    let before_revision = cursor["revision"].as_u64().unwrap().to_string();
    let before = [
        "--snapshot-id",
        cursor["snapshot_id"].as_str().unwrap(),
        "--before-timestamp-ns",
        cursor["observed_at_unix_ns"].as_str().unwrap(),
        "--before-harness-id",
        cursor["harness_id"].as_str().unwrap(),
        "--before-event-id",
        cursor["event_id"].as_str().unwrap(),
        "--before-revision",
        before_revision.as_str(),
    ];
    let older = parse(review(
        NATIVE_SOURCE,
        SESSION,
        &[&selection[..], &before].concat(),
    ));
    assert_eq!(
        older["observations"][0]["observation"]["span_id"],
        "1111111111111111"
    );
    assert_eq!(older["has_more"], false);
    assert_eq!(older["snapshot_id"], first["snapshot_id"]);

    check(import(vec![span(
        WORKER_TRACE,
        "1111111111111113",
        "harness.pi",
        Some(TURN),
        Some(WORKER),
    )]));
    failed(review(NATIVE_SOURCE, SESSION, &worker_turn));
    failed(review("harness.pi", SESSION, &worker_turn));
    let old = parse(review(
        NATIVE_SOURCE,
        SESSION,
        &[&worker_turn[..], &["--snapshot-id", pinned]].concat(),
    ));
    assert_eq!(old, resolved);

    check(run(&db, &["prune", "--before-unix-ns", "2"]));
    failed(review(
        NATIVE_SOURCE,
        SESSION,
        &[
            "--turn-of-trace",
            ROOT_TRACE,
            "--trace-source-id",
            TRACE_SOURCE,
        ],
    ));
    assert!(
        parse(review(NATIVE_SOURCE, SESSION, &selection))["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn span(
    trace_id: &str,
    span_id: &str,
    source: &str,
    turn_id: Option<&str>,
    worker_id: Option<&str>,
) -> Value {
    let attributes = [
        ("session.id", Some(SESSION)),
        ("agenturm.source.id", Some(source)),
        ("agenturm.turn.id", turn_id),
        ("agenturm.worker.id", worker_id),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| json!({"key":key,"value":{"stringValue":value}})))
    .collect::<Vec<_>>();
    json!({
        "traceId":trace_id, "spanId":span_id,
        "startTimeUnixNano":"100", "endTimeUnixNano":"200", "attributes":attributes,
    })
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

fn parse(output: Output) -> Value {
    serde_json::from_str(&check(output)).unwrap()
}

fn failed(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("agenturm-trace-turn-review-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
