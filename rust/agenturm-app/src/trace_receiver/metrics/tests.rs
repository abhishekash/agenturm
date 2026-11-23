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
