//! Content-free translation of one Codex rollout JSONL row.
//! Completion and interruption require explicit source events. A PR, tool
//! result, EOF or ordinary assistant message never supplies a terminal state.

use agenturm_jsonl_source::Result;
use serde_json::{Value, json};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub session_id: String,
    pub worker_id: String,
    pub turn_id: String,
}

pub(crate) fn header_identity(raw: &[u8]) -> Result<Context> {
    let row: Value = serde_json::from_slice(raw)?;
    if row.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err(invalid("Codex source must start with session_meta"));
    }
    let payload = row
        .get("payload")
        .and_then(Value::as_object)
        .ok_or("Codex header payload missing")?;
    let thread = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Codex thread ID missing")?;
    let root = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or(thread);
    if !identifier(thread) || !identifier(root) {
        return Err(invalid("invalid Codex thread identity"));
    }
    Ok(Context {
        session_id: thread.to_owned(),
        worker_id: if root != thread {
            thread.to_owned()
        } else {
            String::new()
        },
        turn_id: String::new(),
    })
}

fn identity(context: &Context, operation: Option<&str>) -> Value {
    let mut value = json!({"session_id":context.session_id});
    if !context.worker_id.is_empty() {
        value["worker_id"] = json!(context.worker_id);
    }
    if !context.turn_id.is_empty() {
        value["turn_id"] = json!(context.turn_id);
    }
    if let Some(operation) = operation {
        value["operation_id"] = json!(operation);
    }
    value
}

fn fact(
    id: String,
    kind: &str,
    stamp: &str,
    context: &Context,
    payload: Value,
    operation: Option<&str>,
) -> Value {
    json!({"event_id":id,"revision":0,"kind":kind,"time_ns":stamp,
           "identity":identity(context, operation),"payload":payload})
}

fn diagnostic(id: String, stamp: &str, code: &str, severity: &str) -> Value {
    json!({"event_id":id,"revision":0,"kind":"diagnostic","time_ns":stamp,
           "payload":{"code":code,"severity":severity}})
}

fn stamp(row: &Value) -> Result<String> {
    let raw = row
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("Codex row timestamp missing")?;
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)?;
    let nanos = parsed.unix_timestamp_nanos();
    if nanos < 0 || nanos > i64::MAX as i128 {
        return Err(invalid("Codex timestamp outside supported range"));
    }
    Ok(nanos.to_string())
}

fn turn_from(payload: &Value, context: &mut Context, clear_if_missing: bool) -> Result<()> {
    if let Some(raw) = payload.get("turn_id") {
        let turn = raw.as_str().ok_or("invalid Codex turn ID")?;
        if !identifier(turn) {
            return Err(invalid("invalid Codex turn ID"));
        }
        context.turn_id = turn.to_owned();
    } else if clear_if_missing {
        context.turn_id.clear();
    }
    Ok(())
}

fn historical_turn(payload: &Value, current: &Context) -> bool {
    payload
        .get("turn_id")
        .and_then(Value::as_str)
        .is_some_and(|turn| !current.turn_id.is_empty() && turn != current.turn_id)
}

fn opaque_operation(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.len() > 4096 {
        return Err(invalid("invalid Codex operation ID"));
    }
    Ok(format!("{:x}", Sha256::digest(raw.as_bytes())))
}

fn tool_category(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("mcp__") {
        "mcp"
    } else if matches!(
        lower.as_str(),
        "exec_command" | "shell_command" | "bash" | "sh" | "terminal" | "functions.exec"
    ) {
        "shell"
    } else if matches!(
        lower.as_str(),
        "read_file" | "write_file" | "apply_patch" | "edit" | "read" | "write"
    ) {
        "file"
    } else {
        "other"
    }
}

fn item_category(kind: &str) -> Option<&'static str> {
    match kind {
        "CommandExecution" => Some("shell"),
        "FileChange" => Some("file"),
        "McpToolCall" => Some("mcp"),
        "Extension" => Some("other"),
        _ => None,
    }
}

