//! Body-free OTLP metric admission. JSON and protobuf share the same typed
//! series identity and replay path.

mod proto;
#[cfg(test)]
mod tests;

use agenturm_kernel::metric::{
    ExponentialBuckets, MAX_METRIC_BUCKETS, MAX_METRIC_EXEMPLARS, MAX_METRIC_POINTS_PER_BATCH,
    MetricData, MetricDescriptor, MetricExemplar, MetricFloat, MetricKind, MetricNumber,
    MetricPoint, MetricQuantile, MetricSeries, MetricTemporality, is_metric_label, is_metric_name,
    is_metric_source, is_metric_unit,
};
use agenturm_kernel::{ExtensionId, Observation, ObservationIdentity, ObservationKind};
use agenturm_store::metric_history::{metric_descriptor_event_id, valid_metric_description};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, btree_map::Entry};

const MALFORMED: &str = "Invalid OTLP metric export";
const MAX_ATTRIBUTES: usize = 64;
const MAX_ATTRIBUTE_DEPTH: usize = 8;
const MAX_METADATA_BYTES: usize = 4_096;

type Fingerprint = [u8; 32];
type DecodeResult<T> = Result<T, &'static str>;

#[derive(Default)]
struct Dimensions {
    values: BTreeMap<String, Fingerprint>,
    service: Option<String>,
}

impl Dimensions {
    fn insert(&mut self, key: &str, digest: Fingerprint, label: Option<&str>) -> DecodeResult<()> {
        if key.is_empty() || key.len() > MAX_METADATA_BYTES || self.values.len() >= MAX_ATTRIBUTES {
            return Err(MALFORMED);
        }
        match self.values.entry(key.to_owned()) {
            Entry::Vacant(slot) => slot.insert(digest),
            Entry::Occupied(_) => return Err(MALFORMED),
        };
        if key == "service.name" {
            self.service = label
                .filter(|value| is_metric_label(value))
                .map(str::to_owned);
        }
        Ok(())
    }

    fn fingerprint(&self) -> Fingerprint {
        let mut hash = Sha256::new();
        hash.update(b"otlp.metric.attributes/v1\0");
        for (key, digest) in &self.values {
            framed(&mut hash, key.as_bytes());
            hash.update(digest);
        }
        hash.finalize().into()
    }
}

struct Context {
    fingerprint: Fingerprint,
    service: Option<String>,
    scope: Option<String>,
}

