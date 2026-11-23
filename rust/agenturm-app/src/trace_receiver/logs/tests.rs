use super::*;
use serde_json::{Value, json};

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
