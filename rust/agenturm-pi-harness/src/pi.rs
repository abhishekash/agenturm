//! A bounded, content-free translation of one actual Pi session JSONL line.
//! Source text is parsed in the child and never placed in a process fact.

use serde_json::{Value, json};
use std::error::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub session_id: String,
    pub worker_id: String,
    pub turn_id: String,
}

pub(crate) fn header_identity(raw: &[u8]) -> Result<Context> {
    let row: Value = serde_json::from_slice(raw)?;
    if row.get("type").and_then(Value::as_str) != Some("session") {
        return Err(invalid("Pi source must start with a session header"));
    }
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Pi session header lacks an ID")?;
    let session_id = row.get("sessionId").and_then(Value::as_str).unwrap_or(id);
    let worker_id = row.get("agentId").and_then(Value::as_str).unwrap_or("");
    if !identifier(id)
        || !identifier(session_id)
        || (!worker_id.is_empty() && !identifier(worker_id))
    {
        return Err(invalid("invalid Pi session identity"));
    }
    Ok(Context {
        session_id: session_id.to_owned(),
        worker_id: worker_id.to_owned(),
        turn_id: String::new(),
    })
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
    kind: &str,
    time_ns: &str,
    context: &Context,
    payload: Value,
    operation: Option<&str>,
) -> Value {
    json!({"event_id":event_id,"revision":0,"kind":kind,"time_ns":time_ns,
           "identity":identity(context, operation),"payload":payload})
}

fn stamp(row: &Value) -> Result<String> {
    let raw = row
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("Pi row lacks timestamp")?;
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)?;
    let nanos = parsed.unix_timestamp_nanos();
    if nanos < 0 || nanos > i64::MAX as i128 {
        return Err(invalid("Pi timestamp exceeds supported range"));
    }
    Ok(nanos.to_string())
}

fn usage(message: &Value) -> Result<Option<Value>> {
    let Some(raw) = message.get("usage") else {
        return Ok(None);
    };
    let fields = raw.as_object().ok_or("Pi usage is not an object")?;
    let mut result = serde_json::Map::new();
    for (source, target) in [
        ("input", "input_tokens"),
        ("output", "output_tokens"),
        ("cacheRead", "cache_read_tokens"),
        ("cacheWrite", "cache_write_tokens"),
    ] {
        if let Some(value) = fields.get(source) {
            let count = value.as_u64().ok_or("invalid Pi usage counter")?;
            if count > i64::MAX as u64 {
                return Err(invalid("Pi usage counter exceeds supported range"));
            }
            result.insert(target.to_owned(), json!(count));
        }
    }
    Ok((!result.is_empty()).then_some(Value::Object(result)))
}

fn category(name: &str) -> &'static str {
    if name.starts_with("mcp__") {
        "mcp"
    } else if matches!(name, "read" | "write" | "edit" | "Read" | "Write" | "Edit") {
        "file"
    } else if matches!(name, "bash" | "shell" | "exec" | "Bash" | "Shell") {
        "shell"
    } else {
        "other"
    }
}