fn framed(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn hash_value(tag: u8, value: &[u8]) -> Fingerprint {
    let mut hash = Sha256::new();
    hash.update([tag]);
    framed(&mut hash, value);
    hash.finalize().into()
}

fn array_fingerprint(
    values: impl Iterator<Item = DecodeResult<Fingerprint>>,
) -> DecodeResult<Fingerprint> {
    let mut hash = Sha256::new();
    hash.update([6]);
    for (index, value) in values.enumerate() {
        if index == MAX_ATTRIBUTES {
            return Err(MALFORMED);
        }
        hash.update(value?);
    }
    Ok(hash.finalize().into())
}

fn double_fingerprint(value: MetricFloat) -> Fingerprint {
    hash_value(4, &value.get().to_bits().to_be_bytes())
}

fn context(
    resource: &Dimensions,
    resource_schema: &str,
    scope: &Dimensions,
    scope_name: &str,
    scope_version: &str,
    scope_schema: &str,
) -> DecodeResult<Context> {
    let mut hash = Sha256::new();
    hash.update(b"otlp.metric.context/v1\0");
    for value in [resource_schema, scope_name, scope_version, scope_schema] {
        if value.len() > MAX_METADATA_BYTES {
            return Err(MALFORMED);
        }
        framed(&mut hash, value.as_bytes());
    }
    hash.update(resource.fingerprint());
    hash.update(scope.fingerprint());
    Ok(Context {
        fingerprint: hash.finalize().into(),
        service: resource.service.clone(),
        scope: is_metric_label(scope_name).then(|| scope_name.to_owned()),
    })
}

fn series(
    context: &Context,
    name: &str,
    unit: &str,
    kind: MetricKind,
    temporality: MetricTemporality,
    monotonic: Option<bool>,
) -> DecodeResult<MetricSeries> {
    if !is_metric_name(name) || !is_metric_unit(unit) {
        return Err(MALFORMED);
    }
    Ok(MetricSeries {
        id: String::new(),
        name: name.to_owned(),
        unit: unit.to_owned(),
        service: context.service.clone(),
        scope: context.scope.clone(),
        kind,
        temporality,
        monotonic,
    })
}

#[derive(Default)]
struct Batch {
    count: usize,
    rows: BTreeMap<String, Observation>,
    descriptions: BTreeMap<String, Option<DescribedPoint>>,
}

struct DescribedPoint {
    observed_at_unix_ns: u64,
    descriptor: MetricDescriptor,
}

impl Batch {
    fn push(
        &mut self,
        source: &ExtensionId,
        context: &Context,
        dimensions: &Dimensions,
        observed_at_unix_ns: u64,
        mut point: MetricPoint,
        description: Option<&str>,
    ) -> DecodeResult<()> {
        self.count += 1;
        if self.count > MAX_METRIC_POINTS_PER_BATCH {
            return Err("OTLP export exceeds 1024 metric points");
        }
        let mut hash = Sha256::new();
        hash.update(b"otlp.metric.series/v1\0");
        hash.update(context.fingerprint);
        hash.update(dimensions.fingerprint());
        hash.update(serde_json::to_vec(&point.series).map_err(|_| MALFORMED)?);
        point.series.id = format!("{:x}", hash.finalize());
        if !point.is_valid_at(observed_at_unix_ns) {
            return Err(MALFORMED);
        }
        let event_id = point.event_id(observed_at_unix_ns);
        if let Some(description) = description {
            self.describe(
                &point.series.id,
                &event_id,
                observed_at_unix_ns,
                description,
            );
        }
        let row = Observation {
            harness_id: source.clone(),
            event_id: event_id.clone(),
            revision: 0,
            identity: ObservationIdentity::source(),
            observed_at_unix_ns,
            kind: ObservationKind::MetricPoint(point),
        };
        match self.rows.entry(event_id) {
            Entry::Vacant(slot) => {
                slot.insert(row);
            }
            Entry::Occupied(slot) if slot.get() == &row => {}
            Entry::Occupied(_) => {
                return Err("Conflicting OTLP metric points for one series and timestamp");
            }
        }
        Ok(())
    }

    fn describe(
        &mut self,
        series: &str,
        point_event_id: &str,
        observed_at_unix_ns: u64,
        description: &str,
    ) {
        let candidate = || DescribedPoint {
            observed_at_unix_ns,
            descriptor: MetricDescriptor {
                series_id: series.into(),
                point_event_id: point_event_id.into(),
                description: description.into(),
            },
        };
        match self.descriptions.entry(series.to_owned()) {
            Entry::Vacant(slot) => {
                slot.insert(Some(candidate()));
            }
            Entry::Occupied(mut slot) => {
                let Some(prior) = slot.get() else {
                    return;
                };
                if prior.descriptor.description != description {
                    // Ambiguous metadata must not discard compatible points.
                    slot.insert(None);
                } else if (observed_at_unix_ns, point_event_id)
                    > (
                        prior.observed_at_unix_ns,
                        prior.descriptor.point_event_id.as_str(),
                    )
                {
                    slot.insert(Some(candidate()));
                }
            }
        }
    }

    fn into_rows(self, source: &ExtensionId) -> Vec<Observation> {
        self.rows
            .into_values()
            .chain(
                self.descriptions
                    .into_values()
                    .flatten()
                    .map(|row| Observation {
                        harness_id: source.clone(),
                        event_id: metric_descriptor_event_id(&row.descriptor),
                        revision: 0,
                        identity: ObservationIdentity::source(),
                        observed_at_unix_ns: row.observed_at_unix_ns,
                        kind: ObservationKind::MetricDescriptor(row.descriptor),
                    }),
            )
            .collect()
    }
}

pub(super) fn decode_json(body: &[u8], source: &ExtensionId) -> Result<Vec<Observation>, String> {
    if !is_metric_source(source.as_str()) {
        return Err(MALFORMED.into());
    }
    let payload: Value = serde_json::from_slice(body).map_err(|_| MALFORMED)?;
    json_batch(&payload, source).map_err(str::to_owned)
}

pub(super) fn decode_protobuf(
    body: &[u8],
    source: &ExtensionId,
) -> Result<Vec<Observation>, String> {
    if !is_metric_source(source.as_str()) {
        return Err(MALFORMED.into());
    }
    proto::decode(body, source).map_err(str::to_owned)
}

fn json_batch(payload: &Value, source: &ExtensionId) -> DecodeResult<Vec<Observation>> {
    if !payload.is_object() {
        return Err(MALFORMED);
    }
    let mut batch = Batch::default();
    for group in array(payload, "resourceMetrics")? {
        if !group.is_object() {
            return Err(MALFORMED);
        }
        let resource = json_dimensions(group.get("resource").unwrap_or(&Value::Null), 0)?;
        for scope in array(group, "scopeMetrics")? {
            if !scope.is_object() {
                return Err(MALFORMED);
            }
            let instrumentation = scope.get("scope").unwrap_or(&Value::Null);
            let context = context(
                &resource,
                optional_string(group, "schemaUrl")?.unwrap_or_default(),
                &json_dimensions(instrumentation, 0)?,
                optional_string(instrumentation, "name")?.unwrap_or_default(),
                optional_string(instrumentation, "version")?.unwrap_or_default(),
                optional_string(scope, "schemaUrl")?.unwrap_or_default(),
            )?;
            for metric in array(scope, "metrics")? {
                let (kind, content) = only_one(
                    [
                        ("gauge", MetricKind::Gauge),
                        ("sum", MetricKind::Sum),
                        ("histogram", MetricKind::Histogram),
                        ("exponentialHistogram", MetricKind::ExponentialHistogram),
                        ("summary", MetricKind::Summary),
                    ]
                    .into_iter()
                    .filter_map(|(key, kind)| metric.get(key).map(|value| (kind, value))),
                )?
                .ok_or(MALFORMED)?;
                if !content.is_object() {
                    return Err(MALFORMED);
                }
                let temporality = json_temporality(content.get("aggregationTemporality"))?;
                let monotonic = content
                    .get("isMonotonic")
                    .map(|value| value.as_bool().ok_or(MALFORMED))
                    .transpose()?;
                let series = series(
                    &context,
                    optional_string(metric, "name")?.ok_or(MALFORMED)?,
                    optional_string(metric, "unit")?.unwrap_or_default(),
                    kind,
                    temporality,
                    monotonic,
                )?;
                let description = metric
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|description| valid_metric_description(description));
                for raw in array(content, "dataPoints")? {
                    let (at, point) = json_point(&series, raw)?;
                    batch.push(
                        source,
                        &context,
                        &json_dimensions(raw, 0)?,
                        at,
                        point,
                        description,
                    )?;
                }
            }
        }
    }
    Ok(batch.into_rows(source))
}

