use super::*;
use serde_json::json;

const JSON_EXPORT: &[u8] = include_bytes!("fixtures/sdk-metrics.json");
const PROTO_EXPORT: &[u8] = include_bytes!("fixtures/sdk-metrics.pb");

#[test]
fn sdk_wire_forms_preserve_metric_shapes_dimensions_exemplars_and_presence() {
    let source = ExtensionId::parse("otlp.fixture").unwrap();
    let from_json = decode_json(JSON_EXPORT, &source).unwrap();
    let from_proto = decode_protobuf(PROTO_EXPORT, &source).unwrap();
    assert_eq!(from_json, from_proto);
    assert_eq!(from_json.len(), 7);
    let point = |at| {
        let row = from_json
            .iter()
            .find(|row| row.observed_at_unix_ns == at)
            .expect("fixture metric timestamp");
        assert_eq!(row.harness_id, source);
        assert_eq!(row.identity, ObservationIdentity::source());
        let ObservationKind::MetricPoint(point) = &row.kind else {
            panic!("metric point observation");
        };
        assert_eq!(row.event_id, point.event_id(at));
        assert_eq!(point.series.service.as_deref(), Some("synthetic"));
        assert_eq!(point.series.scope.as_deref(), Some("metrics-fixture"));
        point
    };
    let gauge = point(14);
    assert_eq!(
        gauge.data,
        MetricData::Gauge {
            value: Some(MetricNumber::Int(i64::MIN))
        }
    );
    let exemplar = &gauge.exemplars[0];
    assert_eq!(
        exemplar.trace_id.as_deref(),
        Some("abababababababababababababababab")
    );
    assert_eq!(exemplar.span_id.as_deref(), Some("cdcdcdcdcdcdcdcd"));
    assert_eq!(
        exemplar.value,
        Some(MetricNumber::Double(MetricFloat::new(-0.0)))
    );
    assert_ne!(gauge.series.id, point(15).series.id);
    assert_eq!(point(15).series.id, point(16).series.id);
    assert!(point(15).no_recorded_value());
    assert_eq!(point(15).data, MetricData::Gauge { value: None });
    assert_eq!(
        point(16).data,
        MetricData::Gauge {
            value: Some(MetricNumber::Int(0))
        }
    );
    assert_eq!(point(20).series.temporality, MetricTemporality::Cumulative);
    assert_eq!(point(20).series.monotonic, Some(true));
    assert_eq!(
        point(20).data,
        MetricData::Sum {
            value: Some(MetricNumber::Int(i64::MAX))
        }
    );
    assert_eq!(point(21).series.temporality, MetricTemporality::Delta);
    assert!(matches!(
        &point(21).data,
        MetricData::Histogram { count: Some(3), bucket_counts, explicit_bounds, .. }
        if bucket_counts == &[1, 2, 0] && explicit_bounds.len() == 2,
    ));
    assert!(matches!(
        &point(22).data,
        MetricData::ExponentialHistogram { scale: Some(-2), zero_count: Some(2), positive: Some(buckets), .. }
        if buckets.offset == -2 && buckets.bucket_counts == [1, 2],
    ));
    assert!(matches!(
        &point(23).data,
        MetricData::Summary { count: Some(7), quantiles, .. }
        if quantiles.len() == 3 && quantiles[0].quantile.get() == 0.0,
    ));
    for row in &from_json {
        let durable = format!("{:?}", row.kind);
        for attribute in [
            "private-exemplar",
            "exemplar-body-marker",
            "hidden-point",
            "point-body-marker",
            "secret-resource",
            "resource-body-marker",
            "hidden-scope",
        ] {
            assert!(
                !durable.contains(attribute),
                "unlisted field escaped admission"
            );
        }
    }
}

fn gauge_export(points: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[
        {"name":"test.gauge","gauge":{"dataPoints":points}}
    ]}]}]}))
    .unwrap()
}

#[test]
fn metric_export_rejects_conflicts_malformed_oneofs_and_over_limit_replays() {
    let source = ExtensionId::parse("otlp.fixture").unwrap();
    let row = json!({"timeUnixNano":"17","asInt":"0"});
    assert_eq!(
        decode_json(&gauge_export(vec![row.clone(), row.clone()]), &source)
            .unwrap()
            .len(),
        1
    );
    for invalid in [
        json!({"timeUnixNano":"17","asInt":"2"}),
        json!({"timeUnixNano":"19","asInt":"0","asDouble":0.0}),
        json!({"timeUnixNano":"20","attributes":[{"key":"same","value":{"intValue":"0"}},{"key":"same","value":{"stringValue":"x"}}]}),
    ] {
        assert!(decode_json(&gauge_export(vec![row.clone(), invalid]), &source).is_err());
    }
    let exact_limit = vec![row.clone(); MAX_METRIC_POINTS_PER_BATCH];
    assert!(decode_json(&gauge_export(exact_limit), &source).is_ok());
    let oversized = vec![row; MAX_METRIC_POINTS_PER_BATCH + 1];
    assert!(decode_json(&gauge_export(oversized), &source).is_err());
    assert!(decode_protobuf(&PROTO_EXPORT[..PROTO_EXPORT.len() - 1], &source).is_err());
}