fn explicit_tool_result(item: &Value) -> Result<Option<&'static str>> {
    let status = item.get("status").and_then(Value::as_str).unwrap_or("");
    let exit = match item.get("exit_code") {
        Some(value) => Some(value.as_i64().ok_or("invalid Codex exit code")?),
        None => None,
    };
    if status.eq_ignore_ascii_case("denied") || status.eq_ignore_ascii_case("declined") {
        return Ok(Some("denied"));
    }
    if matches!(status, "failed" | "error") || exit.is_some_and(|code| code != 0) {
        return Ok(Some("failed"));
    }
    if let Some(error) = item.get("result").and_then(|value| value.get("isError")) {
        return match error.as_bool() {
            Some(true) => Ok(Some("failed")),
            Some(false) => Ok(Some("succeeded")),
            None => Err(invalid("invalid Codex result flag")),
        };
    }
    if matches!(status, "completed" | "success" | "ok") || exit == Some(0) {
        return Ok(Some("succeeded"));
    }
    Ok(None)
}

fn check_category(item: &Value) -> Option<&'static str> {
    let command = item.get("command")?.as_array()?;
    if command.is_empty() || command.len() > 8 {
        return None;
    }
    let words: Vec<&str> = command.iter().map(Value::as_str).collect::<Option<_>>()?;
    let raw = if words.len() >= 3 && matches!(words[1], "-lc" | "-c") {
        words[2]
    } else {
        return None;
    };
    if raw.len() > 160
        || raw.bytes().any(|byte| {
            matches!(
                byte,
                b';' | b'|' | b'&' | b'>' | b'<' | b'\n' | b'\r' | b'$'
            )
        })
    {
        return None;
    }
    let parts: Vec<_> = raw.split_whitespace().collect();
    match parts.as_slice() {
        ["cargo", "test", ..]
        | ["pytest", ..]
        | ["python", "-m", "pytest", ..]
        | ["go", "test", ..] => Some("test"),
        ["cargo", "clippy", ..] | ["ruff", "check", ..] => Some("lint"),
        ["cargo", "build", ..] | ["go", "build", ..] => Some("build"),
        ["npm", "run", name] | ["pnpm", "run", name] | ["yarn", name] => {
            if *name == "check" || *name == "typecheck" || *name == "lint" {
                Some("lint")
            } else if *name == "build" {
                Some("build")
            } else if *name == "test" || name.starts_with("test:") {
                Some("test")
            } else {
                None
            }
        }
        _ => None,
    }
}

fn usage(payload: &Value) -> Result<Option<Value>> {
    let Some(raw) = payload.get("usage") else {
        return Ok(None);
    };
    let fields = raw.as_object().ok_or("Codex usage is not an object")?;
    let mut result = serde_json::Map::new();
    for (source, target) in [
        ("input_tokens", "input_tokens"),
        ("output_tokens", "output_tokens"),
        ("cached_input_tokens", "cache_read_tokens"),
        ("cache_write_input_tokens", "cache_write_tokens"),
        ("total_tokens", "total_tokens"),
    ] {
        if let Some(value) = fields.get(source) {
            let count = value.as_u64().ok_or("invalid Codex usage counter")?;
            if count > i64::MAX as u64 {
                return Err(invalid("Codex usage counter exceeds supported range"));
            }
            result.insert(target.to_owned(), json!(count));
        }
    }
    Ok((!result.is_empty()).then_some(Value::Object(result)))
}

