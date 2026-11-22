//! Bounded, content-free translation of one Claude Code main-transcript row.

use agenturm_jsonl_source::Result;
use agenturm_protocol::observation::{McpCall, McpMethod, is_mcp_identifier, mcp_tool_labels};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::Write;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

// Leave room for the turn/usage fields and the shared JSONL cursor envelope.
// Even two maximum-length labels fit as one exact current declaration.
const MAX_MCP_CHECKPOINT_BYTES: usize = 384;
const MAX_MCP_DECLARATIONS: usize = 4;

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
    mcp: Vec<McpDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct McpDeclaration {
    operation: String,
    worker: String,
    turn: String,
    /// Conflicting declarations with the same anchor withhold name linkage.
    labels: Option<(String, String)>,
}

impl McpDeclaration {
    fn wire_len(&self) -> usize {
        let (server, tool) = self.labels.as_ref().map_or(("-", "-"), |(server, tool)| {
            (server.as_str(), tool.as_str())
        });
        5 + self.operation.len() + self.worker.len() + self.turn.len() + server.len() + tool.len()
    }
}

impl Context {
    fn mcp_worker(&self) -> String {
        if self.worker_id.is_empty() {
            "x".into()
        } else {
            digest(&self.worker_id)
        }
    }

    fn remember_mcp(&mut self, operation: &str, labels: Option<(&str, &str)>) {
        let worker = self.mcp_worker();
        let turn = checkpoint_field(&self.turn_id);
        if let Some(call) = self
            .mcp
            .iter_mut()
            .find(|call| call.operation == operation && call.worker == worker && call.turn == turn)
        {
            if call
                .labels
                .as_ref()
                .is_some_and(|(server, tool)| Some((server.as_str(), tool.as_str())) != labels)
            {
                call.labels = None;
            }
            return;
        }
        let Some((server, tool)) = labels else { return };
        self.mcp.push(McpDeclaration {
            operation: operation.to_owned(),
            worker,
            turn: turn.to_owned(),
            labels: Some((server.to_owned(), tool.to_owned())),
        });
        while self.mcp.len() > MAX_MCP_DECLARATIONS
            || self.mcp.iter().map(McpDeclaration::wire_len).sum::<usize>()
                > MAX_MCP_CHECKPOINT_BYTES
        {
            self.mcp.remove(0);
        }
    }

    fn mcp_labels(&self, operation: &str) -> Option<(&str, &str)> {
        let worker = self.mcp_worker();
        let turn = checkpoint_field(&self.turn_id);
        self.mcp
            .iter()
            .find(|call| call.operation == operation && call.worker == worker && call.turn == turn)
            .and_then(|call| call.labels.as_ref())
            .map(|(server, tool)| (server.as_str(), tool.as_str()))
    }
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
        mcp: Vec::new(),
    })
}

fn checkpoint_field(value: &str) -> &str {
    if value.is_empty() { "x" } else { value }
}

pub(crate) fn checkpoint(context: &Context) -> String {
    let number = |value: Option<u64>| value.map_or("x".to_owned(), |value| value.to_string());
    let mut checkpoint = format!(
        "c.{}.{}.{}.{}.{}.{}",
        checkpoint_field(&context.turn_id),
        checkpoint_field(&context.last_usage_key),
        number(context.last_usage[0]),
        number(context.last_usage[1]),
        number(context.last_usage[2]),
        number(context.last_usage[3])
    );
    for call in &context.mcp {
        let (server, tool) = call.labels.as_ref().map_or(("-", "-"), |(server, tool)| {
            (server.as_str(), tool.as_str())
        });
        write!(
            checkpoint,
            ":{}:{}:{}:{server}:{tool}",
            call.operation, call.worker, call.turn
        )
        .expect("writing a string does not fail");
    }
    checkpoint
}

