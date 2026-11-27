use super::{decode_json as decode_json_bound, decode_protobuf as decode_protobuf_bound, *};
use serde_json::{Value, json};

fn decode_json(bytes: &[u8], source: &ExtensionId) -> Result<DecodedExport, String> {
    decode_json_bound(bytes, source, None, None)
}

fn decode_protobuf(bytes: &[u8], source: &ExtensionId) -> Result<DecodedExport, String> {
    decode_protobuf_bound(bytes, source, None, None)
}

fn source() -> ExtensionId {
    ExtensionId::parse("otlp.log-fixture").unwrap()
}

fn export(records: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "resourceLogs": [{
            "resource": {"attributes":[{"key":"private resource name","value":{}}]},
            "scopeLogs":[{"logRecords":records}]
        }]
    }))
    .unwrap()
}

fn scoped_record() -> Value {
    let text = |key: &str, value: &str| json!({"key":key,"value":{"stringValue":value}});
    json!({
        "eventName":"agent.activity",
        "severityNumber":9,
        "severityText":"private severity text",
        "body":{"stringValue":"private activity text"},
        "timeUnixNano":"123456789",
        "attributes":[
            text("observatory.record_id", "producer:receipt-1"),
            text("session.id", "native-session"),
            text("agenturm.source.id", "harness.codex.fixture"),
            text("agenturm.turn.id", "native-turn"),
            text("agenturm.worker.id", "worker-a"),
            text("agent.activity.result", "failed"),
            {"key":"gen_ai.usage.input_tokens","value":{"intValue":"0"}},
            {"key":"gen_ai.usage.total_tokens","value":{"intValue":"9007199254740993"}},
            {"key":"gen_ai.usage.output_tokens","value":{"boolValue":true}},
            text("gen_ai.usage.cost.currency","EUR"),
            text("gen_ai.usage.cost", "0.000000123"),
            text("agenturm.payload.input","private field"),
            text("agenturm.operation.id","ignored-operation")
        ]
    })
}

#[test]
fn exact_log_metadata_deduplicates_and_counts_unsupported_records() {
    let record = scoped_record();
    let mut unsupported = record.clone();
    unsupported["eventName"] = json!("private event");
    let mut unscoped = record.clone();
    unscoped["attributes"] = json!([]);
    let (rows, skipped, payloads) = decode_json(
        &export(vec![record.clone(), record, unsupported, unscoped]),
        &source(),
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(skipped, 2);
    assert!(payloads.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&json_response(skipped)).unwrap()["partialSuccess"]["rejectedLogRecords"],
        "2"
    );
    let row = &rows[0];
    assert_eq!(row.observed_at_unix_ns, 123456789);
    assert_eq!(row.revision, 0);
    assert_eq!(row.identity.session_id.as_deref(), Some("native-session"));
    assert_eq!(row.identity.turn_id.as_deref(), Some("native-turn"));
    assert_eq!(row.identity.worker_id.as_deref(), Some("worker-a"));
    assert!(row.identity.operation_id.is_none());
    let ObservationKind::OtlpActivityLog(activity) = &row.kind else {
        panic!("expected an activity log");
    };
    assert!(activity.is_valid() && activity.matches_event(&row.event_id));
    assert!(activity.reported_error);
    assert_eq!(
        activity.target_source_id.as_deref(),
        Some("harness.codex.fixture")
    );
    let usage = activity.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(0));
    assert_eq!(usage.total_tokens, Some(9_007_199_254_740_993));
    assert_eq!(usage.output_tokens, None);
    let cost = activity.cost.as_ref().unwrap();
    assert_eq!(cost.currency, "EUR");
    assert_eq!(cost.nanounits, 123);
    let retained = serde_json::to_string(&rows).unwrap();
    assert!(!retained.contains("private"));
    assert!(!retained.contains("producer:receipt-1"));
    assert!(!retained.contains("ignored-operation"));
}