/// Translate one complete row. The shared runtime only checkpoints `next`
/// after every fact from this row fits in a single acknowledged batch.
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
        .ok_or("Codex row type missing")?;
    let mut next = context.clone();
    if kind == "session_meta" {
        let header = header_identity(raw)?;
        if header.session_id != context.session_id || header.worker_id != context.worker_id {
            return Err(invalid("Codex session identity changed inside one file"));
        }
        return Ok((Vec::new(), next));
    }
    let time_ns = stamp(&row)?;
    let base = format!("codex:{origin}:{offset}");
    let payload = row.get("payload").ok_or("Codex row payload missing")?;
    if !payload.is_object() {
        return Err(invalid("Codex row payload is not an object"));
    }
    let mut row_context = context.clone();
    turn_from(payload, &mut row_context, false)?;
    let event = payload.get("type").and_then(Value::as_str).unwrap_or("");
    let mut facts = Vec::new();
    match kind {
        "turn_context" => {
            if payload.get("model").and_then(Value::as_str) == Some("codex-auto-review") {
                return Err(invalid("internal Codex review is outside this adapter"));
            }
        }
        "token_usage_record" => {
            let response = payload
                .get("response_id")
                .and_then(Value::as_str)
                .ok_or("Codex usage response ID missing")?;
            let response_hash = opaque_operation(response)?;
            if let Some(counters) = usage(payload)? {
                facts.push(fact(
                    format!("codex:{origin}:usage:{response_hash}"),
                    "usage",
                    &time_ns,
                    &row_context,
                    counters,
                    None,
                ));
            } else {
                facts.push(diagnostic(
                    format!("{base}:usage-unavailable"),
                    &time_ns,
                    "codex_usage_unavailable",
                    "warning",
                ));
            }
        }
        "response_item" => match event {
            "message" => {
                let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                if matches!(role, "user" | "assistant") {
                    if role == "user" {
                        // Do not attach an unmarked new user message to the
                        // previous turn's exact drilldown key.
                        turn_from(payload, &mut next, true)?;
                        row_context = next.clone();
                    }
                    facts.push(fact(
                        format!("{base}:message"),
                        "message",
                        &time_ns,
                        &row_context,
                        json!({"role":role}),
                        None,
                    ));
                } else {
                    facts.push(diagnostic(
                        format!("{base}:role"),
                        &time_ns,
                        "unsupported_codex_role",
                        "info",
                    ));
                }
            }
            "function_call" | "custom_tool_call" => {
                let call = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .ok_or("Codex tool call ID missing")?;
                let operation = opaque_operation(call)?;
                let category =
                    tool_category(payload.get("name").and_then(Value::as_str).unwrap_or(""));
                facts.push(fact(
                    format!("{base}:tool-call"),
                    "tool",
                    &time_ns,
                    &row_context,
                    json!({"category":category,"result":"started"}),
                    Some(&operation),
                ));
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .ok_or("Codex tool result ID missing")?;
                let operation = opaque_operation(call)?;
                if let Some(flag) = payload.get("isError") {
                    let failed = flag.as_bool().ok_or("invalid Codex result flag")?;
                    facts.push(fact(format!("{base}:tool-result"), "tool", &time_ns,
                        &row_context, json!({"category":"other","result":if failed {"failed"} else {"succeeded"}}),
                        Some(&operation)));
                } else {
                    facts.push(diagnostic(
                        format!("{base}:tool-result-unknown"),
                        &time_ns,
                        "codex_tool_result_unknown",
                        "info",
                    ));
                }
            }
            _ => facts.push(diagnostic(
                format!("{base}:response"),
                &time_ns,
                "unsupported_codex_response",
                "info",
            )),
        },
        "event_msg" => {
            match event {
                "task_started" => {
                    turn_from(payload, &mut next, true)?;
                    row_context = next.clone();
                    facts.push(fact(
                        format!("{base}:lifecycle"),
                        "lifecycle",
                        &time_ns,
                        &row_context,
                        json!({"state":"running"}),
                        None,
                    ));
                }
                "task_complete" | "task_aborted" | "turn_aborted" => {
                    if historical_turn(payload, context) {
                        facts.push(diagnostic(
                            format!("{base}:historical-outcome"),
                            &time_ns,
                            "codex_historical_outcome",
                            "info",
                        ));
                    } else {
                        let (state, reason) = match event {
                            "task_complete" => ("completed", "codex_task_complete"),
                            "task_aborted" => ("interrupted", "codex_task_aborted"),
                            _ => ("interrupted", "codex_turn_aborted"),
                        };
                        facts.push(fact(
                            format!("{base}:lifecycle"),
                            "lifecycle",
                            &time_ns,
                            &row_context,
                            json!({"state":state,"reason_code":reason}),
                            None,
                        ));
                    }
                }
                "approval_request" | "request_user_input" => {
                    if historical_turn(payload, context) {
                        facts.push(diagnostic(
                            format!("{base}:historical-wait"),
                            &time_ns,
                            "codex_historical_wait",
                            "info",
                        ));
                    } else {
                        facts.push(fact(format!("{base}:lifecycle"), "lifecycle", &time_ns,
                        &row_context, json!({"state":"paused","reason_code":if event == "approval_request" {"codex_approval_request"} else {"codex_user_input_request"}}), None));
                    }
                }
                "item_completed" => {
                    let item = payload.get("item").ok_or("Codex completed item missing")?;
                    let item_kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .ok_or("Codex item type missing")?;
                    match item_kind {
                        "UserMessage" | "AgentMessage" => {
                            let role = if item_kind == "UserMessage" {
                                "user"
                            } else {
                                "assistant"
                            };
                            if role == "user" {
                                turn_from(payload, &mut next, true)?;
                                row_context = next.clone();
                            }
                            facts.push(fact(
                                format!("{base}:message"),
                                "message",
                                &time_ns,
                                &row_context,
                                json!({"role":role}),
                                None,
                            ));
                            if role == "user" {
                                facts.push(fact(
                                    format!("{base}:lifecycle"),
                                    "lifecycle",
                                    &time_ns,
                                    &row_context,
                                    json!({"state":"running"}),
                                    None,
                                ));
                            } else if item.get("phase").and_then(Value::as_str)
                                == Some("final_answer")
                            {
                                if historical_turn(payload, context) {
                                    facts.push(diagnostic(
                                        format!("{base}:historical-final"),
                                        &time_ns,
                                        "codex_historical_outcome",
                                        "info",
                                    ));
                                } else {
                                    facts.push(fact(format!("{base}:lifecycle"), "lifecycle", &time_ns,
                                    &row_context, json!({"state":"completed","reason_code":"codex_final_answer"}), None));
                                }
                            }
                        }
                        "CommandExecution" | "FileChange" | "McpToolCall" | "Extension" => {
                            let category =
                                item_category(item_kind).ok_or("unsupported Codex tool")?;
                            if let Some(result) = explicit_tool_result(item)? {
                                let operation = item
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .map(opaque_operation)
                                    .transpose()?;
                                facts.push(fact(
                                    format!("{base}:tool"),
                                    "tool",
                                    &time_ns,
                                    &row_context,
                                    json!({"category":category,"result":result}),
                                    operation.as_deref(),
                                ));
                                if item_kind == "CommandExecution"
                                    && let Some(check) = check_category(item)
                                {
                                    facts.push(fact(format!("{base}:check"), "check", &time_ns,
                                    &row_context, json!({"category":check,"result":if result == "succeeded" {"passed"} else if result == "failed" {"failed"} else {"unknown"}}), None));
                                }
                            } else {
                                facts.push(diagnostic(
                                    format!("{base}:tool-unknown"),
                                    &time_ns,
                                    "codex_tool_result_unknown",
                                    "info",
                                ));
                            }
                        }
                        "Reasoning" | "ContextCompaction" => {}
                        _ => facts.push(diagnostic(
                            format!("{base}:item"),
                            &time_ns,
                            "unsupported_codex_item",
                            "info",
                        )),
                    }
                }
                "token_count" => facts.push(diagnostic(
                    format!("{base}:legacy-usage"),
                    &time_ns,
                    "codex_legacy_usage_unmapped",
                    "info",
                )),
                _ => facts.push(diagnostic(
                    format!("{base}:event"),
                    &time_ns,
                    "unsupported_codex_event",
                    "info",
                )),
            }
        }
        _ => facts.push(diagnostic(
            format!("{base}:row"),
            &time_ns,
            "unsupported_codex_row",
            "info",
        )),
    }
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &str, payload: Value) -> Vec<u8> {
        json!({"type":kind,"timestamp":"2026-11-14T10:00:00Z","payload":payload})
            .to_string()
            .into_bytes()
    }

    fn translate(payload: Value, context: &Context) -> (Vec<Value>, Context) {
        translate_line(&row("event_msg", payload), context, "generation", 10).unwrap()
    }

    #[test]
    fn explicit_terminal_events_keep_distinct_reason_provenance() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let (_, running) = translate(json!({"type":"task_started","turn_id":"turn-1"}), &context);
        for (event, state, reason) in [
            ("task_complete", "completed", "codex_task_complete"),
            ("task_aborted", "interrupted", "codex_task_aborted"),
            ("turn_aborted", "interrupted", "codex_turn_aborted"),
        ] {
            let (facts, _) = translate(
                json!({"type":event,"turn_id":"turn-1","reason":"private reason"}),
                &running,
            );
            assert_eq!(facts[0]["payload"]["state"], state);
            assert_eq!(facts[0]["payload"]["reason_code"], reason);
            assert_eq!(facts[0]["identity"]["turn_id"], "turn-1");
            assert!(!facts[0].to_string().contains("private reason"));
        }
        let (ordinary, _) = translate(
            json!({"type":"item_completed","item":{
            "type":"AgentMessage","phase":"commentary","content":"PR raised"}}),
            &running,
        );
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0]["kind"], "message");
        let (final_answer, _) = translate(
            json!({"type":"item_completed","item":{
            "type":"AgentMessage","phase":"final_answer","content":"private answer"}}),
            &running,
        );
        assert_eq!(
            final_answer[1]["payload"]["reason_code"],
            "codex_final_answer"
        );
        assert!(
            !serde_json::to_string(&final_answer)
                .unwrap()
                .contains("private answer")
        );
    }

    #[test]
    fn command_check_and_usage_are_typed_without_source_content() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let (facts, _) = translate(
            json!({"type":"item_completed","turn_id":"turn-1","item":{
            "type":"CommandExecution","id":"check-1","command":["/usr/bin/zsh","-lc","npm run check"],
            "status":"completed","exit_code":0,"aggregated_output":"PRIVATE OUTPUT"}}),
            &context,
        );
        assert_eq!(facts.len(), 2);
        assert_eq!(
            facts[0]["payload"],
            json!({"category":"shell","result":"succeeded"})
        );
        assert_eq!(
            facts[1]["payload"],
            json!({"category":"lint","result":"passed"})
        );
        assert!(facts[0]["identity"]["operation_id"].as_str().unwrap().len() == 64);
        assert!(!serde_json::to_string(&facts).unwrap().contains("PRIVATE"));
        assert!(!serde_json::to_string(&facts).unwrap().contains("npm"));
        let raw = row(
            "token_usage_record",
            json!({"turn_id":"turn-1","response_id":"response-1",
            "usage":{"input_tokens":7,"output_tokens":3,"cached_input_tokens":2,"reasoning_output_tokens":1}}),
        );
        let (usage, _) = translate_line(&raw, &context, "generation", 20).unwrap();
        assert_eq!(
            usage[0]["payload"],
            json!({"input_tokens":7,"output_tokens":3,"cache_read_tokens":2})
        );
        assert!(
            !serde_json::to_string(&usage)
                .unwrap()
                .contains("response-1")
        );
    }

    #[test]
    fn absent_result_flag_never_becomes_success() {
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let raw = row(
            "response_item",
            json!({"type":"function_call_output","call_id":"call-1",
            "output":"private successful looking text"}),
        );
        let (facts, _) = translate_line(&raw, &context, "generation", 10).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0]["payload"]["code"], "codex_tool_result_unknown");
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }

    #[test]
    fn malformed_identity_usage_and_internal_review_fail_visibly() {
        assert!(
            header_identity(br#"{"type":"session_meta","payload":{"id":"../private"}}"#).is_err()
        );
        let context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        let bad_usage = row(
            "token_usage_record",
            json!({"response_id":"r","usage":{"input_tokens":"7"}}),
        );
        assert!(translate_line(&bad_usage, &context, "generation", 1).is_err());
        let bad_turn = row(
            "event_msg",
            json!({"type":"task_started","turn_id":"../private"}),
        );
        assert!(translate_line(&bad_turn, &context, "generation", 1).is_err());
        let internal = row("turn_context", json!({"model":"codex-auto-review"}));
        assert!(translate_line(&internal, &context, "generation", 1).is_err());
        let bad_result = row(
            "response_item",
            json!({"type":"function_call_output",
            "call_id":"c1","isError":"false"}),
        );
        assert!(translate_line(&bad_result, &context, "generation", 1).is_err());
    }

    #[test]
    fn unmarked_new_user_message_does_not_inherit_old_turn() {
        let mut context =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        context.turn_id = "old-turn".to_owned();
        let (facts, next) = translate(
            json!({"type":"item_completed","item":{
            "type":"UserMessage","content":"private new request"}}),
            &context,
        );
        assert!(next.turn_id.is_empty());
        assert!(facts[0]["identity"].get("turn_id").is_none());
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }

    #[test]
    fn late_old_turn_fact_keeps_row_scope_without_rewinding_checkpoint() {
        let mut current =
            header_identity(br#"{"type":"session_meta","payload":{"id":"thread-1"}}"#).unwrap();
        current.turn_id = "turn-2".to_owned();
        let old_usage = row(
            "token_usage_record",
            json!({"turn_id":"turn-1",
            "response_id":"late-usage","usage":{"input_tokens":3}}),
        );
        let (usage_facts, after_usage) =
            translate_line(&old_usage, &current, "generation", 10).unwrap();
        assert_eq!(usage_facts[0]["identity"]["turn_id"], "turn-1");
        assert_eq!(after_usage.turn_id, "turn-2");
        let unmarked = row(
            "response_item",
            json!({"type":"function_call",
            "call_id":"call-new","name":"functions.exec"}),
        );
        let (tool_facts, after_tool) =
            translate_line(&unmarked, &after_usage, "generation", 20).unwrap();
        assert_eq!(tool_facts[0]["identity"]["turn_id"], "turn-2");
        assert_eq!(after_tool.turn_id, "turn-2");
        let old_terminal = row(
            "event_msg",
            json!({"type":"task_complete","turn_id":"turn-1"}),
        );
        let (terminal_facts, after_terminal) =
            translate_line(&old_terminal, &after_tool, "generation", 30).unwrap();
        assert_eq!(
            terminal_facts[0]["payload"]["code"],
            "codex_historical_outcome"
        );
        assert_eq!(after_terminal.turn_id, "turn-2");
        let old_final = row(
            "event_msg",
            json!({"type":"item_completed","turn_id":"turn-1",
            "item":{"type":"AgentMessage","phase":"final_answer","content":"private old answer"}}),
        );
        let (final_facts, after_final) =
            translate_line(&old_final, &after_terminal, "generation", 40).unwrap();
        assert_eq!(final_facts[0]["identity"]["turn_id"], "turn-1");
        assert_eq!(
            final_facts[1]["payload"]["code"],
            "codex_historical_outcome"
        );
        assert_eq!(after_final.turn_id, "turn-2");
    }
}
