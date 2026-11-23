//! Retained OTLP instrument and point metadata shared by runtime and readers.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

pub const MAX_METRIC_POINTS_PER_BATCH: usize = 1_024;
pub const MAX_METRIC_BUCKETS: usize = 64;
pub const MAX_METRIC_EXEMPLARS: usize = 16;
pub const MAX_METRIC_NAME_BYTES: usize = 255;
pub const MAX_METRIC_LABEL_BYTES: usize = 128;
pub const MAX_METRIC_UNIT_BYTES: usize = 63;

const NO_RECORDED_VALUE: u32 = 1;

pub fn is_metric_source(source: &str) -> bool {
    source
        .strip_prefix("otlp.")
        .is_some_and(|suffix| !suffix.is_empty())
}

/// The producer's floating-point representation. Normalize NaN encodings so
/// equivalent JSON and protobuf observations share a replay identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetricFloat(u64);

impl MetricFloat {
    pub fn new(value: f64) -> Self {
        Self(if value.is_nan() {
            f64::NAN.to_bits()
        } else {
            value.to_bits()
        })
    }

    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl fmt::Display for MetricFloat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.get() {
            value if value.is_nan() => formatter.write_str("NaN"),
            f64::INFINITY => formatter.write_str("Infinity"),
            f64::NEG_INFINITY => formatter.write_str("-Infinity"),
            value => value.fmt(formatter),
        }
    }
}

impl Serialize for MetricFloat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.get() {
            value if value.is_nan() => serializer.serialize_str("NaN"),
            f64::INFINITY => serializer.serialize_str("Infinity"),
            f64::NEG_INFINITY => serializer.serialize_str("-Infinity"),
            value => serializer.serialize_f64(value),
        }
    }
}

impl<'de> Deserialize<'de> for MetricFloat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FloatVisitor;
        impl de::Visitor<'_> for FloatVisitor {
            type Value = MetricFloat;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OTLP double or its nonfinite string")
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(MetricFloat::new(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                self.visit_f64(value as f64)
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                self.visit_f64(value as f64)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "NaN" => self.visit_f64(f64::NAN),
                    "Infinity" => self.visit_f64(f64::INFINITY),
                    "-Infinity" => self.visit_f64(f64::NEG_INFINITY),
                    _ => Err(E::invalid_value(de::Unexpected::Str(value), &self)),
                }
            }
        }
        deserializer.deserialize_any(FloatVisitor)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "representation", content = "value", rename_all = "snake_case")]
pub enum MetricNumber {
    Int(i64),
    Double(MetricFloat),
}

impl fmt::Display for MetricNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => value.fmt(formatter),
            Self::Double(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    Gauge,
    Sum,
    Histogram,
    ExponentialHistogram,
    Summary,
}

impl MetricKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gauge => "Gauge",
            Self::Sum => "Sum",
            Self::Histogram => "Histogram",
            Self::ExponentialHistogram => "Exponential histogram",
            Self::Summary => "Summary",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricTemporality {
    Unspecified,
    Delta,
    Cumulative,
}

impl MetricTemporality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "Unspecified",
            Self::Delta => "Delta",
            Self::Cumulative => "Cumulative",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSeries {
    /// Canonical digest of the producer instrument and typed dimensions. Raw
    /// resource, scope, point, and exemplar attributes stay outside history.
    pub id: String,
    pub name: String,
    pub unit: String,
    pub service: Option<String>,
    pub scope: Option<String>,
    pub kind: MetricKind,
    pub temporality: MetricTemporality,
    pub monotonic: Option<bool>,
}

impl MetricSeries {
    pub fn is_valid(&self) -> bool {
        is_metric_series_id(&self.id)
            && is_metric_name(&self.name)
            && is_metric_unit(&self.unit)
            && self.service.as_deref().is_none_or(is_metric_label)
            && self.scope.as_deref().is_none_or(is_metric_label)
            && (self.kind == MetricKind::Sum || self.monotonic.is_none())
            && (!matches!(self.kind, MetricKind::Gauge | MetricKind::Summary)
                || self.temporality == MetricTemporality::Unspecified)
    }
}

pub fn is_metric_series_id(value: &str) -> bool {
    value.len() == 64 && lowercase_hex(value)
}

pub fn is_metric_name(value: &str) -> bool {
    value.len() <= MAX_METRIC_NAME_BYTES
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && value.bytes().all(name_byte)
}

pub fn is_metric_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_METRIC_LABEL_BYTES && value.bytes().all(name_byte)
}

pub fn is_metric_unit(value: &str) -> bool {
    value.len() <= MAX_METRIC_UNIT_BYTES
        && value
            .bytes()
            .all(|byte| name_byte(byte) || b"{}[]()%*^'".contains(&byte))
}

fn name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte)
}