fn only_one<T>(mut values: impl Iterator<Item = T>) -> DecodeResult<Option<T>> {
    let found = values.next();
    if values.next().is_some() {
        return Err(MALFORMED);
    }
    Ok(found)
}

fn array<'a>(value: &'a Value, key: &str) -> DecodeResult<&'a [Value]> {
    match value.get(key) {
        Some(value) => value.as_array().map(Vec::as_slice).ok_or(MALFORMED),
        None => Ok(&[]),
    }
}

fn optional_string<'a>(value: &'a Value, key: &str) -> DecodeResult<Option<&'a str>> {
    value
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| value.as_str().ok_or(MALFORMED))
        .transpose()
}

fn json_u64(value: &Value) -> DecodeResult<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or(MALFORMED)
}

fn json_i64(value: &Value) -> DecodeResult<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or(MALFORMED)
}

fn json_float(value: &Value) -> DecodeResult<MetricFloat> {
    let number = match value.as_str() {
        Some("NaN") => f64::NAN,
        Some("Infinity") => f64::INFINITY,
        Some("-Infinity") => f64::NEG_INFINITY,
        Some(_) => return Err(MALFORMED),
        None => value.as_f64().ok_or(MALFORMED)?,
    };
    Ok(MetricFloat::new(number))
}