#[test]
fn descriptor_cross_wire_replay_preserves_point_observation_bytes() {
    use crate::trace_import::{admission_cursor, commit_with_store};
    use agenturm_kernel::retained_history::{MetricHistoryPage, MetricHistoryQuery};
    use agenturm_store::Store;

    let source = ExtensionId::parse("otlp.descriptions").unwrap();
    let mut payload: Value = serde_json::from_slice(&gauge_export(vec![
        json!({"timeUnixNano":"17","asInt":"0"}),
        json!({"timeUnixNano":"19","asInt":"0"}),
    ]))
    .unwrap();
    let old = json_batch(&payload, &source).unwrap();
    let initial = "Request throughput per second";
    payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["description"] = json!(initial);
    let described = json_batch(&payload, &source).unwrap();
    assert_eq!(described.len(), old.len() + 1);
    for (prior, retained) in old.iter().zip(&described) {
        assert_eq!(
            serde_json::to_vec(prior).unwrap(),
            serde_json::to_vec(retained).unwrap()
        );
    }

    let protobuf = described_protobuf(initial.as_bytes());
    assert_eq!(decode_protobuf(&protobuf, &source).unwrap(), described);
    let ObservationKind::MetricDescriptor(descriptor) = &described[2].kind else {
        panic!("expected one descriptor for two points in the same series");
    };
    assert_eq!(descriptor.point_event_id, old[1].event_id);
    assert_eq!(described[2].observed_at_unix_ns, 19);

    let prior = |store: &Store| admission_cursor(store, &source).unwrap();
    let mut store = Store::open(":memory:").unwrap();
    assert_eq!(commit_with_store(&mut store, &source, &old), Ok(2));
    assert_eq!(commit_with_store(&mut store, &source, &described), Ok(1));
    let checkpoint = prior(&store);
    let replay = decode_protobuf(&protobuf, &source).unwrap();
    assert_eq!(commit_with_store(&mut store, &source, &replay), Ok(0));
    assert_eq!(prior(&store), checkpoint);
    let query = |snapshot_id| MetricHistoryQuery::Series {
        source_id: Some(source.clone()),
        snapshot_id,
        after: None,
        search: None,
        limit: 1,
    };
    let MetricHistoryPage::Series {
        snapshot_id, rows, ..
    } = store.query_metric_history(&query(None), None).unwrap()
    else {
        panic!("described series");
    };
    assert_eq!(rows[0].description.as_deref(), Some(initial));

    let revised = "Accepted request throughput";
    payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["description"] = json!(revised);
    let correction = json_batch(&payload, &source).unwrap();
    assert_eq!(commit_with_store(&mut store, &source, &correction), Ok(1));
    assert_eq!(commit_with_store(&mut store, &source, &replay), Ok(0));
    for (pin, expected) in [(None, revised), (Some(snapshot_id), initial)] {
        let MetricHistoryPage::Series { rows, .. } =
            store.query_metric_history(&query(pin), None).unwrap()
        else {
            panic!("described series");
        };
        assert_eq!(rows[0].description.as_deref(), Some(expected));
    }

    for omitted in [
        json!(" "),
        json!("x".repeat(513)),
        json!("Authorization: example credential"),
        json!("Escapes\u{001b}"),
        json!({"body":"unknown format"}),
    ] {
        payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["description"] = omitted;
        assert_eq!(json_batch(&payload, &source).unwrap(), old);
    }
}

#[test]
fn conflicting_descriptions_do_not_discard_otherwise_valid_points() {
    let source = ExtensionId::parse("otlp.descriptions").unwrap();
    let mut payload: Value = serde_json::from_slice(&gauge_export(vec![
        json!({"timeUnixNano":"17","asInt":"0"}),
    ]))
    .unwrap();
    let points = json_batch(&payload, &source).unwrap();
    let instrument = &mut payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
    instrument[0]["description"] = json!("Reported latency");
    let mut duplicate = instrument[0].clone();
    duplicate["description"] = json!("Conflicting latency descriptor");
    instrument.as_array_mut().unwrap().push(duplicate);
    assert_eq!(json_batch(&payload, &source).unwrap(), points);
}

fn described_protobuf(description: &[u8]) -> Vec<u8> {
    fn message(tag: u8, bytes: &[u8]) -> Vec<u8> {
        assert!(bytes.len() < 128);
        let mut value = vec![tag << 3 | 2, bytes.len() as u8];
        value.extend_from_slice(bytes);
        value
    }
    let mut gauge = Vec::new();
    for at in [17u64, 19u64] {
        let mut point = vec![3 << 3 | 1];
        point.extend(at.to_le_bytes());
        point.push(6 << 3 | 1);
        point.extend(0i64.to_le_bytes());
        gauge.extend(message(1, &point));
    }
    let mut metric = message(1, b"test.gauge");
    metric.extend(message(2, description));
    metric.extend(message(5, &gauge));
    message(1, &message(2, &message(2, &metric)))
}
