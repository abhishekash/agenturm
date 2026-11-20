//! Content-free facts from one configured Kiro messages or sub-execution file.
//! Kiro JSONL has no session header: identity comes from its bounded path shape.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::path::Path;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn invalid(message: &'static str) -> Box<dyn Error> {
    message.into()
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub session_id: String,
    worker_id: String,
    path_hash: String,
    turn_id: String,
    turn_open: bool,
    queued_id: String,
    queued_stamp: String,
    pending_interactions: Vec<String>,
    open_call: String,
    worker_started: bool,
}

fn path_identity(path: &Path) -> Result<(String, String, String)> {
    if !path.is_absolute() {
        return Err(invalid("Kiro source path must be absolute"));
    }
    let name = path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("invalid Kiro path")?;
    let parent = path.parent().ok_or("invalid Kiro path")?;
    let (session_dir, worker) = if name == "messages.jsonl" {
        (parent, String::new())
    } else if parent.file_name().and_then(|part| part.to_str()) == Some("sub-executions")
        && path.extension().and_then(|part| part.to_str()) == Some("jsonl")
    {
        let worker = path
            .file_stem()
            .and_then(|part| part.to_str())
            .ok_or("invalid Kiro worker")?;
        (
            parent.parent().ok_or("invalid Kiro session path")?,
            worker.to_owned(),
        )
    } else {
        return Err(invalid("unsupported Kiro source path"));
    };
    let session = session_dir
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("invalid Kiro session")?;
    if !session.starts_with("sess_")
        || !identifier(session)
        || (!worker.is_empty() && !identifier(&worker))
    {
        return Err(invalid("invalid Kiro path identity"));
    }
    let rendered = path.to_str().ok_or("Kiro source path must be UTF-8")?;
    let digest = format!("{:x}", Sha256::digest(rendered.as_bytes()));
    Ok((session.to_owned(), worker, digest[..16].to_owned()))
}

pub(crate) fn initial_context(path: &Path, raw: &[u8]) -> Result<Context> {
    let (session, worker, path_hash) = path_identity(path)?;
    let first: Value = serde_json::from_slice(raw)?;
    if first
        .get("payload")
        .and_then(|payload| payload.get("type"))
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(invalid("Kiro first line lacks payload type"));
    }
    let native = if worker.is_empty() {
        session.clone()
    } else {
        worker.clone()
    };
    Ok(Context {
        session_id: native,
        worker_id: worker.clone(),
        path_hash,
        turn_id: worker.clone(),
        turn_open: !worker.is_empty(),
        queued_id: String::new(),
        queued_stamp: "0".to_owned(),
        pending_interactions: Vec::new(),
        open_call: String::new(),
        worker_started: false,
    })
}

pub(crate) fn checkpoint(context: &Context) -> String {
    format!(
        "k{}:{}:{}:{}:{}:{}:{}:{}",
        context.path_hash,
        u8::from(context.turn_open),
        context.turn_id,
        context.queued_id,
        context.queued_stamp,
        context.pending_interactions.join("."),
        u8::from(context.worker_started),
        context.open_call
    )
}

fn digest_key(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].to_owned()
}

fn interaction_key(payload: &serde_json::Map<String, Value>) -> Result<String> {
    let operation = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .ok_or("Kiro interaction lacks call ID")?;
    if !identifier(operation) {
        return Err(invalid("invalid Kiro interaction identity"));
    }
    let round = match payload.get("consentRound") {
        Some(Value::Number(number)) if number.is_i64() || number.is_u64() => number.to_string(),
        Some(Value::Number(_)) => return Err(invalid("unsupported Kiro consent round")),
        _ => "0".to_owned(),
    };
    Ok(digest_key(&format!("{operation}:{round}")))
}

