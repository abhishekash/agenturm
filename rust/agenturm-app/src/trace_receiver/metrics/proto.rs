use super::*;
use crate::trace_import::proto::{Field, fields, varint};
use std::fmt::Write as _;

pub(super) fn decode(body: &[u8], source: &ExtensionId) -> DecodeResult<Vec<Observation>> {
    let mut batch = Batch::default();
    for field in wire_fields(body) {
        if let (1, Field::Bytes(group)) = field? {
            resource_metrics(group, source, &mut batch)?;
        }
    }
    Ok(batch.into_rows())
}

fn wire_fields(input: &[u8]) -> impl Iterator<Item = DecodeResult<(u64, Field<'_>)>> {
    fields(input).map(|field| field.map_err(|_| MALFORMED))
}

fn text(bytes: &[u8]) -> DecodeResult<&str> {
    std::str::from_utf8(bytes).map_err(|_| MALFORMED)
}

fn set<T>(slot: &mut Option<T>, value: T) -> DecodeResult<()> {
    if slot.is_some() {
        return Err(MALFORMED);
    }
    *slot = Some(value);
    Ok(())
}

fn append<T>(list: &mut Vec<T>, value: T, limit: usize) -> DecodeResult<()> {
    if list.len() == limit {
        return Err(MALFORMED);
    }
    list.push(value);
    Ok(())
}

fn resource_metrics(input: &[u8], source: &ExtensionId, batch: &mut Batch) -> DecodeResult<()> {
    let mut resource = None;
    let mut schema = None;
    for field in wire_fields(input) {
        match field? {
            (1, Field::Bytes(bytes)) => set(&mut resource, dimensions(bytes, 1)?)?,
            (3, Field::Bytes(bytes)) => set(&mut schema, text(bytes)?)?,
            _ => {}
        }
    }
    let resource = resource.unwrap_or_default();
    for field in wire_fields(input) {
        if let (2, Field::Bytes(scope)) = field? {
            scope_metrics(scope, source, batch, &resource, schema.unwrap_or_default())?;
        }
    }
    Ok(())
}

fn scope_metrics(
    input: &[u8],
    source: &ExtensionId,
    batch: &mut Batch,
    resource: &Dimensions,
    resource_schema: &str,
) -> DecodeResult<()> {
    let mut instrumentation = None;
    let mut schema = None;
    for field in wire_fields(input) {
        match field? {
            (1, Field::Bytes(bytes)) => set(&mut instrumentation, bytes)?,
            (3, Field::Bytes(bytes)) => set(&mut schema, text(bytes)?)?,
            _ => {}
        }
    }
    let instrumentation = instrumentation.unwrap_or_default();
    let (mut name, mut version) = (None, None);
    for field in wire_fields(instrumentation) {
        match field? {
            (1, Field::Bytes(bytes)) => set(&mut name, text(bytes)?)?,
            (2, Field::Bytes(bytes)) => set(&mut version, text(bytes)?)?,
            _ => {}
        }
    }
    let context = context(
        resource,
        resource_schema,
        &dimensions(instrumentation, 3)?,
        name.unwrap_or_default(),
        version.unwrap_or_default(),
        schema.unwrap_or_default(),
    )?;
    for field in wire_fields(input) {
        if let (2, Field::Bytes(instrument)) = field? {
            metric(instrument, source, batch, &context)?;
        }
    }
    Ok(())
}