#[test]
fn conflicting_binding_or_record_id_denies_the_whole_export() {
    let record = scoped_record();
    for attribute in [
        json!({"key":"agenturm.event.id","value":{"stringValue":"different-producer-id"}}),
        json!({"key":"gen_ai.conversation.id","value":{"stringValue":"other-session"}}),
    ] {
        let mut changed = record.clone();
        changed["attributes"]
            .as_array_mut()
            .unwrap()
            .push(attribute);
        assert!(decode_json(&export(vec![record.clone(), changed]), &source()).is_err());
    }
    let mut conflicting = record.clone();
    conflicting["timeUnixNano"] = json!("123456790");
    assert!(decode_json(&export(vec![record, conflicting]), &source()).is_err());

    for (key, value) in [
        ("agenturm.worker.id", "worker"),
        ("agenturm.turn.id", "turn"),
    ] {
        let record = json!({
            "eventName":EVENT_NAME,"timeUnixNano":"10",
            "attributes":[
                {"key":"agenturm.log.id","value":{"stringValue":"record"}},
                {"key":"session.id","value":{"stringValue":"session"}},
                {"key":key,"value":{"stringValue":value}}
            ]
        });
        assert!(decode_json(&export(vec![record]), &source()).is_err());
    }
}

fn message(tag: u8, bytes: &[u8]) -> Vec<u8> {
    assert!(bytes.len() < 128);
    let mut value = vec![tag << 3 | 2, bytes.len() as u8];
    value.extend_from_slice(bytes);
    value
}

#[test]
fn protobuf_projects_equal_identity_and_skips_arbitrary_body() {
    let mut record = vec![11 << 3 | 1];
    record.extend(15u64.to_le_bytes());
    record.extend([2 << 3, 18]);
    record.extend(message(5, &[0xff])); // Unknown retained body shape is never decoded.
    for (key, text) in [
        ("agenturm.log.id", "fixture"),
        ("gen_ai.conversation.id", "session"),
    ] {
        let mut attribute = message(1, key.as_bytes());
        attribute.extend(message(2, &message(1, text.as_bytes())));
        record.extend(message(6, &attribute));
    }
    record.extend(message(12, EVENT_NAME.as_bytes()));
    let wire = message(1, &message(2, &message(2, &record)));
    let (rows, skipped, payloads) = decode_protobuf(&wire, &source()).unwrap();
    let (json_rows, _, _) = decode_json(
        &export(vec![json!({
            "eventName":EVENT_NAME,"observedTimeUnixNano":15,"severityNumber":18,
            "attributes":[
                {"key":"agenturm.log.id","value":{"stringValue":"fixture"}},
                {"key":"session.id","value":{"stringValue":"session"}}
            ]
        })]),
        &source(),
    )
    .unwrap();
    assert_eq!(rows, json_rows);
    assert_eq!(skipped, 0);
    assert!(payloads.is_empty());
    assert!(decode_protobuf(&wire[..wire.len() - 1], &source()).is_err());

    use crate::trace_import::proto::{Field, fields};
    let response = protobuf_response(3);
    let (1, Field::Bytes(partial)) = fields(&response).next().unwrap().unwrap() else {
        panic!("expected a partial-success protobuf response");
    };
    assert!(matches!(
        fields(partial).next().unwrap(),
        Ok((1, Field::Varint(3)))
    ));
}

#[test]
fn admission_replays_once_and_conflicts_preserve_the_checkpoint() {
    use crate::trace_import::{admission_cursor, commit_with_store};
    use agenturm_store::Store;
    let source = source();
    let record = scoped_record();
    let decode = |record| decode_json(&export(vec![record]), &source).unwrap().0;
    let rows = decode(record.clone());
    let mut store = Store::open(":memory:").unwrap();
    assert_eq!(commit_with_store(&mut store, &source, &rows), Ok(1));
    let first = admission_cursor(&store, &source).unwrap();
    assert_eq!(commit_with_store(&mut store, &source, &rows), Ok(0));

    let mut divergent = record;
    divergent["timeUnixNano"] = json!("123456790");
    assert!(commit_with_store(&mut store, &source, &decode(divergent)).is_err());
    assert_eq!(admission_cursor(&store, &source).unwrap(), first);
}