pub(crate) fn restore(context: &mut Context, checkpoint: &str) -> Result<()> {
    let (base, mcp) = checkpoint.split_once(':').unwrap_or((checkpoint, ""));
    if checkpoint.ends_with(':') {
        return Err(invalid("invalid Claude MCP checkpoint"));
    }
    let fields: Vec<_> = base.split('.').collect();
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
    context.mcp.clear();
    if !mcp.is_empty() {
        let fields = mcp.split(':').collect::<Vec<_>>();
        let (entries, rest) = fields.as_chunks::<5>();
        if !rest.is_empty()
            || entries.len() > MAX_MCP_DECLARATIONS
            || mcp.len() >= MAX_MCP_CHECKPOINT_BYTES
        {
            return Err(invalid("Claude MCP checkpoint exceeds its bounds"));
        }
        for &[operation, worker, turn, server, tool] in entries {
            if operation == "x"
                || !hash(operation)
                || !hash(worker)
                || turn != checkpoint_field(&context.turn_id)
                || context
                    .mcp
                    .iter()
                    .any(|known| known.operation == operation && known.worker == worker)
            {
                return Err(invalid("invalid Claude MCP checkpoint operation"));
            }
            let labels = if server == "-" && tool == "-" {
                None
            } else if is_mcp_identifier(server)
                && is_mcp_identifier(tool)
                && !server.contains("__")
                && !tool.contains("__")
            {
                Some((server.to_owned(), tool.to_owned()))
            } else {
                return Err(invalid("invalid Claude MCP checkpoint labels"));
            };
            context.mcp.push(McpDeclaration {
                operation: operation.to_owned(),
                worker: worker.to_owned(),
                turn: turn.to_owned(),
                labels,
            });
        }
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

fn mcp_payload((server, tool): (&str, &str), result: &str) -> Value {
    json!(McpCall {
        server: Some(server.to_owned()),
        tool: Some(tool.to_owned()),
        method: McpMethod::ToolsCall,
        duration_ns: None,
        result: result.to_owned(),
    })
}

fn append_mcp_details(facts: &mut Vec<Value>, details: Vec<Value>) -> Result<()> {
    use agenturm_jsonl_source::{MAX_STRUCTURAL_TOKENS, structural_tokens};
    // Preserve all existing facts on dense lines. Leave room for the shared
    // cursor and batch envelope within the configured host frame limits.
    const MAX_RECORDS: usize = 256;
    const MAX_BYTES: usize = 64 * 1024 - 1024;
    if details.is_empty() {
        return Ok(());
    }
    let encoded = serde_json::to_vec(facts)?;
    let mut bytes = encoded.len();
    let mut tokens = structural_tokens(&encoded);
    for detail in details {
        let encoded = serde_json::to_vec(&detail)?;
        bytes += encoded.len() + 1;
        tokens += structural_tokens(&encoded) + 1;
        if facts.len() >= MAX_RECORDS || bytes > MAX_BYTES || tokens > MAX_STRUCTURAL_TOKENS - 64 {
            break;
        }
        facts.push(detail);
    }
    Ok(())
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
    let mut mcp_details = Vec::new();
    if kind == "user" {
        let tool_results: Vec<_> = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .collect();
        if tool_results.is_empty() && has_text(content) {
            next.turn_id = digest(if row_key.is_empty() { &base } else { row_key });
            next.mcp.clear();
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
            let failed = match block.get("is_error") {
                None => false,
                Some(Value::Bool(failed)) => *failed,
                Some(_) => return Err(invalid("Claude tool result has invalid is_error")),
            };
            let operation = digest(operation);
            let labels = next.mcp_labels(&operation);
            let result = if failed { "failed" } else { "succeeded" };
            facts.push(fact(
                format!("{base}:tool-result-{index}"),
                revision,
                "tool",
                &time_ns,
                &next,
                Some(&operation),
                json!({"category":if labels.is_some() { "mcp" } else { "other" }, "result":result}),
            ));
            if let Some(labels) = labels {
                mcp_details.push(fact(
                    format!("{base}:mcp-result-{index}"),
                    revision,
                    "mcp_call",
                    &time_ns,
                    &next,
                    Some(&operation),
                    mcp_payload(labels, result),
                ));
            }
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
            let labels = mcp_tool_labels(name);
            next.remember_mcp(&operation, labels);
            facts.push(fact(
                format!("{base}:tool-use-{index}"),
                revision,
                "tool",
                &time_ns,
                &next,
                Some(&operation),
                json!({"category":category(name),"result":"started"}),
            ));
            if let Some(labels) = labels {
                mcp_details.push(fact(
                    format!("{base}:mcp-use-{index}"),
                    revision,
                    "mcp_call",
                    &time_ns,
                    &next,
                    Some(&operation),
                    mcp_payload(labels, "started"),
                ));
            }
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
    append_mcp_details(&mut facts, mcp_details)?;
    Ok((facts, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_mcp_names_require_exact_safe_boundaries() {
        assert_eq!(
            mcp_tool_labels("mcp__server.name__safe-tool"),
            Some(("server.name", "safe-tool"))
        );
        for name in [
            "mcp__bad path__tool",
            "mcp__server__/secret",
            "mcp__server__tool__extra",
            "mcp__server__",
            "mcp__",
            "Read",
        ] {
            assert_eq!(mcp_tool_labels(name), None);
        }
        assert!(mcp_tool_labels(&format!("mcp__{}__tool", "s".repeat(129))).is_none());
    }

    #[test]
    fn bounded_mcp_checkpoint_withholds_ambiguous_or_cross_worker_linkage() {
        let mut context = header_identity(br#"{"sessionId":"session"}"#).unwrap();
        context.turn_id = digest("active-turn");
        context.worker_id = "worker-one".into();
        context.last_usage_key = digest("message");
        context.last_usage = [Some(i64::MAX as u64); 4];
        let operation = digest("call");
        let server = "s".repeat(128);
        let tool = "t".repeat(128);
        context.remember_mcp(&operation, Some((&server, &tool)));
        let saved = checkpoint(&context);
        assert!(saved.len() + 261 <= agenturm_jsonl_source::MAX_CURSOR);

        let mut restored = header_identity(br#"{"sessionId":"session"}"#).unwrap();
        restore(&mut restored, &saved).unwrap();
        assert!(restored.mcp_labels(&operation).is_none());
        restored.worker_id = "worker-one".into();
        assert_eq!(
            restored.mcp_labels(&operation),
            Some((server.as_str(), tool.as_str()))
        );
        restored.remember_mcp(&operation, Some(("another", "tool")));
        assert!(restored.mcp_labels(&operation).is_none());
        let mut restored_again = restored.clone();
        restore(&mut restored_again, &checkpoint(&restored)).unwrap();
        assert!(restored_again.mcp_labels(&operation).is_none());

        let legacy = saved.split_once(':').unwrap().0;
        restore(&mut restored_again, legacy).unwrap();
        assert!(restored_again.mcp.is_empty());
        assert!(restore(&mut restored_again, &format!("{legacy}:")).is_err());
    }

    #[test]
    fn dense_result_rows_keep_the_preexisting_record_budget() {
        let mut context = header_identity(br#"{"sessionId":"session"}"#).unwrap();
        context.remember_mcp(&digest("operation"), Some(("server", "tool")));
        let results = vec![json!({"type":"tool_result","tool_use_id":"operation"}); 128];
        let row = json!({
            "type":"user","sessionId":"session","timestamp":"2026-10-24T12:00:00Z",
            "message":{"content":results},
        });
        let (facts, _) =
            translate_line(&serde_json::to_vec(&row).unwrap(), &context, "origin", 1).unwrap();
        assert_eq!(facts.len(), 256);
        assert_eq!(
            facts.iter().filter(|fact| fact["kind"] == "tool").count(),
            128
        );
    }

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
    fn tool_result_with_malformed_error_flag_refuses_before_ack() {
        let context = header_identity(br#"{"type":"user","sessionId":"session"}"#).unwrap();
        assert!(translate_line(br#"{"type":"user","sessionId":"session","timestamp":"2026-10-24T12:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","is_error":"false","content":"private output"}]}}"#, &context, "origin", 42).is_err());
        let (succeeded, _) = translate_line(br#"{"type":"user","sessionId":"session","timestamp":"2026-10-24T12:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","content":"private output"}]}}"#, &context, "origin", 42).unwrap();
        assert_eq!(succeeded[0]["payload"]["result"], "succeeded");
        let (failed, _) = translate_line(br#"{"type":"user","sessionId":"session","timestamp":"2026-10-24T12:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","is_error":true,"content":"private output"}]}}"#, &context, "origin", 42).unwrap();
        assert_eq!(failed[0]["payload"]["result"], "failed");
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