fn digest_field(value: &str) -> bool {
    value.len() == 24 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn restore(context: &mut Context, raw: &str) -> Result<()> {
    let fields: Vec<_> = raw.split(':').collect();
    if fields.len() != 8
        || fields[0] != format!("k{}", context.path_hash)
        || !matches!(fields[1], "0" | "1")
        || (!fields[2].is_empty() && !identifier(fields[2]))
        || (!fields[3].is_empty() && !identifier(fields[3]))
        || fields[4].parse::<u64>().is_err()
        || (fields[3].is_empty() && fields[4] != "0")
        || (!fields[5].is_empty()
            && (fields[5].split('.').count() > 2 || !fields[5].split('.').all(digest_field)))
        || !matches!(fields[6], "0" | "1")
        || (!fields[7].is_empty()
            && (fields[7].len() != 26
                || !fields[7].is_ascii()
                || !digest_field(&fields[7][..24])
                || !matches!(&fields[7][24..25], "s" | "f" | "d")
                || !matches!(&fields[7][25..26], "m" | "s" | "f" | "o")))
    {
        return Err(invalid(
            "invalid Kiro checkpoint or configured path changed",
        ));
    }
    context.turn_open = fields[1] == "1";
    context.turn_id = fields[2].to_owned();
    context.queued_id = fields[3].to_owned();
    context.queued_stamp = fields[4].to_owned();
    context.pending_interactions = if fields[5].is_empty() {
        Vec::new()
    } else {
        fields[5].split('.').map(str::to_owned).collect()
    };
    context.worker_started = fields[6] == "1";
    context.open_call = fields[7].to_owned();
    Ok(())
}

fn stamp(row: &Value) -> Result<String> {
    let raw = row
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or("Kiro row lacks timestamp")?;
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)?;
    let nanos = parsed.unix_timestamp_nanos();
    if nanos < 0 || nanos > i64::MAX as i128 {
        return Err(invalid("Kiro timestamp exceeds supported range"));
    }
    Ok(nanos.to_string())
}

fn fact(
    base: &str,
    suffix: &str,
    kind: &str,
    time_ns: &str,
    context: &Context,
    payload: Value,
    operation: Option<&str>,
) -> Value {
    let mut identity = json!({"session_id":context.session_id});
    if !context.worker_id.is_empty() {
        identity["worker_id"] = json!(context.worker_id);
    }
    if !context.turn_id.is_empty() {
        identity["turn_id"] = json!(context.turn_id);
    }
    if let Some(operation) = operation {
        identity["operation_id"] = json!(operation);
    }
    json!({"event_id":format!("{base}:{suffix}"),"revision":0,"kind":kind,
           "time_ns":time_ns,"identity":identity,"payload":payload})
}

fn diagnostic(
    base: &str,
    suffix: &str,
    time_ns: &str,
    context: &Context,
    code: &str,
    severity: &str,
) -> Value {
    fact(
        base,
        suffix,
        "diagnostic",
        time_ns,
        context,
        json!({"code":code,"severity":severity}),
        None,
    )
}

fn category(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("mcp_") || lower.starts_with("mcp__") {
        "mcp"
    } else if matches!(lower.as_str(), "run_command" | "execute" | "bash" | "shell") {
        "shell"
    } else if matches!(
        lower.as_str(),
        "fs_write" | "write_file" | "str_replace" | "fs_append" | "delete_file" | "read_file"
    ) {
        "file"
    } else {
        "other"
    }
}

