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

#[test]
fn span_token_usage_keeps_reported_units_and_stable_structural_replay() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("usage.jsonl");
    let mut root = span(ROOT, None, "OK");
    let mut child = span(CHILD, Some(ROOT), "ERROR");
    write_jsonl(&file, &[root.clone(), child.clone()]);
    assert!(check(import_file(&db, &file, &[])).contains("Imported 2 spans"));

    root["attributes"]["gen_ai.usage.input_tokens"] = json!(0);
    root["attributes"]["llm.token_count.prompt"] = json!("0");
    root["attributes"]["llm.token_count.completion"] = json!(9);
    root["attributes"]["gen_ai.usage.cached_input_tokens"] = json!(12);
    child["attributes"]["llm.usage.input_tokens"] = json!(11);
    child["attributes"]["llm.token_count.prompt"] = json!(17);
    child["attributes"]["gen_ai.usage.output_tokens"] = json!(false);
    child["attributes"]["gen_ai.usage.cache_creation.input_tokens"] = json!("9223372036854775807");
    child["attributes"]["gen_ai.usage.reasoning_tokens"] = json!(1);
    child["attributes"]["gen_ai.usage.total_tokens"] = json!(25);
    write_jsonl(&file, &[root.clone(), child]);
    assert!(
        check(import_file(&db, &file, &[]))
            .contains("Imported 2 observations from 2 spans and 2 token usage records")
    );
    assert!(check(import_file(&db, &file, &[])).contains("Imported 0 observations"));
    let raw = check(run(&db, &["query", "--harness-id", SOURCE]));
    assert!(!raw.contains(PRIVATE));
    let page: Value = serde_json::from_str(&raw).unwrap();
    let rows = page["observations"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    let usage: Vec<_> = rows
        .iter()
        .filter(|row| row["observation"]["kind"] == "span_token_usage")
        .collect();
    assert_eq!(usage.len(), 2);
    for row in &usage {
        assert_eq!(row["identity"]["session_id"], SESSION);
        assert_eq!(row["identity"]["worker_id"], WORKER);
        assert_eq!(row["identity"]["turn_id"], TURN);
        assert_eq!(row["observation"]["target_source_id"], HARNESS);
        assert_eq!(row["observation"]["span"]["trace_id"], TRACE);
    }
    assert_eq!(
        usage[0]["observation"],
        json!({
            "kind":"span_token_usage", "target_source_id":HARNESS,
            "span":{"trace_id":TRACE,"span_id":ROOT},
            "input_tokens":0, "output_tokens":9, "cache_read_input_tokens":12,
        })
    );
    assert_eq!(
        usage[1]["observation"],
        json!({
            "kind":"span_token_usage", "target_source_id":HARNESS,
            "span":{"trace_id":TRACE,"span_id":CHILD},
            "cache_write_input_tokens":i64::MAX, "reasoning_output_tokens":1, "total_tokens":25,
        })
    );
    let json_file = scratch.0.join("usage.json");
    let mut otlp = otlp_span(ROOT, None, 1);
    otlp["attributes"].as_array_mut().unwrap().extend([
        json!({"key":"llm.usage.input_tokens","value":{"intValue":"0"}}),
        json!({"key":"gen_ai.usage.output_tokens","value":{"intValue":"9"}}),
        json!({"key":"gen_ai.usage.cache_read.input_tokens","value":{"intValue":12}}),
    ]);
    std::fs::write(
        &json_file,
        json!({"resourceSpans":[{"scopeSpans":[{"spans":[otlp]}]}]}).to_string(),
    )
    .unwrap();
    assert!(
        check(run(
            &db,
            &[
                "import-traces",
                "--file",
                json_file.to_str().unwrap(),
                "--source-id",
                SOURCE
            ]
        ))
        .contains("Imported 0 observations")
    );
}