fn metric(
    input: &[u8],
    source: &ExtensionId,
    batch: &mut Batch,
    context: &Context,
) -> DecodeResult<()> {
    let (mut name, mut unit, mut payload) = (None, None, None);
    for field in wire_fields(input) {
        let (number, Field::Bytes(bytes)) = field? else {
            continue;
        };
        let kind = match number {
            1 => {
                set(&mut name, text(bytes)?)?;
                continue;
            }
            3 => {
                set(&mut unit, text(bytes)?)?;
                continue;
            }
            5 => MetricKind::Gauge,
            7 => MetricKind::Sum,
            9 => MetricKind::Histogram,
            10 => MetricKind::ExponentialHistogram,
            11 => MetricKind::Summary,
            _ => continue,
        };
        set(&mut payload, (kind, bytes))?;
    }
    let (kind, data) = payload.ok_or(MALFORMED)?;
    let (mut aggregation, mut monotonic) = (None, None);
    for field in wire_fields(data) {
        match field? {
            (2, Field::Varint(value))
                if matches!(
                    kind,
                    MetricKind::Sum | MetricKind::Histogram | MetricKind::ExponentialHistogram
                ) =>
            {
                set(&mut aggregation, temporality(value)?)?;
            }
            (3, Field::Varint(value)) if kind == MetricKind::Sum => {
                set(&mut monotonic, boolean(value)?)?
            }
            _ => {}
        }
    }
    let series = series(
        context,
        name.ok_or(MALFORMED)?,
        unit.unwrap_or_default(),
        kind,
        aggregation.unwrap_or(MetricTemporality::Unspecified),
        monotonic,
    )?;
    for field in wire_fields(data) {
        if let (1, Field::Bytes(bytes)) = field? {
            let (observed_at, attributes, point) = point(bytes, &series)?;
            batch.push(source, context, &attributes, observed_at, point)?;
        }
    }
    Ok(())
}

fn float(bits: u64) -> MetricFloat {
    MetricFloat::new(f64::from_bits(bits))
}

fn zigzag(value: u64) -> DecodeResult<i32> {
    let value = u32::try_from(value).map_err(|_| MALFORMED)?;
    Ok((value >> 1) as i32 ^ -((value & 1) as i32))
}

fn boolean(value: u64) -> DecodeResult<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(MALFORMED),
    }
}

fn point(input: &[u8], series: &MetricSeries) -> DecodeResult<(u64, Dimensions, MetricPoint)> {
    let (attribute_field, flags_field, exemplar_field) = match series.kind {
        MetricKind::Gauge | MetricKind::Sum => (7, 8, Some(5)),
        MetricKind::Histogram => (9, 10, Some(8)),
        MetricKind::ExponentialHistogram => (1, 10, Some(11)),
        MetricKind::Summary => (7, 8, None),
    };
    let (mut at, mut start, mut flags) = (None, None, None);
    let mut exemplars = Vec::new();
    let mut dimensions = Dimensions::default();
    for field in wire_fields(input) {
        match field? {
            (2, Field::Fixed64(value)) => set(&mut start, value)?,
            (3, Field::Fixed64(value)) => set(&mut at, value)?,
            (number, Field::Varint(value)) if number == flags_field => {
                set(&mut flags, u32::try_from(value).map_err(|_| MALFORMED)?)?;
            }
            (number, Field::Bytes(value)) if number == attribute_field => {
                attribute(&mut dimensions, value, 0)?
            }
            (number, Field::Bytes(value)) if Some(number) == exemplar_field => {
                append(&mut exemplars, exemplar(value)?, MAX_METRIC_EXEMPLARS)?;
            }
            _ => {}
        }
    }
    let data = match series.kind {
        MetricKind::Gauge => MetricData::Gauge {
            value: number(input, 4)?,
        },
        MetricKind::Sum => MetricData::Sum {
            value: number(input, 4)?,
        },
        MetricKind::Histogram => histogram(input)?,
        MetricKind::ExponentialHistogram => exponential(input)?,
        MetricKind::Summary => summary(input)?,
    };
    Ok((
        at.ok_or(MALFORMED)?,
        dimensions,
        MetricPoint {
            series: series.clone(),
            start_unix_ns: start.filter(|value| *value != 0),
            flags: flags.unwrap_or_default(),
            data,
            exemplars,
        },
    ))
}

fn number(input: &[u8], double_field: u64) -> DecodeResult<Option<MetricNumber>> {
    let mut value = None;
    for field in wire_fields(input) {
        match field? {
            (number, Field::Fixed64(bits)) if number == double_field => {
                set(&mut value, MetricNumber::Double(float(bits)))?
            }
            (6, Field::Fixed64(bits)) => set(&mut value, MetricNumber::Int(bits as i64))?,
            _ => {}
        }
    }
    Ok(value)
}

fn id(bytes: &[u8], limit: usize) -> DecodeResult<Option<String>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() != limit {
        return Err(MALFORMED);
    }
    let mut encoded = String::with_capacity(limit * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("write to string");
    }
    Ok(Some(encoded))
}