fn value<'a, T>(
    item: &'a Value,
    key: &str,
    parse: impl FnOnce(&'a Value) -> DecodeResult<T>,
) -> DecodeResult<Option<T>> {
    item.get(key)
        .filter(|value| !value.is_null())
        .map(parse)
        .transpose()
}

fn json_temporality(raw: Option<&Value>) -> DecodeResult<MetricTemporality> {
    let number = match raw {
        None => 0,
        Some(value) => match value.as_str() {
            Some("AGGREGATION_TEMPORALITY_UNSPECIFIED") => 0,
            Some("AGGREGATION_TEMPORALITY_DELTA") => 1,
            Some("AGGREGATION_TEMPORALITY_CUMULATIVE") => 2,
            _ => json_u64(value)?,
        },
    };
    temporality(number)
}

fn temporality(value: u64) -> DecodeResult<MetricTemporality> {
    match value {
        0 => Ok(MetricTemporality::Unspecified),
        1 => Ok(MetricTemporality::Delta),
        2 => Ok(MetricTemporality::Cumulative),
        _ => Err(MALFORMED),
    }
}

fn json_number(raw: &Value) -> DecodeResult<Option<MetricNumber>> {
    match (
        value(raw, "asInt", json_i64)?,
        value(raw, "asDouble", json_float)?,
    ) {
        (None, None) => Ok(None),
        (Some(value), None) => Ok(Some(MetricNumber::Int(value))),
        (None, Some(value)) => Ok(Some(MetricNumber::Double(value))),
        _ => Err(MALFORMED),
    }
}

fn json_array<'a, T>(
    raw: &'a Value,
    key: &str,
    limit: usize,
    parse: impl FnMut(&'a Value) -> DecodeResult<T>,
) -> DecodeResult<Vec<T>> {
    let raw = array(raw, key)?;
    if raw.len() > limit {
        return Err(MALFORMED);
    }
    raw.iter().map(parse).collect()
}

fn json_buckets(raw: &Value) -> DecodeResult<ExponentialBuckets> {
    if !raw.is_object() {
        return Err(MALFORMED);
    }
    Ok(ExponentialBuckets {
        offset: value(raw, "offset", json_i64)?
            .map(i32::try_from)
            .transpose()
            .map_err(|_| MALFORMED)?
            .unwrap_or_default(),
        bucket_counts: json_array(raw, "bucketCounts", MAX_METRIC_BUCKETS, json_u64)?,
    })
}

fn json_point(series: &MetricSeries, raw: &Value) -> DecodeResult<(u64, MetricPoint)> {
    let at = value(raw, "timeUnixNano", json_u64)?.ok_or(MALFORMED)?;
    let start_unix_ns = value(raw, "startTimeUnixNano", json_u64)?.filter(|start| *start != 0);
    let flags = value(raw, "flags", json_u64)?
        .map(u32::try_from)
        .transpose()
        .map_err(|_| MALFORMED)?
        .unwrap_or_default();
    let exemplars = json_array(raw, "exemplars", MAX_METRIC_EXEMPLARS, |value| {
        Ok(MetricExemplar {
            time_unix_ns: json_u64(value.get("timeUnixNano").ok_or(MALFORMED)?)?,
            value: json_number(value)?,
            trace_id: optional_string(value, "traceId")?
                .filter(|id| !id.is_empty())
                .map(str::to_ascii_lowercase),
            span_id: optional_string(value, "spanId")?
                .filter(|id| !id.is_empty())
                .map(str::to_ascii_lowercase),
        })
    })?;
    let count = || value(raw, "count", json_u64);
    let sum = || value(raw, "sum", json_float);
    let data = match series.kind {
        MetricKind::Gauge => MetricData::Gauge {
            value: json_number(raw)?,
        },
        MetricKind::Sum => MetricData::Sum {
            value: json_number(raw)?,
        },
        MetricKind::Histogram => MetricData::Histogram {
            count: count()?,
            sum: sum()?,
            min: value(raw, "min", json_float)?,
            max: value(raw, "max", json_float)?,
            explicit_bounds: json_array(raw, "explicitBounds", MAX_METRIC_BUCKETS - 1, json_float)?,
            bucket_counts: json_array(raw, "bucketCounts", MAX_METRIC_BUCKETS, json_u64)?,
        },
        MetricKind::ExponentialHistogram => MetricData::ExponentialHistogram {
            count: count()?,
            sum: sum()?,
            min: value(raw, "min", json_float)?,
            max: value(raw, "max", json_float)?,
            scale: value(raw, "scale", json_i64)?
                .map(i32::try_from)
                .transpose()
                .map_err(|_| MALFORMED)?,
            zero_count: value(raw, "zeroCount", json_u64)?,
            zero_threshold: value(raw, "zeroThreshold", json_float)?,
            positive: value(raw, "positive", json_buckets)?,
            negative: value(raw, "negative", json_buckets)?,
        },
        MetricKind::Summary => MetricData::Summary {
            count: count()?,
            sum: sum()?,
            quantiles: json_array(raw, "quantileValues", MAX_METRIC_BUCKETS, |item| {
                if !item.is_object() {
                    return Err(MALFORMED);
                }
                Ok(MetricQuantile {
                    quantile: value(item, "quantile", json_float)?
                        .unwrap_or_else(|| MetricFloat::new(0.0)),
                    value: value(item, "value", json_float)?
                        .unwrap_or_else(|| MetricFloat::new(0.0)),
                })
            })?,
        },
    };
    Ok((
        at,
        MetricPoint {
            series: series.clone(),
            start_unix_ns,
            flags,
            data,
            exemplars,
        },
    ))
}