#[test]
fn maximum_export_commits_span_cost_and_usage_atomically() {
    let scratch = Scratch::new();
    let file = scratch.0.join("maximum.jsonl");
    let db = scratch.0.join("history.sqlite");
    let mut rows: Vec<_> = (1..=1_024)
        .map(|i| {
            let mut row = span(&format!("{i:016x}"), None, "OK");
            row["attributes"]["llm.cost_usd"] = json!(0);
            row["attributes"]["gen_ai.usage.input_tokens"] = json!(i);
            row
        })
        .collect();
    rows.last_mut().unwrap()["span_id"] = json!("malformed");
    write_jsonl(&file, &rows);
    failure(import_file(&db, &file, &[]));
    assert!(!db.exists());

    rows.last_mut().unwrap()["span_id"] = json!(format!("{:016x}", 1_024));
    write_jsonl(&file, &rows);
    let receipt = check(import_file(&db, &file, &[]));
    assert!(
        receipt.contains("Imported 3072 observations from 1024 spans"),
        "{receipt}"
    );
    assert!(check(import_file(&db, &file, &[])).contains("Imported 0 observations"));
    let raw = check(run(
        &db,
        &["query", "--harness-id", SOURCE, "--limit", "4096"],
    ));
    let page: Value = serde_json::from_str(&raw).unwrap();
    let observations = page["observations"].as_array().unwrap();
    for kind in ["trace_span", "reported_cost", "span_token_usage"] {
        assert_eq!(
            observations
                .iter()
                .filter(|row| row["observation"]["kind"] == kind)
                .count(),
            1_024
        );
    }
}

#[test]
fn opted_in_payloads_use_an_exact_grant_and_keep_replay_retirement() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("spans.jsonl");
    let scope = ["--payload-scope", HARNESS];
    let mut root = span(ROOT, None, "OK");
    root["attributes"]["agenturm.payload.input"] = json!("bounded model input");
    root["attributes"]["agenturm.payload.output"] = json!("bounded model response");
    write_jsonl(&file, &[root.clone()]);

    let wrong_binding = failure(import_file(&db, &file, &["--payload-scope", "standalone"]));
    assert!(wrong_binding.contains("configured explicit source and session"));
    assert!(!db.exists());
    assert!(check(import_file(&db, &file, &scope)).contains("Imported 1 spans"));
    let cursor = source_cursor(&db);
    let metadata = || check(run(&db, &["query", "--harness-id", SOURCE]));
    assert!(!metadata().contains("bounded model"));
    let selection = payload_selection(&db, ROOT, Some(HARNESS));
    let preview = || preview_payload(&db, &scratch, &selection);
    assert!(!preview().status.success());
    grant_payloads(&db, HARNESS);
    let expected = json!({"origin": "producer_captured_telemetry", "payload": {
        "input": "bounded model input", "output": "bounded model response",
    }});
    assert_eq!(
        serde_json::from_str::<Value>(&check(preview())).unwrap(),
        expected
    );

    let mut wrong_worker = selection.clone();
    wrong_worker["worker_id"] = json!("worker/two");
    assert!(
        !preview_payload(&db, &scratch, &wrong_worker)
            .status
            .success()
    );
    assert!(check(import_file(&db, &file, &scope)).contains("Imported 0 spans"));
    assert!(check(import_file(&db, &file, &[])).contains("Imported 0 spans"));
    assert_eq!(source_cursor(&db), cursor);

    let mut conflict = root.clone();
    conflict["attributes"]["agenturm.payload.input"] = json!(PRIVATE);
    let earlier_new_span = span("0202020202020202", None, "OK");
    let before = metadata();
    write_jsonl(&file, &[earlier_new_span, conflict.clone()]);
    assert!(failure(import_file(&db, &file, &scope)).contains("Cannot commit"));
    assert_eq!(metadata(), before);
    assert_eq!(source_cursor(&db), cursor);

    check(run(
        &db,
        &["prune", "--before-unix-ns", &(END + 1).to_string()],
    ));
    assert!(!preview().status.success());
    write_jsonl(&file, &[root]);
    assert!(check(import_file(&db, &file, &scope)).contains("Imported 0 spans"));
    write_jsonl(&file, &[conflict]);
    assert!(failure(import_file(&db, &file, &scope)).contains("Cannot commit"));
    assert_eq!(source_cursor(&db), cursor);
    assert!(
        Store::open_read_only(&db)
            .unwrap()
            .observations()
            .unwrap()
            .is_empty()
    );
    assert!(!preview().status.success());
}

