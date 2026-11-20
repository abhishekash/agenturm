//! Bounded, content-free translation of one Claude Code main-transcript row.

use agenturm_jsonl_source::Result;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::error::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..32].to_owned()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub session_id: String,
    pub turn_id: String,
    worker_id: String,
    last_usage_key: String,
    last_usage: [Option<u64>; 4],
}

pub(crate) fn header_identity(raw: &[u8]) -> Result<Context> {
    let row: Value = serde_json::from_slice(raw)?;
    let session = row
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("Claude main transcript first row lacks sessionId")?;
    if !identifier(session) {
        return Err(invalid("invalid Claude session identity"));
    }
    Ok(Context {
        session_id: session.to_owned(),
        turn_id: String::new(),
        worker_id: String::new(),
        last_usage_key: String::new(),
        last_usage: [None; 4],
    })
}

pub(crate) fn checkpoint(context: &Context) -> String {
    fn field(value: &str) -> &str {
        if value.is_empty() { "x" } else { value }
    }
    let number = |value: Option<u64>| value.map_or("x".to_owned(), |value| value.to_string());
    format!(
        "c.{}.{}.{}.{}.{}.{}",
        field(&context.turn_id),
        field(&context.last_usage_key),
        number(context.last_usage[0]),
        number(context.last_usage[1]),
        number(context.last_usage[2]),
        number(context.last_usage[3])
    )
}

pub(crate) fn restore(context: &mut Context, checkpoint: &str) -> Result<()> {
    let fields: Vec<_> = checkpoint.split('.').collect();
    if fields.len() != 7 || fields[0] != "c" {
        return Err(invalid("invalid Claude turn checkpoint"));
    }
    let hash = |value: &str| {
        value == "x"
            || value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    if !hash(fields[1]) || !hash(fields[2]) {
        return Err(invalid("invalid Claude checkpoint identity"));
    }
    context.turn_id = if fields[1] == "x" {
        String::new()
    } else {
        fields[1].to_owned()
    };
    context.last_usage_key = if fields[2] == "x" {
        String::new()
    } else {
        fields[2].to_owned()
    };
    for (index, raw) in fields[3..].iter().enumerate() {
        context.last_usage[index] = if *raw == "x" {
            None
        } else {
            let value = raw.parse::<u64>()?;
            if value > i64::MAX as u64 {
                return Err(invalid("Claude checkpoint usage exceeds range"));
            }
            Some(value)
        };
    }
    if context.last_usage_key.is_empty() && context.last_usage.iter().any(Option::is_some) {
        return Err(invalid("Claude checkpoint usage lacks identity"));
    }
    Ok(())
}

pub(crate) fn revision(offset: u64) -> Result<u32> {
    if offset > (1 << 31) - 1 {
        return Err(invalid("Claude source offset exceeds revision budget"));
    }
    Ok(u32::try_from(offset)?)
}

fn timestamp(row: &Value) -> Result<String> {
    let raw = row
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("Claude row lacks timestamp")?;
    let stamp = OffsetDateTime::parse(raw, &Rfc3339)?.unix_timestamp_nanos();
    if stamp < 0 || stamp > i64::MAX as i128 {
        return Err(invalid("Claude timestamp exceeds supported range"));
    }
    Ok(stamp.to_string())
}

fn identity(context: &Context, operation: Option<&str>) -> Value {
    let mut result = json!({"session_id":context.session_id});
    if !context.worker_id.is_empty() {
        result["worker_id"] = json!(context.worker_id);
    }
    if !context.turn_id.is_empty() {
        result["turn_id"] = json!(context.turn_id);
    }
    if let Some(operation) = operation {
        result["operation_id"] = json!(operation);
    }
    result
}

fn fact(
    event_id: String,
    revision: u32,
    kind: &str,
    time_ns: &str,
    context: &Context,
    operation: Option<&str>,
    payload: Value,
) -> Value {
    json!({"event_id":event_id,"revision":revision,"kind":kind,
           "time_ns":time_ns,"identity":identity(context,operation),
           "payload":payload})
}

fn has_text(content: &Value) -> bool {
    match content {
        Value::String(text) => !text.is_empty(),
        Value::Array(blocks) => blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
        }),
        _ => false,
    }
}

