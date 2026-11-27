use super::*;
use serde_json::json;

const FINDING: &str = "0123456789abcdef";
const TRACE: &str = "0123456789abcdef0123456789abcdef";
const SPAN: &str = "abcdef0123456789";
const RECEIVER: &str = "receiver";
const BODY: &str = "protected source text";

fn finding() -> Value {
    json!({
        "id":FINDING,"receiver_id":RECEIVER,"kind":"span_error","severity":"high",
        "producer":"fixture","service":"fixture","agent_id":"actor","session_id":"session",
        "trace_id":TRACE,"turn_id":null,"time_unix_nano":101,"occurrence_count":3,
        "trace_count":2,"session_count":1,"title":"private finding name","detail":BODY,
        "evidence":[{"type":"span","id":SPAN}]
    })
}

fn page_value(rows: Vec<Value>) -> Value {
    json!({
        "receiver_id":RECEIVER,"ownership_ready":true,"findings":rows,
        "next_cursor":null,"has_more":false
    })
}

fn projection(row: Value) -> LiveFindingPage {
    let local = LocalPolicy::default();
    project_page(
        serde_json::from_value(page_value(vec![row])).unwrap(),
        RECEIVER,
        None,
        &local,
        &local,
    )
    .unwrap()
}

fn exact_source() -> Value {
    let scope = json!({
        "service":"fixture","agent_id":"actor","session_id":"session",
        "trace_id":TRACE,"turn_id":null
    });
    let mut member_scope = scope.clone();
    member_scope["span_id"] = json!(SPAN);
    json!({
        "receiver_id":RECEIVER,"finding_id":FINDING,"finding_scope":scope,
        "references_total":1,"references_clipped":false,
        "sources":[{
            "type":"span","id":SPAN,"scope":member_scope,"complete":true,
            "record":{"name":"fixture","status":"ERROR","attributes":{"exception.message":BODY,"arbitrary":"opaque config material"},"resource_attributes":{}}
        }]
    })
}

#[test]
fn metadata_is_closed_and_private_availability_follows_a_pinned_policy() {
    let page = projection(finding());
    let row = &page.rows[0];
    assert_eq!(row.kind, LiveFindingKind::SpanError);
    assert_eq!(row.severity, LiveFindingSeverity::High);
    assert_eq!(row.occurrence_count, 3);
    assert_eq!(row.target.members[0].span_id.as_deref(), Some(SPAN));
    assert!(row.source_available);
    let evidence = format!("{page:?}");
    assert!(!evidence.contains("private finding name") && !evidence.contains(BODY));

    let before: LocalPolicy = serde_json::from_value(json!({
        "capture_content":true,"capture_policy_schema":1,"capture_global_revision":1
    }))
    .unwrap();
    let revoked: LocalPolicy = serde_json::from_value(json!({
        "capture_content":false,"capture_policy_schema":1,"capture_global_revision":2
    }))
    .unwrap();
    let changed = project_page(
        serde_json::from_value(page_value(vec![finding()])).unwrap(),
        RECEIVER,
        None,
        &before,
        &revoked,
    )
    .unwrap();
    assert!(!changed.rows[0].source_available);
    assert!(changed.rows[0].target.policy_witness.is_none());
    assert_eq!(changed.rows[0].target.members.len(), 1);
}

#[test]
fn continuation_requires_strictly_older_rows_at_a_bounded_key() {
    let cursor = "MTAxLDAxMjM0NTY3ODlhYmNkZWY";
    assert_eq!(cursor_key(cursor), Some((101, FINDING.into())));
    assert!(cursor_key("MTAxLDAxMjM0NTY3ODlhYmNkZWZ").is_none());
    assert!(cursor_key("/////////////").is_none());
    let policy = LocalPolicy::default();
    let validate = |row| {
        project_page(
            serde_json::from_value(page_value(vec![row])).unwrap(),
            RECEIVER,
            Some(cursor),
            &policy,
            &policy,
        )
    };
    assert!(validate(finding()).is_err());
    let mut old = finding();
    old["time_unix_nano"] = json!(100);
    assert!(validate(old).is_ok());
}

#[test]
fn source_requires_current_finding_owner_and_selected_member_identity() {
    let target = &projection(finding()).rows[0].target;
    let verify = |wire| {
        verify_source(
            serde_json::from_value(wire).unwrap(),
            target,
            &Owner::of(target),
        )
    };
    let source = verify(exact_source()).unwrap();
    let body: String = serde_json::from_slice(&private_body(&source).unwrap()).unwrap();
    assert!(body.contains(BODY));
    for (path, replacement) in [
        ("/receiver_id", json!("different receiver")),
        ("/finding_scope/agent_id", json!("other")),
        ("/sources/0/scope/session_id", json!("sibling")),
        ("/sources/0/scope/span_id", json!("1111111111111111")),
        ("/sources/0/id", json!("1111111111111111")),
        ("/references_total", json!(2)),
    ] {
        let mut response = exact_source();
        *response.pointer_mut(path).unwrap() = replacement;
        assert!(verify(response).is_err(), "{path}");
    }

    let mut missing = exact_source();
    missing["sources"][0]["scope"]
        .as_object_mut()
        .unwrap()
        .remove("turn_id");
    assert!(serde_json::from_value::<SourceWire>(missing).is_err());
}

#[test]
fn direct_records_omit_unrecognized_envelopes_and_reject_secret_fields() {
    let target = &projection(finding()).rows[0].target;
    let verify = |wire| {
        verify_source(
            serde_json::from_value(wire).unwrap(),
            target,
            &Owner::of(target),
        )
    };
    let mut malicious = exact_source();
    malicious["sources"][0]["record"]["raw_record"] = json!("unexpected body");
    assert!(verify(malicious).is_err());
    let protected =
        serde_json::from_slice::<String>(&private_body(&verify(exact_source()).unwrap()).unwrap())
            .unwrap();
    assert!(!protected.contains("opaque config material"));
    let mut secret = exact_source();
    secret["sources"][0]["record"]["attributes"]["exception.message"] =
        json!("authorization: hidden value");
    assert!(verify(secret).is_err());
    let mut malformed = exact_source();
    malformed["sources"][0]["record"]["attributes"]["exception.message"] =
        json!({"message":"ambiguous object"});
    assert!(verify(malformed).is_err());

    let mut new = exact_source();
    new["sources"][0]["record"]["attributes"]["exception.message"] = json!("different body");
    assert!(verify(new).unwrap() != verify(exact_source()).unwrap());
}
