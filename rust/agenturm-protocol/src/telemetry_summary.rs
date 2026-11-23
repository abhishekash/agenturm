//! Closed, metadata-only projections of producer-reported OTLP activity.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSpanSummary {
    pub trace_id: String,
    pub span_id: String,
    pub agent_run: bool,
    pub reported_error: bool,
}

impl TraceSpanSummary {
    pub fn is_valid(&self) -> bool {
        (self.agent_run || self.reported_error)
            && valid_otlp_id(&self.trace_id, 16)
            && valid_otlp_id(&self.span_id, 8)
    }

    pub fn matches_event(&self, event_id: &str) -> bool {
        event_id
            .strip_prefix("otlp:")
            .and_then(|event| event.strip_suffix(":summary"))
            .and_then(|ids| ids.split_once(':'))
            == Some((self.trace_id.as_str(), self.span_id.as_str()))
    }
}

fn valid_otlp_id(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|byte| byte != b'0')
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OtlpActivityLog {
    pub reported_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LogTokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<LogCost>,
}

impl OtlpActivityLog {
    pub fn is_valid(&self) -> bool {
        self.target_source_id.as_ref().is_none_or(|id| {
            !id.starts_with("otlp.")
                && id.len() <= 96
                && id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && id.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_/".contains(&byte)
                })
        }) && self.usage.as_ref().is_none_or(LogTokenUsage::is_valid)
            && self.cost.as_ref().is_none_or(LogCost::is_valid)
    }

    pub fn matches_event(&self, event_id: &str) -> bool {
        event_id.strip_prefix("otlp-log:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogTokenUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

impl LogTokenUsage {
    pub fn is_valid(&self) -> bool {
        let counters = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_input_tokens,
            self.cache_write_input_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ];
        counters.iter().any(Option::is_some)
            && counters
                .into_iter()
                .flatten()
                .all(|count| count <= i64::MAX as u64)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogCost {
    pub currency: String,
    pub nanounits: u64,
}

impl LogCost {
    pub fn is_valid(&self) -> bool {
        self.currency.len() == 3
            && self.currency.bytes().all(|byte| byte.is_ascii_uppercase())
            && self.nanounits <= i64::MAX as u64
    }
}