#[test]
fn payload_selection_rejects_other_fresh_history_and_requires_refresh_after_restore() {
    let scratch = Scratch::new();
    let original = scratch.0.join("original.sqlite");
    let unrelated = scratch.0.join("unrelated.sqlite");
    let restored = scratch.0.join("restored.sqlite");
    let file = scratch.0.join("spans.jsonl");
    for (db, body) in [
        (&original, "Captured original text"),
        (&unrelated, "Captured unrelated text"),
    ] {
        let mut row = span(ROOT, None, "OK");
        row["attributes"]["agenturm.payload.input"] = json!(body);
        write_jsonl(&file, &[row]);
        check(import_file(db, &file, &["--payload-scope", HARNESS]));
        grant_payloads(db, HARNESS);
    }
    let original_selection = payload_selection(&original, ROOT, Some(HARNESS));
    let unrelated_selection = payload_selection(&unrelated, ROOT, Some(HARNESS));
    assert_eq!(
        original_selection["snapshot_id"],
        unrelated_selection["snapshot_id"]
    );
    assert_ne!(
        original_selection["history_identity"],
        unrelated_selection["history_identity"]
    );
    let error = failure(preview_payload(&unrelated, &scratch, &original_selection));
    assert!(error.contains("restored or replaced"));
    assert!(
        check(preview_payload(&unrelated, &scratch, &unrelated_selection))
            .contains("Captured unrelated text")
    );

    let mut legacy = original_selection.clone();
    legacy.as_object_mut().unwrap().remove("history_identity");
    let error = failure(preview_payload(&original, &scratch, &legacy));
    assert!(error.contains("Refresh it with trace-payload snapshot"));
    assert!(
        check(preview_payload(&original, &scratch, &original_selection))
            .contains("Captured original text")
    );

    check(run(
        &original,
        &["backup", "--output", restored.to_str().unwrap()],
    ));
    grant_payloads(&restored, HARNESS);
    let error = failure(preview_payload(&restored, &scratch, &original_selection));
    assert!(error.contains("restored or replaced"));
    let refreshed = payload_selection(&restored, ROOT, Some(HARNESS));
    assert_eq!(
        refreshed["history_identity"],
        original_selection["history_identity"]
    );
    assert_ne!(refreshed["snapshot_id"], original_selection["snapshot_id"]);
    assert!(
        check(preview_payload(&restored, &scratch, &refreshed)).contains("Captured original text")
    );
}

#[test]
fn agent_harness_profile_maps_only_selected_payload_slots() {
    let scratch = Scratch::new();
    let file = scratch.0.join("agent-harness.jsonl");
    let mut root = span(ROOT, None, "OK");
    root["name"] = json!("agent.run");
    root["attributes"] = json!({
        "agent.task": "Summarize these notes", "agent.summary": "The summary is ready",
        "agent.cost_usd": 500.0, "agent.usage.input_tokens": 999,
        "agenturm.payload.input": PRIVATE,
    });
    let mut tool = span(CHILD, Some(ROOT), "OK");
    tool["name"] = json!("tool.call");
    tool["attributes"] = json!({
        "tool.arguments": "{\"path\":\"notes.txt\"}", "tool.result": "A bounded note",
        "tool.proposed_arguments": PRIVATE, "tool.error_message": PRIVATE,
    });
    tool["events"] = json!([{"name": "hitl.decision", "attributes": {"hitl.rationale": PRIVATE}}]);
    let mut model = span("3333333333333333", Some(ROOT), "OK");
    model["name"] = json!("llm.complete");
    model["attributes"] = json!({"tool.result": PRIVATE, "llm.cost_usd": 0.001,
        "llm.cost_known": true, "llm.usage.input_tokens": 17});
    write_jsonl(&file, &[root, tool, model]);
    let db = scratch.0.join("history.sqlite");
    let session = ["--session-id", SESSION];
    assert!(
        failure(import_file(
            &db,
            &file,
            &["--payload-format", "agent-harness"]
        ))
        .contains("--payload-scope")
    );
    check(import_file(&db, &file, &session));
    assert!(
        failure(import_file(
            &db,
            &file,
            &[
                "--session-id",
                SESSION,
                "--payload-scope",
                "standalone",
                "--payload-format",
                "agent-harness",
            ]
        ))
        .contains("Cannot commit")
    );

    let db = scratch.0.join("opted-in.sqlite");
    let options = [
        "--session-id",
        SESSION,
        "--payload-scope",
        "standalone",
        "--payload-format",
        "agent-harness",
    ];
    assert_eq!(
        check(import_file(&db, &file, &options)),
        "Imported 5 observations from 3 spans, 1 reported cost records and 1 token usage records.\n"
    );
    grant_payloads(&db, "standalone");
    for (span_id, input, output) in [
        (ROOT, "Summarize these notes", "The summary is ready"),
        (CHILD, "{\"path\":\"notes.txt\"}", "A bounded note"),
    ] {
        let selection = payload_selection(&db, span_id, None);
        let preview = check(preview_payload(&db, &scratch, &selection));
        assert_eq!(
            serde_json::from_str::<Value>(&preview).unwrap()["payload"],
            json!({"input": input, "output": output})
        );
        let metadata = check(run(&db, &["query", "--harness-id", SOURCE]));
        assert!(!metadata.contains(input) && !metadata.contains(output));
    }
    let model_selection = payload_selection(&db, "3333333333333333", None);
    assert!(
        !preview_payload(&db, &scratch, &model_selection)
            .status
            .success()
    );
    assert!(check(import_file(&db, &file, &options)).contains("Imported 0 observations"));
}