fn lowercase_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_otel_id(value: &str, bytes: usize) -> bool {
    value.len() == 2 * bytes && lowercase_hex(value) && value.bytes().any(|byte| byte != b'0')
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricExemplar {
    pub time_unix_ns: u64,
    pub value: Option<MetricNumber>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
}

impl MetricExemplar {
    fn is_valid(&self) -> bool {
        self.time_unix_ns > 0
            && self.time_unix_ns <= i64::MAX as u64
            && self
                .trace_id
                .as_deref()
                .is_none_or(|id| valid_otel_id(id, 16))
            && self
                .span_id
                .as_deref()
                .is_none_or(|id| valid_otel_id(id, 8))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExponentialBuckets {
    pub offset: i32,
    pub bucket_counts: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricQuantile {
    pub quantile: MetricFloat,
    pub value: MetricFloat,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetricData {
    Gauge {
        value: Option<MetricNumber>,
    },
    Sum {
        value: Option<MetricNumber>,
    },
    Histogram {
        count: Option<u64>,
        sum: Option<MetricFloat>,
        min: Option<MetricFloat>,
        max: Option<MetricFloat>,
        explicit_bounds: Vec<MetricFloat>,
        bucket_counts: Vec<u64>,
    },
    ExponentialHistogram {
        count: Option<u64>,
        sum: Option<MetricFloat>,
        min: Option<MetricFloat>,
        max: Option<MetricFloat>,
        scale: Option<i32>,
        zero_count: Option<u64>,
        zero_threshold: Option<MetricFloat>,
        positive: Option<ExponentialBuckets>,
        negative: Option<ExponentialBuckets>,
    },
    Summary {
        count: Option<u64>,
        sum: Option<MetricFloat>,
        quantiles: Vec<MetricQuantile>,
    },
}

impl MetricData {
    pub fn kind(&self) -> MetricKind {
        match self {
            Self::Gauge { .. } => MetricKind::Gauge,
            Self::Sum { .. } => MetricKind::Sum,
            Self::Histogram { .. } => MetricKind::Histogram,
            Self::ExponentialHistogram { .. } => MetricKind::ExponentialHistogram,
            Self::Summary { .. } => MetricKind::Summary,
        }
    }

    fn is_valid(&self) -> bool {
        match self {
            Self::Gauge { .. } | Self::Sum { .. } => true,
            Self::Histogram {
                explicit_bounds,
                bucket_counts,
                ..
            } => {
                explicit_bounds.len() < MAX_METRIC_BUCKETS
                    && bucket_counts.len() <= MAX_METRIC_BUCKETS
                    && (bucket_counts.is_empty()
                        || bucket_counts.len() == explicit_bounds.len() + 1)
                    && explicit_bounds.iter().all(|bound| bound.get().is_finite())
                    && explicit_bounds
                        .windows(2)
                        .all(|pair| pair[0].get() < pair[1].get())
            }
            Self::ExponentialHistogram {
                positive,
                negative,
                zero_threshold,
                ..
            } => {
                positive
                    .iter()
                    .chain(negative)
                    .all(|buckets| buckets.bucket_counts.len() <= MAX_METRIC_BUCKETS)
                    && zero_threshold
                        .is_none_or(|value| value.get().is_finite() && value.get() >= 0.0)
            }
            Self::Summary { quantiles, .. } => {
                quantiles.len() <= MAX_METRIC_BUCKETS
                    && quantiles
                        .iter()
                        .all(|item| (0.0..=1.0).contains(&item.quantile.get()))
                    && quantiles
                        .windows(2)
                        .all(|pair| pair[0].quantile.get() < pair[1].quantile.get())
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricPoint {
    pub series: MetricSeries,
    pub start_unix_ns: Option<u64>,
    pub flags: u32,
    pub data: MetricData,
    pub exemplars: Vec<MetricExemplar>,
}

impl MetricPoint {
    pub fn no_recorded_value(&self) -> bool {
        self.flags & NO_RECORDED_VALUE != 0
    }

    pub fn is_valid_at(&self, observed_at_unix_ns: u64) -> bool {
        self.series.is_valid()
            && observed_at_unix_ns > 0
            && observed_at_unix_ns <= i64::MAX as u64
            && self
                .start_unix_ns
                .is_none_or(|start| start > 0 && start <= observed_at_unix_ns)
            && self.data.kind() == self.series.kind
            && self.data.is_valid()
            && self.exemplars.len() <= MAX_METRIC_EXEMPLARS
            && self.exemplars.iter().all(MetricExemplar::is_valid)
            && (self.series.kind != MetricKind::Summary || self.exemplars.is_empty())
    }

    pub fn event_id(&self, observed_at_unix_ns: u64) -> String {
        format!(
            "otlp-metric:{}:{}:{observed_at_unix_ns}",
            self.series.id,
            self.start_unix_ns
                .map_or_else(|| "x".to_owned(), |start| start.to_string())
        )
    }

    pub fn matches_event(&self, event_id: &str, observed_at_unix_ns: u64) -> bool {
        event_id == self.event_id(observed_at_unix_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_numbers_preserve_signed_integers_and_nonfinite_doubles() {
        for number in [
            MetricNumber::Int(i64::MIN),
            MetricNumber::Int(i64::MAX),
            MetricNumber::Double(MetricFloat::new(-0.0)),
            MetricNumber::Double(MetricFloat::new(f64::INFINITY)),
            MetricNumber::Double(MetricFloat::new(f64::from_bits(0x7ff8_1234_5678_0000))),
        ] {
            let wire = serde_json::to_string(&number).unwrap();
            assert_eq!(serde_json::from_str::<MetricNumber>(&wire).unwrap(), number);
        }
    }
}