fn error_record() -> Value {
    json!({
        "eventName": "private event name",
        "severityText": "private severity",
        "severityNumber": 17,
        "timeUnixNano": "0",
        "observedTimeUnixNano": "987654321",
        "body": {"stringValue":"private ordinary log text"}
    })
}

fn attribute(key: &str, text: &str) -> Value {
    json!({"key": key, "value": {"stringValue": text}})
}

#[test]
fn ordinary_errors_are_source_scoped_and_anonymous_replay_is_body_free() {
    use crate::trace_import::{admission_cursor, commit_with_store};
    use agenturm_store::Store;

    let record = error_record();
    let mut different_body = record.clone();
    different_body["body"] = json!({"arrayValue": {"values": [{"stringValue": "different"}]}});
    let mut no_time = record.clone();
    no_time["observedTimeUnixNano"] = json!("0");
    let mut info = record.clone();
    info["severityNumber"] = json!(16);
    let (rows, rejected, private) = decode_json(
        &export(vec![no_time, info, record.clone(), different_body.clone()]),
        &source(),
    )
    .unwrap();
    assert_eq!(rejected, 2);
    assert!(private.is_empty());
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.identity, ObservationIdentity::source());
    let ObservationKind::OtlpErrorLog(log) = &row.kind else {
        panic!("expected ordinary error metadata");
    };
    assert!(log.matches_event(&row.event_id));
    assert!(log.is_valid_at(row.observed_at_unix_ns));
    assert_eq!(log.severity_number, 17);
    assert_eq!(log.source_time_unix_ns, None);
    assert_eq!(log.observed_time_unix_ns, Some(987654321));
    assert_eq!(log.trace_id, None);
    assert_eq!(log.span_id, None);
    assert_eq!(log.target_source_id, None);
    assert!(!serde_json::to_string(&rows).unwrap().contains("private"));

    let source = source();
    let mut store = Store::open(":memory:").unwrap();
    assert_eq!(commit_with_store(&mut store, &source, &rows), Ok(1));
    let checkpoint = admission_cursor(&store, &source).unwrap();
    let (replay, _, _) = decode_json(&export(vec![different_body]), &source).unwrap();
    assert_eq!(rows, replay);
    assert_eq!(commit_with_store(&mut store, &source, &replay), Ok(0));
    assert_eq!(admission_cursor(&store, &source).unwrap(), checkpoint);
}

#[test]
fn native_scope_is_explicit_and_conflicts_never_fall_back_to_launch_scope() {
    let source = source();
    let mut record = error_record();
    record["traceId"] = json!("AB".repeat(16));
    record["spanId"] = json!("CD".repeat(8));
    record["attributes"] = json!([
        attribute("gen_ai.conversation.id", "native-session"),
        attribute("agenturm.turn.id", "turn"),
        attribute("agenturm.worker.id", "worker"),
    ]);
    let mut request = json!({
        "resourceLogs":[{
            "resource":{"attributes":[
                attribute("session.id", "native-session"),
                attribute("agenturm.source.id", "harness.codex.fixture")
            ]},
            "scopeLogs":[{"logRecords":[record]}]
        }]
    });
    let decode = |request: &Value| {
        decode_json_bound(
            &serde_json::to_vec(request).unwrap(),
            &source,
            Some("launch"),
            None,
        )
    };
    let (rows, _, _) = decode(&request).unwrap();
    assert_eq!(
        rows[0].identity.session_id.as_deref(),
        Some("native-session")
    );
    assert_eq!(rows[0].identity.turn_id.as_deref(), Some("turn"));
    assert_eq!(rows[0].identity.worker_id.as_deref(), Some("worker"));
    let ObservationKind::OtlpErrorLog(log) = &rows[0].kind else {
        panic!("expected ordinary error metadata");
    };
    assert_eq!(log.trace_id.as_deref(), Some("ab".repeat(16).as_str()));
    assert_eq!(log.span_id.as_deref(), Some("cd".repeat(8).as_str()));
    assert_eq!(
        log.target_source_id.as_deref(),
        Some("harness.codex.fixture")
    );

    request["resourceLogs"][0]["resource"]["attributes"][0] = attribute("session.id", "other");
    assert!(decode(&request).is_err());
    request["resourceLogs"][0]["resource"]["attributes"] =
        json!([attribute("agenturm.source.id", "harness.codex.fixture")]);
    request["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"] = json!([]);
    assert!(decode(&request).is_err());
    request["resourceLogs"][0]["resource"]["attributes"] = json!([]);
    let (rows, _, _) = decode(&request).unwrap();
    assert_eq!(rows[0].identity.session_id.as_deref(), Some("launch"));
    assert!(rows[0].identity.turn_id.is_none());
    assert!(rows[0].identity.worker_id.is_none());
}