/// Returns all facts for one source line and the context to checkpoint only
/// when the caller accepts that entire vector into its poll batch.
pub(crate) fn translate_line(
    raw: &[u8],
    context: &Context,
    origin: &str,
    offset: u64,
) -> Result<(Vec<Value>, Context)> {
    let row: Value = serde_json::from_slice(raw)?;
    let kind = row
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Pi row lacks type")?;
    let mut next = context.clone();
    if kind == "session" {
        let header = header_identity(raw)?;
        if header.session_id != context.session_id || header.worker_id != context.worker_id {
            return Err(invalid("Pi session identity changed inside one file"));
        }
        return Ok((vec![], next));
    }
    let row_id = row.get("id").and_then(Value::as_str);
    let base = if let Some(id) = row_id {
        if !identifier(id) {
            return Err(invalid("invalid Pi row ID"));
        }
        format!("pi:{origin}:{id}")
    } else {
        format!("pi:{origin}:offset-{offset}")
    };
    if base.len() > 230 {
        return Err(invalid("Pi event identity exceeds wire limit"));
    }
    if kind != "message" {
        // Unsupported Pi metadata is visible as a coarse diagnostic. An
        // adapter never claims complete coverage by silently skipping it.
        let time_ns = row
            .get("timestamp")
            .map_or(Ok("0".to_owned()), |_| stamp(&row))?;
        return Ok((
            vec![
                json!({"event_id":format!("{base}:unsupported"),"revision":0,
            "kind":"diagnostic","time_ns":time_ns,
            "payload":{"code":"unsupported_pi_row","severity":"info"}}),
            ],
            next,
        ));
    }
    let time_ns = stamp(&row)?;
    let message = row
        .get("message")
        .and_then(Value::as_object)
        .ok_or("Pi message is not an object")?;
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or("Pi message lacks role")?;
    let mut facts = Vec::new();
    match role {
        "user" => {
            let id = row_id.ok_or("Pi user message lacks ID")?;
            next.turn_id = id.to_owned();
            facts.push(fact(
                format!("{base}:message"),
                "message",
                &time_ns,
                &next,
                json!({"role":"user"}),
                None,
            ));
            facts.push(fact(
                format!("{base}:lifecycle"),
                "lifecycle",
                &time_ns,
                &next,
                json!({"state":"running"}),
                None,
            ));
        }
        "assistant" => {
            row_id.ok_or("Pi assistant message lacks ID")?;
            facts.push(fact(
                format!("{base}:message"),
                "message",
                &time_ns,
                &next,
                json!({"role":"assistant"}),
                None,
            ));
            if let Some(counters) = usage(&row["message"])? {
                facts.push(fact(
                    format!("{base}:usage"),
                    "usage",
                    &time_ns,
                    &next,
                    counters,
                    None,
                ));
            }
            if let Some(content) = message.get("content") {
                let blocks = content
                    .as_array()
                    .ok_or("Pi assistant content is not an array")?;
                if blocks.len() > 128 {
                    return Err(invalid("Pi assistant content block limit exceeded"));
                }
                for (index, block) in blocks.iter().enumerate() {
                    if block.get("type").and_then(Value::as_str) != Some("toolCall") {
                        continue;
                    }
                    let operation = block
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or("Pi tool call lacks ID")?;
                    if !identifier(operation) {
                        return Err(invalid("invalid Pi tool call identity"));
                    }
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    facts.push(fact(
                        format!("{base}:tool-{index}"),
                        "tool",
                        &time_ns,
                        &next,
                        json!({"category":category(name),"result":"started"}),
                        Some(operation),
                    ));
                }
            }
            match message.get("stopReason").and_then(Value::as_str) {
                Some("stop") => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"completed"}),
                    None,
                )),
                Some("error") => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"interrupted","reason_code":"pi_stop_error"}),
                    None,
                )),
                _ => facts.push(fact(
                    format!("{base}:lifecycle"),
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"running"}),
                    None,
                )),
            }
        }
        "toolResult" => {
            row_id.ok_or("Pi tool result lacks ID")?;
            let operation = message
                .get("toolCallId")
                .and_then(Value::as_str)
                .ok_or("Pi tool result lacks call ID")?;
            if !identifier(operation) {
                return Err(invalid("invalid Pi tool result identity"));
            }
            let failed = message
                .get("isError")
                .and_then(Value::as_bool)
                .ok_or("Pi tool result lacks explicit isError")?;
            let name = message
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("");
            // The current user turn may have advanced since this call began.
            // The host can relate a result to the original turn only from one
            // policy-current, admitted call fact with this operation ID.
            let mut result_context = next.clone();
            result_context.turn_id.clear();
            facts.push(fact(
                format!("{base}:tool-result"),
                "tool",
                &time_ns,
                &result_context,
                json!({"category":category(name),
                                          "result":if failed {"failed"} else {"succeeded"}}),
                Some(operation),
            ));
            facts.push(fact(
                format!("{base}:lifecycle"),
                "lifecycle",
                &time_ns,
                &next,
                json!({"state":"running"}),
                None,
            ));
        }
        _ => facts.push(
            json!({"event_id":format!("{base}:unsupported"),"revision":0,
            "kind":"diagnostic","time_ns":time_ns,
            "payload":{"code":"unsupported_pi_role","severity":"warning"}}),
        ),
    }
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_assistant_row_yields_usage_two_tools_and_no_false_completion() {
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        let (facts, next) = translate_line(br#"{"type":"message","id":"a-1","timestamp":"2026-10-24T12:00:01Z","message":{"role":"assistant","stopReason":"toolUse","usage":{"input":8,"output":3},"content":[{"type":"toolCall","id":"c-1","name":"read","arguments":{"path":"secret"}},{"type":"toolCall","id":"c-2","name":"bash","arguments":{"command":"secret"}}]}}"#, &context, "file", 10).unwrap();
        assert_eq!(facts.len(), 5);
        assert_eq!(facts[1]["kind"], "usage");
        assert_eq!(facts[2]["identity"]["operation_id"], "c-1");
        assert_eq!(facts[3]["payload"]["category"], "shell");
        assert_eq!(facts[4]["payload"]["state"], "running");
        assert!(!serde_json::to_string(&facts).unwrap().contains("secret"));
        assert_eq!(next, context);
    }

    #[test]
    fn explicit_stop_and_error_are_terminal_but_missing_result_is_not_success() {
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        for (stop, state) in [("stop", "completed"), ("error", "interrupted")] {
            let row = format!(
                r#"{{"type":"message","id":"a","timestamp":"2026-10-24T12:00:00Z","message":{{"role":"assistant","stopReason":"{stop}"}}}}"#
            );
            let (facts, _) = translate_line(row.as_bytes(), &context, "file", 0).unwrap();
            assert_eq!(facts[1]["payload"]["state"], state);
        }
        assert!(translate_line(br#"{"type":"message","id":"t","timestamp":"2026-10-24T12:00:00Z","message":{"role":"toolResult","toolCallId":"c","toolName":"read"}}"#, &context, "file", 0).is_err());
    }

    #[test]
    fn malformed_identity_usage_and_time_fail_before_fact_emission() {
        assert!(header_identity(br#"{"type":"session","id":"../private"}"#).is_err());
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        for raw in [
            br#"{"type":"message","id":"a","timestamp":"not-a-time","message":{"role":"assistant"}}"#.as_slice(),
            br#"{"type":"message","id":"a","timestamp":"2026-10-24T12:00:00Z","message":{"role":"assistant","usage":{"input":-1}}}"#.as_slice(),
            br#"{"type":"message","id":"a","timestamp":"2026-10-24T12:00:00Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"../bad","name":"read"}]}}"#.as_slice(),
        ] {
            assert!(translate_line(raw, &context, "file", 0).is_err());
        }
    }

    #[test]
    fn explicit_pi_session_and_worker_ids_remain_distinct() {
        let context = header_identity(
            br#"{"type":"session","id":"file-id","sessionId":"session-1","agentId":"worker-1"}"#,
        )
        .unwrap();
        let (facts, next) = translate_line(br#"{"type":"message","id":"user-1","timestamp":"2026-10-24T12:00:00Z","message":{"role":"user"}}"#, &context, "file", 0).unwrap();
        assert_eq!(facts[0]["identity"]["session_id"], "session-1");
        assert_eq!(facts[0]["identity"]["worker_id"], "worker-1");
        assert_eq!(next.turn_id, "user-1");
    }

    #[test]
    fn result_does_not_borrow_current_turn_from_a_later_user() {
        let context = Context {
            session_id: "pi-1".into(),
            worker_id: String::new(),
            turn_id: "user-2".into(),
        };
        let (facts, next) = translate_line(
            br#"{"type":"message","id":"result-1","timestamp":"2026-10-24T12:00:03Z","message":{"role":"toolResult","toolCallId":"call-1","toolName":"read","isError":true,"content":"private output"}}"#,
            &context,
            "file",
            100,
        )
        .unwrap();
        assert_eq!(facts[0]["identity"]["operation_id"], "call-1");
        assert!(facts[0]["identity"].get("turn_id").is_none());
        assert_eq!(facts[1]["identity"]["turn_id"], "user-2");
        assert_eq!(next, context);
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }
}