#[test]
fn invalid_payload_type_text_and_total_reject_before_admission() {
    let scratch = Scratch::new();
    let db = scratch.0.join("history.sqlite");
    let file = scratch.0.join("spans.jsonl");
    let scope = ["--payload-scope", HARNESS];
    let mut valid = span(ROOT, None, "OK");
    valid["attributes"]["agenturm.payload.input"] = json!("bounded model input");
    for payload in [
        Value::Null,
        json!([PRIVATE]),
        json!(format!("sk-{PRIVATE}")),
        json!("x".repeat(2049)),
    ] {
        let mut invalid = span(CHILD, Some(ROOT), "OK");
        invalid["attributes"]["agenturm.payload.output"] = payload;
        write_jsonl(&file, &[valid.clone(), invalid]);
        let error = failure(import_file(&db, &file, &scope));
        assert!(!error.contains("bounded model input"));
        assert!(!db.exists());
    }
    let mut rows: Vec<_> = (1..=33)
        .map(|id| {
            let mut row = span(&format!("{id:016x}"), None, "OK");
            row["attributes"]["agenturm.payload.input"] = json!("x".repeat(2048));
            row
        })
        .collect();
    write_jsonl(&file, &rows);
    assert!(failure(import_file(&db, &file, &scope)).contains("exceeds 64 KiB"));
    assert!(!db.exists());
    rows.pop();
    write_jsonl(&file, &rows);
    assert!(check(import_file(&db, &file, &scope)).contains("Imported 32 spans"));
}

fn grant_payloads(db: &Path, scope: &str) {
    check(run(
        db,
        &[
            "trace-payload",
            "grant",
            "--source-id",
            SOURCE,
            "--payload-scope",
            scope,
        ],
    ));
}

fn payload_selection(db: &Path, span_id: &str, native_source_id: Option<&str>) -> Value {
    let snapshot = check(run(
        db,
        &[
            "trace-payload",
            "snapshot",
            "--source-id",
            SOURCE,
            "--session-id",
            SESSION,
        ],
    ));
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
    json!({
        "history_identity": snapshot["history_identity"],
        "binding": {"source_id": SOURCE, "native_source_id": native_source_id},
        "trace_id": TRACE, "span_id": span_id, "session_id": SESSION,
        "turn_id": native_source_id.map(|_| TURN), "worker_id": native_source_id.map(|_| WORKER),
        "snapshot_id": snapshot["snapshot_id"],
    })
}

fn preview_payload(db: &Path, scratch: &Scratch, selection: &Value) -> Output {
    let path = scratch.0.join("selection.json");
    std::fs::write(&path, selection.to_string()).unwrap();
    run(
        db,
        &[
            "trace-payload",
            "preview",
            "--selection",
            path.to_str().unwrap(),
        ],
    )
}

fn span(id: &str, parent: Option<&str>, status: &str) -> Value {
    json!({
        "trace_id": TRACE, "span_id": id, "parent_span_id": parent,
        "start_unix_nano": START, "end_unix_nano": END.to_string(),
        "status": status, "name": PRIVATE,
        "attributes": {
            "session.id": SESSION, "agenturm.source.id": HARNESS,
            "agenturm.turn.id": TURN, "agenturm.worker.id": WORKER,
            "gen_ai.input.messages": PRIVATE,
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
    Store::open_read_only(db)
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