#[test]
fn protobuf_and_json_error_records_share_identity_and_opt_in_text() {
    let source = source();
    let binding = TelemetryBinding {
        source_id: source.clone(),
        native_source_id: None,
    };
    let mut record = vec![11 << 3 | 1];
    record.extend(987654321u64.to_le_bytes());
    record.extend([2 << 3, 17]);
    record.extend(message(9, &[0xab; 16]));
    record.extend(message(10, &[0xcd; 8]));
    record.extend(message(5, &message(1, b"private ordinary log text")));
    let wire = message(1, &message(2, &message(2, &record)));
    let mut json_record = error_record();
    json_record["traceId"] = json!("ab".repeat(16));
    json_record["spanId"] = json!("cd".repeat(8));
    let json = export(vec![json_record.clone()]);
    let (json_rows, _, _) = decode_json(&json, &source).unwrap();
    let (pb_rows, _, _) = decode_protobuf(&wire, &source).unwrap();
    assert_eq!(json_rows, pb_rows);
    let json_capture = decode_json_bound(&json, &source, None, Some(&binding)).unwrap();
    let pb_capture = decode_protobuf_bound(&wire, &source, None, Some(&binding)).unwrap();
    assert_eq!(json_capture, pb_capture);
    assert_eq!(json_capture.0, json_rows);
    assert_eq!(json_capture.2[0].event_id, json_rows[0].event_id);
    assert_eq!(
        json_capture.2[0].payload.input.as_deref(),
        Some("private ordinary log text"),
    );
    assert!(json_capture.2[0].payload.output.is_none());
    let (_, _, payloads) = decode_json_bound(
        &export(vec![scoped_record(), json_record]),
        &source,
        None,
        Some(&binding),
    )
    .unwrap();
    assert_eq!(payloads.len(), 1);
}

#[test]
fn explicit_error_id_is_immutable_and_private_capture_checks_scope_and_bounds() {
    let source = source();
    let binding = TelemetryBinding {
        source_id: source.clone(),
        native_source_id: None,
    };
    let mut record = error_record();
    record["attributes"] = json!([attribute("agenturm.log.id", "record-1")]);
    record["observedTimeUnixNano"] = json!("0");
    let (rows, _, _) = decode_json(&export(vec![record.clone()]), &source).unwrap();
    assert_eq!(rows[0].observed_at_unix_ns, 0);
    assert!(!rows[0].event_id.contains("record-1"));
    let mut divergent = record.clone();
    divergent["severityNumber"] = json!(18);
    assert!(decode_json(&export(vec![record.clone(), divergent]), &source).is_err());

    let decode = |records| decode_json_bound(&export(records), &source, None, Some(&binding));
    let mut divergent = record.clone();
    divergent["body"] = json!({"stringValue":"different body"});
    assert!(decode(vec![record.clone(), divergent]).is_err());
    let mut invalid = record.clone();
    invalid["body"] = json!({"stringValue":"x".repeat(2049)});
    assert!(decode(vec![invalid]).is_err());
    let mut invalid = record.clone();
    invalid["body"] = json!({"stringValue":"contains\u{1b}control"});
    assert!(decode(vec![invalid]).is_err());
    let mut foreign = record.clone();
    foreign["attributes"] = json!([
        attribute("agenturm.log.id", "record-1"),
        attribute("agenturm.source.id", "harness.codex.other"),
        attribute("session.id", "native-session"),
    ]);
    assert!(decode(vec![foreign]).is_err());
    record["body"] = json!({"kvlistValue":{"values":[attribute("x", "private")]}});
    assert!(decode(vec![record]).unwrap().2.is_empty());
}