fn json_dimensions(item: &Value, depth: usize) -> DecodeResult<Dimensions> {
    let mut dimensions = Dimensions::default();
    for value in array(item, "attributes")? {
        json_dimension(&mut dimensions, value, depth)?;
    }
    Ok(dimensions)
}

fn json_dimension(
    dimensions: &mut Dimensions,
    attribute: &Value,
    depth: usize,
) -> DecodeResult<()> {
    let key = optional_string(attribute, "key")?.ok_or(MALFORMED)?;
    let value = attribute.get("value").unwrap_or(&Value::Null);
    dimensions.insert(
        key,
        json_attribute(value, depth)?,
        value.get("stringValue").and_then(Value::as_str),
    )
}

fn json_attribute(value: &Value, depth: usize) -> DecodeResult<Fingerprint> {
    if depth >= MAX_ATTRIBUTE_DEPTH {
        return Err(MALFORMED);
    }
    if value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(hash_value(0, b""));
    }
    let object = value.as_object().ok_or(MALFORMED)?;
    let (key, field) = only_one(object.iter())?.ok_or(MALFORMED)?;
    match key.as_str() {
        "stringValue" => Ok(hash_value(1, field.as_str().ok_or(MALFORMED)?.as_bytes())),
        "boolValue" => Ok(hash_value(
            2,
            &[u8::from(field.as_bool().ok_or(MALFORMED)?)],
        )),
        "intValue" => Ok(hash_value(3, &json_i64(field)?.to_be_bytes())),
        "doubleValue" => Ok(double_fingerprint(json_float(field)?)),
        "bytesValue" => Ok(hash_value(5, &base64(field.as_str().ok_or(MALFORMED)?)?)),
        "arrayValue" => array_fingerprint(
            array(field, "values")?
                .iter()
                .map(|value| json_attribute(value, depth + 1)),
        ),
        "kvlistValue" => {
            let mut dimensions = Dimensions::default();
            for item in array(field, "values")? {
                json_dimension(&mut dimensions, item, depth + 1)?;
            }
            Ok(hash_value(7, &dimensions.fingerprint()))
        }
        _ => Err(MALFORMED),
    }
}

fn base64(value: &str) -> DecodeResult<Vec<u8>> {
    let data = value.trim_end_matches('=');
    let padding = value.len() - data.len();
    if padding > 2 || (padding != 0 && !value.len().is_multiple_of(4)) || data.len() % 4 == 1 {
        return Err(MALFORMED);
    }
    let mut output = Vec::with_capacity(data.len() / 4 * 3 + 2);
    let mut pending = 0u16;
    let mut bits = 0;
    for byte in data.bytes() {
        let index = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err(MALFORMED),
        };
        pending = (pending << 6) | u16::from(index);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((pending >> bits) as u8);
            pending &= (1 << bits) - 1;
        }
    }
    if pending != 0 {
        return Err(MALFORMED);
    }
    Ok(output)
}