fn category(name: &str) -> &'static str {
    if name.starts_with("mcp__") {
        "mcp"
    } else if matches!(name, "Bash" | "bash" | "Shell" | "shell") {
        "shell"
    } else if matches!(
        name,
        "Read"
            | "Write"
            | "Edit"
            | "Glob"
            | "Grep"
            | "MultiEdit"
            | "read"
            | "write"
            | "edit"
            | "glob"
            | "grep"
    ) {
        "file"
    } else {
        "other"
    }
}

fn count(value: &Value) -> Result<u64> {
    let amount = value.as_u64().ok_or("invalid Claude usage counter")?;
    if amount > i64::MAX as u64 {
        return Err(invalid("Claude usage counter exceeds supported range"));
    }
    Ok(amount)
}

fn usage(raw: &Value, context: &mut Context, key: &str) -> Result<Option<Value>> {
    let Some(fields) = raw.as_object() else {
        return Err(invalid("Claude usage is not an object"));
    };
    let incoming = [
        fields.get("input_tokens").map(count).transpose()?,
        fields.get("output_tokens").map(count).transpose()?,
        fields
            .get("cache_read_input_tokens")
            .map(count)
            .transpose()?,
        fields
            .get("cache_creation_input_tokens")
            .map(count)
            .transpose()?,
    ];
    if incoming.iter().all(Option::is_none) {
        return Ok(None);
    }
    if context.last_usage_key != key {
        context.last_usage_key = key.to_owned();
        context.last_usage = [None; 4];
    }
    for (index, value) in incoming.into_iter().enumerate() {
        if let Some(value) = value {
            context.last_usage[index] = Some(context.last_usage[index].unwrap_or(0).max(value));
        }
    }
    let mut values = Map::new();
    let [input, output, cache_read, cache_write] = context.last_usage;
    if input.is_some() || cache_read.is_some() || cache_write.is_some() {
        let total = input
            .unwrap_or(0)
            .checked_add(cache_read.unwrap_or(0))
            .and_then(|sum| sum.checked_add(cache_write.unwrap_or(0)))
            .ok_or("Claude input usage exceeds supported range")?;
        if total > i64::MAX as u64 {
            return Err(invalid("Claude input usage exceeds supported range"));
        }
        values.insert("input_tokens".to_owned(), json!(total));
    }
    if let Some(value) = output {
        values.insert("output_tokens".to_owned(), json!(value));
    }
    if let Some(value) = cache_read {
        values.insert("cache_read_tokens".to_owned(), json!(value));
    }
    if let Some(value) = cache_write {
        values.insert("cache_write_tokens".to_owned(), json!(value));
    }
    Ok((!values.is_empty()).then_some(Value::Object(values)))
}

fn blocks(content: &Value) -> Result<&[Value]> {
    match content {
        Value::Array(blocks) if blocks.len() <= 128 => Ok(blocks),
        Value::Array(_) => Err(invalid("Claude content block budget exceeded")),
        Value::String(_) | Value::Null => Ok(&[]),
        _ => Err(invalid("invalid Claude message content shape")),
    }
}

