//! Explicit producer counters recorded independently of span structure.

use super::{Attribute, AttributeValue};
use agenturm_kernel::{SpanTokenUsage, TraceSpan, TraceSpanIdentity};
use serde_json::Value;

const INPUT: &[&str] = &[
    "gen_ai.usage.input_tokens",
    "llm.usage.input_tokens",
    "llm.token_count.prompt",
];
const OUTPUT: &[&str] = &[
    "gen_ai.usage.output_tokens",
    "llm.usage.output_tokens",
    "llm.token_count.completion",
];
const CACHE_READ: &[&str] = &[
    "gen_ai.usage.cache_read.input_tokens",
    "gen_ai.usage.cache_read_input_tokens",
    "gen_ai.usage.cached_input_tokens",
    "llm.usage.cache_read_input_tokens",
    "llm.token_count.prompt_details.cache_read",
    "cache_read_input_tokens",
    "cached_input_tokens",
];
const CACHE_WRITE: &[&str] = &[
    "gen_ai.usage.cache_write.input_tokens",
    "gen_ai.usage.cache_write_input_tokens",
    "gen_ai.usage.cache_creation.input_tokens",
    "gen_ai.usage.cache_creation_input_tokens",
    "llm.usage.cache_creation_input_tokens",
    "llm.token_count.prompt_details.cache_write",
    "cache_creation_input_tokens",
];
const REASONING: &[&str] = &[
    "gen_ai.usage.reasoning.output_tokens",
    "gen_ai.usage.reasoning_tokens",
    "llm.usage.reasoning_tokens",
    "llm.token_count.completion_details.reasoning",
    "reasoning_tokens",
];
const TOTAL: &[&str] = &[
    "gen_ai.usage.total_tokens",
    "llm.usage.total_tokens",
    "llm.token_count.total",
];

pub(crate) fn is_token_key(key: &str) -> bool {
    [INPUT, OUTPUT, CACHE_READ, CACHE_WRITE, REASONING, TOTAL]
        .iter()
        .any(|aliases| aliases.contains(&key))
}

pub(super) fn reported(attributes: &[Attribute], span: &TraceSpan) -> Option<SpanTokenUsage> {
    let usage = SpanTokenUsage {
        span: TraceSpanIdentity {
            trace_id: span.trace_id.clone(),
            span_id: span.span_id.clone(),
        },
        target_source_id: span.target_source_id.clone(),
        input_tokens: counter(attributes, INPUT),
        output_tokens: counter(attributes, OUTPUT),
        cache_read_input_tokens: counter(attributes, CACHE_READ),
        cache_write_input_tokens: counter(attributes, CACHE_WRITE),
        reasoning_output_tokens: counter(attributes, REASONING),
        total_tokens: counter(attributes, TOTAL),
    };
    usage.is_valid().then_some(usage)
}

fn counter(attributes: &[Attribute], keys: &[&str]) -> Option<u64> {
    let mut found = None;
    for attribute in attributes
        .iter()
        .filter(|attr| keys.contains(&attr.key.as_str()))
    {
        let count = read_count(attribute.value.as_ref()?)?;
        if found.is_some_and(|prior| prior != count) {
            return None;
        }
        found = Some(count);
    }
    found
}

fn read_count(value: &AttributeValue) -> Option<u64> {
    if !value.bool_value.is_null() || value.double_value.is_some() {
        return None;
    }
    match (&value.string_value, &value.int_value) {
        (Value::String(text), None) => parse_count(text),
        (Value::Null, Some(integer)) => {
            let integer = integer.get();
            if integer.starts_with('"') {
                parse_count(&serde_json::from_str::<String>(integer).ok()?)
            } else {
                parse_count(integer)
            }
        }
        _ => None,
    }
}

fn parse_count(text: &str) -> Option<u64> {
    if text.is_empty() || text.len() > 19 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok().map(|count| count as u64)
}
