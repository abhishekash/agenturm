//! Bounded metadata-only metric and usage views shared by stores and readers.

use crate::metric::{MetricPoint, MetricSeries};
use crate::{ExtensionId, ReadSnapshot, TraceSpan};
use serde::{Deserialize, Serialize};

pub const MAX_METRIC_HISTORY_PAGE: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSeriesKey {
    pub source_id: ExtensionId,
    pub series_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSeriesPosition {
    pub series: MetricSeriesKey,
    pub snapshot_id: ReadSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricPointPosition {
    pub series: MetricSeriesKey,
    pub snapshot_id: ReadSnapshot,
    pub observed_at_unix_ns: u64,
    pub event_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetricHistoryQuery {
    Series {
        source_id: Option<ExtensionId>,
        snapshot_id: Option<ReadSnapshot>,
        after: Option<MetricSeriesPosition>,
        limit: usize,
    },
    Points {
        series: MetricSeriesKey,
        snapshot_id: ReadSnapshot,
        before: Option<MetricPointPosition>,
        limit: usize,
    },
    Exemplar {
        point: MetricPointPosition,
        exemplar_index: usize,
    },
}

impl MetricHistoryQuery {
    pub fn source_id(&self) -> Option<&ExtensionId> {
        match self {
            Self::Series { source_id, .. } => source_id.as_ref(),
            Self::Points { series, .. } => Some(&series.source_id),
            Self::Exemplar { point, .. } => Some(&point.series.source_id),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSeriesEntry {
    pub key: MetricSeriesKey,
    pub series: MetricSeries,
    pub latest_unix_ns: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricPointEntry {
    pub position: MetricPointPosition,
    pub point: MetricPoint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricExemplarTrace {
    pub snapshot_id: ReadSnapshot,
    pub source_id: ExtensionId,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub worker_id: Option<String>,
    pub span: TraceSpan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetricHistoryPage {
    Series {
        snapshot_id: ReadSnapshot,
        rows: Vec<MetricSeriesEntry>,
        next: Option<MetricSeriesPosition>,
    },
    Points {
        snapshot_id: ReadSnapshot,
        rows: Vec<MetricPointEntry>,
        next: Option<MetricPointPosition>,
    },
    Exemplar {
        trace: Option<Box<MetricExemplarTrace>>,
    },
}

pub const USAGE_DAYS: usize = 7;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsageHistoryQuery {
    pub source_id: Option<ExtensionId>,
    pub session_id: Option<String>,
    pub snapshot_id: Option<ReadSnapshot>,
    /// Exclusive UTC window end, usually the time the operator opened Usage.
    pub end_unix_ns: u64,
}

/// Integer decimal totals preserve the full range of several u64 reports.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTotals {
    pub reports: usize,
    pub input_tokens: Option<String>,
    pub output_tokens: Option<String>,
    pub cache_read_tokens: Option<String>,
    pub cache_write_tokens: Option<String>,
    pub reasoning_output_tokens: Option<String>,
    pub total_tokens: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsageDay {
    pub start_unix_ns: u64,
    pub native: UsageTotals,
    /// Producer span counters can overlap with native counters and with one
    /// another. Keep their observed sum separate from native reports.
    pub span: UsageTotals,
    pub retention_partial: bool,
    pub scan_partial: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsageHistoryPage {
    pub snapshot_id: ReadSnapshot,
    pub end_unix_ns: u64,
    pub days: [UsageDay; USAGE_DAYS],
    pub retention_cutoff_unix_ns: u64,
    pub scan_limited: bool,
}