pub(crate) fn translate_line(
    raw: &[u8],
    context: &Context,
    origin: &str,
    offset: u64,
) -> Result<(Vec<Value>, Context)> {
    let row: Value = serde_json::from_slice(raw)?;
    let payload = row
        .get("payload")
        .and_then(Value::as_object)
        .ok_or("Kiro row lacks payload")?;
    let kind = payload
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Kiro payload lacks type")?;
    let row_id = row.get("id").and_then(Value::as_str);
    if let Some(id) = row_id
        && !identifier(id)
    {
        return Err(invalid("invalid Kiro row ID"));
    }
    let base = row_id.map_or_else(
        || format!("kiro:{origin}:offset-{offset}"),
        |id| format!("kiro:{origin}:{id}"),
    );
    let time_ns = stamp(&row)?;
    let mut next = context.clone();
    let mut facts = Vec::new();
    if !next.worker_id.is_empty()
        && !next.worker_started
        && matches!(
            kind,
            "user" | "assistant" | "tool_call" | "tool_result" | "turn_start"
        )
    {
        next.worker_started = true;
        facts.push(fact(
            &base,
            "worker-start",
            "lifecycle",
            &time_ns,
            &next,
            json!({"state":"running"}),
            None,
        ));
    }
    match kind {
        "user" => {
            let id = row_id.ok_or("Kiro user row lacks ID")?;
            if payload.get("source").and_then(Value::as_str) == Some("steer") {
                facts.push(fact(
                    &base,
                    "message",
                    "message",
                    &time_ns,
                    &next,
                    json!({"role":"user"}),
                    None,
                ));
            } else if next.turn_open {
                if !next.queued_id.is_empty() {
                    return Err(invalid(
                        "multiple queued Kiro prompts are not representable",
                    ));
                }
                next.queued_id = id.to_owned();
                next.queued_stamp = time_ns.clone();
                facts.push(diagnostic(
                    &base,
                    "queued",
                    &time_ns,
                    &next,
                    "kiro_queued_prompt",
                    "info",
                ));
            } else {
                if !next.open_call.is_empty() {
                    return Err(invalid("Kiro turn changed with unresolved tool call"));
                }
                next.turn_id = id.to_owned();
                next.turn_open = true;
                next.pending_interactions.clear();
                facts.push(fact(
                    &base,
                    "message",
                    "message",
                    &time_ns,
                    &next,
                    json!({"role":"user"}),
                    None,
                ));
                facts.push(fact(
                    &base,
                    "lifecycle",
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"running"}),
                    None,
                ));
            }
        }
        "turn_start" => {
            let id = row_id.ok_or("Kiro turn start lacks ID")?;
            if !next.turn_open && !next.open_call.is_empty() {
                return Err(invalid("Kiro turn changed with unresolved tool call"));
            }
            if !next.queued_id.is_empty() {
                let queued_base = format!("kiro:{origin}:{}", next.queued_id);
                next.turn_id.clone_from(&next.queued_id);
                facts.push(fact(
                    &queued_base,
                    "message",
                    "message",
                    &next.queued_stamp,
                    &next,
                    json!({"role":"user"}),
                    None,
                ));
                next.queued_id.clear();
                next.queued_stamp = "0".to_owned();
            } else if !next.turn_open {
                let reported = payload
                    .get("turnId")
                    .and_then(Value::as_str)
                    .or_else(|| payload.get("executionId").and_then(Value::as_str))
                    .unwrap_or(id);
                if !identifier(reported) {
                    return Err(invalid("invalid Kiro turn identity"));
                }
                next.turn_id = reported.to_owned();
            }
            next.turn_open = true;
            next.pending_interactions.clear();
            facts.push(fact(
                &base,
                "lifecycle",
                "lifecycle",
                &time_ns,
                &next,
                json!({"state":"running"}),
                None,
            ));
        }
        "turn_end" => {
            row_id.ok_or("Kiro turn end lacks ID")?;
            next.turn_open = false;
            next.pending_interactions.clear();
            let state = match payload.get("stopReason").and_then(Value::as_str) {
                Some("end_turn") => Some(("completed", None)),
                Some("cancelled") => Some(("interrupted", Some("kiro_cancelled"))),
                Some("error") => Some(("failed", Some("kiro_error"))),
                _ => None,
            };
            if let Some((state, reason)) = state {
                let payload = reason.map_or_else(
                    || json!({"state":state}),
                    |code| json!({"state":state,"reason_code":code}),
                );
                facts.push(fact(
                    &base,
                    "lifecycle",
                    "lifecycle",
                    &time_ns,
                    &next,
                    payload,
                    None,
                ));
            } else {
                facts.push(diagnostic(
                    &base,
                    "unknown-stop",
                    &time_ns,
                    &next,
                    "kiro_unknown_stop_reason",
                    "warning",
                ));
            }
        }
        "assistant" => {
            row_id.ok_or("Kiro assistant row lacks ID")?;
            if payload.get("operationType").and_then(Value::as_str) == Some("Say") {
                facts.push(fact(
                    &base,
                    "message",
                    "message",
                    &time_ns,
                    &next,
                    json!({"role":"assistant"}),
                    None,
                ));
            } else {
                facts.push(diagnostic(
                    &base,
                    "other-assistant",
                    &time_ns,
                    &next,
                    "kiro_non_message_assistant",
                    "info",
                ));
            }
        }
        "tool_call" | "tool_result" => {
            row_id.ok_or("Kiro tool row lacks ID")?;
            let operation = payload
                .get("toolCallId")
                .and_then(Value::as_str)
                .ok_or("Kiro tool row lacks call ID")?;
            if !identifier(operation) {
                return Err(invalid("invalid Kiro tool call identity"));
            }
            let (category, result) = if kind == "tool_call" {
                let category = category(
                    payload
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                );
                let denial = payload
                    .get("_meta")
                    .and_then(|value| value.get("kiro"))
                    .and_then(|value| value.get("policyDenial"));
                let status = match payload.get("status").and_then(Value::as_str) {
                    Some("denied") => "d",
                    Some("failed") => "f",
                    _ if denial.is_some_and(Value::is_object) => "d",
                    _ => "s",
                };
                let category_code = match category {
                    "mcp" => "m",
                    "shell" => "s",
                    "file" => "f",
                    _ => "o",
                };
                let key = digest_key(operation);
                if !next.open_call.is_empty() && !next.open_call.starts_with(&key) {
                    return Err(invalid(
                        "multiple unresolved Kiro tool calls are not representable",
                    ));
                }
                next.open_call = format!("{key}{status}{category_code}");
                let result = if status == "d" {
                    "denied"
                } else if status == "f" {
                    "failed"
                } else {
                    "started"
                };
                (category, result)
            } else {
                let key = digest_key(operation);
                if !next.open_call.starts_with(&key) || next.open_call.len() != 26 {
                    return Err(invalid("Kiro result has no matching unresolved call"));
                }
                let status = &next.open_call[24..25];
                let category = match &next.open_call[25..26] {
                    "m" => "mcp",
                    "s" => "shell",
                    "f" => "file",
                    _ => "other",
                };
                let result = if status == "d" {
                    "denied"
                } else if status == "f"
                    || payload.get("success").and_then(Value::as_bool) == Some(false)
                {
                    "failed"
                } else if payload.get("success").and_then(Value::as_bool) == Some(true) {
                    "succeeded"
                } else {
                    return Err(invalid("Kiro result lacks explicit success"));
                };
                next.open_call.clear();
                (category, result)
            };
            facts.push(fact(
                &base,
                "tool",
                "tool",
                &time_ns,
                &next,
                json!({"category":category,"result":result}),
                Some(operation),
            ));
        }
        "pending_interaction" => {
            let key = interaction_key(payload)?;
            if !next.pending_interactions.contains(&key) {
                if next.pending_interactions.len() == 2 {
                    return Err(invalid(
                        "more than two pending Kiro interactions are not representable",
                    ));
                }
                next.pending_interactions.push(key);
            }
            facts.push(diagnostic(
                &base,
                "permission",
                &time_ns,
                &next,
                "kiro_permission_unrepresented",
                "info",
            ));
            facts.push(fact(
                &base,
                "waiting",
                "lifecycle",
                &time_ns,
                &next,
                json!({"state":"paused","reason_code":"kiro_interaction_pending"}),
                None,
            ));
        }
        "interaction_resolved" => {
            let key = interaction_key(payload)?;
            let prior_count = next.pending_interactions.len();
            next.pending_interactions.retain(|pending| pending != &key);
            facts.push(diagnostic(
                &base,
                "permission",
                &time_ns,
                &next,
                "kiro_permission_unrepresented",
                "info",
            ));
            if prior_count > 0 && next.pending_interactions.is_empty() && next.turn_open {
                facts.push(fact(
                    &base,
                    "resumed",
                    "lifecycle",
                    &time_ns,
                    &next,
                    json!({"state":"running"}),
                    None,
                ));
            }
        }
        "usage_summary" => {
            // Kiro reports credits, not token counters. The current plugin
            // usage schema has no credit unit, so never invent token usage.
            facts.push(diagnostic(
                &base,
                "credits",
                &time_ns,
                &next,
                "kiro_credit_usage_unrepresented",
                "warning",
            ));
        }
        "session_metadata"
            if payload.get("key").and_then(Value::as_str) == Some("displayError") =>
        {
            facts.push(diagnostic(
                &base,
                "display-error",
                &time_ns,
                &next,
                "kiro_display_error",
                "error",
            ));
        }
        "sub_agent_start" | "sub_agent_complete" => {
            facts.push(diagnostic(
                &base,
                "subagent",
                &time_ns,
                &next,
                "kiro_subagent_relation_unresolved",
                "warning",
            ));
        }
        _ => {
            facts.push(diagnostic(
                &base,
                "unsupported",
                &time_ns,
                &next,
                "unsupported_kiro_row",
                "info",
            ));
        }
    }
    if checkpoint(&next).len() > 200 {
        return Err(invalid("Kiro checkpoint exceeds bounded cursor budget"));
    }
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, second: u8, payload: Value) -> Vec<u8> {
        serde_json::to_vec(
            &json!({"id":id,"timestamp":format!("2026-11-18T12:00:{second:02}Z"),
                                  "payload":payload}),
        )
        .unwrap()
    }

    #[test]
    fn path_binding_and_worker_identity_fail_closed() {
        let first = row("a", 0, json!({"type":"assistant","operationType":"Say"}));
        let a = initial_context(Path::new("/x/sess_one/messages.jsonl"), &first).unwrap();
        let b = initial_context(Path::new("/y/sess_one/messages.jsonl"), &first).unwrap();
        assert_eq!(a.session_id, "sess_one");
        assert!(restore(&mut b.clone(), &checkpoint(&a)).is_err());
        let worker = initial_context(
            Path::new("/x/sess_one/sub-executions/child-1.jsonl"),
            &first,
        )
        .unwrap();
        assert_eq!(worker.session_id, "child-1");
        assert_eq!(worker.worker_id, "child-1");
        assert!(checkpoint(&worker).len() <= 200);
    }

    #[test]
    fn queued_user_is_released_into_new_turn_after_restart() {
        let first = row("u1", 0, json!({"type":"user","content":"private"}));
        let mut context = initial_context(Path::new("/x/sess_one/messages.jsonl"), &first).unwrap();
        let (_, after_user) = translate_line(&first, &context, "generation", 0).unwrap();
        let queued = row("u2", 1, json!({"type":"user","content":"queued private"}));
        let (_, after_queued) = translate_line(&queued, &after_user, "generation", 10).unwrap();
        restore(&mut context, &checkpoint(&after_queued)).unwrap();
        let started = row(
            "turn-two",
            2,
            json!({"type":"turn_start","turnId":"turn-two"}),
        );
        let (facts, next) = translate_line(&started, &context, "generation", 20).unwrap();
        assert_eq!(facts[0]["event_id"], "kiro:generation:u2:message");
        assert_eq!(facts[0]["identity"]["turn_id"], "u2");
        assert_eq!(facts[1]["identity"]["turn_id"], "u2");
        assert!(next.queued_id.is_empty());
        assert!(!serde_json::to_string(&facts).unwrap().contains("private"));
    }

    #[test]
    fn explicit_stop_and_result_flags_control_status() {
        let first = row("u1", 0, json!({"type":"user"}));
        let context = initial_context(Path::new("/x/sess_one/messages.jsonl"), &first).unwrap();
        for (reason, state) in [
            ("end_turn", "completed"),
            ("cancelled", "interrupted"),
            ("error", "failed"),
        ] {
            let stop = row("stop", 1, json!({"type":"turn_end","stopReason":reason}));
            let (facts, _) = translate_line(&stop, &context, "generation", 0).unwrap();
            assert_eq!(facts[0]["payload"]["state"], state);
        }
        let result = row("r", 1, json!({"type":"tool_result","toolCallId":"call"}));
        assert!(translate_line(&result, &context, "generation", 0).is_err());
        let unknown = row("stop", 1, json!({"type":"turn_end","stopReason":"other"}));
        let (facts, _) = translate_line(&unknown, &context, "generation", 0).unwrap();
        assert_eq!(facts[0]["payload"]["code"], "kiro_unknown_stop_reason");
    }

    #[test]
    fn single_pending_interaction_is_checkpointed_and_resolved_exactly() {
        let first = row("u1", 0, json!({"type":"user"}));
        let context = initial_context(Path::new("/x/sess_one/messages.jsonl"), &first).unwrap();
        let (_, running) = translate_line(&first, &context, "generation", 0).unwrap();
        let pending = row(
            "p1",
            1,
            json!({"type":"pending_interaction","toolCallId":"call-1"}),
        );
        let (facts, waiting) = translate_line(&pending, &running, "generation", 1).unwrap();
        assert_eq!(facts[1]["payload"]["state"], "paused");
        let mut restored = context.clone();
        restore(&mut restored, &checkpoint(&waiting)).unwrap();
        let other = row(
            "r1",
            2,
            json!({"type":"interaction_resolved","toolCallId":"other"}),
        );
        let (facts, still_waiting) = translate_line(&other, &restored, "generation", 2).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(still_waiting.pending_interactions.len(), 1);
        let matched = row(
            "r2",
            3,
            json!({"type":"interaction_resolved","toolCallId":"call-1"}),
        );
        let (facts, resumed) = translate_line(&matched, &still_waiting, "generation", 3).unwrap();
        assert_eq!(facts[1]["payload"]["state"], "running");
        assert!(resumed.pending_interactions.is_empty());
        let second = row(
            "p2",
            4,
            json!({"type":"pending_interaction","toolCallId":"call-2"}),
        );
        let (_, waiting) = translate_line(&second, &running, "generation", 4).unwrap();
        let overlapping = row(
            "p3",
            5,
            json!({"type":"pending_interaction","toolCallId":"call-3"}),
        );
        let (_, two_pending) = translate_line(&overlapping, &waiting, "generation", 5).unwrap();
        let overflow = row(
            "p4",
            6,
            json!({"type":"pending_interaction","toolCallId":"call-4"}),
        );
        assert!(translate_line(&overflow, &two_pending, "generation", 6).is_err());
    }

    #[test]
    fn consent_rounds_are_distinct_across_checkpoint_restore() {
        let first = row("u1", 0, json!({"type":"user"}));
        let context = initial_context(Path::new("/x/sess_one/messages.jsonl"), &first).unwrap();
        let (_, mut current) = translate_line(&first, &context, "generation", 0).unwrap();
        for round in [1, 2] {
            let pending = row(
                &format!("pending-{round}"),
                round,
                json!({"type":"pending_interaction","toolCallId":"same-call","consentRound":round}),
            );
            (_, current) = translate_line(&pending, &current, "generation", round as u64).unwrap();
        }
        let mut restored = context.clone();
        restore(&mut restored, &checkpoint(&current)).unwrap();
        let old_round = row(
            "resolved-one",
            3,
            json!({"type":"interaction_resolved","toolCallId":"same-call","consentRound":1}),
        );
        let (facts, waiting) = translate_line(&old_round, &restored, "generation", 3).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(waiting.pending_interactions.len(), 1);
        let new_round = row(
            "resolved-two",
            4,
            json!({"type":"interaction_resolved","toolCallId":"same-call","consentRound":2}),
        );
        let (facts, running) = translate_line(&new_round, &waiting, "generation", 4).unwrap();
        assert_eq!(facts[1]["payload"]["state"], "running");
        assert!(running.pending_interactions.is_empty());
    }

    #[test]
    fn call_status_survives_restart_and_new_turn_with_open_call_fails() {
        let first = row("u1", 0, json!({"type":"user"}));
        let context = initial_context(Path::new("/x/sess_one/messages.jsonl"), &first).unwrap();
        let (_, running) = translate_line(&first, &context, "generation", 0).unwrap();
        let denied = row(
            "call",
            1,
            json!({"type":"tool_call","toolCallId":"call-1","toolName":"run_command","status":"denied"}),
        );
        let (facts, waiting) = translate_line(&denied, &running, "generation", 1).unwrap();
        assert_eq!(facts[0]["payload"]["result"], "denied");
        let mut restored = context.clone();
        restore(&mut restored, &checkpoint(&waiting)).unwrap();
        let result = row(
            "result",
            2,
            json!({"type":"tool_result","toolCallId":"call-1","success":true}),
        );
        let (facts, finished) = translate_line(&result, &restored, "generation", 2).unwrap();
        assert_eq!(facts[0]["payload"]["result"], "denied");
        assert_eq!(facts[0]["payload"]["category"], "shell");
        assert!(finished.open_call.is_empty());
        let end = row("end", 3, json!({"type":"turn_end","stopReason":"end_turn"}));
        let (_, ended) = translate_line(&end, &waiting, "generation", 3).unwrap();
        let next_user = row("u2", 4, json!({"type":"user"}));
        assert!(translate_line(&next_user, &ended, "generation", 4).is_err());
    }
}
