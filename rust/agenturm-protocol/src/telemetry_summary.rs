//! Closed, metadata-only projections of producer-reported OTLP activity.

use crate::observation::valid_otlp_id;
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
        valid_otlp_id(&self.trace_id, 16) && valid_otlp_id(&self.span_id, 8)
    }

    pub fn matches_event(&self, event_id: &str) -> bool {
        event_id
            .strip_prefix("otlp:")
            .and_then(|event| event.strip_suffix(":summary"))
            .and_then(|ids| ids.split_once(':'))
            == Some((self.trace_id.as_str(), self.span_id.as_str()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_success_classification_is_explicit_and_event_bound() {
        let summary = TraceSpanSummary {
            trace_id: "11".repeat(16),
            span_id: "22".repeat(8),
            agent_run: false,
            reported_error: false,
        };
        assert!(summary.is_valid());
        assert!(summary.matches_event(&format!(
            "otlp:{}:{}:summary",
            summary.trace_id, summary.span_id
        )));
        assert!(!summary.matches_event(&format!("otlp:{}:{}", summary.trace_id, summary.span_id)));
        assert!(serde_json::from_str::<TraceSpanSummary>(&format!(r#"{{"trace_id":"{}","span_id":"{}","agent_run":false,"reported_error":false,"name":"private"}}"#, summary.trace_id, summary.span_id)).is_err());
    }

    #[test]
    fn activity_metadata_preserves_reported_zero_without_accepting_a_body() {
        let mut log = OtlpActivityLog {
            reported_error: false,
            target_source_id: Some("harness.codex".into()),
            usage: Some(LogTokenUsage {
                input_tokens: Some(0),
                output_tokens: None,
                cache_read_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            }),
            cost: Some(LogCost {
                currency: "USD".into(),
                nanounits: 0,
            }),
        };
        assert!(log.is_valid());
        assert!(log.matches_event(&format!("otlp-log:{}", "a".repeat(64))));
        assert!(!log.matches_event("otlp-log:untrusted-raw-id"));
        let mut encoded = serde_json::to_value(&log).unwrap();
        encoded["body"] = serde_json::json!("private");
        assert!(serde_json::from_value::<OtlpActivityLog>(encoded).is_err());
        log.target_source_id = Some("otlp.another".into());
        assert!(!log.is_valid());
    }
}
