//! A bounded, content-free translation of one actual Pi session JSONL line.
//! Source text is parsed in the child and never placed in a process fact.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
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

fn operation_id(value: &str) -> Option<Cow<'_, str>> {
    if identifier(value) {
        return Some(Cow::Borrowed(value));
    }
    let (call, item) = value.split_once('|')?;
    let call_suffix = call.strip_prefix("call_")?;
    let item_suffix = item
        .strip_prefix("fc_")
        .or_else(|| item.strip_prefix("ctc_"))?;
    if call_suffix.is_empty() || item_suffix.is_empty() || !identifier(call) || !identifier(item) {
        return None;
    }
    // Pi combines the provider call and item IDs. Hash the pair into a
    // namespace containing a slash, which plain native Pi IDs cannot contain.
    Some(Cow::Owned(format!("pi-call/{:x}", Sha256::digest(value))))
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
        if offset == 0
            && let Some(parent) = row.get("parentSessionId").and_then(Value::as_str)
            && identifier(parent)
            && parent != header.session_id
            && let Ok(time_ns) = stamp(&row)
        {
            return Ok((
                vec![fact(
                    format!("pi:{origin}:{offset}:relationship"),
                    "relationship",
                    &time_ns,
                    &header,
                    json!({"relationship_kind":"delegated","parent_session_id":parent}),
                    None,
                )],
                next,
            ));
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
                    let native_operation = block
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or("Pi tool call lacks ID")?;
                    let operation =
                        operation_id(native_operation).ok_or("invalid Pi tool call identity")?;
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    facts.push(fact(
                        format!("{base}:tool-{index}"),
                        "tool",
                        &time_ns,
                        &next,
                        json!({"category":category(name),"result":"started"}),
                        Some(&operation),
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
            let native_operation = message
                .get("toolCallId")
                .and_then(Value::as_str)
                .ok_or("Pi tool result lacks call ID")?;
            let operation =
                operation_id(native_operation).ok_or("invalid Pi tool result identity")?;
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
                Some(&operation),
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
    fn composite_provider_operation_ids_pair_calls_and_results() {
        let context = header_identity(br#"{"type":"session","id":"pi-1"}"#).unwrap();
        for native_id in ["call_abc|fc_xyz", "call_abc|ctc_xyz"] {
            let call = json!({
                "type":"message","id":"call","timestamp":"2026-10-24T12:00:00Z",
                "message":{"role":"assistant","stopReason":"toolUse",
                    "content":[{"type":"toolCall","id":native_id,"name":"read"}]}
            });
            let result = json!({
                "type":"message","id":"result","timestamp":"2026-10-24T12:00:01Z",
                "message":{"role":"toolResult","toolCallId":native_id,"toolName":"read","isError":false}
            });
            let (call_facts, next) =
                translate_line(call.to_string().as_bytes(), &context, "file", 0).unwrap();
            let (result_facts, _) =
                translate_line(result.to_string().as_bytes(), &next, "file", 1).unwrap();
            let [call, result] = [&call_facts, &result_facts]
                .map(|facts| facts.iter().find(|fact| fact["kind"] == "tool").unwrap());
            let id = call["identity"]["operation_id"].as_str().unwrap();
            assert!(id.starts_with("pi-call/"));
            assert_eq!(id, result["identity"]["operation_id"]);
            assert!(!call.to_string().contains(native_id));
            assert!(!result.to_string().contains(native_id));
        }
        assert_eq!(operation_id("plain-id").as_deref(), Some("plain-id"));
        for malformed in [
            "call_|fc_abc",
            "call_abc|fc_",
            "call_abc|other",
            "call_abc|fc_abc|fc_xyz",
        ] {
            assert!(operation_id(malformed).is_none());
        }
    }

    #[test]
    fn relationship_requires_an_explicit_distinct_parent_session_id() {
        let header = br#"{"type":"session","id":"child","timestamp":"2026-10-24T12:00:00Z","parentSessionId":"parent"}"#;
        let context = header_identity(header).unwrap();
        let (facts, _) = translate_line(header, &context, "file", 0).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["identity"], json!({"session_id":"child"}));
        assert_eq!(facts[0]["kind"], "relationship");
        assert_eq!(
            facts[0]["payload"],
            json!({"relationship_kind":"delegated","parent_session_id":"parent"}),
        );
        assert!(
            translate_line(header, &context, "file", 10)
                .unwrap()
                .0
                .is_empty()
        );

        for (field, value) in [
            ("parentSession", "path/to/private.jsonl"),
            ("parentAgentId", "worker"),
            ("parentSessionId", "../private"),
            ("parentSessionId", "child"),
        ] {
            let mut raw = json!({"type":"session","id":"child","timestamp":"2026-10-24T12:00:00Z"});
            raw[field] = json!(value);
            let (facts, _) =
                translate_line(raw.to_string().as_bytes(), &context, "file", 0).unwrap();
            assert!(facts.is_empty());
        }
    }

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