/// Translate an entire line; the shared runtime commits its cursor only if
/// every returned fact fits in one acknowledged batch.
pub(crate) fn translate_line(
    raw: &[u8],
    context: &Context,
    origin: &str,
    offset: u64,
) -> Result<(Vec<Value>, Context)> {
    let row: Value = serde_json::from_slice(raw)?;
    if let Some(session) = row.get("sessionId")
        && session.as_str() != Some(context.session_id.as_str())
    {
        return Err(invalid("Claude session identity changed inside one file"));
    }
    let worker = row.get("agentId").and_then(Value::as_str);
    if row.get("agentId").is_some() && !worker.is_some_and(identifier) {
        return Err(invalid("invalid Claude worker identity"));
    }
    let kind = row
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Claude row lacks type")?;
    let row_key = row.get("uuid").and_then(Value::as_str).unwrap_or("");
    if row_key.len() > 512 {
        return Err(invalid("Claude row identity exceeds limit"));
    }
    let base = if row_key.is_empty() {
        format!("claude:{origin}:offset-{offset}")
    } else {
        format!("claude:{origin}:{}", digest(row_key))
    };
    let revision = revision(offset)?;
    let time_ns = if matches!(kind, "user" | "assistant") {
        timestamp(&row)?
    } else {
        row.get("timestamp")
            .map_or(Ok("0".to_owned()), |_| timestamp(&row))?
    };
    let mut next = context.clone();
    next.worker_id = worker.unwrap_or("").to_owned();
    if !matches!(kind, "user" | "assistant") {
        return Ok((
            vec![json!({"event_id":format!("{base}:unsupported"),
            "revision":revision,"kind":"diagnostic","time_ns":time_ns,
            "payload":{"code":"unsupported_claude_row","severity":"info"}})],
            next,
        ));
    }
    let message = row
        .get("message")
        .and_then(Value::as_object)
        .ok_or("Claude message is not an object")?;
    let content = message.get("content").unwrap_or(&Value::Null);
    let blocks = blocks(content)?;
    let mut facts = Vec::new();
    if kind == "user" {
        let tool_results: Vec<_> = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .collect();
        if tool_results.is_empty() && has_text(content) {
            next.turn_id = digest(if row_key.is_empty() { &base } else { row_key });
            facts.push(fact(
                format!("{base}:message"),
                revision,
                "message",
                &time_ns,
                &next,
                None,
                json!({"role":"user"}),
            ));
            facts.push(fact(
                format!("{base}:lifecycle"),
                revision,
                "lifecycle",
                &time_ns,
                &next,
                None,
                json!({"state":"running"}),
            ));
        } else if tool_results.is_empty() {
            facts.push(json!({"event_id":format!("{base}:unsupported-user"),
                "revision":revision,"kind":"diagnostic","time_ns":time_ns,
                "payload":{"code":"unsupported_claude_user","severity":"info"}}));
        }
        for (index, block) in tool_results.iter().enumerate() {
            let operation = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .ok_or("Claude tool result lacks tool_use_id")?;
            if operation.is_empty() || operation.len() > 512 {
                return Err(invalid("invalid Claude tool result identity"));
            }
            let failed = block
                .get("is_error")
                .and_then(Value::as_bool)
                .ok_or("Claude tool result lacks explicit is_error")?;
            let operation = digest(operation);
            facts.push(fact(
                format!("{base}:tool-result-{index}"),
                revision,
                "tool",
                &time_ns,
                &next,
                Some(&operation),
                json!({"category":"other","result":if failed {"failed"} else {"succeeded"}}),
            ));
            facts.push(fact(
                format!("{base}:lifecycle-{index}"),
                revision,
                "lifecycle",
                &time_ns,
                &next,
                None,
                json!({"state":"running"}),
            ));
        }
    } else {
        if has_text(content) {
            facts.push(fact(
                format!("{base}:message"),
                revision,
                "message",
                &time_ns,
                &next,
                None,
                json!({"role":"assistant"}),
            ));
        }
        if let Some(raw_usage) = message.get("usage") {
            let message_key = message
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&base);
            if message_key.len() > 512 {
                return Err(invalid("Claude model message identity exceeds limit"));
            }
            let key = digest(message_key);
            if let Some(counters) = usage(raw_usage, &mut next, &key)? {
                facts.push(fact(
                    format!("claude:{origin}:usage:{key}"),
                    revision,
                    "usage",
                    &time_ns,
                    &next,
                    None,
                    counters,
                ));
            }
        }
        for (index, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let operation = block
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Claude tool use lacks ID")?;
            if operation.is_empty() || operation.len() > 512 {
                return Err(invalid("invalid Claude tool use identity"));
            }
            let operation = digest(operation);
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            facts.push(fact(
                format!("{base}:tool-use-{index}"),
                revision,
                "tool",
                &time_ns,
                &next,
                Some(&operation),
                json!({"category":category(name),"result":"started"}),
            ));
        }
        match message.get("stop_reason").and_then(Value::as_str) {
            Some("end_turn") => facts.push(fact(
                format!("{base}:lifecycle"),
                revision,
                "lifecycle",
                &time_ns,
                &next,
                None,
                json!({"state":"completed"}),
            )),
            Some("tool_use") | None => facts.push(fact(
                format!("{base}:lifecycle"),
                revision,
                "lifecycle",
                &time_ns,
                &next,
                None,
                json!({"state":"running"}),
            )),
            Some(_) => {
                facts.push(fact(
                    format!("{base}:lifecycle"),
                    revision,
                    "lifecycle",
                    &time_ns,
                    &next,
                    None,
                    json!({"state":"running"}),
                ));
                facts.push(json!({"event_id":format!("{base}:unsupported-stop"),
                    "revision":revision,"kind":"diagnostic","time_ns":time_ns,
                    "payload":{"code":"unsupported_claude_stop_reason","severity":"warning"}}));
            }
        }
    }
    for (index, block) in blocks.iter().enumerate() {
        let supported = matches!(
            (kind, block.get("type").and_then(Value::as_str)),
            ("user", Some("text" | "tool_result")) | ("assistant", Some("text" | "tool_use"))
        );
        if !supported {
            facts.push(
                json!({"event_id":format!("{base}:unsupported-block-{index}"),
                "revision":revision,"kind":"diagnostic","time_ns":time_ns,
                "payload":{"code":"unsupported_claude_block","severity":"info"}}),
            );
        }
    }
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_and_usage_payloads_never_copy_private_content() {
        let context = header_identity(
            br#"{"type":"user","sessionId":"session","timestamp":"2026-10-24T12:00:00Z"}"#,
        )
        .unwrap();
        let (facts, _) = translate_line(br#"{"type":"assistant","sessionId":"session","uuid":"a","timestamp":"2026-10-24T12:00:01Z","message":{"id":"msg_1","content":[{"type":"text","text":"private reply"},{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"private command"}}],"usage":{"input_tokens":3,"output_tokens":4,"cache_read_input_tokens":2,"cache_creation_input_tokens":1},"stop_reason":"tool_use"}}"#, &context, "origin", 42).unwrap();
        let serialized = serde_json::to_string(&facts).unwrap();
        assert!(!serialized.contains("private"));
        assert_eq!(facts[1]["payload"]["input_tokens"], 6);
        assert_eq!(facts[2]["payload"]["category"], "shell");
    }

    #[test]
    fn tool_result_without_explicit_error_flag_refuses_before_ack() {
        let context = header_identity(br#"{"type":"user","sessionId":"session"}"#).unwrap();
        assert!(translate_line(br#"{"type":"user","sessionId":"session","timestamp":"2026-10-24T12:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","content":"private output"}]}}"#, &context, "origin", 42).is_err());
    }

    #[test]
    fn checkpoint_persists_stream_maxima_and_rejects_malformed_state() {
        let mut context = header_identity(br#"{"type":"user","sessionId":"session"}"#).unwrap();
        context.turn_id = digest("turn");
        let first = json!({"input_tokens":3,"output_tokens":2});
        assert_eq!(
            usage(&first, &mut context, &digest("msg"))
                .unwrap()
                .unwrap()["input_tokens"],
            3
        );
        let saved = checkpoint(&context);
        assert!(saved.len() < 200);
        let mut restored = header_identity(br#"{"type":"user","sessionId":"session"}"#).unwrap();
        restore(&mut restored, &saved).unwrap();
        assert_eq!(restored.turn_id, context.turn_id);
        let lower = json!({"input_tokens":1,"output_tokens":1});
        let corrected = usage(&lower, &mut restored, &digest("msg"))
            .unwrap()
            .unwrap();
        assert_eq!(corrected["input_tokens"], 3);
        assert_eq!(corrected["output_tokens"], 2);
        assert!(restore(&mut restored, "c.bad").is_err());
    }

    #[test]
    fn unknown_stop_and_unhandled_blocks_keep_coverage_visible_without_terminal_claim() {
        let context = header_identity(br#"{"type":"user","sessionId":"session"}"#).unwrap();
        let (facts, _) = translate_line(
            br#"{"type":"assistant","sessionId":"session","uuid":"a","timestamp":"2026-10-24T12:00:01Z","message":{"content":[{"type":"thinking","thinking":"private"}],"stop_reason":"max_tokens"}}"#,
            &context, "origin", 42).unwrap();
        assert!(
            facts
                .iter()
                .any(|fact| fact["kind"] == "lifecycle" && fact["payload"]["state"] == "running")
        );
        assert_eq!(
            facts
                .iter()
                .filter(|fact| fact["kind"] == "diagnostic")
                .count(),
            2
        );
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }
}