fn exemplar(input: &[u8]) -> DecodeResult<MetricExemplar> {
    let (mut at, mut span, mut trace) = (None, None, None);
    for field in wire_fields(input) {
        match field? {
            (2, Field::Fixed64(value)) => set(&mut at, value)?,
            (4, Field::Bytes(value)) => set(&mut span, id(value, 8)?)?,
            (5, Field::Bytes(value)) => set(&mut trace, id(value, 16)?)?,
            _ => {}
        }
    }
    Ok(MetricExemplar {
        time_unix_ns: at.ok_or(MALFORMED)?,
        value: number(input, 3)?,
        trace_id: trace.flatten(),
        span_id: span.flatten(),
    })
}

fn packed_fixed64<T>(
    list: &mut Vec<T>,
    bytes: &[u8],
    convert: impl Fn(u64) -> T,
) -> DecodeResult<()> {
    let (chunks, remainder) = bytes.as_chunks::<8>();
    if !remainder.is_empty() || list.len() + chunks.len() > MAX_METRIC_BUCKETS {
        return Err(MALFORMED);
    }
    list.extend(
        chunks
            .iter()
            .map(|chunk| convert(u64::from_le_bytes(*chunk))),
    );
    Ok(())
}

fn histogram(input: &[u8]) -> DecodeResult<MetricData> {
    let (mut count, mut sum, mut min, mut max) = (None, None, None, None);
    let (mut explicit_bounds, mut bucket_counts) = (Vec::new(), Vec::new());
    for field in wire_fields(input) {
        match field? {
            (4, Field::Fixed64(value)) => set(&mut count, value)?,
            (5, Field::Fixed64(value)) => set(&mut sum, float(value))?,
            (6, Field::Fixed64(value)) => append(&mut bucket_counts, value, MAX_METRIC_BUCKETS)?,
            (6, Field::Bytes(values)) => {
                packed_fixed64(&mut bucket_counts, values, std::convert::identity)?
            }
            (7, Field::Fixed64(value)) => {
                append(&mut explicit_bounds, float(value), MAX_METRIC_BUCKETS)?
            }
            (7, Field::Bytes(values)) => packed_fixed64(&mut explicit_bounds, values, float)?,
            (11, Field::Fixed64(value)) => set(&mut min, float(value))?,
            (12, Field::Fixed64(value)) => set(&mut max, float(value))?,
            _ => {}
        }
    }
    Ok(MetricData::Histogram {
        count,
        sum,
        min,
        max,
        explicit_bounds,
        bucket_counts,
    })
}

fn exponential(input: &[u8]) -> DecodeResult<MetricData> {
    let (mut count, mut sum, mut min, mut max) = (None, None, None, None);
    let (mut scale, mut zero_count, mut zero_threshold, mut positive, mut negative) =
        (None, None, None, None, None);
    for field in wire_fields(input) {
        match field? {
            (4, Field::Fixed64(value)) => set(&mut count, value)?,
            (5, Field::Fixed64(value)) => set(&mut sum, float(value))?,
            (6, Field::Varint(value)) => set(&mut scale, zigzag(value)?)?,
            (7, Field::Fixed64(value)) => set(&mut zero_count, value)?,
            (8, Field::Bytes(value)) => set(&mut positive, buckets(value)?)?,
            (9, Field::Bytes(value)) => set(&mut negative, buckets(value)?)?,
            (12, Field::Fixed64(value)) => set(&mut min, float(value))?,
            (13, Field::Fixed64(value)) => set(&mut max, float(value))?,
            (14, Field::Fixed64(value)) => set(&mut zero_threshold, float(value))?,
            _ => {}
        }
    }
    Ok(MetricData::ExponentialHistogram {
        count,
        sum,
        min,
        max,
        scale,
        zero_count,
        zero_threshold,
        positive,
        negative,
    })
}

fn buckets(input: &[u8]) -> DecodeResult<ExponentialBuckets> {
    let mut offset = None;
    let mut bucket_counts = Vec::new();
    for field in wire_fields(input) {
        match field? {
            (1, Field::Varint(value)) => set(&mut offset, zigzag(value)?)?,
            (2, Field::Varint(value)) => append(&mut bucket_counts, value, MAX_METRIC_BUCKETS)?,
            (2, Field::Bytes(mut values)) => {
                while !values.is_empty() {
                    let value = varint(&mut values).map_err(|_| MALFORMED)?;
                    append(&mut bucket_counts, value, MAX_METRIC_BUCKETS)?;
                }
            }
            _ => {}
        }
    }
    Ok(ExponentialBuckets {
        offset: offset.unwrap_or_default(),
        bucket_counts,
    })
}

fn summary(input: &[u8]) -> DecodeResult<MetricData> {
    let (mut count, mut sum) = (None, None);
    let mut quantiles = Vec::new();
    for field in wire_fields(input) {
        match field? {
            (4, Field::Fixed64(value)) => set(&mut count, value)?,
            (5, Field::Fixed64(value)) => set(&mut sum, float(value))?,
            (6, Field::Bytes(value)) => {
                append(&mut quantiles, quantile(value)?, MAX_METRIC_BUCKETS)?
            }
            _ => {}
        }
    }
    Ok(MetricData::Summary {
        count,
        sum,
        quantiles,
    })
}

fn quantile(input: &[u8]) -> DecodeResult<MetricQuantile> {
    let (mut quantile, mut value) = (None, None);
    for field in wire_fields(input) {
        match field? {
            (1, Field::Fixed64(bits)) => set(&mut quantile, float(bits))?,
            (2, Field::Fixed64(bits)) => set(&mut value, float(bits))?,
            _ => {}
        }
    }
    Ok(MetricQuantile {
        quantile: quantile.unwrap_or_else(|| float(0)),
        value: value.unwrap_or_else(|| float(0)),
    })
}

fn dimensions(input: &[u8], attribute_field: u64) -> DecodeResult<Dimensions> {
    let mut result = Dimensions::default();
    for field in wire_fields(input) {
        if let (number, Field::Bytes(bytes)) = field?
            && number == attribute_field
        {
            attribute(&mut result, bytes, 0)?;
        }
    }
    Ok(result)
}

fn attribute(dimensions: &mut Dimensions, input: &[u8], depth: usize) -> DecodeResult<()> {
    let (mut key, mut value) = (None, None);
    for field in wire_fields(input) {
        match field? {
            (1, Field::Bytes(data)) => set(&mut key, text(data)?)?,
            (2, Field::Bytes(data)) => set(&mut value, data)?,
            _ => {}
        }
    }
    let (digest, label) = any(value.unwrap_or_default(), depth)?;
    dimensions.insert(key.ok_or(MALFORMED)?, digest, label)
}

fn any(input: &[u8], depth: usize) -> DecodeResult<(Fingerprint, Option<&str>)> {
    if depth >= MAX_ATTRIBUTE_DEPTH {
        return Err(MALFORMED);
    }
    let mut result = None;
    for field in wire_fields(input) {
        let known = match field? {
            (1, Field::Bytes(data)) => (hash_value(1, data), Some(text(data)?)),
            (2, Field::Varint(value)) => (hash_value(2, &[u8::from(boolean(value)?)]), None),
            (3, Field::Varint(value)) => (hash_value(3, &value.to_be_bytes()), None),
            (4, Field::Fixed64(value)) => (double_fingerprint(float(value)), None),
            (5, Field::Bytes(data)) => (
                array_fingerprint(wire_fields(data).filter_map(|item| match item {
                    Ok((1, Field::Bytes(value))) => {
                        Some(any(value, depth + 1).map(|entry| entry.0))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                }))?,
                None,
            ),
            (6, Field::Bytes(data)) => {
                let mut dimensions = Dimensions::default();
                for field in wire_fields(data) {
                    if let (1, Field::Bytes(value)) = field? {
                        attribute(&mut dimensions, value, depth + 1)?;
                    }
                }
                (hash_value(7, &dimensions.fingerprint()), None)
            }
            (7, Field::Bytes(data)) => (hash_value(5, data), None),
            _ => continue,
        };
        set(&mut result, known)?;
    }
    Ok(result.unwrap_or_else(|| (hash_value(0, b""), None)))
}
